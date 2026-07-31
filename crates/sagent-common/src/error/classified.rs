use super::failover::FailoverReason;

/// 分类错误——对 Provider 返回的原始错误进行分类，供下游重试/降级/切换逻辑使用。
/// 这不是 Rust 的 `Error` trait 替代，而是各 adapter 通过 `fn classify(&self) -> ClassifiedError` 方法
/// 返回的分类信息。
#[derive(Debug, Clone)]
pub struct ClassifiedError {
    /// 故障分类原因
    pub reason: FailoverReason,
    /// HTTP 状态码（如有）
    pub status_code: Option<u16>,
    /// 是否可重试
    pub retryable: bool,
    /// 是否应触发上下文压缩
    pub should_compress: bool,
    /// 是否应轮换凭证
    pub should_rotate_credential: bool,
    /// 是否应故障转移到备用 Provider
    pub should_fallback: bool,
    /// 人类可读的错误描述（中文）
    pub message: String,
}

impl ClassifiedError {
    /// 创建一个新的分类错误，仅需提供 reason 和 message，其余字段使用合理默认值。
    pub fn new(reason: FailoverReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            status_code: None,
            retryable: false,
            should_compress: false,
            should_rotate_credential: false,
            should_fallback: false,
            message: message.into(),
        }
    }

    /// 设置 HTTP 状态码（构建器模式）。
    pub fn with_status_code(mut self, code: u16) -> Self {
        self.status_code = Some(code);
        self
    }

    /// 标记为可重试（构建器模式）。
    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }

    /// 标记为应触发上下文压缩（构建器模式）。
    pub fn compress(mut self) -> Self {
        self.should_compress = true;
        self
    }

    /// 标记为应轮换凭证（构建器模式）。
    pub fn rotate_credential(mut self) -> Self {
        self.should_rotate_credential = true;
        self
    }

    /// 标记为应故障转移（构建器模式）。
    pub fn fallback(mut self) -> Self {
        self.should_fallback = true;
        self
    }
}

impl Default for ClassifiedError {
    fn default() -> Self {
        Self {
            reason: FailoverReason::Unknown,
            status_code: None,
            retryable: false,
            should_compress: false,
            should_rotate_credential: false,
            should_fallback: false,
            message: String::from("未知错误"),
        }
    }
}

impl std::fmt::Display for ClassifiedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.reason, self.message)
    }
}

impl std::error::Error for ClassifiedError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default() {
        let err = ClassifiedError::default();
        assert_eq!(err.reason, FailoverReason::Unknown);
        assert_eq!(err.status_code, None);
        assert!(!err.retryable);
        assert!(!err.should_compress);
        assert!(!err.should_rotate_credential);
        assert!(!err.should_fallback);
        assert_eq!(err.message, "未知错误");
    }

    #[test]
    fn test_new() {
        let err = ClassifiedError::new(FailoverReason::Auth, "认证失败，请检查 API Key 是否正确");
        assert_eq!(err.reason, FailoverReason::Auth);
        assert_eq!(err.status_code, None);
        assert!(!err.retryable);
        assert_eq!(err.message, "认证失败，请检查 API Key 是否正确");
    }

    #[test]
    fn test_builder_pattern() {
        let err = ClassifiedError::new(FailoverReason::RateLimit, "请求过于频繁，请稍后重试")
            .with_status_code(429)
            .retryable()
            .fallback();

        assert_eq!(err.reason, FailoverReason::RateLimit);
        assert_eq!(err.status_code, Some(429));
        assert!(err.retryable);
        assert!(!err.should_compress);
        assert!(!err.should_rotate_credential);
        assert!(err.should_fallback);
    }

    #[test]
    fn test_compress_flag() {
        let err =
            ClassifiedError::new(FailoverReason::ContextOverflow, "上下文超出模型限制，需要压缩")
                .compress()
                .retryable();

        assert_eq!(err.reason, FailoverReason::ContextOverflow);
        assert!(err.should_compress);
        assert!(err.retryable);
    }

    #[test]
    fn test_display() {
        let err = ClassifiedError::new(FailoverReason::Timeout, "请求超时，Provider 无响应");
        let display = err.to_string();
        assert!(display.contains("timeout"));
        assert!(display.contains("请求超时，Provider 无响应"));
    }

    #[test]
    fn test_is_std_error() {
        let err = ClassifiedError::new(FailoverReason::Unknown, "测试错误");
        let _: &dyn std::error::Error = &err;
    }
}
