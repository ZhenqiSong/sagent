use std::fmt;

/// 故障转移原因——对齐 Python 版 `FailoverReason` 的 23 个枚举值。
/// `as_str()` 返回短横线命名字符串，用于下游重试/降级逻辑。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailoverReason {
    /// 认证失败（临时性，如 token 过期可刷新）
    Auth,
    /// 认证失败（永久性，如 API key 无效）
    AuthPermanent,
    /// 计费问题（余额不足等）
    Billing,
    /// 速率限制（本侧主动限流）
    RateLimit,
    /// 上游速率限制（Provider 返回 429）
    UpstreamRateLimit,
    /// Provider 过载
    Overloaded,
    /// Provider 服务端错误（5xx）
    ServerError,
    /// 请求超时
    Timeout,
    /// SSL 证书校验失败
    SslCertVerification,
    /// 上下文溢出（超出模型最大 token 限制）
    ContextOverflow,
    /// 请求体过大
    PayloadTooLarge,
    /// 图片过大
    ImageTooLarge,
    /// 模型不存在或无权访问
    ModelNotFound,
    /// Provider 策略拦截
    ProviderPolicyBlocked,
    /// 内容策略拦截（安全审核不通过）
    ContentPolicyBlocked,
    /// 响应格式错误（无法解析 Provider 返回）
    FormatError,
    /// 加密内容无效
    InvalidEncryptedContent,
    /// 多模态工具内容不支持
    MultimodalToolContentUnsupported,
    /// Thinking 签名错误
    ThinkingSignature,
    /// 长上下文 tier 不支持
    LongContextTier,
    /// OAuth 长上下文 Beta 被禁止
    OauthLongContextBetaForbidden,
    /// LlamaCpp 语法模式错误
    LlamaCppGrammarPattern,
    /// 未知错误
    Unknown,
}

impl FailoverReason {
    /// 返回短横线命名的字符串表示，与 Python 版语义一致。
    pub fn as_str(&self) -> &'static str {
        match self {
            FailoverReason::Auth => "auth",
            FailoverReason::AuthPermanent => "auth-permanent",
            FailoverReason::Billing => "billing",
            FailoverReason::RateLimit => "rate-limit",
            FailoverReason::UpstreamRateLimit => "upstream-rate-limit",
            FailoverReason::Overloaded => "overloaded",
            FailoverReason::ServerError => "server-error",
            FailoverReason::Timeout => "timeout",
            FailoverReason::SslCertVerification => "ssl-cert-verification",
            FailoverReason::ContextOverflow => "context-overflow",
            FailoverReason::PayloadTooLarge => "payload-too-large",
            FailoverReason::ImageTooLarge => "image-too-large",
            FailoverReason::ModelNotFound => "model-not-found",
            FailoverReason::ProviderPolicyBlocked => "provider-policy-blocked",
            FailoverReason::ContentPolicyBlocked => "content-policy-blocked",
            FailoverReason::FormatError => "format-error",
            FailoverReason::InvalidEncryptedContent => "invalid-encrypted-content",
            FailoverReason::MultimodalToolContentUnsupported => {
                "multimodal-tool-content-unsupported"
            }
            FailoverReason::ThinkingSignature => "thinking-signature",
            FailoverReason::LongContextTier => "long-context-tier",
            FailoverReason::OauthLongContextBetaForbidden => {
                "oauth-long-context-beta-forbidden"
            }
            FailoverReason::LlamaCppGrammarPattern => "llama-cpp-grammar-pattern",
            FailoverReason::Unknown => "unknown",
        }
    }
}

impl fmt::Display for FailoverReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_as_str_all_variants() {
        assert_eq!(FailoverReason::Auth.as_str(), "auth");
        assert_eq!(FailoverReason::AuthPermanent.as_str(), "auth-permanent");
        assert_eq!(FailoverReason::Billing.as_str(), "billing");
        assert_eq!(FailoverReason::RateLimit.as_str(), "rate-limit");
        assert_eq!(
            FailoverReason::UpstreamRateLimit.as_str(),
            "upstream-rate-limit"
        );
        assert_eq!(FailoverReason::Overloaded.as_str(), "overloaded");
        assert_eq!(FailoverReason::ServerError.as_str(), "server-error");
        assert_eq!(FailoverReason::Timeout.as_str(), "timeout");
        assert_eq!(
            FailoverReason::SslCertVerification.as_str(),
            "ssl-cert-verification"
        );
        assert_eq!(
            FailoverReason::ContextOverflow.as_str(),
            "context-overflow"
        );
        assert_eq!(
            FailoverReason::PayloadTooLarge.as_str(),
            "payload-too-large"
        );
        assert_eq!(FailoverReason::ImageTooLarge.as_str(), "image-too-large");
        assert_eq!(FailoverReason::ModelNotFound.as_str(), "model-not-found");
        assert_eq!(
            FailoverReason::ProviderPolicyBlocked.as_str(),
            "provider-policy-blocked"
        );
        assert_eq!(
            FailoverReason::ContentPolicyBlocked.as_str(),
            "content-policy-blocked"
        );
        assert_eq!(FailoverReason::FormatError.as_str(), "format-error");
        assert_eq!(
            FailoverReason::InvalidEncryptedContent.as_str(),
            "invalid-encrypted-content"
        );
        assert_eq!(
            FailoverReason::MultimodalToolContentUnsupported.as_str(),
            "multimodal-tool-content-unsupported"
        );
        assert_eq!(
            FailoverReason::ThinkingSignature.as_str(),
            "thinking-signature"
        );
        assert_eq!(
            FailoverReason::LongContextTier.as_str(),
            "long-context-tier"
        );
        assert_eq!(
            FailoverReason::OauthLongContextBetaForbidden.as_str(),
            "oauth-long-context-beta-forbidden"
        );
        assert_eq!(
            FailoverReason::LlamaCppGrammarPattern.as_str(),
            "llama-cpp-grammar-pattern"
        );
        assert_eq!(FailoverReason::Unknown.as_str(), "unknown");
    }

    #[test]
    fn test_display_matches_as_str() {
        let all = [
            FailoverReason::Auth,
            FailoverReason::AuthPermanent,
            FailoverReason::Billing,
            FailoverReason::RateLimit,
            FailoverReason::UpstreamRateLimit,
            FailoverReason::Overloaded,
            FailoverReason::ServerError,
            FailoverReason::Timeout,
            FailoverReason::SslCertVerification,
            FailoverReason::ContextOverflow,
            FailoverReason::PayloadTooLarge,
            FailoverReason::ImageTooLarge,
            FailoverReason::ModelNotFound,
            FailoverReason::ProviderPolicyBlocked,
            FailoverReason::ContentPolicyBlocked,
            FailoverReason::FormatError,
            FailoverReason::InvalidEncryptedContent,
            FailoverReason::MultimodalToolContentUnsupported,
            FailoverReason::ThinkingSignature,
            FailoverReason::LongContextTier,
            FailoverReason::OauthLongContextBetaForbidden,
            FailoverReason::LlamaCppGrammarPattern,
            FailoverReason::Unknown,
        ];
        for reason in all {
            assert_eq!(reason.to_string(), reason.as_str());
        }
    }
}
