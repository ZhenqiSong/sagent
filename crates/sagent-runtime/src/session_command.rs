//! Session Actor mailbox 命令和 reply 类型。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 5 Actor 命令

use sagent_session::{AppendMessage, MessageRange};
use sagent_types::message::Message;
use sagent_types::session::Session;
use tokio::sync::oneshot;

use crate::error::ActorError;
use crate::event_bus::EventReceiver;
use crate::session_snapshot::SessionSnapshot;

/// Actor 命令的返回值。
pub enum CommandReply {
    /// 当前快照。
    Snapshot(SessionSnapshot),
    /// 追加成功的消息。
    Message(Message),
    /// 消息查询结果。
    Messages(Vec<Message>),
    /// 关闭后的 Session。
    Closed(Session),
    /// 新建订阅的接收端。
    Subscription(EventReceiver),
    /// 无返回值命令。
    Ack,
}

/// Session Actor 支持的最小命令集合。
pub enum ActorCommand {
    /// 获取当前内存快照。
    GetSnapshot {
        /// 命令响应通道。
        reply: oneshot::Sender<Result<CommandReply, ActorError>>,
    },
    /// 追加一条 Message。
    AppendMessage {
        /// Repository typed input。
        input: AppendMessage,
        /// 命令响应通道。
        reply: oneshot::Sender<Result<CommandReply, ActorError>>,
    },
    /// 读取消息窗口。
    ListMessages {
        /// Message 分页范围。
        range: MessageRange,
        /// 命令响应通道。
        reply: oneshot::Sender<Result<CommandReply, ActorError>>,
    },
    /// 关闭 Session。
    Close {
        /// 可选关闭原因。
        reason: Option<String>,
        /// 命令响应通道。
        reply: oneshot::Sender<Result<CommandReply, ActorError>>,
    },
    /// 创建 live event 订阅。
    Subscribe {
        /// 命令响应通道。
        reply: oneshot::Sender<Result<CommandReply, ActorError>>,
    },
    /// 移除一个订阅者；当前订阅实现通过关闭 receiver 触发自动清理。
    DetachSubscriber {
        /// 要移除的订阅 ID。
        subscriber_id: u64,
        /// 命令响应通道。
        reply: oneshot::Sender<Result<CommandReply, ActorError>>,
    },
    /// 请求 Actor 在已接受命令完成后退出。
    Shutdown {
        /// 命令响应通道。
        reply: oneshot::Sender<Result<CommandReply, ActorError>>,
    },
}
