//! 配置默认值。
//!
//! 所有默认行为值集中在此模块，避免散落在 CLI 或加载分支中。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 1 配置默认值

/// 当前支持的配置版本。
pub const CONFIG_VERSION: u32 = 1;

/// 默认优雅关闭超时时间（毫秒）。
pub const SHUTDOWN_TIMEOUT_MS: u64 = 5_000;
/// 默认最大活跃 Session 数量。
pub const MAX_LIVE_SESSIONS: u32 = 128;
/// 默认 Session Actor mailbox 容量。
pub const ACTOR_MAILBOX_CAPACITY: u32 = 256;
/// 默认事件缓冲容量。
pub const EVENT_BUFFER_CAPACITY: u32 = 256;
/// 默认 SQLite busy timeout（毫秒）。
pub const BUSY_TIMEOUT_MS: u64 = 5_000;
/// 默认单行请求大小上限（字节）。
pub const MAX_LINE_BYTES: u64 = 1_048_576;
/// 默认响应大小上限（字节）。
pub const MAX_RESPONSE_BYTES: u64 = 4_194_304;

/// 最小允许的非零时间和容量值。
pub const MIN_POSITIVE: u64 = 1;
/// 最大允许的关闭超时时间和 busy timeout（毫秒）。
pub const MAX_TIMEOUT_MS: u64 = 600_000;
/// 最大允许的活跃 Session 数量。
pub const MAX_LIVE_SESSIONS_LIMIT: u32 = 16_384;
/// 最大允许的 mailbox 和 event buffer 容量。
pub const MAX_BUFFER_CAPACITY: u32 = 65_536;
/// 最大允许的单行请求大小（字节）。
pub const MAX_LINE_BYTES_LIMIT: u64 = 16 * 1_048_576;
/// 最大允许的响应大小（字节）。
pub const MAX_RESPONSE_BYTES_LIMIT: u64 = 64 * 1_048_576;
