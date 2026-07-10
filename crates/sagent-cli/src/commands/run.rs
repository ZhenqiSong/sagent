//! Run 子命令 — 启动交互式对话或单次对话。

use anyhow::Result;

/// 执行 Run 子命令。
///
/// 如果提供了 `message` 则执行单次对话，
/// 否则进入 REPL 交互模式。
///
/// # 参数
///
/// * `profile` - 要使用的 Profile 名称，`None` 使用默认 Profile
/// * `message` - 单次对话消息，空 Vec 表示进入 REPL 模式
///
/// # 示例
///
/// ```rust,ignore
/// // REPL 模式
/// execute(None, vec![]).await?;
/// // 单次对话
/// execute(Some("dev".into()), vec!["帮我查一下 Rust 新闻".into()]).await?;
/// ```
pub async fn execute(profile: Option<String>, message: Vec<String>) -> Result<()> {
    let profile = profile.as_deref().unwrap_or("default");

    if message.is_empty() {
        tracing::info!("进入 REPL 交互模式 (profile={})", profile);
        tracing::warn!("REPL 模式尚未实现");
    } else {
        let msg = message.join(" ");
        tracing::info!("单次对话: {} (profile={})", msg, profile);
        tracing::warn!("对话功能尚未实现");
    }

    Ok(())
}
