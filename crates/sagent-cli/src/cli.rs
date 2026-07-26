//! sagent CLI 主控制结构。

use console::Term;
use sagent_core::agent::AIAgent;

use crate::config::{self, SAgentCLIConfig, ToolProcessMode};
use crate::cli_core::active_session::ActiveSessionLease;
use std::io::{self, Write};
use crate::utils;

/// sagent CLI 主控制结构。
///
/// 管理终端输出、用户交互界面，后续可持有 Agent 实例、配置等全局状态。
pub struct SAgentCLI {
    /// 终端处理器，用于彩色输出、光标控制、用户输入等。
    pub console: Term,
    pub config: SAgentCLIConfig,
    pub compact: bool,
    pub tool_progress_mode: ToolProcessMode,
    active_session_lease: Option<ActiveSessionLease>,
    /// 当前活动的对话会话 ID
    session_id: String,
    /// 是否是恢复的会话
    resumed: bool,
    // Agent 实例
    agent: Option<AIAgent>
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
            agent: None
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
        if !self.claim_active_session(Some("cli"), None){
            return Ok(())
        }

        utils::theme::detect_light_mode();
        
        // 清空屏幕
        let (lines, _) = Term::stdout().size();
        if lines > 2 {
            let n = lines.saturating_sub(1);
            io::stdout().write_all(&vec![b'\n'; n as usize])?;  // N 个 0x0A 字节
            io::stdout().flush()?;
        }

        self.show_banner()?;
        Ok(())
    }

    fn show_banner(&mut self) -> anyhow::Result<()>{
        self.console.clear_screen()?;
        let ctx_len = self.agent.as_ref()
            .and_then(|a| a.context_compressor.as_ref())
            .map(|cc| cc.engine_state.context_length);

        // 是否使用紧凑模式
        let (_, term_width) = Term::stdout().size();
        if self.compact || term_width < 80{

        }

        Ok(())
    }

    fn 

    /// 尝试获取活动会话租约。
    ///
    /// 如果已有租约，则直接返回 `true`；否则尝试获取新租约，并根据 `stderr` 参数决定是否输出错误信息。
    fn claim_active_session(&mut self, surface: Option<&str>, stderr: Option<bool>) -> bool {
        let _stderr = stderr.unwrap_or(false);

        if self.active_session_lease.is_some() {
            return true;
        }

        match ActiveSessionLease::try_acquire_active_session(
            &self.session_id,
            surface,
            &self.config
        ) {
            Ok(lease) => {
                self.active_session_lease = Some(lease);
                true
            }
            Err(e) => {
                let msg = e.to_string();
                if _stderr {
                    eprintln!("{}", msg);
                } else {
                    self._console_print(&format!("[bold red]{}[/]", msg)).ok();
                }
                false
            }
        }
    }

    /// 使用终端 console 输出信息。
    ///
    /// 封装 [`console::Term`] 的输出能力，提供统一的终端打印接口。
    /// 未来可扩展为支持彩色前缀（如 `[INFO]`、`[WARN]`、`[ERROR]`）和
    /// 更丰富的格式控制。
    pub fn _console_print(&self, msg: &str) -> anyhow::Result<()> {
        self.console.write_line(msg)?;
        Ok(())
    }
}