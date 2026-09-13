//! Session 创建、查询、展示和搜索命令的业务实现。
//!
//! 作者：SongZQ

use std::time::SystemTime;

use anyhow::{Context, Result};
use clap::Subcommand;
use sagent_store::SessionStorage;
use sagent_types::MessageId;

use crate::{commands::CommandContext, output::print_output};

// 只读查询与创建各自拥有不同的 I/O 生命周期；物理拆分避免参数分发文件同时承载
// SQL 查询、日期换算和输出格式，但仍由本模块统一维护 `session` 命令的公开边界。
mod creation;
mod lifecycle;
mod query;

#[cfg(test)]
use creation::session_id_from_clock;
#[allow(unused_imports)]
pub use creation::{create, create_with_id};
use creation::{create_with_storage, rfc3339_now};
use lifecycle::{
    handle_archive, handle_finish, handle_rename, handle_restore, handle_rewind, handle_unarchive,
};
#[allow(unused_imports)]
pub use query::{list, render_list, render_search, render_show, search, show};
use query::{list_with_storage, search_with_storage, show_with_storage};

/// `session` 分组下的命令参数与处理器。
#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    /// 创建新的空会话。
    Create {
        /// 可选的人类可读标题。
        #[arg(long)]
        title: Option<String>,
        /// 可选的模型标识。
        #[arg(long)]
        model: Option<String>,
    },
    /// 按 ID 查看可见消息。
    Show {
        /// 目标会话 ID。
        session_id: String,
        /// 返回消息条数。
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long, default_value_t = 0)]
        offset: u32,
    },
    /// 在 FTS5 索引中搜索消息。
    Search {
        /// 搜索表达式。
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// 列出会话摘要。
    List {
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long, default_value_t = 0)]
        offset: u32,
        /// 同时列出已归档会话；隐藏会话仍不会显示。
        #[arg(long)]
        include_archived: bool,
    },
    /// 更新会话标题。
    Rename { session_id: String, title: String },
    /// 将会话归档。
    Archive { session_id: String },
    /// 取消会话归档。
    Unarchive { session_id: String },
    /// 设置会话结束原因并结束会话。
    Finish {
        session_id: String,
        #[arg(long)]
        reason: String,
    },
    /// 将当前可见消息回退到指定检查点。
    Rewind {
        session_id: String,
        message_id: String,
    },
    /// 从回退检查点恢复消息可见性。
    Restore {
        session_id: String,
        message_id: String,
    },
}

impl SessionCommand {
    /// 执行 session 子命令并按用户选择的格式输出。
    pub fn execute(self, context: &CommandContext) -> Result<()> {
        match self {
            Self::Create { title, model } => handle_create(context, title, model),
            Self::Show {
                session_id,
                limit,
                offset,
            } => handle_show(context, &session_id, limit, offset),
            Self::Search {
                query,
                limit,
                session_id,
            } => handle_search(context, &query, limit, session_id.as_deref()),
            Self::List {
                limit,
                offset,
                include_archived,
            } => handle_list(context, limit, offset, include_archived),
            Self::Rename { session_id, title } => handle_rename(context, &session_id, &title),
            Self::Archive { session_id } => handle_archive(context, &session_id),
            Self::Unarchive { session_id } => handle_unarchive(context, &session_id),
            Self::Finish { session_id, reason } => handle_finish(context, &session_id, &reason),
            Self::Rewind {
                session_id,
                message_id,
            } => handle_rewind(context, &session_id, &message_id),
            Self::Restore {
                session_id,
                message_id,
            } => handle_restore(context, &session_id, &message_id),
        }
    }
}

/// 创建会话并输出新 ID。
fn handle_create(
    context: &CommandContext,
    title: Option<String>,
    model: Option<String>,
) -> Result<()> {
    let session_id = create_with_storage(context.storage()?, title, model)?;
    let value = serde_json::json!({ "session_id": session_id.as_str() });
    print_output(
        context.format,
        &value,
        vec![format!("已创建会话: {}", session_id.as_str())],
    )
}

/// 读取并展示单个会话。
fn handle_show(context: &CommandContext, session_id: &str, limit: u32, offset: u32) -> Result<()> {
    let detail = show_with_storage(context.storage()?, session_id, limit, offset)?;
    print_output(context.format, &detail, render_show(&detail))
}

/// 搜索当前 profile 的消息。
fn handle_search(
    context: &CommandContext,
    query: &str,
    limit: u32,
    session_id: Option<&str>,
) -> Result<()> {
    let hits = search_with_storage(context.storage()?, query, limit, session_id)?;
    print_output(context.format, &hits, render_search(&hits))
}

/// 列出当前 profile 的会话。
fn handle_list(
    context: &CommandContext,
    limit: u32,
    offset: u32,
    include_archived: bool,
) -> Result<()> {
    let sessions = list_with_storage(context.storage()?, limit, offset, include_archived)?;
    print_output(context.format, &sessions, render_list(&sessions))
}

/// 为当前 Profile 申请可写领域端口；只供明确的生命周期命令使用。
fn with_writable_storage<T>(
    context: &CommandContext,
    operation: impl FnOnce(&mut dyn SessionStorage) -> Result<T>,
) -> Result<T> {
    let mut dependencies = context.storage()?.open_write()?;
    operation(dependencies.session_mut())
}

/// 生成生命周期写操作共用的 UTC 毫秒时间戳。
fn now_rfc3339() -> Result<String> {
    rfc3339_now(SystemTime::now())
}

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

    use sagent_store::{NewMessage, NewSession, Store};
    use sagent_types::SessionId;

    use crate::commands::profile;

    use super::{
        create_with_id, list, render_search, rfc3339_now, search, session_id_from_clock, show,
    };

    fn test_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("sagent-cli-session-{name}-{}", std::process::id()))
    }

    #[test]
    fn creates_and_lists_session_in_selected_profile() {
        let root = test_root("create");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");
        profile::create(Some(&root), "coder").expect("应能创建 profile");
        profile::select(Some(&root), "coder").expect("应能选择 profile");

        let id = create_with_id(
            Some(&root),
            None,
            SessionId::new("fixed-session"),
            Some("迁移讨论".to_owned()),
            Some("test-model".to_owned()),
            "2026-08-30T13:00:00.000Z".to_owned(),
        )
        .expect("应能创建会话");
        let sessions = list(Some(&root), None, 20, 0, false).expect("应能列出会话");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        assert_eq!(sessions[0].title.as_deref(), Some("迁移讨论"));
        assert!(
            create_with_id(
                Some(&root),
                None,
                id,
                None,
                None,
                "2026-08-30T13:00:01.000Z".to_owned()
            )
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

        let id = create_with_id(
            Some(&root),
            None,
            SessionId::new("custom-database-session"),
            None,
            None,
            "2026-08-30T13:30:00.000Z".to_owned(),
        )
        .expect("应能在自定义数据库中创建会话");
        let sessions = list(Some(&root), None, 20, 0, false).expect("应能读取自定义数据库");

        assert_eq!(sessions[0].id, id);
        assert!(root.join("data").join("custom.db").is_file());
        assert!(!root.join("state.db").exists());
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn show_hides_compaction_summary_and_search_hides_rewound_message() {
        let root = test_root("visibility");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");
        let id = create_with_id(
            Some(&root),
            None,
            SessionId::new("visibility-session"),
            None,
            None,
            "2026-08-30T14:00:00.000Z".to_owned(),
        )
        .expect("应能创建会话");
        let database = root.join("state.db");
        let mut store = Store::open_readwrite(&database).expect("应能打开数据库");
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

        let detail = show(Some(&root), None, id.as_str(), 50, 0).expect("应能展示会话");
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
        let mut store = Store::open_readwrite(&database).expect("应能重新打开数据库");
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

        let hits = search(
            Some(&root),
            None,
            "searchable",
            20,
            Some(search_id.as_str()),
        )
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
        assert_eq!(rfc3339_now(UNIX_EPOCH).unwrap(), "1970-01-01T00:00:00.000Z");
        let first = session_id_from_clock(UNIX_EPOCH).unwrap();
        let second = session_id_from_clock(UNIX_EPOCH).unwrap();
        assert_ne!(first, second);
        assert!(first.as_str().starts_with("19700101_000000_"));
        assert_eq!(first.as_str().len(), 16 + 32);
    }
}
