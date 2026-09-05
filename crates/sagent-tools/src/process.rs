//! Terminal 子进程启动、输出读取和进程树终止。

use std::collections::HashSet;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::time::{Duration, timeout};

#[cfg(windows)]
use std::ptr::null;

/// 终止进程的原因，用于调试和上层结果映射。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TerminationReason {
    Timeout,
    Cancelled,
    Shutdown,
}

/// 只保存当前执行中的调用标识，不保存 Session/Turn 内容。
#[derive(Debug, Clone, Default)]
pub struct ProcessSupervisor {
    active: Arc<Mutex<HashSet<String>>>,
}

/// Windows Job Object 或 POSIX no-op guard，保证执行结束时不会遗留子进程树。
#[derive(Debug)]
pub struct ProcessTreeGuard {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

// Windows HANDLE 的所有权随 guard 一起移动，不能在两个线程同时使用同一 handle；
// 这里的 Send/Sync 只允许把所有权交给执行任务，实际 Win32 调用仍在单个任务中完成。
unsafe impl Send for ProcessTreeGuard {}
unsafe impl Sync for ProcessTreeGuard {}

#[cfg(windows)]
impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(not(windows))]
impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {}
}

/// 创建进程树监管对象。Windows 使用带 `KILL_ON_JOB_CLOSE` 的 Job Object。
pub fn create_process_tree_guard() -> std::io::Result<ProcessTreeGuard> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        let handle = CreateJobObjectW(null(), null());
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&info) as u32,
        );
        if ok == 0 {
            windows_sys::Win32::Foundation::CloseHandle(handle);
            return Err(std::io::Error::last_os_error());
        }
        Ok(ProcessTreeGuard { handle })
    }
    #[cfg(not(windows))]
    {
        Ok(ProcessTreeGuard {})
    }
}

/// 将已启动的子进程加入 Job Object；POSIX 由 `setsid` 建立进程组。
pub fn attach_process_tree_guard(guard: &ProcessTreeGuard, pid: u32) -> std::io::Result<()> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };
        let process = OpenProcess(
            PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        );
        if process.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let ok = AssignProcessToJobObject(guard.handle, process);
        windows_sys::Win32::Foundation::CloseHandle(process);
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (guard, pid);
        Ok(())
    }
}

impl ProcessSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, tool_call_id: impl Into<String>) {
        self.active
            .lock()
            .expect("process registry poisoned")
            .insert(tool_call_id.into());
    }

    pub fn unregister(&self, tool_call_id: &str) {
        self.active
            .lock()
            .expect("process registry poisoned")
            .remove(tool_call_id);
    }

    pub fn active_count(&self) -> usize {
        self.active.lock().expect("process registry poisoned").len()
    }
}

/// 读取一个 stdout/stderr 管道，内存最多保留 limit 个字符。
pub async fn read_bounded<R>(mut reader: R, limit: usize) -> BoundedOutput
where
    R: AsyncRead + Unpin,
{
    let limit = limit.max(1);
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut total_bytes = 0_usize;
    let mut truncated = false;
    loop {
        let count = match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(count) => count,
            Err(_) => break,
        };
        total_bytes = total_bytes.saturating_add(count);
        if bytes.len() < limit.saturating_mul(4) {
            let remaining = limit.saturating_mul(4).saturating_sub(bytes.len());
            bytes.extend_from_slice(&buffer[..count.min(remaining)]);
        }
        if total_bytes > limit.saturating_mul(4) {
            truncated = true;
        }
    }

    let decoded = String::from_utf8_lossy(&bytes).into_owned();
    let mut content = decoded.chars().take(limit).collect::<String>();
    if decoded.chars().count() > limit {
        truncated = true;
    }
    if content.is_empty() && truncated {
        content = "[输出已截断]".chars().take(limit).collect();
    }
    BoundedOutput { content, truncated }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BoundedOutput {
    pub content: String,
    pub truncated: bool,
}

/// 通过当前平台 shell 启动命令。
pub fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut process = Command::new("cmd.exe");
        process.args(["/C", command]);
        configure_command(process)
    }
    #[cfg(not(windows))]
    {
        use std::env;
        let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut process = Command::new(shell);
        process.args(["-c", command]);
        configure_command(process)
    }
}

fn configure_command(mut process: Command) -> Command {
    process
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    process
}

/// 终止当前子进程及其子孙进程。
pub async fn terminate_tree(child: &mut Child, _reason: TerminationReason) {
    #[cfg(windows)]
    {
        if let Some(pid) = child.id() {
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
        }
    }
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // shell 以独立 process group 启动；负 pid 代表向整个 group 发信号。
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
        }
    }

    if timeout(Duration::from_millis(300), child.wait())
        .await
        .is_err()
    {
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = child.kill().await;
        let _ = timeout(Duration::from_secs(2), child.wait()).await;
    }
}

/// 当 shell 已经退出但后代仍持有 stdout/stderr pipe 时，按原始 pid 清理整个树。
pub async fn terminate_pid_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

#[cfg(unix)]
pub fn configure_process_group(process: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        process.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
pub fn configure_process_group(_process: &mut Command) {}

/// 结束时排空已退出进程的管道，避免子进程继承的 pipe 句柄造成任务悬挂。
pub async fn drain_output(
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
    limit: usize,
) -> (BoundedOutput, BoundedOutput) {
    let stdout = async move {
        match stdout {
            Some(reader) => read_bounded(reader, limit).await,
            None => BoundedOutput {
                content: String::new(),
                truncated: false,
            },
        }
    };
    let stderr = async move {
        match stderr {
            Some(reader) => read_bounded(reader, limit).await,
            None => BoundedOutput {
                content: String::new(),
                truncated: false,
            },
        }
    };
    tokio::join!(stdout, stderr)
}

pub fn sanitize_environment(
    environment: impl IntoIterator<Item = (String, String)>,
) -> std::collections::HashMap<String, String> {
    environment
        .into_iter()
        .filter(|(key, _)| !is_secret_key(key))
        .collect()
}

fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    key.contains("API_KEY")
        || key.ends_with("_TOKEN")
        || key.ends_with("_PASSWORD")
        || key.ends_with("_SECRET")
        || key.ends_with("_PRIVATE_KEY")
        || key == "AUTHORIZATION"
}
