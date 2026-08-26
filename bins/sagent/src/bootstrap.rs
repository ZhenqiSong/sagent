//! 进程启动引导模块。
//!
//! 负责在 CLI 启动早期向环境注入 Sagent 自身的标识环境变量，
//! 供子进程或插件识别当前运行环境。
//!
//! @author   songzq
//! @created  2026-08-26

use sagent_core::env::set_env_default;

/// 向环境注入 Sagent 标识变量。
///
/// 使用「仅未设置时才写入」的默认值策略，避免覆盖用户或上层
/// 环境已显式指定的值。通常在 CLI 启动的最早期调用一次。
///
/// 设置的环境变量:
/// - `AI_AGENT`: 标识当前运行在 AI Agent 上下文中，值为 `sagent`
/// - `SAGENT`: 标识当前进程为 Sagent，值为 `true`
pub fn advertise_sagent_env() {
    set_env_default("AI_AGENT", "sagent");
    set_env_default("SAGENT", "true");
}
