//! 工具注册和 schema 处理错误。

use thiserror::Error;

/// ToolRegistry 的稳定错误边界。
#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum RegistryError {
    /// 工具名称为空。
    #[error("工具名称不能为空")]
    EmptyName,
    /// 工具名称包含不稳定或不允许的字符。
    #[error("工具名称非法：{0}")]
    InvalidName(String),
    /// 同一个 registry 中不能重复注册工具。
    #[error("工具已经注册：{0}")]
    DuplicateName(String),
    /// 模型 schema 的根节点必须是 JSON object。
    #[error("工具 input schema 必须是 JSON object")]
    InvalidSchema,
    /// 工具超时时间必须大于零。
    #[error("工具 timeout_ms 必须大于零")]
    InvalidTimeout,
    /// 工具输出上限必须大于零。
    #[error("工具 output_limit 必须大于零")]
    InvalidOutputLimit,
    /// 查询不存在的工具。
    #[error("未知工具：{0}")]
    UnknownTool(String),
    /// 预留给未来 schema 序列化扩展的稳定错误分类。
    #[error("工具 schema 序列化失败：{0}")]
    Serialization(String),
}
