//! 进程相关的通用工具函数。
//!
//! 提供进程存活判断、启动时间查询等基础能力，
//! 供 CLI、Gateway 等上游 crate 复用。

use sysinfo::{Pid, ProcessesToUpdate, System};

/// 获取指定进程的启动时间（Unix 时间戳，秒）。
///
/// # 参数
///
/// * `pid` - 目标进程的 PID
///
/// # 返回值
///
/// 返回进程启动时间的 Unix 时间戳（秒），进程不存在时返回 `None`。
///
/// # 示例
///
/// ```ignore
/// use sagent_core::utils::process::process_start_time;
/// let pid = std::process::id();
/// if let Some(start_time) = process_start_time(pid) {
///     println!("当前进程启动时间: {}", start_time);
/// }
/// ```
pub fn process_start_time(pid: u32) -> Option<f64> {
    let mut system = System::new();
    system.refresh_processes(
        ProcessesToUpdate::Some(&[Pid::from(pid as usize)]),
        true,
    );
    system
        .process(Pid::from(pid as usize))
        .map(|p| p.start_time() as f64)
}

/// 检查给定 PID 对应的进程是否仍然存活。
///
/// 同时比较进程启动时间，防止 PID 被回收后误判为存活。
///
/// # 参数
///
/// * `pid` - 目标进程的 PID
/// * `expected_start_time` - 预期的进程启动时间，`None` 表示仅靠 PID 判断
///
/// # 返回值
///
/// `true` 表示进程存活且启动时间匹配（或未提供启动时间仅 PID 匹配），
/// `false` 表示进程不存在或启动时间不匹配。
///
/// # 示例
///
/// ```ignore
/// use sagent_core::utils::process::{pid_alive, process_start_time};
/// let pid = std::process::id();
/// let start = process_start_time(pid);
/// assert!(pid_alive(pid, start));
/// ```
pub fn pid_alive(pid: u32, expected_start_time: Option<f64>) -> bool {
    let Some(expected) = expected_start_time else {
        // 未提供启动时间，仅靠 PID 判断
        return pid_exists(pid);
    };
    process_start_time(pid)
        .map_or(false, |current| (current - expected).abs() < 1.0)
}


/// 判断指定 PID 对应的进程是否存在。
pub fn pid_exists(pid: u32) -> bool{
    let mut system = System::new();
    system.refresh_processes(
        ProcessesToUpdate::Some(&[Pid::from(pid as usize)]),
        true,
    );
    system.process(Pid::from_u32(pid)).is_some()
}