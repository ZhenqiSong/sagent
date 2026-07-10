//! sagent CLI 主控制结构。

use console::Term;

/// sagent CLI 主控制结构。
///
/// 管理终端输出、用户交互界面，后续可持有 Agent 实例、配置等全局状态。
pub struct SAgentCLI {
    /// 终端处理器，用于彩色输出、光标控制、用户输入等。
    pub console: Term,
}

/// sagent CLI 配置项。
///
/// 控制 CLI 的行为偏好，如默认 Profile、REPL 历史记录等。
pub struct SAgentCLIConfig {
    /// 默认使用的 Profile 名称，`None` 表示使用内置默认值。
    pub default_profile: Option<String>,
    /// 是否启用 REPL 历史记录。
    pub history_enabled: bool,
    /// 历史记录文件的最大行数。
    pub max_history_lines: usize,
}

impl Default for SAgentCLIConfig {
    fn default() -> Self {
        Self {
            default_profile: None,
            history_enabled: true,
            max_history_lines: 1000,
        }
    }
}

fn load_cli_config() -> anyhow::Result<SAgentCLIConfig> {
    Ok(SAgentCLIConfig::default())
}

impl SAgentCLI {
    /// 创建一个新的 CLI 实例，绑定到标准终端。
    ///
    /// # 示例
    ///
    /// ```ignore
    /// use sagent_cli::SAgentCLI;
    ///
    /// let cli = SAgentCLI::new();
    /// cli.console.write_line("sagent 已启动")?;
    /// # Ok::<_, anyhow::Error>(())
    /// ```
    pub fn new() -> Self {
        Self {
            console: Term::stdout(),
        }
    }
}

impl Default for SAgentCLI {
    /// 使用 [`SAgentCLI::new()`] 创建默认实例。
    fn default() -> Self {
        Self::new()
    }
}
