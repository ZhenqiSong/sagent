//! Store 的事务与只读边界测试。
//!
//! 测试保留 crate 私有可见性，以验证连接写保护而不把 SQLite connection 暴露给调用方。

use std::{fs, path::PathBuf};

use rusqlite::Connection;

use super::{
    EventQuery, MessageQuery, MessageSearchQuery, NewGeneration, NewMessage, NewSession,
    RestoreResult, SCHEMA_VERSION, StartTurn, Store,
};
use sagent_types::{EventSequence, MessageId, SessionId, TurnId};

fn test_path(name: &str) -> PathBuf {
    // 每个测试使用独立文件名，避免并行测试共享数据库；文件位于系统临时目录，
    // 不会触碰开发者真实的 SAGENT_HOME。
    std::env::temp_dir().join(format!("sagent-store-{name}-{}.db", std::process::id()))
}

fn remove_if_exists(path: &std::path::Path) {
    // 清理函数允许目标不存在，便于在测试开始前消除上次异常留下的临时文件。
    let _ = fs::remove_file(path);
}
// 将不同生命周期的 fixture 分开，避免单个 Store 测试文件同时承载打开、分支和持久化语义。
#[path = "store_tests/branch.rs"]
mod branch;
#[path = "store_tests/core.rs"]
mod core;
#[path = "store_tests/durability.rs"]
mod durability;
