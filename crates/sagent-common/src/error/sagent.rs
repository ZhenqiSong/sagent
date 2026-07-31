use super::classified::ClassifiedError;

/// 顶层错误类型——所有 sagent crate 的统一错误出口。
/// 库 crate（`sagent-common`/`sagent-proto`/`sagent-config`/`sagent-core` 等）使用此枚举；
/// 二进制入口（`sagent-cli`）可通过 `anyhow` 进一步包装。
#[derive(Debug, thiserror::Error)]
pub enum SagentError {
    /// SSL/TLS 配置错误
    #[error("SSL 配置错误：{0}")]
    SslConfig(String),

    /// Provider 返回了空流（流式响应异常中断）
    #[error("Provider 返回了空响应流，可能是连接中断或服务端异常")]
    EmptyStream,

    /// 未知的 MoA (Mixture of Agents) 预设名称
    #[error("未知的 MoA 预设：{0}")]
    MoaPresetNotFound(String),

    /// 包装标准 I/O 错误
    #[error("I/O 错误：{0}")]
    Io(#[from] std::io::Error),

    /// 包装分类错误（含故障转移原因与重试策略）
    #[error("{0}")]
    Classified(#[from] ClassifiedError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::classified::ClassifiedError;
    use crate::error::failover::FailoverReason;

    #[test]
    fn test_display_chinese() {
        let err = SagentError::SslConfig(String::from("证书链不完整"));
        assert!(err.to_string().contains("SSL 配置错误"));
        assert!(err.to_string().contains("证书链不完整"));

        let err = SagentError::EmptyStream;
        assert!(err.to_string().contains("空响应流"));

        let err = SagentError::MoaPresetNotFound(String::from("fast-team-v2"));
        assert!(err.to_string().contains("MoA 预设"));
        assert!(err.to_string().contains("fast-team-v2"));
    }

    #[test]
    fn test_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "文件未找到");
        let sagent_err: SagentError = io_err.into();
        assert!(matches!(sagent_err, SagentError::Io(_)));
        assert!(sagent_err.to_string().contains("I/O 错误"));
    }

    #[test]
    fn test_from_classified() {
        let classified = ClassifiedError::new(FailoverReason::RateLimit, "请求频率过高");
        let sagent_err: SagentError = classified.into();
        assert!(matches!(sagent_err, SagentError::Classified(_)));
        assert!(sagent_err.to_string().contains("请求频率过高"));
    }

    #[test]
    fn test_from_classified_via_try() {
        fn returns_classified() -> Result<(), ClassifiedError> {
            Err(ClassifiedError::new(FailoverReason::Timeout, "请求超时"))
        }

        fn calls_with_try() -> Result<(), SagentError> {
            returns_classified()?;
            Ok(())
        }

        let result = calls_with_try();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("请求超时"));
    }

    #[test]
    fn test_is_std_error() {
        let err = SagentError::EmptyStream;
        let _: &dyn std::error::Error = &err;
    }
}
