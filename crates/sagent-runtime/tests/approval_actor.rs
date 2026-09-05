use std::time::Duration;

use sagent_agent::ApprovalDecision;
use sagent_runtime::{ApprovalError, ApprovalManager, ApprovalOutcome, ApprovalRequest};
use sagent_types::{SessionId, ToolCallId, TurnId};
use tokio_util::sync::CancellationToken;

fn request(session: &SessionId, turn: TurnId, policy_key: &str) -> ApprovalRequest {
    ApprovalRequest {
        approval_id: sagent_types::ApprovalId::new(),
        session_id: session.clone(),
        turn_id: turn,
        tool_call_id: ToolCallId::new(),
        tool_name: "terminal".into(),
        summary: "该 terminal 命令需要审批".into(),
        policy_key: policy_key.into(),
        expires_at: "2026-09-05T00:00:00Z".into(),
    }
}

#[tokio::test]
async fn duplicate_resolution_is_rejected_without_second_execution() {
    let session = SessionId::new("approval-session");
    let turn = TurnId::new();
    let mut manager = ApprovalManager::new(Duration::from_secs(1));
    let approval = request(&session, turn, "terminal:recursive_delete");
    let id = approval.approval_id;
    let waiter = manager.register(approval).expect("应能登记审批");

    manager
        .resolve(&session, &turn, id, ApprovalDecision::Once)
        .expect("第一次响应应成功");
    assert_eq!(
        waiter
            .wait(Duration::from_secs(1), CancellationToken::new())
            .await,
        ApprovalOutcome::Approved(ApprovalDecision::Once)
    );
    assert_eq!(
        manager.resolve(&session, &turn, id, ApprovalDecision::Always),
        Err(ApprovalError::AlreadyResolved)
    );
}

#[tokio::test]
async fn cancellation_invalidates_a_late_response() {
    let session = SessionId::new("approval-session");
    let turn = TurnId::new();
    let mut manager = ApprovalManager::default();
    let approval = request(&session, turn, "terminal:force_push");
    let id = approval.approval_id;
    let waiter = manager.register(approval).expect("应能登记审批");
    manager.cancel_for_turn(&turn);
    assert_eq!(
        waiter
            .wait(Duration::from_secs(1), CancellationToken::new())
            .await,
        ApprovalOutcome::Cancelled
    );
    assert_eq!(
        manager.resolve(&session, &turn, id, ApprovalDecision::Once),
        Err(ApprovalError::Cancelled)
    );
}
