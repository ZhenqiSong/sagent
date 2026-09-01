//! 面向 JSON-RPC 的只读会话服务适配层。

use sagent_store::{MessageQuery, SessionListQuery, Store};
use sagent_types::{SessionId, SessionSummary, StoredMessage};

use crate::{
    ProtocolError, SessionDetailDto, SessionListParams, SessionListResult, SessionMessageDto,
    SessionResumeParams, SessionResumeResult, SessionSummaryDto,
};

/// 会话列表与快照读取的默认分页大小。
pub const DEFAULT_PAGE_LIMIT: u32 = 50;
/// 单次 RPC 请求允许读取的最大记录数。
pub const MAX_PAGE_LIMIT: u32 = 200;

/// `session.*` 的只读服务接口，便于 dispatch 使用 fake 实现进行测试。
pub trait SessionReadService {
    /// 按归档过滤和分页读取会话摘要。
    fn list_sessions(&self, params: &SessionListParams)
    -> Result<SessionListResult, ProtocolError>;

    /// 读取一个会话及其默认可见消息快照。
    fn resume_session(
        &self,
        params: &SessionResumeParams,
    ) -> Result<SessionResumeResult, ProtocolError>;
}

/// 绑定一个只读 Store 的会话服务。
#[derive(Debug)]
pub struct SessionService {
    store: Store,
}

impl SessionService {
    /// 用已经打开的只读 Store 创建服务。
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    /// 取得底层只读 Store，供进程启动阶段执行连接检查。
    pub fn store(&self) -> &Store {
        &self.store
    }
}

impl SessionReadService for SessionService {
    fn list_sessions(
        &self,
        params: &SessionListParams,
    ) -> Result<SessionListResult, ProtocolError> {
        let limit = checked_limit(params.limit)?;
        let sessions = self
            .store
            .list_sessions_with(&SessionListQuery {
                include_archived: params.include_archived,
                limit,
                offset: params.offset,
                ..SessionListQuery::default()
            })
            .map_err(store_error)?;

        Ok(SessionListResult {
            sessions: sessions.into_iter().map(SessionSummaryDto::from).collect(),
            limit,
            offset: params.offset,
        })
    }

    fn resume_session(
        &self,
        params: &SessionResumeParams,
    ) -> Result<SessionResumeResult, ProtocolError> {
        if params.session_id.trim().is_empty() {
            return Err(ProtocolError::InvalidParams(
                "session_id must not be empty".to_owned(),
            ));
        }
        let limit = checked_limit(params.message_limit)?;
        let session_id = SessionId::new(params.session_id.clone());
        let summary = self
            .store
            .get_session(&session_id)
            .map_err(store_error)?
            .ok_or_else(|| ProtocolError::SessionNotFound(params.session_id.clone()))?;
        let messages = self
            .store
            .get_messages_for_display(
                &session_id,
                &MessageQuery {
                    limit: Some(limit),
                    offset: params.message_offset,
                    ..MessageQuery::default()
                },
            )
            .map_err(store_error)?;

        Ok(SessionResumeResult {
            detail: SessionDetailDto {
                session: summary.into(),
                messages: messages.into_iter().map(SessionMessageDto::from).collect(),
            },
            message_limit: limit,
            message_offset: params.message_offset,
        })
    }
}

fn store_error(error: anyhow::Error) -> ProtocolError {
    ProtocolError::Internal(error.to_string())
}

fn checked_limit(limit: Option<u32>) -> Result<u32, ProtocolError> {
    let limit = limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    if limit == 0 || limit > MAX_PAGE_LIMIT {
        return Err(ProtocolError::InvalidParams(format!(
            "limit must be between 1 and {MAX_PAGE_LIMIT}"
        )));
    }
    Ok(limit)
}

impl From<SessionSummary> for SessionSummaryDto {
    fn from(value: SessionSummary) -> Self {
        Self {
            id: value.id.as_str().to_owned(),
            source: value.source,
            model: value.model,
            title: value.title,
            started_at: value.started_at,
            ended_at: value.ended_at,
            end_reason: value.end_reason,
            last_active: value.last_active,
            preview: value.preview,
            message_count: value.message_count,
        }
    }
}

impl From<StoredMessage> for SessionMessageDto {
    fn from(value: StoredMessage) -> Self {
        Self {
            id: value.id.get(),
            session_id: value.session_id.as_str().to_owned(),
            role: value.role,
            content: value.content,
            timestamp: value.timestamp,
            tool_call_id: value.tool_call_id,
            tool_name: value.tool_name,
            tool_calls: value.tool_calls,
            reasoning: value.reasoning,
            finish_reason: value.finish_reason,
            display_kind: value.display_kind,
            display_metadata: value.display_metadata,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use sagent_store::{NewMessage, NewSession};
    use sagent_types::SessionId;

    use super::{DEFAULT_PAGE_LIMIT, SessionReadService, SessionService};
    use crate::{ProtocolError, SessionListParams, SessionResumeParams};

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sagent-protocol-service-{name}-{}.db",
            std::process::id()
        ))
    }

    fn remove(path: &std::path::Path) {
        let _ = fs::remove_file(path);
    }

    fn create_store(path: &std::path::Path) {
        let session_id = SessionId::new("session-1");
        let mut store = sagent_store::Store::open_readwrite(path).expect("应能创建测试数据库");
        store
            .create_session(&NewSession {
                id: session_id.clone(),
                source: Some("tui".to_owned()),
                model: Some("test-model".to_owned()),
                title: Some("协议测试".to_owned()),
                started_at: "2026-08-30T10:00:00Z".to_owned(),
            })
            .expect("应能创建会话");
        store
            .append_message(&NewMessage::new(
                session_id.clone(),
                "user",
                "可见提问",
                "2026-08-30T10:01:00Z",
            ))
            .expect("应能追加用户消息");
        store
            .append_message(&NewMessage::new(
                session_id,
                "assistant",
                "可见回答",
                "2026-08-30T10:02:00Z",
            ))
            .expect("应能追加回答消息");
    }

    #[test]
    fn lists_sessions_with_default_limit_and_archived_filter() {
        let path = test_path("list");
        remove(&path);
        create_store(&path);
        let store = sagent_store::Store::open_readonly(&path).expect("应能只读打开数据库");
        let service = SessionService::new(store);

        let result = service
            .list_sessions(&SessionListParams::default())
            .expect("应能读取会话列表");
        assert_eq!(result.limit, DEFAULT_PAGE_LIMIT);
        assert_eq!(result.sessions.len(), 1);
        assert_eq!(result.sessions[0].id, "session-1");
        assert_eq!(result.sessions[0].message_count, 2);
        remove(&path);
    }

    #[test]
    fn resumes_visible_messages_and_maps_missing_session() {
        let path = test_path("resume");
        remove(&path);
        create_store(&path);
        let store = sagent_store::Store::open_readonly(&path).expect("应能只读打开数据库");
        let service = SessionService::new(store);

        let result = service
            .resume_session(&SessionResumeParams {
                session_id: "session-1".to_owned(),
                message_limit: Some(1),
                message_offset: 1,
            })
            .expect("应能恢复会话快照");
        assert_eq!(result.detail.session.id, "session-1");
        assert_eq!(result.detail.messages.len(), 1);
        assert_eq!(result.detail.messages[0].content, "可见回答");

        let error = service
            .resume_session(&SessionResumeParams {
                session_id: "missing".to_owned(),
                message_limit: None,
                message_offset: 0,
            })
            .expect_err("不存在的会话必须失败");
        assert!(matches!(error, ProtocolError::SessionNotFound(id) if id == "missing"));
        remove(&path);
    }

    #[test]
    fn rejects_zero_and_oversized_page_limits() {
        let path = test_path("limits");
        remove(&path);
        create_store(&path);
        let store = sagent_store::Store::open_readonly(&path).expect("应能只读打开数据库");
        let service = SessionService::new(store);

        for limit in [Some(0), Some(super::MAX_PAGE_LIMIT + 1)] {
            let error = service
                .list_sessions(&SessionListParams {
                    include_archived: false,
                    limit,
                    offset: 0,
                })
                .expect_err("非法分页大小必须失败");
            assert!(matches!(error, ProtocolError::InvalidParams(_)));
        }
        remove(&path);
    }
}
