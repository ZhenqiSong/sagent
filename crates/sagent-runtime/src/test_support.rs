//! runtime 单元测试使用的 fake worker。

use crate::input::{ActorInput, WorkerEvent};
use sagent_types::TurnId;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// fake worker 的可重复执行脚本。
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum FakeWorkerStep {
    /// 发送一个文本片段。
    Delta(&'static str),
    /// 发送最终文本。
    Final(&'static str),
    /// 发送可控失败。
    Fail(&'static str),
    /// 等待取消，用于制造 interrupt 测试窗口。
    WaitForCancel,
}

/// 按脚本向 Actor mailbox 发送事件。
pub(crate) async fn run_fake_worker(
    sender: mpsc::Sender<ActorInput>,
    turn_id: TurnId,
    cancellation: CancellationToken,
    steps: &[FakeWorkerStep],
) {
    for step in steps {
        if cancellation.is_cancelled() {
            let _ = sender
                .send(ActorInput::Worker(WorkerEvent::Cancelled { turn_id }))
                .await;
            return;
        }

        match step {
            FakeWorkerStep::Delta(text) => {
                let _ = sender
                    .send(ActorInput::Worker(WorkerEvent::TextDelta {
                        turn_id,
                        text: (*text).into(),
                    }))
                    .await;
            }
            FakeWorkerStep::Final(text) => {
                let _ = sender
                    .send(ActorInput::Worker(WorkerEvent::FinalText {
                        turn_id,
                        text: (*text).into(),
                    }))
                    .await;
            }
            FakeWorkerStep::Fail(reason) => {
                let _ = sender
                    .send(ActorInput::Worker(WorkerEvent::Failed {
                        turn_id,
                        reason: (*reason).into(),
                    }))
                    .await;
            }
            FakeWorkerStep::WaitForCancel => {
                cancellation.cancelled().await;
                let _ = sender
                    .send(ActorInput::Worker(WorkerEvent::Cancelled { turn_id }))
                    .await;
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FakeWorkerStep, run_fake_worker};
    use crate::input::{ActorInput, WorkerEvent};
    use sagent_types::TurnId;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn fake_worker_emits_delta_and_final_in_script_order() {
        let (sender, mut receiver) = mpsc::channel(8);
        let turn_id = TurnId::new();

        run_fake_worker(
            sender,
            turn_id,
            CancellationToken::new(),
            &[
                FakeWorkerStep::Delta("a"),
                FakeWorkerStep::Delta("b"),
                FakeWorkerStep::Final("ab"),
            ],
        )
        .await;

        let events = [
            receiver.recv().await,
            receiver.recv().await,
            receiver.recv().await,
        ];

        match events[0].as_ref() {
            Some(ActorInput::Worker(WorkerEvent::TextDelta { text, .. })) => {
                assert_eq!(text, "a")
            }
            _ => panic!("第一个事件应为文本片段"),
        }
        match events[1].as_ref() {
            Some(ActorInput::Worker(WorkerEvent::TextDelta { text, .. })) => {
                assert_eq!(text, "b")
            }
            _ => panic!("第二个事件应为文本片段"),
        }
        match events[2].as_ref() {
            Some(ActorInput::Worker(WorkerEvent::FinalText { text, .. })) => {
                assert_eq!(text, "ab")
            }
            _ => panic!("第三个事件应为最终文本"),
        }
    }

    #[tokio::test]
    async fn fake_worker_observes_cancellation() {
        let (sender, mut receiver) = mpsc::channel(4);
        let token = CancellationToken::new();
        let child = token.child_token();
        let turn_id = TurnId::new();

        let worker = tokio::spawn(run_fake_worker(
            sender,
            turn_id,
            child,
            &[FakeWorkerStep::WaitForCancel],
        ));
        token.cancel();

        worker.await.expect("fake worker 不应 panic");
        assert!(matches!(
            receiver.recv().await,
            Some(ActorInput::Worker(WorkerEvent::Cancelled { .. }))
        ));
    }

    #[tokio::test]
    async fn fake_worker_can_emit_failure() {
        let (sender, mut receiver) = mpsc::channel(4);
        let turn_id = TurnId::new();

        run_fake_worker(
            sender,
            turn_id,
            CancellationToken::new(),
            &[FakeWorkerStep::Fail("provider unavailable")],
        )
        .await;

        assert!(matches!(
            receiver.recv().await,
            Some(ActorInput::Worker(WorkerEvent::Failed { reason, .. }))
                if reason == "provider unavailable"
        ));
    }
}
