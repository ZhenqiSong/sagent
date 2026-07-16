//! sagent CLI 主控制结构。

use console::Term;

use crate::config::{self, SAgentCLIConfig, ToolProcessMode};
use crate::cli_core::active_session::try_acquire_active_session;

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
    session_id: String,
    /// 是否是恢复的会话
    resumed: bool,
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

        let resumed = (&resume).is_some();
        let now = chrono::Local::now().naive_local();
        let session_id = resume.unwrap_or_else(|| {
            let timestamp_str = now.format("%Y%m%d_%H%M%S").to_string();
            let short_uuid = &uuid::Uuid::new_v4().simple().to_string()[..6];
            let sid = format!("{}_{}", timestamp_str, short_uuid);
            tracing::info!(session_id=%sid, "生成新的会话 ID");
            sid
        });

        Self {
            console: Term::stdout(),
            config: config.clone(),
            compact: compact.unwrap_or(config.display.compact),
            tool_progress_mode: config.display.tool_process,
            active_session_lease: None,
            session_id,
            resumed: resumed,
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
        // self.claim_active_session(session_id);
        if self.claim_active_session(Some("cli"), None){
            return Ok(())
        }
        Ok(())
    }

    fn claim_active_session(&mut self, surface: Option<&str>, _stderr: Option<bool>) -> bool {
        if let Some(_) = self.active_session_lease {
            return true;
        }

        let _ = try_acquire_active_session(
            &self.session_id,
            surface,
            &self.config
        );
        false
    }
}