//! Session 恢复边界。
//!
//! 恢复只接受 Repository 已提交且 transcript 自洽的快照，不补发历史消息事件。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 6 recovery helper

use sagent_session::{Repository, RepositoryError};
use sagent_types::ids::SessionId;

use crate::session_snapshot::SessionSnapshot;

/// 从 Repository 恢复一个可交给 Actor 的快照。
pub fn recover_session(
    repository: &mut Repository,
    session_id: &SessionId,
) -> Result<SessionSnapshot, RepositoryError> {
    SessionSnapshot::from_repository(repository, session_id)
}
