//! Session CLI 命令处理器。
//!
//! 本模块负责命令分发、输入校验和输出适配；会话创建、查询及生命周期写入由
//! `service` 模块提供。处理器通过 `SessionService` 使用共享上下文，但不持有数据库连接
//! 或命令值。
//!
//! 作者：SongZQ

use anyhow::{Context, Result};
use sagent_types::{MessageId, SearchHit, SessionDetail, SessionSummary};

use super::{SessionCommand, service::SessionService};
use crate::{
    commands::{Command, CommandContext, CommandHandler},
    output::print_output,
};

/// Session 命令的领域处理器。
///
/// 处理器拥有本次 CLI 调用的会话服务，但不保存待执行的命令值；服务统一管理上下文、
/// 存储装配和会话领域操作。
pub struct SessionHandler {
    /// 持有命令上下文的会话领域服务。
    service: SessionService,
}

impl SessionHandler {
    /// 创建拥有命令上下文的 Session 处理器。
    pub(crate) fn new(context: CommandContext) -> Self {
        Self {
            service: SessionService::new(context),
        }
    }

    /// 执行传入的 Session 子命令。
    fn execute_command(&self, command: SessionCommand) -> Result<()> {
        match command {
            SessionCommand::Create { title, model } => self.handle_create(title, model),
            SessionCommand::Show {
                session_id,
                limit,
                offset,
            } => self.handle_show(&session_id, limit, offset),
            SessionCommand::Search {
                query,
                limit,
                session_id,
            } => self.handle_search(&query, limit, session_id.as_deref()),
            SessionCommand::List {
                limit,
                offset,
                include_archived,
            } => self.handle_list(limit, offset, include_archived),
            SessionCommand::Rename { session_id, title } => self.handle_rename(&session_id, &title),
            SessionCommand::Archive { session_id } => self.handle_archive(&session_id),
            SessionCommand::Unarchive { session_id } => self.handle_unarchive(&session_id),
            SessionCommand::Finish { session_id, reason } => {
                self.handle_finish(&session_id, &reason)
            }
            SessionCommand::Rewind {
                session_id,
                message_id,
            } => self.handle_rewind(&session_id, &message_id),
            SessionCommand::Restore {
                session_id,
                message_id,
            } => self.handle_restore(&session_id, &message_id),
        }
    }

    /// 创建会话并输出新 ID。
    fn handle_create(&self, title: Option<String>, model: Option<String>) -> Result<()> {
        let session_id = self.service.create_session(title, model)?;
        let value = serde_json::json!({ "session_id": session_id.as_str() });
        print_output(
            self.service.context().format,
            &value,
            vec![format!("已创建会话: {}", session_id.as_str())],
        )
    }

    /// 读取并展示单个会话。
    fn handle_show(&self, session_id: &str, limit: u32, offset: u32) -> Result<()> {
        let detail = self.service.show_session(session_id, limit, offset)?;
        print_output(self.service.context().format, &detail, render_show(&detail))
    }

    /// 搜索当前 Profile 的消息。
    fn handle_search(&self, query: &str, limit: u32, session_id: Option<&str>) -> Result<()> {
        let hits = self.service.search_messages(query, limit, session_id)?;
        print_output(self.service.context().format, &hits, render_search(&hits))
    }

    /// 列出当前 Profile 的会话。
    fn handle_list(&self, limit: u32, offset: u32, include_archived: bool) -> Result<()> {
        let sessions = self
            .service
            .list_sessions(limit, offset, include_archived)?;
        print_output(
            self.service.context().format,
            &sessions,
            render_list(&sessions),
        )
    }

    /// 修改会话标题并输出状态。
    fn handle_rename(&self, session_id: &str, title: &str) -> Result<()> {
        let title = validate_title(title)?;
        let updated_at = self.service.now_rfc3339()?;
        let changed = self
            .service
            .rename_session(session_id, title, &updated_at)?;
        let value = serde_json::json!({
            "operation": "rename",
            "session_id": session_id,
            "title": title,
            "changed": changed,
            "updated_at": updated_at,
        });
        print_output(
            self.service.context().format,
            &value,
            vec![format!("已重命名会话: {session_id}")],
        )
    }

    /// 归档会话，使其从默认列表中隐藏。
    fn handle_archive(&self, session_id: &str) -> Result<()> {
        self.handle_archive_state(session_id, true)
    }

    /// 取消会话归档，使其重新出现在默认列表中。
    fn handle_unarchive(&self, session_id: &str) -> Result<()> {
        self.handle_archive_state(session_id, false)
    }

    /// 归档与取消归档共享的存储写入和输出逻辑。
    fn handle_archive_state(&self, session_id: &str, archived: bool) -> Result<()> {
        let updated_at = self.service.now_rfc3339()?;
        let changed = if archived {
            self.service.archive_session(session_id, &updated_at)?
        } else {
            self.service.unarchive_session(session_id, &updated_at)?
        };
        let operation = if archived { "archive" } else { "unarchive" };
        print_lifecycle_result(
            self.service.context(),
            operation,
            session_id,
            changed,
            &updated_at,
        )
    }

    /// 结束会话并记录调用方提供的原因。
    fn handle_finish(&self, session_id: &str, reason: &str) -> Result<()> {
        let reason = validate_reason(reason)?;
        let updated_at = self.service.now_rfc3339()?;
        let changed = self
            .service
            .finish_session(session_id, reason, &updated_at)?;
        let value = serde_json::json!({
            "operation": "finish",
            "session_id": session_id,
            "reason": reason,
            "changed": changed,
            "updated_at": updated_at,
        });
        print_output(
            self.service.context().format,
            &value,
            vec![format!("已结束会话: {session_id}")],
        )
    }

    /// 回退到一条 user 消息，将该消息及之后的活动消息软删除以保留审计历史。
    fn handle_rewind(&self, session_id: &str, message_id: &str) -> Result<()> {
        let message_id = parse_message_id(message_id)?;
        let updated_at = self.service.now_rfc3339()?;
        let result = self
            .service
            .rewind_session(session_id, message_id, &updated_at)?;
        let value = serde_json::json!({
            "operation": "rewind",
            "session_id": session_id,
            "target_message_id": result.target_message.id.get(),
            "rewound_count": result.rewound_count,
            "new_head_id": result.new_head_id.as_ref().map(MessageId::get),
            "updated_at": updated_at,
        });
        print_output(
            self.service.context().format,
            &value,
            vec![format!(
                "已回退会话: {session_id}（{} 条消息）",
                result.rewound_count
            )],
        )
    }

    /// 恢复由指定回退起点隐藏的消息；若已有新活动分支则拒绝合并。
    fn handle_restore(&self, session_id: &str, message_id: &str) -> Result<()> {
        let message_id = parse_message_id(message_id)?;
        let updated_at = self.service.now_rfc3339()?;
        let result = self
            .service
            .restore_session(session_id, message_id.clone(), &updated_at)?;
        let value = serde_json::json!({
            "operation": "restore",
            "session_id": session_id,
            "target_message_id": message_id.get(),
            "restored_count": result.restored_count,
            "new_head_id": result.new_head_id.as_ref().map(MessageId::get),
            "updated_at": updated_at,
        });
        print_output(
            self.service.context().format,
            &value,
            vec![format!(
                "已恢复会话分支: {session_id}（{} 条消息）",
                result.restored_count
            )],
        )
    }
}

impl CommandHandler for SessionHandler {
    /// 执行传入的顶层 Session 命令，拒绝错误的领域类型。
    fn execute(&mut self, command: Command) -> Result<()> {
        let Command::Session { command } = command else {
            anyhow::bail!("SessionHandler 收到非 session 命令");
        };
        self.execute_command(command)
    }
}

/// 将会话摘要转换为稳定的文本行。
fn render_list(sessions: &[SessionSummary]) -> Vec<String> {
    sessions
        .iter()
        .map(|session| {
            format!(
                "{}\t{}\t{}\t{}",
                session.id.as_str(),
                session.title.as_deref().unwrap_or("-"),
                session.message_count,
                session.last_active.as_deref().unwrap_or("-")
            )
        })
        .collect()
}

/// 将会话详情转换为稳定的文本行。
fn render_show(detail: &SessionDetail) -> Vec<String> {
    let mut lines = vec![
        format!("ID: {}", detail.session.id.as_str()),
        format!("标题: {}", detail.session.title.as_deref().unwrap_or("-")),
        format!("来源: {}", detail.session.source.as_deref().unwrap_or("-")),
        format!("模型: {}", detail.session.model.as_deref().unwrap_or("-")),
        format!(
            "开始时间: {}",
            detail.session.started_at.as_deref().unwrap_or("-")
        ),
        format!(
            "结束时间: {}",
            detail.session.ended_at.as_deref().unwrap_or("-")
        ),
        format!("消息数: {}", detail.session.message_count),
        "消息:".to_owned(),
    ];
    lines.extend(
        detail
            .messages
            .iter()
            .map(|message| format!("[{}] {}", message.role, message.content)),
    );
    lines
}

/// 将全文搜索命中转换为稳定的文本行。
fn render_search(hits: &[SearchHit]) -> Vec<String> {
    hits.iter()
        .map(|hit| {
            format!(
                "{}\t{}\t{:.6}\t{}",
                hit.session_id.as_str(),
                hit.message_id
                    .as_ref()
                    .expect("消息搜索命中必须包含消息 ID")
                    .get(),
                hit.rank.unwrap_or_default(),
                hit.snippet
            )
        })
        .collect()
}

/// 校验并裁剪会话标题，保证存储层收到有意义的值。
fn validate_title(title: &str) -> Result<&str> {
    let title = title.trim();
    if title.is_empty() {
        anyhow::bail!("会话标题不能为空");
    }
    if title.len() > 256 {
        anyhow::bail!("会话标题不能超过 256 个字节");
    }
    Ok(title)
}

/// 校验会话结束原因，避免写入空的生命周期说明。
fn validate_reason(reason: &str) -> Result<&str> {
    let reason = reason.trim();
    if reason.is_empty() {
        anyhow::bail!("会话结束原因不能为空");
    }
    Ok(reason)
}

/// 将 CLI 的文本参数转换为 SQLite 正整数消息主键。
fn parse_message_id(value: &str) -> Result<MessageId> {
    let message_id: i64 = value
        .parse()
        .with_context(|| format!("消息 ID 必须是正整数：{value}"))?;
    if message_id <= 0 {
        anyhow::bail!("消息 ID 必须是正整数：{value}");
    }
    Ok(MessageId::new(message_id))
}

/// 输出归档类生命周期操作的统一结果。
fn print_lifecycle_result(
    context: &CommandContext,
    operation: &str,
    session_id: &str,
    changed: bool,
    updated_at: &str,
) -> Result<()> {
    let value = serde_json::json!({
        "operation": operation,
        "session_id": session_id,
        "changed": changed,
        "updated_at": updated_at,
    });
    let action = if operation == "archive" {
        "已归档"
    } else {
        "已恢复"
    };
    print_output(
        context.format,
        &value,
        vec![format!("{action}会话: {session_id}")],
    )
}

#[cfg(test)]
mod tests {
    use std::{fs, time::UNIX_EPOCH};

    use sagent_config::Profile;
    use sagent_store::{NewMessage, NewSession, SqliteDatabase};
    use sagent_types::SessionId;

    use super::super::service::SessionService;
    use super::render_search;
    use crate::{
        commands::{CommandContext, profile::ProfileService},
        output::OutputFormat,
    };

    fn test_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("sagent-cli-session-{name}-{}", std::process::id()))
    }

    #[test]
    fn creates_and_lists_session_in_selected_profile() {
        let root = test_root("create");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");
        let mut profile = Profile::from_root(&root).expect("应能创建 profile 对象");
        let profile_path = ProfileService::create(profile.root_path().unwrap(), "coder")
            .expect("应能创建 profile");
        profile
            .register("coder", profile_path)
            .expect("应能更新 profile 索引");
        profile.select("coder").expect("应能选择 profile");
        let service = SessionService::new(CommandContext::new(
            Some(root.clone()),
            None,
            OutputFormat::Text,
        ));

        let id = service
            .create_session_with_id(
                SessionId::new("fixed-session"),
                Some("迁移讨论".to_owned()),
                Some("test-model".to_owned()),
                "2026-08-30T13:00:00.000Z".to_owned(),
            )
            .expect("应能创建会话");
        let sessions = service.list_sessions(20, 0, false).expect("应能列出会话");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        assert_eq!(sessions[0].title.as_deref(), Some("迁移讨论"));
        assert!(
            service
                .create_session_with_id(id, None, None, "2026-08-30T13:00:01.000Z".to_owned())
                .is_err()
        );
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn session_commands_use_configured_sqlite_path() {
        let root = test_root("custom-database");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");
        fs::write(
            root.join("config.yaml"),
            "storage:\n  kind: sqlite\n  path: data/custom.db\n",
        )
        .expect("应能写入自定义 SQLite 配置");
        let service = SessionService::new(CommandContext::new(
            Some(root.clone()),
            None,
            OutputFormat::Text,
        ));

        let id = service
            .create_session_with_id(
                SessionId::new("custom-database-session"),
                None,
                None,
                "2026-08-30T13:30:00.000Z".to_owned(),
            )
            .expect("应能在自定义数据库中创建会话");
        let sessions = service
            .list_sessions(20, 0, false)
            .expect("应能读取自定义数据库");

        assert_eq!(sessions[0].id, id);
        assert!(root.join("data").join("custom.db").is_file());
        assert!(!root.join("state.db").exists());
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn readonly_session_list_does_not_create_missing_database() {
        let root = test_root("readonly-list");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");
        let service = SessionService::new(CommandContext::new(
            Some(root.clone()),
            None,
            OutputFormat::Text,
        ));

        // 会话列表是只读命令：缺失数据库应报告错误，而不是隐式创建或迁移它。
        assert!(service.list_sessions(20, 0, false).is_err());
        assert!(!root.join("state.db").exists());
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn show_hides_compaction_summary_and_search_hides_rewound_message() {
        let root = test_root("visibility");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");
        let service = SessionService::new(CommandContext::new(
            Some(root.clone()),
            None,
            OutputFormat::Text,
        ));
        let id = service
            .create_session_with_id(
                SessionId::new("visibility-session"),
                None,
                None,
                "2026-08-30T14:00:00.000Z".to_owned(),
            )
            .expect("应能创建会话");
        let database = root.join("state.db");
        let mut store = SqliteDatabase::open_readwrite(&database).expect("应能打开数据库");
        store
            .append_message(&NewMessage::new(
                id.clone(),
                "user",
                "保留内容",
                "2026-08-30T14:01:00.000Z",
            ))
            .unwrap();
        store
            .archive_and_compact(
                &id,
                &[NewMessage::compressed_summary(
                    id.clone(),
                    "assistant",
                    "内部摘要",
                    "2026-08-30T14:02:00.000Z",
                )],
                "2026-08-30T14:02:00.000Z",
            )
            .unwrap();
        let rewound = store
            .append_message(&NewMessage::new(
                id.clone(),
                "user",
                "rewound-message",
                "2026-08-30T14:03:00.000Z",
            ))
            .unwrap();
        store
            .rewind_to_message(&id, rewound, "2026-08-30T14:04:00.000Z")
            .unwrap();
        drop(store);

        let detail = service
            .show_session(id.as_str(), 50, 0)
            .expect("应能展示会话");
        assert!(
            detail
                .messages
                .iter()
                .any(|message| message.content == "保留内容")
        );
        assert!(
            detail
                .messages
                .iter()
                .all(|message| message.content != "内部摘要")
        );
        let search_id = SessionId::new("search-session");
        let mut store = SqliteDatabase::open_readwrite(&database).expect("应能重新打开数据库");
        store
            .create_session(&NewSession {
                id: search_id.clone(),
                source: Some("cli".to_owned()),
                model: None,
                title: None,
                started_at: "2026-08-30T14:05:00.000Z".to_owned(),
            })
            .unwrap();
        store
            .append_message(&NewMessage::new(
                search_id.clone(),
                "user",
                "searchable-kept-message",
                "2026-08-30T14:06:00.000Z",
            ))
            .unwrap();
        let rewound = store
            .append_message(&NewMessage::new(
                search_id.clone(),
                "user",
                "searchable-rewound-message",
                "2026-08-30T14:07:00.000Z",
            ))
            .unwrap();
        store
            .rewind_to_message(&search_id, rewound, "2026-08-30T14:08:00.000Z")
            .unwrap();
        drop(store);

        let hits = service
            .search_messages("searchable", 20, Some(search_id.as_str()))
            .expect("应能搜索");
        assert!(
            render_search(&hits)
                .iter()
                .any(|line| line.contains("kept"))
        );
        assert!(
            render_search(&hits)
                .iter()
                .all(|line| !line.contains("rewound"))
        );
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn clock_helpers_produce_timestamp_and_unique_ids() {
        assert_eq!(
            SessionService::rfc3339_now(UNIX_EPOCH).unwrap(),
            "1970-01-01T00:00:00.000Z"
        );
        let first = SessionService::session_id_from_clock(UNIX_EPOCH).unwrap();
        let second = SessionService::session_id_from_clock(UNIX_EPOCH).unwrap();
        assert_ne!(first, second);
        assert!(first.as_str().starts_with("19700101_000000_"));
        assert_eq!(first.as_str().len(), 16 + 32);
    }
}
