//! sagent CLI 主控制结构。

use console::Term;

/// sagent CLI 主控制结构。
///
/// 管理终端输出、用户交互界面，后续可持有 Agent 实例、配置等全局状态。
pub struct SAgentCLI {
    /// 终端处理器，用于彩色输出、光标控制、用户输入等。
    pub console: Term,
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
