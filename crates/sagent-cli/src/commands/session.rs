//! Session 创建、查询、展示和搜索命令的业务实现。
//!
//! 作者：SongZQ

use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use clap::Subcommand;
use sagent_config::{SagentPaths, normalize_profile_name, resolve_active_paths};
use sagent_store::{MessageQuery, MessageSearchQuery, NewSession, Store};
use sagent_types::{SearchHit, SessionDetail, SessionId, SessionSummary};
use uuid::Uuid;

use crate::{commands::CommandContext, output::print_output};

/// `session` 分组下的命令参数与处理器。
#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    Create {
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        model: Option<String>,
    },
    Show {
        session_id: String,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long, default_value_t = 0)]
        offset: u32,
    },
    Search {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long)]
        session_id: Option<String>,
    },
    List {
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long, default_value_t = 0)]
        offset: u32,
    },
}

impl SessionCommand {
    /// 执行 session 子命令并按用户选择的格式输出。
    pub fn execute(self, context: &CommandContext) -> Result<()> {
        match self {
            Self::Create { title, model } => {
                let session_id = create(
                    context.home.as_deref(),
                    context.profile.as_deref(),
                    title,
                    model,
                )?;
                let value = serde_json::json!({ "session_id": session_id.as_str() });
                print_output(
                    context.format,
                    &value,
                    vec![format!("已创建会话: {}", session_id.as_str())],
                )
            }
            Self::Show {
                session_id,
                limit,
                offset,
            } => {
                let detail = show(
                    context.home.as_deref(),
                    context.profile.as_deref(),
                    &session_id,
                    limit,
                    offset,
                )?;
                print_output(context.format, &detail, render_show(&detail))
            }
            Self::Search {
                query,
                limit,
                session_id,
            } => {
                let hits = search(
                    context.home.as_deref(),
                    context.profile.as_deref(),
                    &query,
                    limit,
                    session_id.as_deref(),
                )?;
                print_output(context.format, &hits, render_search(&hits))
            }
            Self::List { limit, offset } => {
                let sessions = list(
                    context.home.as_deref(),
                    context.profile.as_deref(),
                    limit,
                    offset,
                )?;
                print_output(context.format, &sessions, render_list(&sessions))
            }
        }
    }
}

/// 解析当前命令实际访问的 profile 路径。
fn current_paths(home: Option<&Path>, profile_override: Option<&str>) -> Result<SagentPaths> {
    let profile = profile_override.map(normalize_profile_name).transpose()?;
    resolve_active_paths(home, profile.as_ref())
}

/// 从当前 profile 读取会话列表，供文本和 JSON 输出共用。
pub fn list(
    home: Option<&Path>,
    profile_override: Option<&str>,
    limit: u32,
    offset: u32,
) -> Result<Vec<SessionSummary>> {
    let paths = current_paths(home, profile_override)?;
    let store = Store::open_readonly(&paths.state_db)
        .with_context(|| format!("打开当前 profile 数据库失败：{}", paths.state_db.display()))?;
    store.list_sessions(limit, offset)
}

/// 将已经读取的会话列表渲染为稳定文本行。
pub fn render_list(sessions: &[SessionSummary]) -> Vec<String> {
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

/// 从当前 profile 加载会话详情及其用户可见消息。
pub fn show(
    home: Option<&Path>,
    profile_override: Option<&str>,
    session_id: &str,
    limit: u32,
    offset: u32,
) -> Result<SessionDetail> {
    let paths = current_paths(home, profile_override)?;
    let store = Store::open_readonly(&paths.state_db)
        .with_context(|| format!("打开当前 profile 数据库失败：{}", paths.state_db.display()))?;
    let session_id = SessionId::new(session_id);
    let session = store
        .get_session(&session_id)?
        .with_context(|| format!("会话不存在：{}", session_id.as_str()))?;
    let messages = store.get_messages_for_display(
        &session_id,
        &MessageQuery {
            limit: Some(limit),
            offset,
            latest: true,
            ..MessageQuery::default()
        },
    )?;
    Ok(SessionDetail { session, messages })
}

/// 将会话详情渲染为稳定文本行。
pub fn render_show(detail: &SessionDetail) -> Vec<String> {
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

/// 在当前 profile 搜索消息。
///
/// 默认搜索活动消息和压缩归档消息，过滤用户已经回退的普通非活动分支。
pub fn search(
    home: Option<&Path>,
    profile_override: Option<&str>,
    query: &str,
    limit: u32,
    session_id: Option<&str>,
) -> Result<Vec<SearchHit>> {
    let paths = current_paths(home, profile_override)?;
    let store = Store::open_readonly(&paths.state_db)
        .with_context(|| format!("打开当前 profile 数据库失败：{}", paths.state_db.display()))?;
    let mut search = MessageSearchQuery::new(query);
    search.limit = limit;
    search.session_id = session_id.map(SessionId::new);
    store.search_messages(&search)
}

/// 将已经读取的搜索命中渲染为稳定文本行。
pub fn render_search(hits: &[SearchHit]) -> Vec<String> {
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

/// 创建会话并返回写入数据库的 ID。
pub fn create(
    home: Option<&Path>,
    profile_override: Option<&str>,
    title: Option<String>,
    model: Option<String>,
) -> Result<SessionId> {
    let now = SystemTime::now();
    let session_id = session_id_from_clock(now)?;
    let started_at = rfc3339_now(now)?;
    create_with_id(home, profile_override, session_id, title, model, started_at)
}

/// 使用调用方指定的 ID 与时间创建会话，供生产编排和确定性测试复用。
pub fn create_with_id(
    home: Option<&Path>,
    profile_override: Option<&str>,
    session_id: SessionId,
    title: Option<String>,
    model: Option<String>,
    started_at: String,
) -> Result<SessionId> {
    let paths = current_paths(home, profile_override)?;
    let mut store = Store::open_readwrite(&paths.state_db)
        .with_context(|| format!("打开当前 profile 数据库失败：{}", paths.state_db.display()))?;
    store.create_session(&NewSession {
        id: session_id.clone(),
        source: Some("cli".to_owned()),
        model,
        title,
        started_at,
    })?;
    Ok(session_id)
}

/// 生成与 Python Hermes 兼容的时间前缀，并使用完整 UUID v4 防止碰撞。
fn session_id_from_clock(now: SystemTime) -> Result<SessionId> {
    let timestamp = rfc3339_now(now)?;
    let (date, time_with_zone) = timestamp
        .split_once('T')
        .context("无法生成会话 ID 时间前缀")?;
    let time = time_with_zone
        .get(..8)
        .context("无法读取会话 ID 的时间部分")?;
    let prefix = format!("{}_{}", date.replace('-', ""), time.replace(':', ""));
    Ok(SessionId::new(format!(
        "{}_{}",
        prefix,
        Uuid::new_v4().simple()
    )))
}

/// 生成当前 UTC 的 RFC 3339 毫秒时间戳。
fn rfc3339_now(now: SystemTime) -> Result<String> {
    let duration = now
        .duration_since(UNIX_EPOCH)
        .context("系统时间早于 Unix epoch，无法创建会话")?;
    let seconds = i64::try_from(duration.as_secs()).context("系统时间超出可表示范围")?;
    let days = seconds.div_euclid(86_400);
    let seconds_in_day = seconds.rem_euclid(86_400);
    let (year, month, day) = utc_date_from_days(days);
    let hour = seconds_in_day / 3_600;
    let minute = seconds_in_day % 3_600 / 60;
    let second = seconds_in_day % 60;
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        duration.subsec_millis()
    ))
}

/// 把 Unix 秒数转换为 UTC 公历日期。
fn utc_date_from_days(days_since_unix_epoch: i64) -> (i64, u32, u32) {
    let days = days_since_unix_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    (year, month as u32, day as u32)
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
        let sessions = list(Some(&root), None, 20, 0).expect("应能列出会话");
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
