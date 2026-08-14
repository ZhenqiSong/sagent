//! SQLite 数据库错误类型。
//!
//! 错误区分 I/O、SQLite、migration 和不兼容 schema，避免初始化失败后继续接收请求。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 3 数据库错误

use std::path::PathBuf;

/// SQLite 数据库基础设施错误。
#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    /// 数据库父目录创建失败。
    #[error("创建数据库目录失败: {path}: {source}")]
    CreateParent {
        /// 数据库父目录。
        path: PathBuf,
        /// 底层 I/O 错误。
        #[source]
        source: std::io::Error,
    },
    /// SQLite connection 打开失败。
    #[error("打开 SQLite 数据库失败: {path}: {source}")]
    Open {
        /// 数据库文件路径。
        path: PathBuf,
        /// SQLite 错误。
        #[source]
        source: rusqlite::Error,
    },
    /// SQLite 操作失败。
    #[error("SQLite 操作失败: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// 配置值无法用于数据库初始化。
    #[error("数据库配置无效: {0}")]
    Config(String),
    /// migration 执行失败。
    #[error("migration {version} ({name}) 失败: {source}")]
    Migration {
        /// migration 版本。
        version: u32,
        /// migration 名称。
        name: &'static str,
        /// 底层 SQLite 错误。
        #[source]
        source: rusqlite::Error,
    },
    /// 数据库不是受支持的 Sagent schema。
    #[error("DatabaseSchemaUnsupported: {reason}")]
    Unsupported {
        /// 不兼容原因。
        reason: String,
    },
    /// 初始化后的 schema 缺少关键对象。
    #[error("数据库 schema 校验失败: {object}")]
    SchemaInvalid {
        /// 缺失或错误的对象描述。
        object: String,
    },
}
