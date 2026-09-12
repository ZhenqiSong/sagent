//! 受 Profile 范围约束的会话全文搜索工具。

use std::{path::PathBuf, sync::Arc};

use sagent_store::{MessageSearchQuery, Store};
use sagent_types::SessionId;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{TRUNCATION_MARKER, ToolResult};

const TOOL_NAME: &str = "session_search";
const DEFAULT_LIMIT: u32 = 20;
const DEFAULT_MAX_RESULTS: u32 = 50;
const DEFAULT_MAX_SNIPPET_CHARS: usize = 500;

/// `session_search` 的模型输入。
#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSearchRequest {
    /// FTS5 查询字符串；空白查询会被拒绝。
    pub query: String,
    /// 最多返回的结果数；超出服务上限时收敛到上限。
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// 可选的当前会话范围；省略时仍限制在当前 Profile 数据库。
    #[serde(default)]
    pub session_id: Option<SessionId>,
}

impl SessionSearchRequest {
    /// 创建使用默认结果数的会话搜索请求。
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            limit: DEFAULT_LIMIT,
            session_id: None,
        }
    }
}

fn default_limit() -> u32 {
    DEFAULT_LIMIT
}

/// 搜索工具的稳定输出约束。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SessionSearchLimits {
    /// 服务允许返回的最大命中数。
    pub max_results: u32,
    /// 单个 snippet 的最大字符数。
    pub max_snippet_chars: usize,
    /// 结果 JSON 摘要的最大字符数。
    pub max_output_chars: usize,
}

impl Default for SessionSearchLimits {
    fn default() -> Self {
        Self {
            max_results: DEFAULT_MAX_RESULTS,
            max_snippet_chars: DEFAULT_MAX_SNIPPET_CHARS,
            max_output_chars: 16 * 1024,
        }
    }
}

/// 绑定单个 Profile state.db 的只读搜索服务。
///
/// 服务只保存绝对数据库路径，不保存可跨调用共享的 SQLite Connection；每次搜索短暂
/// 打开只读 Store，使不同 Actor/连接不会共享非线程安全连接，也不能通过参数切换 Profile。
#[derive(Debug, Clone)]
pub struct SessionSearchService {
    state_db: Arc<PathBuf>,
    limits: SessionSearchLimits,
}

impl SessionSearchService {
    /// 创建 Profile 固定的会话搜索服务。
    pub fn new(state_db: impl Into<PathBuf>, limits: SessionSearchLimits) -> Self {
        Self {
            state_db: Arc::new(state_db.into()),
            limits,
        }
    }

    /// 执行只读 FTS 查询；不会写数据库、执行 shell 或读取任意文件。
    pub async fn search(
        &self,
        tool_call_id: sagent_types::ToolCallId,
        request: SessionSearchRequest,
        cancellation: CancellationToken,
    ) -> ToolResult {
        if cancellation.is_cancelled() {
            return failure(tool_call_id, "cancelled", "会话搜索已取消", &self.limits);
        }
        if request.query.trim().is_empty() {
            return failure(
                tool_call_id,
                "invalid_query",
                "搜索词不能为空",
                &self.limits,
            );
        }
        let limit = request.limit.max(1).min(self.limits.max_results.max(1));
        let query = MessageSearchQuery {
            query: request.query,
            session_id: request.session_id,
            include_inactive: false,
            limit,
        };
        let path = (*self.state_db).clone();
        let cancellation_for_query = cancellation.clone();
        let query_task = tokio::task::spawn_blocking(move || {
            if cancellation_for_query.is_cancelled() {
                return Err(SearchError::Cancelled);
            }
            let store = Store::open_readonly(&path).map_err(|_| SearchError::Store)?;
            store
                .search_messages(&query)
                .map_err(|_| SearchError::Store)
        });
        let hits = tokio::select! {
            _ = cancellation.cancelled() => return failure(tool_call_id, "cancelled", "会话搜索已取消", &self.limits),
            result = query_task => match result {
                Ok(Ok(hits)) => hits,
                Ok(Err(SearchError::Cancelled)) => return failure(tool_call_id, "cancelled", "会话搜索已取消", &self.limits),
                Ok(Err(SearchError::Store)) | Err(_) => return failure(tool_call_id, "search_unavailable", "会话搜索暂不可用", &self.limits),
            },
        };

        let output = SearchOutput {
            results: hits
                .into_iter()
                .map(|hit| SearchResult {
                    session_id: hit.session_id,
                    message_id: hit.message_id,
                    snippet: bound_snippet(hit.snippet, self.limits.max_snippet_chars),
                })
                .collect(),
        };
        let content = match serde_json::to_string(&output) {
            Ok(content) => content,
            Err(_) => {
                return failure(
                    tool_call_id,
                    "search_failed",
                    "无法格式化搜索结果",
                    &self.limits,
                );
            }
        };
        ToolResult::success(
            tool_call_id,
            TOOL_NAME,
            content,
            self.limits.max_output_chars.max(1),
            None,
        )
    }
}

enum SearchError {
    Cancelled,
    Store,
}

#[derive(Serialize)]
struct SearchOutput {
    results: Vec<SearchResult>,
}

#[derive(Serialize)]
struct SearchResult {
    session_id: SessionId,
    message_id: Option<sagent_types::MessageId>,
    snippet: String,
}

fn bound_snippet(mut snippet: String, max_chars: usize) -> String {
    let max_chars = max_chars.max(1);
    if snippet.chars().count() <= max_chars {
        return snippet;
    }
    let marker_len = TRUNCATION_MARKER.chars().count();
    if max_chars <= marker_len {
        return TRUNCATION_MARKER.chars().take(max_chars).collect();
    }
    snippet.truncate(
        snippet
            .char_indices()
            .nth(max_chars - marker_len)
            .map_or(snippet.len(), |(index, _)| index),
    );
    format!("{snippet}{TRUNCATION_MARKER}")
}

fn failure(
    tool_call_id: sagent_types::ToolCallId,
    kind: &str,
    message: &str,
    limits: &SessionSearchLimits,
) -> ToolResult {
    ToolResult::failure(
        tool_call_id,
        TOOL_NAME,
        kind,
        message,
        limits.max_output_chars.max(1),
        None,
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sagent_store::{NewMessage, NewSession, Store};
    use sagent_types::{MessageId, SessionId, ToolCallId};
    use tokio_util::sync::CancellationToken;

    use super::{SessionSearchLimits, SessionSearchRequest, SessionSearchService};

    fn fixture() -> (std::path::PathBuf, SessionId) {
        let path = std::env::temp_dir().join(format!(
            "sagent-session-search-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let session = SessionId::new("search-session");
        let mut store = Store::open_readwrite(&path).unwrap();
        store
            .create_session(&NewSession {
                id: session.clone(),
                source: None,
                model: None,
                title: None,
                started_at: "2026-09-10T00:00:00Z".into(),
            })
            .unwrap();
        store
            .append_message(&NewMessage::new(
                session.clone(),
                "user",
                "中文消息与 emoji 🚀",
                "2026-09-10T00:00:01Z",
            ))
            .unwrap();
        store
            .append_message(&NewMessage::new(
                session.clone(),
                "assistant",
                "Rust FTS 搜索",
                "2026-09-10T00:00:02Z",
            ))
            .unwrap();
        (path, session)
    }

    #[tokio::test]
    async fn searches_cjk_with_stable_ids_and_bounded_snippet() {
        let (path, session) = fixture();
        let service = SessionSearchService::new(
            &path,
            SessionSearchLimits {
                max_snippet_chars: 6,
                ..Default::default()
            },
        );
        let mut request = SessionSearchRequest::new("中文消息");
        request.session_id = Some(session.clone());
        let result = service
            .search(ToolCallId::new(), request, CancellationToken::new())
            .await;
        assert!(result.ok);
        assert!(result.content.contains("search-session"));
        assert!(result.content.contains("message_id"));
        assert!(result.content.chars().count() < 16 * 1024);
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn rejects_empty_and_pre_cancelled_queries_without_store_side_effects() {
        let (path, _) = fixture();
        let service = SessionSearchService::new(&path, Default::default());
        let empty = service
            .search(
                ToolCallId::new(),
                SessionSearchRequest::new("  "),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(empty.error_kind.as_deref(), Some("invalid_query"));
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = service
            .search(
                ToolCallId::new(),
                SessionSearchRequest::new("Rust"),
                cancellation,
            )
            .await;
        assert_eq!(cancelled.error_kind.as_deref(), Some("cancelled"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn search_result_types_keep_strong_message_references() {
        let id = MessageId::new(7);
        assert_eq!(serde_json::to_value(id).unwrap(), 7);
    }
}
