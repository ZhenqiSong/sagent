//! 日志初始化模块。
//!
//! 使用 tracing + tracing-subscriber，所有日志写 stderr，不污染 stdout 协议通道。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 日志初始化

use tracing_subscriber::EnvFilter;

/// 初始化日志子系统。
///
/// 所有日志写 stderr，绝不写 stdout。默认级别为 `info`，通过 `RUST_LOG` 覆盖。
/// 此函数幂等——重复调用不添加重复 subscriber，不会 panic。
///
/// # 示例
///
/// ```ignore
/// // 在 main 函数开始时调用
/// sagent_api::logging::init();
/// ```
pub fn init() {
    init_with_level("info");
}

/// 使用指定级别初始化日志子系统。
///
/// # 参数
///
/// * `default_level` - 默认日志级别（当 RUST_LOG 未设置时使用）
pub fn init_with_level(default_level: &str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    // 使用 try_init 确保幂等——重复调用不会 panic
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .with_target(true)
        .try_init();
}
