//! CLI session 的生命周期写操作。
//!
//! 这些命令统一经父模块申请可写领域端口执行，因此 Profile 解析、事务错误语义和输出
//! 格式与创建/查询命令保持一致；本模块不自行打开其他路径的数据库。

use anyhow::Result;
use sagent_types::{MessageId, SessionId};

use super::{
    SessionCommandHandler, now_rfc3339, parse_message_id, print_lifecycle_result, print_output,
    validate_reason, validate_title,
};
/// Session 生命周期命令的处理方法。
impl SessionCommandHandler<'_> {
    /// 修改会话标题。
    pub(super) fn handle_rename(&self, session_id: &str, title: &str) -> Result<()> {
        let title = validate_title(title)?;
        let updated_at = now_rfc3339()?;
        let changed = self.with_writable_storage(|store| {
            store.update_session_title(&SessionId::new(session_id), Some(title), &updated_at)
        })?;
        let value = serde_json::json!({
            "operation": "rename",
            "session_id": session_id,
            "title": title,
            "changed": changed,
            "updated_at": updated_at,
        });
        print_output(
            self.context().format,
            &value,
            vec![format!("已重命名会话: {session_id}")],
        )
    }

    /// 归档会话，使其从默认列表中隐藏。
    pub(super) fn handle_archive(&self, session_id: &str) -> Result<()> {
        self.handle_archive_state(session_id, true)
    }

    /// 取消会话归档，使其重新出现在默认列表中。
    pub(super) fn handle_unarchive(&self, session_id: &str) -> Result<()> {
        self.handle_archive_state(session_id, false)
    }

    /// 归档与取消归档共享的存储写入和输出逻辑。
    fn handle_archive_state(&self, session_id: &str, archived: bool) -> Result<()> {
        let updated_at = now_rfc3339()?;
        let changed = self.with_writable_storage(|store| {
            store.set_session_archived(&SessionId::new(session_id), archived, &updated_at)
        })?;
        let operation = if archived { "archive" } else { "unarchive" };
        print_lifecycle_result(self.context(), operation, session_id, changed, &updated_at)
    }

    /// 结束会话并记录调用方提供的原因。
    pub(super) fn handle_finish(&self, session_id: &str, reason: &str) -> Result<()> {
        let reason = validate_reason(reason)?;
        let updated_at = now_rfc3339()?;
        let changed = self.with_writable_storage(|store| {
            store.finish_session(&SessionId::new(session_id), reason, &updated_at)
        })?;
        let value = serde_json::json!({
            "operation": "finish",
            "session_id": session_id,
            "reason": reason,
            "changed": changed,
            "updated_at": updated_at,
        });
        print_output(
            self.context().format,
            &value,
            vec![format!("已结束会话: {session_id}")],
        )
    }

    /// 回退到一条 user 消息，将该消息及之后的活动消息软删除以保留审计历史。
    pub(super) fn handle_rewind(&self, session_id: &str, message_id: &str) -> Result<()> {
        let message_id = parse_message_id(message_id)?;
        let updated_at = now_rfc3339()?;
        let result = self.with_writable_storage(|store| {
            store.rewind_to_message(&SessionId::new(session_id), message_id, &updated_at)
        })?;
        let value = serde_json::json!({
            "operation": "rewind",
            "session_id": session_id,
            "target_message_id": result.target_message.id.get(),
            "rewound_count": result.rewound_count,
            "new_head_id": result.new_head_id.as_ref().map(MessageId::get),
            "updated_at": updated_at,
        });
        print_output(
            self.context().format,
            &value,
            vec![format!(
                "已回退会话: {session_id}（{} 条消息）",
                result.rewound_count
            )],
        )
    }

    /// 恢复由指定回退起点隐藏的消息；若已有新活动分支则拒绝合并。
    pub(super) fn handle_restore(&self, session_id: &str, message_id: &str) -> Result<()> {
        let message_id = parse_message_id(message_id)?;
        let updated_at = now_rfc3339()?;
        let result = self.with_writable_storage(|store| {
            store.restore_rewound_from(&SessionId::new(session_id), message_id.clone(), &updated_at)
        })?;
        let value = serde_json::json!({
            "operation": "restore",
            "session_id": session_id,
            "target_message_id": message_id.get(),
            "restored_count": result.restored_count,
            "new_head_id": result.new_head_id.as_ref().map(MessageId::get),
            "updated_at": updated_at,
        });
        print_output(
            self.context().format,
            &value,
            vec![format!(
                "已恢复会话分支: {session_id}（{} 条消息）",
                result.restored_count
            )],
        )
    }
}
