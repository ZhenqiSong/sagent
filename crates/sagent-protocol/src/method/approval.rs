//! `approval.*` 方法的协议 DTO。

use sagent_types::{ApprovalId, SessionId, TurnId};
use serde::{Deserialize, Serialize};

/// 用户提交的审批决定；Runtime 接线时显式转换为 agent 领域 enum。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionDto {
    Once,
    Session,
    Always,
    Deny,
}

/// `approval.respond` 参数。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRespondParams {
    /// 目标会话。
    pub session_id: SessionId,
    /// 审批所属 Turn。
    pub turn_id: TurnId,
    /// 待处理的审批请求。
    pub approval_id: ApprovalId,
    /// 用户选择的审批范围。
    pub decision: ApprovalDecisionDto,
}

/// `approval.respond` 的立即受理结果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRespondResult {
    /// 表示决定已进入 Runtime mailbox。
    pub status: ApprovalRespondStatus,
}

/// 实际工具执行和 Turn 结果仍由 event 宣布。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRespondStatus {
    Accepted,
}
