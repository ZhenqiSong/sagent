//! Provider 错误分类。

use thiserror::Error;

/// Provider 层向 Runtime 暴露的稳定错误分类。
///
/// 错误不携带 API key、Authorization header、完整 prompt 或完整响应体，避免敏感信息
/// 进入日志、事件和 RPC 错误响应。Runtime 会把这些错误转换成 `WorkerEvent::Failed`，
/// 取消类错误则转换成 `WorkerEvent::Cancelled`。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderError {
    /// 当前 Profile 的 Provider 配置无效或不完整。
    #[error("provider 配置错误：{0}")]
    Configuration(String),
    /// Provider 拒绝了凭据。
    #[error("provider 鉴权失败")]
    Authentication,
    /// Provider 限流；retry-after 只保留安全的秒数元数据。
    #[error("provider 请求被限流")]
    RateLimited { retry_after_seconds: Option<u64> },
    /// Provider 返回 5xx 等远端服务错误。
    #[error("provider 远端服务错误：HTTP {status}")]
    RemoteServer { status: u16 },
    /// DNS、连接或读取超时等传输错误。
    #[error("provider 网络错误：{0}")]
    Transport(String),
    /// SSE framing 或 JSON 解析错误。
    #[error("provider 响应解析失败：{0}")]
    Protocol(String),
    /// 连接在收到 finish 之前结束，不能伪造 assistant final。
    #[error("provider 流未正常结束")]
    IncompleteStream,
    /// 由 Runtime 的 CancellationToken 请求取消。
    #[error("provider 请求已取消")]
    Cancelled,
    /// Actor 已停止，Provider 不应继续向 sink 发送事件。
    #[error("Actor 事件接收器已关闭")]
    EventSinkClosed,
}

#[cfg(test)]
mod tests {
    use super::ProviderError;

    #[test]
    fn errors_do_not_include_secret_fields() {
        let key = "secret-api-key";
        let errors = [
            ProviderError::Configuration("endpoint missing".into()),
            ProviderError::Authentication,
            ProviderError::RateLimited {
                retry_after_seconds: Some(3),
            },
            ProviderError::RemoteServer { status: 503 },
            ProviderError::Transport("timeout".into()),
            ProviderError::Protocol("invalid JSON".into()),
            ProviderError::IncompleteStream,
            ProviderError::Cancelled,
            ProviderError::EventSinkClosed,
        ];

        for error in errors {
            assert!(!error.to_string().contains(key));
        }
    }

    #[test]
    fn rate_limit_keeps_only_retry_metadata() {
        let error = ProviderError::RateLimited {
            retry_after_seconds: Some(7),
        };

        assert_eq!(
            error,
            ProviderError::RateLimited {
                retry_after_seconds: Some(7)
            }
        );
        assert_eq!(error.to_string(), "provider 请求被限流");
    }
}
