
use std::{fs, io::{self, Write}, path::PathBuf};

use crate::{cli_core::file_lock::FileLock, config::SAgentCLIConfig};
use sagent_common::get_sagent_home;
use sagent_core::utils::process::{pid_alive, process_start_time};
use uuid::Uuid;

pub(crate) struct ActiveSessionLease {
    pub lease_id: String,
    pub session_id: String,
    pub surface: String,
    pub enabled: bool,
    pub released: bool,
}

impl ActiveSessionLease {
    fn new(lease_id: &str, session_id: &str, surface: &str) -> Self{
        Self {
            lease_id: lease_id.to_string(),
            session_id: session_id.to_string(),
            surface: surface.to_string(),
            enabled: true,
            released: false,
        }
    }
}


#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SessionEntry{
    lease_id: String,
    session_id: String,
    surface: String,
    pid: u32,
    process_start_time: Option<f64>,
    started_at: f64,
    updated_at: f64,
}

impl SessionEntry {
    fn new(lease_id: &str, session_id: &str, surface: &str) -> Self{
        let pid = std::process::id();
        let process_start_time = process_start_time(pid);
        let started_at = chrono::Utc::now().timestamp() as f64 / 1000.0;
        let updated_at = started_at;
        Self {
            lease_id: lease_id.to_string(),
            session_id: session_id.to_string(),
            surface: surface.to_string(),
            pid,
            process_start_time,
            started_at,
            updated_at,
        }
    }

    /// 从文件读取活跃会话条目列表。
    ///
    /// 文件不存在时返回空向量（视为零活跃会话），
    /// 读取或反序列化出错时记录日志并同样返回空向量。
    fn read_from(path: &PathBuf) -> Vec<Self> {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => {
                tracing::error!(error=%e, "读取会话状态文件失败");
                return Vec::new();
            }
        };
        serde_json::from_str(&raw)
            .inspect_err(|e| tracing::error!(error=%e, "反序列化会话状态文件失败"))
            .unwrap_or_default()
    }
    
    fn write_to(path: &PathBuf, entries: &[Self]) -> anyhow::Result<()>{
        if let Some(parent) = path.parent(){
            std::fs::create_dir_all(&parent).unwrap();
        }

        let tmp_name = format!(
            "{}.{}.{}.tmp",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("active_sessions"),
            std::process::id(),
            Uuid::new_v4().simple(),
        );
        let temp_path = path.with_file_name(&tmp_name);
        
        {
            let file = fs::File::create(&temp_path)?;
            let mut writer = io::BufWriter::new(file);
            serde_json::to_writer(&mut writer, &entries)?;
            writer.flush()?;
        }
        

        fs::rename(&temp_path, path)?;
        Ok(())
    }
}

/// 获取活跃会话状态目录路径。
fn state_dir() -> PathBuf{
    get_sagent_home().join("runtime")
}

/// 获取活跃会话状态文件路径。
fn state_path() -> PathBuf{
    state_dir().join("active_sessions.json")
}

/// 获取活跃会话状态文件锁路径。
#[allow(dead_code)]
fn lock_path() -> PathBuf{
    state_dir().join("active_sessions.lock")
}


/// 清理已退出的会话。
fn prune_dead(entries: &[SessionEntry]) -> Vec<SessionEntry>{
    entries
    .iter()
    .filter(
        |v| pid_alive(v.pid, v.process_start_time)
    )
    .cloned()
    .collect()
}


/// 尝试获取活跃会话的租约。
///
/// # 参数
///
/// * `session_id` - 当前会话 ID
/// * `surface` - 调用来源标识（如 "cli"、"tui"），None 表示使用默认值
/// * `config` - CLI 配置引用
///
/// # 返回值
///
/// 返回 `Ok(true)` 表示成功获取租约，`Ok(false)` 表示已达上限。
pub(crate) fn try_acquire_active_session(
    session_id: &str,
    surface: Option<&str>,
    config: &SAgentCLIConfig,
) -> (Option<ActiveSessionLease>, Option<String>) {
    let _surface = surface.unwrap_or("cli");
    let lease_id = uuid::Uuid::new_v4().simple().to_string();
    // 未配置最大并发数时跳过会话限制检查
    let Some(_max_sessions) = config.max_concurrent_sessions else {
        return (
            Some(ActiveSessionLease{
                lease_id: lease_id,
                session_id: session_id.to_string(),
                surface: _surface.to_string(),
                enabled: true,
                released: false,
            }), None
        )
    };

    let _now = chrono::Utc::now().timestamp() as f64 / 1000.0;
    let _session_entry = SessionEntry::new(&lease_id, session_id, _surface);

    let state_path = state_path();
    let lock = FileLock::new(state_path.to_str().unwrap());
    let limit_reached = match lock.run_with_lock(|| -> anyhow::Result<bool> {
        let _raw_entries = SessionEntry::read_from(&state_path);
        let mut _entries = prune_dead(&_raw_entries);
        let pruned = _raw_entries.len() - _entries.len();
        if pruned > 0 {
            tracing::info!("已删除 {} 个已退出的会话", pruned);
        }
        let active_count = _entries.len();
        if active_count >= _max_sessions as usize {
            SessionEntry::write_to(&state_path, &_entries)?;
            tracing::info!(
                active=%active_count, max=%_max_sessions, surface=%_surface,
                "达到最大会话限制"
            );
            return Ok(true);
        }
        // 未达上限：登记当前会话并写回状态文件
        _entries.push(_session_entry);
        SessionEntry::write_to(&state_path, &_entries)?;
        Ok(false)
    }) {
        Ok(limit) => limit,
        Err(e) => {
            tracing::error!(error=%e, "文件加锁或写入失败，无法获取会话租约");
            return (None, None);
        }
    };

    if limit_reached {
        return (None, Some("达到最大会话限制".to_string()));
    }


    (
        Some(ActiveSessionLease::new(&lease_id, session_id, _surface)),
        None
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 创建唯一的临时目录路径（基于系统临时目录 + UUID）。
    fn temp_dir_path() -> PathBuf {
        std::env::temp_dir().join(format!("sagent-test-{}", Uuid::new_v4().simple()))
    }

    /// 快速创建测试用配置，仅设置需要的字段。
    fn make_config(max_sessions: Option<u32>) -> SAgentCLIConfig {
        SAgentCLIConfig {
            max_concurrent_sessions: max_sessions,
            ..Default::default()
        }
    }

    /// 运行闭包并确保临时目录在闭包结束后被清理。
    fn with_temp_dir<F: FnOnce(&PathBuf)>(f: F) {
        let dir = temp_dir_path();
        fs::create_dir_all(&dir).unwrap();
        f(&dir);
        fs::remove_dir_all(&dir).ok();
    }

    /// 清理全局活跃会话状态（测试前/后使用）。
    fn cleanup_state() {
        let p = state_path();
        let _ = fs::remove_file(&p);
        let _ = fs::remove_file(p.with_extension("lock"));
    }

    // ── SessionEntry::read_from ────────────────────────────────────────

    #[test]
    fn test_read_from_missing_file_returns_empty() {
        let path = temp_dir_path().join("nonexistent.json");
        let entries = SessionEntry::read_from(&path);
        assert!(
            entries.is_empty(),
            "缺失文件时应返回空向量"
        );
    }

    #[test]
    fn test_read_from_invalid_json_returns_empty() {
        with_temp_dir(|dir| {
            let path = dir.join("invalid.json");
            fs::write(&path, b"not valid json{{{").unwrap();
            let entries = SessionEntry::read_from(&path);
            assert!(
                entries.is_empty(),
                "无效 JSON 内容时应返回空向量"
            );
        });
    }

    #[test]
    fn test_read_from_empty_file_returns_empty() {
        with_temp_dir(|dir| {
            let path = dir.join("empty.json");
            fs::write(&path, b"").unwrap();
            let entries = SessionEntry::read_from(&path);
            assert!(
                entries.is_empty(),
                "空文件时应返回空向量"
            );
        });
    }

    // ── SessionEntry::write_to ─────────────────────────────────────────

    #[test]
    fn test_write_and_read_roundtrip() {
        with_temp_dir(|dir| {
            let path = dir.join("sessions.json");
            let entry = SessionEntry::new("lease-1", "session-1", "cli");
            
            SessionEntry::write_to(&path, &[entry.clone()]).unwrap();
            assert!(path.exists(), "写入后文件应存在");

            let read_back = SessionEntry::read_from(&path);
            assert_eq!(read_back.len(), 1);
            assert_eq!(read_back[0].lease_id, "lease-1");
            assert_eq!(read_back[0].session_id, "session-1");
            assert_eq!(read_back[0].surface, "cli");
            assert_eq!(read_back[0].pid, entry.pid);
            assert!(
                (read_back[0].started_at - entry.started_at).abs() < 1.0,
                "时间戳应一致"
            );
        });
    }

    #[test]
    fn test_write_multiple_entries_roundtrip() {
        with_temp_dir(|dir| {
            let path = dir.join("multi.json");
            let entries: Vec<_> = (0..5)
                .map(|i| {
                    SessionEntry::new(
                        &format!("lease-{}", i),
                        &format!("sess-{}", i),
                        "tui",
                    )
                })
                .collect();

            SessionEntry::write_to(&path, &entries).unwrap();
            let read_back = SessionEntry::read_from(&path);
            assert_eq!(read_back.len(), 5);
            for (i, e) in read_back.iter().enumerate() {
                assert_eq!(e.lease_id, format!("lease-{}", i));
                assert_eq!(e.session_id, format!("sess-{}", i));
            }
        });
    }

    #[test]
    fn test_write_creates_parent_directories() {
        with_temp_dir(|dir| {
            let path = dir.join("deep").join("nested").join("sessions.json");
            let entry = SessionEntry::new("lease-x", "sess-x", "cli");

            // 父目录不存在，write_to 应自动创建
            SessionEntry::write_to(&path, &[entry]).unwrap();
            assert!(path.exists(), "写入后文件应存在");
        });
    }

    #[test]
    fn test_write_atomic_rename_preserves_data() {
        with_temp_dir(|dir| {
            let path = dir.join("atomic.json");

            // 第一次写入
            let e1 = SessionEntry::new("lease-a", "sess-a", "cli");
            SessionEntry::write_to(&path, &[e1]).unwrap();

            // 第二次写入覆盖
            let e2 = SessionEntry::new("lease-b", "sess-b", "cli");
            SessionEntry::write_to(&path, &[e2.clone()]).unwrap();

            let read_back = SessionEntry::read_from(&path);
            assert_eq!(read_back.len(), 1);
            assert_eq!(read_back[0].lease_id, "lease-b");
            // 验证没有残留 .tmp 文件
            let tmp_files: Vec<_> = fs::read_dir(dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .map(|n| n.ends_with(".tmp"))
                        .unwrap_or(false)
                })
                .collect();
            assert!(tmp_files.is_empty(), "不应残留临时文件");
        });
    }

    // ── prune_dead ─────────────────────────────────────────────────────

    #[test]
    fn test_prune_dead_keeps_current_process() {
        let entry = SessionEntry::new("lease-alive", "sess-alive", "cli");
        // entry 使用当前进程 PID + 正确启动时间，应被保留
        let result = prune_dead(&[entry]);
        assert_eq!(result.len(), 1, "当前活跃进程应被保留");
        assert_eq!(result[0].lease_id, "lease-alive");
    }

    #[test]
    fn test_prune_dead_removes_mismatched_start_time() {
        let mut entry = SessionEntry::new("lease-dead", "sess-dead", "cli");
        // 覆盖为不匹配的时间戳，模拟 PID 被回收后启动时间变化的场景
        entry.process_start_time = Some(0.0);
        let result = prune_dead(&[entry]);
        assert!(
            result.is_empty(),
            "启动时间不匹配的进程应被移除"
        );
    }

    #[test]
    fn test_prune_dead_handles_empty_input() {
        let result = prune_dead(&[]);
        assert!(result.is_empty(), "空输入应返回空结果");
    }

    #[test]
    fn test_prune_dead_mixed_alive_and_dead() {
        let alive = SessionEntry::new("lease-alive", "sess-alive", "cli");

        let mut dead1 = SessionEntry::new("lease-dead1", "sess-dead1", "cli");
        dead1.process_start_time = Some(0.0);

        let mut dead2 = SessionEntry::new("lease-dead2", "sess-dead2", "tui");
        dead2.process_start_time = Some(9999999999.0);

        let result = prune_dead(&[dead1.clone(), alive.clone(), dead2.clone()]);
        assert_eq!(result.len(), 1, "只应保留当前活跃进程");
        assert_eq!(result[0].lease_id, alive.lease_id);
    }

    // ── try_acquire_active_session ─────────────────────────────────────

    #[test]
    fn test_acquire_with_no_max_limit_succeeds() {
        let config = make_config(None);
        let (lease, err_msg) =
            try_acquire_active_session("test-sess", Some("test"), &config);

        assert!(lease.is_some(), "未设上限时应成功获取租约");
        assert!(err_msg.is_none(), "不应有错误消息");

        let lease = lease.unwrap();
        assert_eq!(lease.session_id, "test-sess");
        assert_eq!(lease.surface, "test");
        assert!(!lease.released);
        assert!(!lease.lease_id.is_empty());
    }

    #[test]
    fn test_acquire_defaults_surface_to_cli() {
        let config = make_config(None);
        let (lease, _) =
            try_acquire_active_session("default-sess", None, &config);

        assert!(lease.is_some());
        assert_eq!(
            lease.unwrap().surface,
            "cli",
            "未提供 surface 时应默认为 'cli'"
        );
    }

    #[test]
    fn test_acquire_generates_unique_lease_ids() {
        let config = make_config(None);
        let (l1, _) =
            try_acquire_active_session("s1", Some("test"), &config);
        let (l2, _) =
            try_acquire_active_session("s2", Some("test"), &config);

        assert!(l1.is_some());
        assert!(l2.is_some());
        // 每次调用应生成唯一的 lease_id
        assert_ne!(
            l1.unwrap().lease_id,
            l2.unwrap().lease_id,
            "每次获取租约应生成唯一 ID"
        );
    }

    // 注意：以下测试会访问真实 sagent 运行时目录（~/.sagent/runtime/），
    // 使用文件锁进行序列化保护。每个测试在开始前和结束后均执行清理。
    // 若需完全隔离的临时目录进行测试，可考虑在 sagent-common 中暴露
    // SAGENT_HOME_OVERRIDE 或提供 #[cfg(test)] 钩子。

    #[test]
    fn test_acquire_with_max_sessions_not_exceeded_succeeds() {
        cleanup_state();
        let config = make_config(Some(5));
        let (lease, err_msg) =
            try_acquire_active_session("max-sess-test", Some("test"), &config);

        assert!(lease.is_some(), "未达上限时应成功获取租约");
        assert!(err_msg.is_none());

        // 验证状态文件包含我们的条目
        let entries = SessionEntry::read_from(&state_path());
        assert!(
            entries.iter().any(|e| e.session_id == "max-sess-test"),
            "状态文件应包含当前会话条目"
        );

        cleanup_state();
    }

    #[test]
    fn test_acquire_with_max_sessions_limit_reached() {
        cleanup_state();

        // 前置：预填充一个使用当前进程信息的条目，使其在 prune_dead 中存活
        let pre_entry = SessionEntry::new("pre-lease", "pre-sess", "cli");
        // 直接写文件，避开 FileLock
        SessionEntry::write_to(&state_path(), &[pre_entry]).unwrap();

        // 上限设为 1，当前已有 1 个活跃条目，应触发限流
        let config = make_config(Some(1));
        let (lease, err_msg) =
            try_acquire_active_session("blocked-sess", Some("test"), &config);

        assert!(lease.is_none(), "达到上限时应返回 None");
        assert!(
            err_msg.as_deref() == Some("达到最大会话限制"),
            "达到上限时应返回对应错误消息，实际为: {:?}",
            err_msg
        );

        cleanup_state();
    }
}

