//! sagent CLI 主控制结构。

use console::Term;

use crate::config::{self, SAgentCLIConfig, ToolProcessMode};

/// sagent CLI 主控制结构。
///
/// 管理终端输出、用户交互界面，后续可持有 Agent 实例、配置等全局状态。
pub struct SAgentCLI {
    /// 终端处理器，用于彩色输出、光标控制、用户输入等。
    pub console: Term,
    pub config: SAgentCLIConfig,
    pub compact: bool,
    pub tool_progress_mode: ToolProcessMode,
    active_session_lease: Option<String>,
    /// 当前活动的对话会话 ID
    resume: Option<String>,
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
    pub fn new(
        compact: Option<bool>,
        resume: Option<String>
    ) -> Self {
        let config = config::load_cli_config()
            .unwrap_or_else(|e|{
                tracing::warn!(error=%e, "加载 CLI 配置失败，使用默认配置");
                SAgentCLIConfig::default()
            }
        );

        Self {
            console: Term::stdout(),
            config: config.clone(),
            compact: compact.unwrap_or(config.display.compact),
            tool_progress_mode: config.display.tool_process,
            active_session_lease: None,
            resume: resume,
        }
    }
}

impl Default for SAgentCLI {
    /// 使用 [`SAgentCLI::new()`] 创建默认实例。
    fn default() -> Self {
        Self::new(
             Some(false),
             None
        )
    }
}


impl SAgentCLI {
    pub fn run(&mut self) -> anyhow::Result<()> {
        self.console.write_line("sagent 已启动")?;
        Ok(())
    }

    fn claim_active_session(&mut self, session_id: String) -> bool{
        if let Some(_) = self.active_session_lease{
            return true;
        }

        true

    }
}