//! 基础 CLI 命令实现。
//!
//! CLI 只通过 Runtime service/API 访问 Session，不直接调用 Repository。
//!
//! @author   songzq
//! @created  2026-08-18
//! @change   2026-08-18 初始版本：Phase 1 Step 8 Session CLI

use clap::{Args, Subcommand};
use sagent_config::{ConfigError, ConfigLoader};
use sagent_runtime::{ActorError, RuntimeError, RuntimeHandle, SessionView};
use sagent_session::{CreateSession, ListSessions, MessageRange};
use sagent_types::ids::SessionId;
use sagent_types::session::{Session, SessionStatus};
use sagent_types::version::{Capabilities, ProtocolVersion};
use serde::Serialize;
use thiserror::Error;

/// Session CLI 子命令。
#[derive(Subcommand)]
pub(crate) enum SessionAction {
    /// 创建一个空 Session。
    Create(CreateArgs),
    /// 列出已持久化 Session。
    List(ListArgs),
    /// 查看 Session 和消息窗口。
    Get(GetArgs),
    /// 恢复 Session 到当前 Runtime。
    Resume(ResumeArgs),
}

/// `session create` 参数。
#[derive(Args)]
pub(crate) struct CreateArgs {
    /// 用户可见标题。
    #[arg(long)]
    pub(crate) title: Option<String>,
    /// 创建来源。
    #[arg(long, default_value = "cli")]
    pub(crate) source: String,
    /// 保存到 Session 的工作目录。
    #[arg(long)]
    pub(crate) cwd: Option<String>,
    /// 输出 JSON。
    #[arg(long)]
    pub(crate) json: bool,
}

/// `session list` 参数。
#[derive(Args)]
pub(crate) struct ListArgs {
    /// 最大返回数量。
    #[arg(long)]
    pub(crate) limit: Option<u32>,
    /// 按来源过滤。
    #[arg(long)]
    pub(crate) source: Option<String>,
    /// 按状态过滤。
    #[arg(long)]
    pub(crate) status: Option<String>,
    /// 输出 JSON。
    #[arg(long)]
    pub(crate) json: bool,
}

/// `session get` 参数。
#[derive(Args)]
pub(crate) struct GetArgs {
    /// Session ID。
    pub(crate) session_id: String,
    /// 最大返回消息数量。
    #[arg(long)]
    pub(crate) limit: Option<u32>,
    /// 只返回此 sequence 之后的消息。
    #[arg(long)]
    pub(crate) after_sequence: Option<u64>,
    /// 输出 JSON。
    #[arg(long)]
    pub(crate) json: bool,
}

/// `session resume` 参数。
#[derive(Args)]
pub(crate) struct ResumeArgs {
    /// Session ID。
    pub(crate) session_id: String,
    /// 输出 JSON。
    #[arg(long)]
    pub(crate) json: bool,
}

/// CLI 可展示的错误。
#[derive(Debug, Error)]
pub(crate) enum CliError {
    /// 配置加载失败。
    #[error("配置错误: {0}")]
    Config(#[from] ConfigError),
    /// Runtime 启动或操作失败。
    #[error("Runtime 错误: {0}")]
    Runtime(#[from] RuntimeError),
    /// Session Actor 操作失败。
    #[error("Session 操作失败: {0}")]
    Actor(#[from] ActorError),
    /// Session 状态参数无效。
    #[error("无效的 Session 状态: {0}，可选值为 active、closed、recovering")]
    InvalidStatus(String),
    /// JSON 输出失败。
    #[error("JSON 输出失败: {0}")]
    Json(#[from] serde_json::Error),
}

/// 执行 Session CLI 命令。
pub(crate) fn run_session(action: SessionAction) -> Result<(), CliError> {
    match action {
        SessionAction::Create(args) => run_create(args),
        SessionAction::List(args) => run_list(args),
        SessionAction::Get(args) => run_get(args),
        SessionAction::Resume(args) => run_resume(args),
    }
}

/// 执行 `health` 命令。
pub(crate) fn run_health(json: bool) -> Result<(), CliError> {
    let health = with_runtime(|_, _| Ok(health_value()))?;
    if json {
        print_json(&health)
    } else {
        println!("sagent health: ok");
        Ok(())
    }
}

/// 执行 `protocol describe` 命令。
pub(crate) fn run_protocol_describe(json: bool) -> Result<(), CliError> {
    let version = Capabilities::runtime_capabilities().protocol_version();
    if json {
        print_json(&version)
    } else {
        println!("Protocol: {}", version.protocol);
        println!("Version: {}", version.version);
        println!("Runtime version: {}", version.runtime_version);
        println!("Features: {}", version.features.join(", "));
        Ok(())
    }
}

fn run_create(args: CreateArgs) -> Result<(), CliError> {
    let session = with_runtime(|runtime, rt| {
        let mut input = CreateSession::new(args.source);
        input.title = args.title;
        input.cwd = args.cwd;
        let handle = rt.block_on(runtime.create_session(input))?;
        Ok(rt.block_on(handle.snapshot())?.session)
    })?;
    if args.json {
        print_json(&session)
    } else {
        println!("已创建 Session: {}", session.id.0);
        Ok(())
    }
}

fn run_list(args: ListArgs) -> Result<(), CliError> {
    let status = args.status.as_deref().map(parse_status).transpose()?;
    let sessions = with_runtime(|runtime, rt| {
        Ok(rt.block_on(runtime.list_sessions(ListSessions {
            limit: args.limit,
            before: None,
            source: args.source,
            status,
        }))?)
    })?;
    if args.json {
        print_json(&sessions)
    } else if sessions.is_empty() {
        println!("没有 Session");
        Ok(())
    } else {
        for session in sessions {
            let title = session.title.as_deref().unwrap_or("-");
            println!(
                "{}\t{}\t{}\t{}\t{}",
                session.id.0,
                status_name(&session.status),
                session.source,
                title,
                session.updated_at
            );
        }
        Ok(())
    }
}

fn run_get(args: GetArgs) -> Result<(), CliError> {
    let session_id = SessionId(args.session_id);
    let response = with_runtime(|runtime, rt| {
        let view = rt
            .block_on(runtime.get_session(&session_id))?
            .ok_or_else(|| RuntimeError::SessionNotFound(session_id.clone()))?;
        let (session, messages) = match view {
            SessionView::Live(handle) => {
                let snapshot = rt.block_on(handle.snapshot())?;
                let messages = rt.block_on(handle.list_messages(MessageRange {
                    limit: args.limit,
                    after_sequence: args.after_sequence,
                }))?;
                (snapshot.session, messages)
            },
            SessionView::Snapshot(snapshot) => {
                let messages = snapshot
                    .messages
                    .into_iter()
                    .filter(|message| {
                        args.after_sequence.map_or(true, |sequence| message.sequence > sequence)
                    })
                    .take(args.limit.unwrap_or(50) as usize)
                    .collect();
                (snapshot.session, messages)
            },
        };
        Ok(SessionResponse {
            session,
            messages,
            has_more: false,
        })
    })?;
    if args.json {
        print_json(&response)
    } else {
        print_session(&response.session);
        println!("Messages: {}", response.messages.len());
        Ok(())
    }
}

fn run_resume(args: ResumeArgs) -> Result<(), CliError> {
    let session_id = SessionId(args.session_id);
    let response = with_runtime(|runtime, rt| {
        let handle = rt.block_on(runtime.resume_session(&session_id))?;
        let snapshot = rt.block_on(handle.snapshot())?;
        Ok(SessionResponse {
            session: snapshot.session,
            messages: snapshot.messages,
            has_more: false,
        })
    })?;
    if args.json {
        print_json(&response)
    } else {
        println!("已恢复 Session: {}", response.session.id.0);
        println!("Messages: {}", response.messages.len());
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct SessionResponse {
    session: Session,
    messages: Vec<sagent_types::message::Message>,
    has_more: bool,
}

fn parse_status(value: &str) -> Result<SessionStatus, CliError> {
    match value {
        "active" => Ok(SessionStatus::Active),
        "closed" => Ok(SessionStatus::Closed),
        "recovering" => Ok(SessionStatus::Recovering),
        _ => Err(CliError::InvalidStatus(value.to_string())),
    }
}

fn print_session(session: &Session) {
    println!("Session: {}", session.id.0);
    println!("Source: {}", session.source);
    println!("Title: {}", session.title.as_deref().unwrap_or("-"));
    println!("Status: {}", status_name(&session.status));
    println!("Messages: {}", session.message_count);
    println!("Revision: {}", session.revision);
}

fn status_name(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Active => "active",
        SessionStatus::Closed => "closed",
        SessionStatus::Recovering => "recovering",
    }
}

fn print_json<T: Serialize>(value: &T) -> Result<(), CliError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn health_value() -> serde_json::Value {
    let version = ProtocolVersion::default();
    serde_json::json!({
        "status": "ok",
        "protocol": version.protocol,
        "version": version.version,
    })
}

fn with_runtime<T, F>(operation: F) -> Result<T, CliError>
where
    F: FnOnce(&RuntimeHandle, &tokio::runtime::Runtime) -> Result<T, CliError>,
{
    let config = ConfigLoader::discover()?.load()?;
    let runtime = sagent_runtime::Runtime::open(config)?;
    let async_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::Runtime(RuntimeError::Config(error.to_string())))?;
    let result = operation(&runtime, &async_runtime);
    let shutdown = async_runtime.block_on(runtime.shutdown());
    match (result, shutdown) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(CliError::Runtime(error)),
        (Ok(value), Ok(())) => Ok(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_values_are_strict() {
        assert_eq!(parse_status("active").unwrap(), SessionStatus::Active);
        assert!(parse_status("ACTIVE").is_err());
    }

    #[test]
    fn health_has_stable_shape() {
        let value = health_value();
        assert_eq!(value["status"], "ok");
        assert_eq!(value["protocol"], "sagent.rpc");
    }
}
