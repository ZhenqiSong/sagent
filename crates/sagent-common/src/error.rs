use thiserror::Error;

/// sagent 统一错误类型。
///
/// 涵盖配置、IO、序列化、Provider、Tool 等各类错误场景。
#[derive(Error, Debug)]
pub enum SagentError {
    /// 配置相关错误
    #[error("配置错误: {0}")]
    Config(String),

    /// IO 错误
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    /// 序列化/反序列化错误
    #[error("序列化错误: {0}")]
    Serde(#[from] serde_json::Error),

    /// LLM Provider 调用错误
    #[error("Provider 错误: {0}")]
    Provider(String),

    /// Tool 执行错误
    #[error("Tool 错误: {0}")]
    Tool(String),

    /// 资源未找到
    #[error("未找到: {0}")]
    NotFound(String),

    /// 其他通用错误
    #[error("{0}")]
    Other(String),
}
