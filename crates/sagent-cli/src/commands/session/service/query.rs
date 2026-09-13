//! Session 只读查询服务。
//!
//! 查询把分页、可见性和全文搜索参数转换为存储端口请求；展示格式由上层 handler 负责。
//!
//! 作者：SongZQ

use anyhow::{Context, Result};
use sagent_store::{MessageQuery, MessageSearchQuery, SessionListQuery};
use sagent_types::{SearchHit, SessionDetail, SessionId, SessionSummary};

use super::SessionService;

impl SessionService {
    /// 读取当前 Profile 的会话列表。
    pub(crate) fn list_sessions(
        &self,
        limit: u32,
        offset: u32,
        include_archived: bool,
    ) -> Result<Vec<SessionSummary>> {
        let dependencies = self.storage()?.open_read()?;
        dependencies.query().list_sessions(&SessionListQuery {
            include_archived,
            limit,
            offset,
            ..SessionListQuery::default()
        })
    }

    /// 读取当前 Profile 的单个会话及其可见消息。
    pub(crate) fn show_session(
        &self,
        session_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<SessionDetail> {
        let dependencies = self.storage()?.open_read()?;
        let query = dependencies.query();
        let session_id = SessionId::new(session_id);
        let session = query
            .get_session(&session_id)?
            .with_context(|| format!("会话不存在：{}", session_id.as_str()))?;
        let messages = query.get_messages_for_display(
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

    /// 搜索当前 Profile 的可见消息。
    pub(crate) fn search_messages(
        &self,
        query: &str,
        limit: u32,
        session_id: Option<&str>,
    ) -> Result<Vec<SearchHit>> {
        let dependencies = self.storage()?.open_read()?;
        let mut search = MessageSearchQuery::new(query);
        search.limit = limit;
        search.session_id = session_id.map(SessionId::new);
        dependencies.search().search_messages(&search)
    }
}
