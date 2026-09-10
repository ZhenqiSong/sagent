//! CLI session 的只读查询和文本渲染。
//!
//! 读取路径始终以当前 Profile 解析出只读 Store；输出渲染与 Clap 分发分开，保证 JSON
//! 与文本模式共享同一个领域结果而不在 handler 中复制查询规则。

use std::path::Path;

use anyhow::{Context, Result};
use sagent_store::{MessageQuery, MessageSearchQuery, SessionListQuery, Store};
use sagent_types::{SearchHit, SessionDetail, SessionId, SessionSummary};

use super::current_paths;
/// 从当前 profile 读取会话列表，供文本和 JSON 输出共用。
pub fn list(
    home: Option<&Path>,
    profile_override: Option<&str>,
    limit: u32,
    offset: u32,
    include_archived: bool,
) -> Result<Vec<SessionSummary>> {
    let paths = current_paths(home, profile_override)?;
    let store = Store::open_readonly(&paths.state_db)
        .with_context(|| format!("打开当前 profile 数据库失败：{}", paths.state_db.display()))?;
    store.list_sessions_with(&SessionListQuery {
        include_archived,
        limit,
        offset,
        ..SessionListQuery::default()
    })
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
