use std::sync::LazyLock;
use regex::Regex;

// ── 预编译正则模式（LazyLock 保证全局单例，线程安全） ──

/// JWT token 模式（三段 base64url，用 `.` 分隔）
static JWT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,}")
        .expect("JWT 正则编译失败")
});

/// Slack token 模式（xox 开头 + 连字符分段）
static SLACK_TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"xox[abps]-\d{10,13}-\d{10,13}-[a-zA-Z0-9]{24,}").expect("Slack token 正则编译失败")
});

/// API key 通用模式（sk- / key- / api- / token- 前缀 + 字母数字连字符混合，至少 20 字符）
static API_KEY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:sk|key|api|token)[-_][A-Za-z0-9_-]{16,}").expect("API key 正则编译失败")
});

/// 手机号模式（中国大陆 1xx-xxxx-xxxx，含分隔符，词边界限定）
static PHONE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b1[3-9]\d[-\s]?\d{4}[-\s]?\d{4}\b").expect("手机号正则编译失败")
});

/// 邮箱模式
static EMAIL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}").expect("邮箱正则编译失败")
});

/// UUID 模式（标准格式 + 无连字符格式）
static UUID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[0-9a-fA-F]{32}")
        .expect("UUID 正则编译失败")
});

// ── Redact trait ──

/// 敏感信息遮蔽 trait。
///
/// 实现此 trait 的类型可通过 `redact()` 方法对自身进行敏感信息脱敏。
/// 默认实现将自身的 `Display` 字符串传入 `redact_str()`。
pub trait Redact {
    /// 返回脱敏后的字符串。
    fn redact(&self) -> String;
}

/// 为所有实现了 `Display` 的类型提供默认 Redact 实现。
impl<T: std::fmt::Display> Redact for T {
    fn redact(&self) -> String {
        redact_str(&self.to_string())
    }
}

// ── 公共函数 ──

/// 对字符串进行敏感信息脱敏，返回脱敏后的新字符串。
///
/// 覆盖以下敏感信息类型：
/// - JWT token → `[JWT已脱敏]`
/// - Slack token → `[SlackToken已脱敏]`
/// - API key（sk-/key-/api-/token- 前缀） → `[APIKey已脱敏]`
/// - 手机号 → `[手机号已脱敏]`
/// - 邮箱 → `[邮箱已脱敏]`
/// - UUID → `[UUID已脱敏]`
pub fn redact_str(input: &str) -> String {
    let mut result = String::from(input);

    result = JWT_RE.replace_all(&result, "[JWT已脱敏]").into_owned();
    result = SLACK_TOKEN_RE.replace_all(&result, "[SlackToken已脱敏]").into_owned();
    result = API_KEY_RE.replace_all(&result, "[APIKey已脱敏]").into_owned();
    result = PHONE_RE.replace_all(&result, "[手机号已脱敏]").into_owned();
    result = EMAIL_RE.replace_all(&result, "[邮箱已脱敏]").into_owned();
    result = UUID_RE.replace_all(&result, "[UUID已脱敏]").into_owned();

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── JWT 脱敏 ──

    #[test]
    fn test_redact_jwt() {
        let input = "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let output = redact_str(input);
        assert!(!output.contains("eyJhbGciOiJIUzI1NiJ9"));
        assert!(output.contains("[JWT已脱敏]"));
    }

    #[test]
    fn test_redact_jwt_in_log_line() {
        let input =
            "Authorization: Bearer abcdefghijklmnopqrstuvwxyz1234567890.abcdefghijklmnopqrstuvwxyz1234567890.abcdefghijklmnopqrstuvwxyz1234567890 extra text";
        let output = redact_str(input);
        assert!(output.contains("[JWT已脱敏]"));
        assert!(output.contains("extra text")); // 非敏感部分保留
    }

    // ── Slack token 脱敏 ──

    #[test]
    fn test_redact_slack_token() {
        let input = "token: xoxb-1234567890123-1234567890123-abcdefghijklmnopqrstuvwx";
        let output = redact_str(input);
        assert!(output.contains("[SlackToken已脱敏]"));
        assert!(!output.contains("xoxb-1234567890123"));
    }

    // ── API key 脱敏 ──

    #[test]
    fn test_redact_api_key_sk_prefix() {
        let input = "Authorization: sk-proj-abcdefghijklmnopqrstuvwxyz123456";
        let output = redact_str(input);
        assert!(output.contains("[APIKey已脱敏]"));
        assert!(!output.contains("sk-proj-abcdef"));
    }

    #[test]
    fn test_redact_api_key_key_prefix() {
        let input = "X-API-Key: key-1234567890abcdefghijklmnopqrstuv";
        let output = redact_str(input);
        assert!(output.contains("[APIKey已脱敏]"));
    }

    // ── 手机号脱敏 ──

    #[test]
    fn test_redact_phone_number() {
        let input = "联系电话：13812345678";
        let output = redact_str(input);
        assert!(output.contains("[手机号已脱敏]"));
        assert!(!output.contains("13812345678"));
    }

    #[test]
    fn test_redact_phone_number_with_dash() {
        let input = "电话 139-1234-5678";
        let output = redact_str(input);
        assert!(output.contains("[手机号已脱敏]"));
    }

    // ── 邮箱脱敏 ──

    #[test]
    fn test_redact_email() {
        let input = "联系 admin@example.com 获取帮助";
        let output = redact_str(input);
        assert!(output.contains("[邮箱已脱敏]"));
        assert!(!output.contains("admin@example.com"));
    }

    #[test]
    fn test_redact_email_with_subdomain() {
        let input = "support@mail.sub.example.co.uk";
        let output = redact_str(input);
        assert!(output.contains("[邮箱已脱敏]"));
    }

    // ── UUID 脱敏 ──

    #[test]
    fn test_redact_uuid_standard() {
        let input = "session: 550e8400-e29b-41d4-a716-446655440000";
        let output = redact_str(input);
        assert!(output.contains("[UUID已脱敏]"));
        assert!(!output.contains("550e8400-e29b"));
    }

    #[test]
    fn test_redact_uuid_no_dash() {
        let input = "id=550e8400e29b41d4a716446655440000";
        let output = redact_str(input);
        assert!(
            output.contains("[UUID已脱敏]"),
            "预期包含 [UUID已脱敏]，实际输出: {output}"
        );
    }

    // ── 多种模式混合 ──

    #[test]
    fn test_redact_multiple_patterns() {
        let input = "user: alice@example.com, phone: 13812345678, token: sk-abcdefghijklmnopqrstuvwxyz123456";
        let output = redact_str(input);
        assert!(output.contains("[邮箱已脱敏]"));
        assert!(output.contains("[手机号已脱敏]"));
        assert!(output.contains("[APIKey已脱敏]"));
    }

    // ── 无敏感信息不修改 ──

    #[test]
    fn test_redact_no_sensitive_data() {
        let input = "这是一条普通日志，不包含任何敏感信息。Agent 正在处理用户请求。";
        let output = redact_str(input);
        assert_eq!(input, output);
    }

    #[test]
    fn test_redact_empty_string() {
        assert_eq!(redact_str(""), "");
    }

    // ── Redact trait ──

    #[test]
    fn test_redact_trait_on_string() {
        let s = String::from("email: test@example.com");
        let redacted = s.redact();
        assert!(redacted.contains("[邮箱已脱敏]"));
        // 原始字符串不变
        assert_eq!(s, "email: test@example.com");
    }

    #[test]
    fn test_redact_trait_on_str() {
        let redacted = "phone: 13800001111".redact();
        assert!(redacted.contains("[手机号已脱敏]"));
    }
}
