//! SessionActor 内部的审批状态机。
//!
//! 审批状态属于单个会话运行时，不使用进程级全局队列。`ApprovalManager` 由
//! `SessionActor` 独占，外部只能通过 Actor mailbox 提交 `ResolveApproval`。

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use sagent_agent::ApprovalDecision;
use sagent_types::{ApprovalId, SessionId, ToolCallId, TurnId};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::oneshot;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

/// 展示给客户端的待审批请求。
///
/// `summary` 必须是脱敏后的文本，不能携带完整环境变量、API key 或密码。
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub approval_id: ApprovalId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub tool_call_id: ToolCallId,
    pub tool_name: String,
    pub summary: String,
    pub policy_key: String,
    pub expires_at: String,
}

impl ApprovalRequest {
    /// 构造审批请求；空的工具名、策略名和摘要会被拒绝，避免生成不可操作的 UI 卡片。
    pub fn new(
        session_id: SessionId,
        turn_id: TurnId,
        tool_call_id: ToolCallId,
        tool_name: impl Into<String>,
        summary: impl Into<String>,
        policy_key: impl Into<String>,
        expires_at: impl Into<String>,
    ) -> Result<Self, ApprovalError> {
        let request = Self {
            approval_id: ApprovalId::new(),
            session_id,
            turn_id,
            tool_call_id,
            tool_name: tool_name.into(),
            summary: summary.into(),
            policy_key: policy_key.into(),
            expires_at: expires_at.into(),
        };
        if request.tool_name.trim().is_empty()
            || request.summary.trim().is_empty()
            || request.policy_key.trim().is_empty()
        {
            return Err(ApprovalError::InvalidRequest);
        }
        Ok(request)
    }
}

/// 审批等待结束的原因。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ApprovalOutcome {
    Approved(ApprovalDecision),
    Denied,
    TimedOut,
    Cancelled,
}

/// 审批管理失败的稳定错误分类。
#[derive(Debug, Clone, Eq, Error, PartialEq)]
pub enum ApprovalError {
    #[error("approval request is invalid")]
    InvalidRequest,
    #[error("approval request already exists")]
    AlreadyPending,
    #[error("approval request was not found")]
    NotFound,
    #[error("approval request belongs to another session")]
    SessionMismatch,
    #[error("approval request belongs to another turn")]
    TurnMismatch,
    #[error("approval request has already been resolved")]
    AlreadyResolved,
    #[error("approval request has expired")]
    Expired,
    #[error("approval request was cancelled")]
    Cancelled,
}

#[derive(Debug)]
struct PendingApproval {
    request: ApprovalRequest,
    responder: oneshot::Sender<ApprovalOutcome>,
    deadline: Instant,
}

/// 可取消、带超时的审批等待句柄。
pub struct ApprovalWaiter {
    approval_id: ApprovalId,
    receiver: oneshot::Receiver<ApprovalOutcome>,
}

impl ApprovalWaiter {
    pub fn approval_id(&self) -> ApprovalId {
        self.approval_id
    }

    /// 等待用户决定、超时或当前 Turn 被取消。
    pub async fn wait(
        self,
        timeout_duration: Duration,
        cancellation: CancellationToken,
    ) -> ApprovalOutcome {
        let mut receiver = self.receiver;
        // 不在这里删除 Manager 中的 pending 记录：Actor 需要先收到 outcome，
        // 才能以同一串行顺序持久化 timeout/cancel 的终态事实。
        tokio::select! {
            result = &mut receiver => result.unwrap_or(ApprovalOutcome::Cancelled),
            _ = cancellation.cancelled() => ApprovalOutcome::Cancelled,
            _ = sleep(timeout_duration) => ApprovalOutcome::TimedOut,
        }
    }
}

/// 单个 SessionActor 独占的审批管理器。
#[derive(Debug)]
pub struct ApprovalManager {
    pending: HashMap<ApprovalId, PendingApproval>,
    session_rules: HashMap<SessionId, HashSet<String>>,
    permanent_rules: HashSet<String>,
    finished: HashMap<ApprovalId, ApprovalError>,
    timeout: Duration,
}

impl ApprovalManager {
    pub fn new(timeout: Duration) -> Self {
        Self {
            pending: HashMap::new(),
            session_rules: HashMap::new(),
            permanent_rules: HashSet::new(),
            finished: HashMap::new(),
            timeout,
        }
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// 判断 Session 或永久策略是否已经允许该 policy key。
    pub fn is_allowed(&self, session_id: &SessionId, policy_key: &str) -> bool {
        self.permanent_rules.contains(policy_key)
            || self
                .session_rules
                .get(session_id)
                .is_some_and(|rules| rules.contains(policy_key))
    }

    /// 登记一个 pending 请求并返回非阻塞 waiter。
    pub fn register(&mut self, request: ApprovalRequest) -> Result<ApprovalWaiter, ApprovalError> {
        if self.pending.contains_key(&request.approval_id) {
            return Err(ApprovalError::AlreadyPending);
        }
        if let Some(error) = self.finished.get(&request.approval_id) {
            return Err(error.clone());
        }
        let (sender, receiver) = oneshot::channel();
        let approval_id = request.approval_id;
        self.pending.insert(
            approval_id,
            PendingApproval {
                request,
                responder: sender,
                deadline: Instant::now() + self.timeout,
            },
        );
        Ok(ApprovalWaiter {
            approval_id,
            receiver,
        })
    }

    /// 处理用户决定。校验在 Manager 内完成，防止错误 Session/Turn 越权。
    pub fn resolve(
        &mut self,
        session_id: &SessionId,
        turn_id: &TurnId,
        approval_id: ApprovalId,
        decision: ApprovalDecision,
    ) -> Result<(), ApprovalError> {
        // 先临时移出 map，令同一 approval 不能被重复 resolve；若归属校验失败
        // 再原样插回，避免错误客户端把真正等待者的请求消费掉。
        let Some(pending) = self.pending.remove(&approval_id) else {
            return Err(self
                .finished
                .get(&approval_id)
                .cloned()
                .unwrap_or(ApprovalError::NotFound));
        };
        if Instant::now() >= pending.deadline {
            self.finished.insert(approval_id, ApprovalError::Expired);
            let _ = pending.responder.send(ApprovalOutcome::TimedOut);
            return Err(ApprovalError::Expired);
        }
        if pending.request.session_id != *session_id {
            self.pending.insert(approval_id, pending);
            return Err(ApprovalError::SessionMismatch);
        }
        if pending.request.turn_id != *turn_id {
            self.pending.insert(approval_id, pending);
            return Err(ApprovalError::TurnMismatch);
        }

        let outcome = match decision {
            ApprovalDecision::Once => ApprovalOutcome::Approved(ApprovalDecision::Once),
            ApprovalDecision::Session => {
                self.session_rules
                    .entry(session_id.clone())
                    .or_default()
                    .insert(pending.request.policy_key.clone());
                ApprovalOutcome::Approved(ApprovalDecision::Session)
            }
            ApprovalDecision::Always => {
                // Always 的作用域是当前 Runtime 进程；持久化为用户偏好要经过
                // 单独的配置/审计流程，不能由一次 RPC 决定直接写入长期配置。
                self.permanent_rules
                    .insert(pending.request.policy_key.clone());
                ApprovalOutcome::Approved(ApprovalDecision::Always)
            }
            ApprovalDecision::Deny => ApprovalOutcome::Denied,
        };
        let _ = pending.responder.send(outcome);
        self.finished
            .insert(approval_id, ApprovalError::AlreadyResolved);
        Ok(())
    }

    /// 取消一个 Turn 的全部 pending 请求；迟到的 resolve 将返回 NotFound。
    pub fn cancel_for_turn(&mut self, turn_id: &TurnId) {
        for (_, pending) in self
            .pending
            .extract_if(|_, pending| pending.request.turn_id == *turn_id)
        {
            self.finished
                .insert(pending.request.approval_id, ApprovalError::Cancelled);
            let _ = pending.responder.send(ApprovalOutcome::Cancelled);
        }
    }

    /// 清理 waiter 已经观察到的超时；后续 resolve 会稳定返回 NotFound。
    pub fn expire(&mut self, approval_id: ApprovalId) {
        if let Some(pending) = self.pending.remove(&approval_id) {
            self.finished.insert(approval_id, ApprovalError::Expired);
            let _ = pending.responder.send(ApprovalOutcome::TimedOut);
        }
    }

    pub fn pending(&self) -> Vec<ApprovalRequest> {
        self.pending
            .values()
            .map(|pending| pending.request.clone())
            .collect()
    }
}

impl Default for ApprovalManager {
    fn default() -> Self {
        Self::new(Duration::from_secs(300))
    }
}

#[cfg(test)]
mod tests {
    use super::{ApprovalManager, ApprovalOutcome, ApprovalRequest};
    use sagent_agent::ApprovalDecision;
    use sagent_types::{SessionId, ToolCallId, TurnId};
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    fn request(session: &str, turn: TurnId, policy: &str) -> ApprovalRequest {
        ApprovalRequest::new(
            SessionId::new(session),
            turn,
            ToolCallId::new(),
            "terminal",
            "该命令需要审批",
            policy,
            "2026-09-05T00:00:00Z",
        )
        .unwrap()
    }

    #[tokio::test]
    async fn once_resolves_only_current_waiter() {
        let session = SessionId::new("session-1");
        let turn = TurnId::new();
        let mut manager = ApprovalManager::new(Duration::from_secs(1));
        let approval = request("session-1", turn, "terminal:recursive_delete");
        let id = approval.approval_id;
        let waiter = manager.register(approval).unwrap();
        manager
            .resolve(&session, &turn, id, ApprovalDecision::Once)
            .unwrap();
        assert_eq!(
            waiter
                .wait(Duration::from_secs(1), CancellationToken::new())
                .await,
            ApprovalOutcome::Approved(ApprovalDecision::Once)
        );
        assert!(!manager.is_allowed(&session, "terminal:recursive_delete"));
    }

    #[tokio::test]
    async fn session_and_always_create_their_expected_scopes() {
        let session = SessionId::new("session-1");
        let turn = TurnId::new();
        let mut manager = ApprovalManager::default();
        let first = request("session-1", turn, "terminal:recursive_delete");
        let waiter = manager.register(first.clone()).unwrap();
        manager
            .resolve(
                &session,
                &turn,
                first.approval_id,
                ApprovalDecision::Session,
            )
            .unwrap();
        let _ = waiter
            .wait(Duration::from_secs(1), CancellationToken::new())
            .await;
        assert!(manager.is_allowed(&session, "terminal:recursive_delete"));
        assert!(!manager.is_allowed(&SessionId::new("session-2"), "terminal:recursive_delete"));

        let second = request("session-1", turn, "terminal:force_push");
        let waiter = manager.register(second.clone()).unwrap();
        manager
            .resolve(
                &session,
                &turn,
                second.approval_id,
                ApprovalDecision::Always,
            )
            .unwrap();
        let _ = waiter
            .wait(Duration::from_secs(1), CancellationToken::new())
            .await;
        assert!(manager.is_allowed(&SessionId::new("session-2"), "terminal:force_push"));
    }

    #[tokio::test]
    async fn timeout_and_cancel_remove_pending_requests() {
        let session = SessionId::new("session-1");
        let turn = TurnId::new();
        let mut manager = ApprovalManager::new(Duration::from_millis(20));
        let first = request("session-1", turn, "terminal:recursive_delete");
        let waiter = manager.register(first).unwrap();
        assert_eq!(
            waiter
                .wait(Duration::from_millis(1), CancellationToken::new())
                .await,
            ApprovalOutcome::TimedOut
        );
        // waiter timeout 后 Manager 仍需由 Actor 的 timeout 事件清理；重复 resolve 不得执行。
        let second = request("session-1", turn, "terminal:force_push");
        let id = second.approval_id;
        let waiter = manager.register(second).unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert_eq!(
            waiter.wait(Duration::from_secs(1), cancellation).await,
            ApprovalOutcome::Cancelled
        );
        manager.cancel_for_turn(&turn);
        assert_eq!(
            manager.resolve(&session, &turn, id, ApprovalDecision::Deny),
            Err(super::ApprovalError::Cancelled)
        );
    }

    #[test]
    fn mismatched_session_does_not_consume_pending_request() {
        let turn = TurnId::new();
        let mut manager = ApprovalManager::default();
        let approval = request("session-1", turn, "terminal:recursive_delete");
        let id = approval.approval_id;
        let _waiter = manager.register(approval).unwrap();
        let error = manager
            .resolve(
                &SessionId::new("session-2"),
                &turn,
                id,
                ApprovalDecision::Once,
            )
            .unwrap_err();
        assert_eq!(error, super::ApprovalError::SessionMismatch);
        assert_eq!(manager.pending().len(), 1);
    }
}
