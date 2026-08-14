//! Session Actor 的 live event bus。
//!
//! 事件只在对应 Repository 事务提交后创建；订阅者使用有界 Tokio channel，
//! 断开的订阅者会在下一次发布时被清理。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 5 event bus

use std::sync::atomic::{AtomicU64, Ordering};

use sagent_types::ids::SessionId;
use sagent_types::message::Message;
use sagent_types::session::Session;
use tokio::sync::mpsc;

/// Session 生命周期和持久化事件。
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// Session 创建成功。
    Created { session: Session, seq: u64 },
    /// Message 追加成功。
    MessageAppended {
        message: Message,
        revision: u64,
        seq: u64,
    },
    /// Session 关闭成功。
    Closed { session: Session, seq: u64 },
    /// Session 从数据库恢复成功。
    Recovered { session: Session, seq: u64 },
    /// Actor 处理失败。
    Failed {
        session_id: SessionId,
        error: String,
        seq: u64,
    },
}

impl SessionEvent {
    /// 返回事件所属 Session。
    pub fn session_id(&self) -> &SessionId {
        match self {
            Self::Created { session, .. }
            | Self::Closed { session, .. }
            | Self::Recovered { session, .. } => &session.id,
            Self::MessageAppended { message, .. } => &message.session_id,
            Self::Failed { session_id, .. } => session_id,
        }
    }

    /// 返回事件序号。
    pub fn seq(&self) -> u64 {
        match self {
            Self::Created { seq, .. }
            | Self::MessageAppended { seq, .. }
            | Self::Closed { seq, .. }
            | Self::Recovered { seq, .. }
            | Self::Failed { seq, .. } => *seq,
        }
    }
}

/// Session event 发布器和订阅者集合。
#[derive(Debug)]
pub struct EventBus {
    capacity: usize,
    sequence: AtomicU64,
    subscriber_sequence: AtomicU64,
    subscribers: Vec<(u64, mpsc::Sender<SessionEvent>)>,
}

/// 单个订阅者的事件接收端。
pub struct EventReceiver {
    id: u64,
    receiver: mpsc::Receiver<SessionEvent>,
}

impl EventReceiver {
    /// 返回订阅 ID，供 `detach_subscriber` 使用。
    pub fn id(&self) -> u64 {
        self.id
    }

    /// 接收下一条事件。
    pub async fn recv(&mut self) -> Option<SessionEvent> {
        self.receiver.recv().await
    }
}

impl EventBus {
    /// 创建一个有界 event bus。
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "event bus capacity must be positive");
        Self {
            capacity,
            sequence: AtomicU64::new(0),
            subscriber_sequence: AtomicU64::new(0),
            subscribers: Vec::new(),
        }
    }

    /// 创建订阅；事件不补发订阅前历史。
    pub fn subscribe(&mut self) -> EventReceiver {
        let (sender, receiver) = mpsc::channel(self.capacity);
        let id = self.subscriber_sequence.fetch_add(1, Ordering::Relaxed) + 1;
        self.subscribers.push((id, sender));
        EventReceiver { id, receiver }
    }

    /// 按订阅 ID 移除订阅者。
    pub fn detach(&mut self, id: u64) {
        self.subscribers.retain(|(subscriber_id, _)| *subscriber_id != id);
    }

    /// 分配下一个 Session event sequence。
    pub fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// 向所有当前订阅者非阻塞发布事件，并清理断开或满载的订阅者。
    pub fn publish(&mut self, event: SessionEvent) {
        let mut retained = Vec::with_capacity(self.subscribers.len());
        for (id, subscriber) in self.subscribers.drain(..) {
            match subscriber.try_send(event.clone()) {
                Ok(()) => retained.push((id, subscriber)),
                Err(mpsc::error::TrySendError::Full(_)) => {},
                Err(mpsc::error::TrySendError::Closed(_)) => {},
            }
        }
        self.subscribers = retained;
    }
}
