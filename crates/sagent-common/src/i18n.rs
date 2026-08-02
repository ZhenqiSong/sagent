/// 国际化键枚举——占位实现。
///
/// 当前阶段返回硬编码的中文字符串。后续阶段接入真 i18n 框架后，
/// 各变体将映射到多语言资源文件中的对应条目。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum I18nKey {
    /// 通用：操作成功
    OperationSuccess,
    /// 通用：操作失败
    OperationFailed,
    /// 通用：未知错误
    UnknownError,
    /// 通用：功能尚未实现
    NotImplemented,
    /// 配置：配置文件未找到
    ConfigNotFound,
    /// 配置：配置格式无效
    ConfigInvalid,
    /// 认证：认证失败
    AuthFailed,
    /// 认证：凭证过期
    CredentialExpired,
    /// 网络：请求超时
    RequestTimeout,
    /// 网络：连接失败
    ConnectionFailed,
    /// 速率限制：请求过于频繁
    RateLimited,
    /// 上下文：上下文超出限制
    ContextOverflow,
    /// 工具：工具执行失败
    ToolExecutionFailed,
    /// 工具：工具未找到
    ToolNotFound,
    /// 会话：会话未找到
    SessionNotFound,
    /// 会话：会话已过期
    SessionExpired,
    /// 平台：平台未连接
    PlatformNotConnected,
    /// 平台：消息发送失败
    MessageSendFailed,
}

/// 临时国际化函数——返回 `I18nKey` 对应的中文字符串。
///
/// 后续阶段替换为真 i18n 实现（如 `fluent` / `rust-i18n`），
/// 根据用户语言偏好返回对应翻译。
pub fn t(key: I18nKey) -> &'static str {
    match key {
        I18nKey::OperationSuccess => "操作成功",
        I18nKey::OperationFailed => "操作失败",
        I18nKey::UnknownError => "发生未知错误，请查看日志获取详细信息",
        I18nKey::NotImplemented => "此功能尚未实现",
        I18nKey::ConfigNotFound => "配置文件未找到，请运行 sagent setup 进行初始化",
        I18nKey::ConfigInvalid => "配置文件格式无效，请检查 config.yaml 语法",
        I18nKey::AuthFailed => "认证失败，请检查 API Key 是否正确",
        I18nKey::CredentialExpired => "凭证已过期，请重新登录",
        I18nKey::RequestTimeout => "请求超时，请检查网络连接后重试",
        I18nKey::ConnectionFailed => "连接失败，无法访问远程服务",
        I18nKey::RateLimited => "请求过于频繁，请稍后重试",
        I18nKey::ContextOverflow => "上下文超出模型处理限制，正在自动压缩历史消息",
        I18nKey::ToolExecutionFailed => "工具执行失败",
        I18nKey::ToolNotFound => "未找到指定工具",
        I18nKey::SessionNotFound => "会话未找到，可能已被删除或过期",
        I18nKey::SessionExpired => "会话已过期，请创建新会话",
        I18nKey::PlatformNotConnected => "消息平台未连接，请先运行 sagent gateway setup",
        I18nKey::MessageSendFailed => "消息发送失败，请检查平台连接状态",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_keys_return_non_empty_chinese() {
        let all_keys = [
            I18nKey::OperationSuccess,
            I18nKey::OperationFailed,
            I18nKey::UnknownError,
            I18nKey::NotImplemented,
            I18nKey::ConfigNotFound,
            I18nKey::ConfigInvalid,
            I18nKey::AuthFailed,
            I18nKey::CredentialExpired,
            I18nKey::RequestTimeout,
            I18nKey::ConnectionFailed,
            I18nKey::RateLimited,
            I18nKey::ContextOverflow,
            I18nKey::ToolExecutionFailed,
            I18nKey::ToolNotFound,
            I18nKey::SessionNotFound,
            I18nKey::SessionExpired,
            I18nKey::PlatformNotConnected,
            I18nKey::MessageSendFailed,
        ];

        for key in all_keys {
            let text = t(key);
            assert!(!text.is_empty(), "I18nKey::{key:?} 返回了空字符串");
            // 确保包含中文字符（Unicode CJK 范围）
            let has_chinese = text.chars().any(|c| c >= '\u{4E00}' && c <= '\u{9FFF}');
            assert!(
                has_chinese,
                "I18nKey::{key:?} 的翻译不包含中文字符: \"{text}\""
            );
        }
    }

    #[test]
    fn test_each_key_has_unique_translation() {
        use std::collections::HashSet;

        let translations: Vec<&str> = [
            I18nKey::OperationSuccess,
            I18nKey::OperationFailed,
            I18nKey::UnknownError,
            I18nKey::NotImplemented,
            I18nKey::ConfigNotFound,
            I18nKey::ConfigInvalid,
            I18nKey::AuthFailed,
            I18nKey::CredentialExpired,
            I18nKey::RequestTimeout,
            I18nKey::ConnectionFailed,
            I18nKey::RateLimited,
            I18nKey::ContextOverflow,
            I18nKey::ToolExecutionFailed,
            I18nKey::ToolNotFound,
            I18nKey::SessionNotFound,
            I18nKey::SessionExpired,
            I18nKey::PlatformNotConnected,
            I18nKey::MessageSendFailed,
        ]
        .iter()
        .map(|&k| t(k))
        .collect();

        let unique: HashSet<_> = translations.iter().collect();
        assert_eq!(
            translations.len(),
            unique.len(),
            "存在重复的翻译文本，每个 I18nKey 应映射到唯一的字符串"
        );
    }
}
