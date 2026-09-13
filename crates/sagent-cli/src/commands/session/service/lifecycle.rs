//! Session 生命周期写入服务。
//!
//! 每个方法只负责通过当前 `SessionService` 的存储端口执行一个完整的领域写操作；输入
//! 校验、时间生成和 CLI 输出留在 handler，避免服务同时承担协议适配职责。
//!
//! 作者：SongZQ

use anyhow::Result;
use sagent_store::{RestoreResult, RewindResult, SessionWriteStorage};
use sagent_types::{MessageId, SessionId};

use super::SessionService;

impl SessionService {
    /// 修改会话标题并返回目标是否存在。
    pub(crate) fn rename_session(
        &self,
        session_id: &str,
        title: &str,
        updated_at: &str,
    ) -> Result<bool> {
        self.with_writable_storage(|store| {
            store.update_session_title(&SessionId::new(session_id), Some(title), updated_at)
        })
    }

    /// 归档会话并返回目标是否存在。
    pub(crate) fn archive_session(&self, session_id: &str, updated_at: &str) -> Result<bool> {
        self.set_archive_state(session_id, true, updated_at)
    }

    /// 取消会话归档并返回目标是否存在。
    pub(crate) fn unarchive_session(&self, session_id: &str, updated_at: &str) -> Result<bool> {
        self.set_archive_state(session_id, false, updated_at)
    }

    /// 执行归档状态变更的共享存储操作。
    fn set_archive_state(
        &self,
        session_id: &str,
        archived: bool,
        updated_at: &str,
    ) -> Result<bool> {
        self.with_writable_storage(|store| {
            store.set_session_archived(&SessionId::new(session_id), archived, updated_at)
        })
    }

    /// 结束会话并写入结束原因。
    pub(crate) fn finish_session(
        &self,
        session_id: &str,
        reason: &str,
        ended_at: &str,
    ) -> Result<bool> {
        self.with_writable_storage(|store| {
            store.finish_session(&SessionId::new(session_id), reason, ended_at)
        })
    }

    /// 回退指定消息并返回存储层生成的恢复检查点。
    pub(crate) fn rewind_session(
        &self,
        session_id: &str,
        message_id: MessageId,
        updated_at: &str,
    ) -> Result<RewindResult> {
        self.with_writable_storage(|store| {
            store.rewind_to_message(&SessionId::new(session_id), message_id, updated_at)
        })
    }

    /// 恢复指定回退起点并返回恢复数量及新的活动头。
    pub(crate) fn restore_session(
        &self,
        session_id: &str,
        message_id: MessageId,
        updated_at: &str,
    ) -> Result<RestoreResult> {
        self.with_writable_storage(|store| {
            store.restore_rewound_from(&SessionId::new(session_id), message_id, updated_at)
        })
    }

    /// 在当前命令上下文的可写会话端口上执行一个原子业务操作。
    fn with_writable_storage<T>(
        &self,
        operation: impl FnOnce(&mut dyn SessionWriteStorage) -> Result<T>,
    ) -> Result<T> {
        let mut dependencies = self.storage()?.open_write()?;
        operation(dependencies.session_mut())
    }
}
