//! Terminal 子进程启动、输出读取和进程树终止。

use std::collections::HashMap;
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

/// 一个仍受终端监督器管理的子进程摘要。
///
/// 该结构仅用于生命周期诊断：调用标识可关联到审计事件，PID 可帮助 CI 在进程树
/// 清理失败时定位宿主进程；两者都不包含命令、环境变量、Session 或 Turn 内容。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ActiveProcess {
    /// 当前工具调用的稳定标识。
    pub tool_call_id: String,
    /// shell 进程的宿主 PID；Unix 下它同时是独立 process group 的 leader。
    pub process_id: u32,
}

/// 只保存当前执行中的调用标识和 shell PID，不保存命令或 Session/Turn 内容。
#[derive(Debug, Clone, Default)]
pub struct ProcessSupervisor {
    active: Arc<Mutex<HashMap<String, u32>>>,
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
    /// 创建空的活动进程登记表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个正在执行的工具调用及其 shell PID。
    pub fn register(&self, tool_call_id: impl Into<String>, process_id: u32) {
        self.active
            .lock()
            .expect("process registry poisoned")
            .insert(tool_call_id.into(), process_id);
    }

    /// 移除已完成或已取消的工具调用。
    pub fn unregister(&self, tool_call_id: &str) {
        self.active
            .lock()
            .expect("process registry poisoned")
            .remove(tool_call_id);
    }

    /// 返回当前登记的活动进程数量。
    pub fn active_count(&self) -> usize {
        self.active.lock().expect("process registry poisoned").len()
    }

    /// 返回活动进程的稳定快照，供关闭失败诊断而非业务决策使用。
    ///
    /// 输出按调用标识排序，使 CI 的失败日志可比较；调用方不能通过该快照终止或修改
    /// 进程，真正的取消仍必须走拥有 child handle 的 terminal 执行任务。
    pub fn active_processes(&self) -> Vec<ActiveProcess> {
        let mut processes = self
            .active
            .lock()
            .expect("process registry poisoned")
            .iter()
            .map(|(tool_call_id, process_id)| ActiveProcess {
                tool_call_id: tool_call_id.clone(),
                process_id: *process_id,
            })
            .collect::<Vec<_>>();
        processes.sort_by(|left, right| left.tool_call_id.cmp(&right.tool_call_id));
        processes
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

/// 受字符上限约束的 stdout 或 stderr 读取结果。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BoundedOutput {
    /// 已解码且受限的输出文本。
    pub content: String,
    /// 是否因达到上限而丢弃了部分输出。
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
/// 将 Unix shell 放入独立 session/process group，供取消时终止整棵子进程树。
pub fn configure_process_group(process: &mut Command) {
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
/// 非 Unix 平台不使用 process group；Windows 由 Job Object 和 taskkill 监督进程树。
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

/// 移除明显的密钥、令牌和密码变量，生成可传给子进程的环境快照。
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
