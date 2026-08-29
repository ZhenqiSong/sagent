//! Profile 名称的规范化与安全校验。
//!
//! 作者：SongZQ
//! 创建日期：2026-08-29

use anyhow::{Result, bail};

/// 已规范化、可安全用于 Profile 路径解析的名称。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileName(String);

impl ProfileName {
    /// 返回内部字符串的只读引用（零拷贝，不转移所有权）。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 规范化 profile 名称并校验合法性。
///
/// 先去除首尾空白并转为小写；空名称非法；`default` 为保留名称直接放行。
/// 其余名称必须满足：长度不超过 64，只能由小写字母、数字组成，
/// `_` 和 `-` 允许出现但不能作为首字符。
///
/// 成功返回规范化后的 `ProfileName`，失败返回错误。
pub fn normalize_profile_name(value: &str) -> Result<ProfileName> {
    // Profile 名称最终作为目录名使用，统一小写可避免跨平台的大小写歧义。
    let normalized = value.trim().to_lowercase();

    if normalized.is_empty() {
        bail!("profile 名称不能为空");
    }

    if normalized == "default" {
        // default 对应根数据目录，而不是 `profiles/default` 子目录。
        return Ok(ProfileName(normalized));
    }

    let valid = normalized.len() <= 64
        && normalized.chars().enumerate().all(|(index, ch)| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || (index > 0 && matches!(ch, '_' | '-'))
        });

    if !valid {
        // 拒绝路径分隔符、空格和非 ASCII 字符，避免目录穿越及跨平台路径行为不一致。
        bail!("profile 名称只能包含小写字母、数字、下划线和连字符，且长度不能超过 64 个字符")
    }

    Ok(ProfileName(normalized))
}

#[cfg(test)]
mod tests {
    use super::normalize_profile_name;

    #[test]
    fn normalizes_whitespace_and_ascii_case() {
        let profile = normalize_profile_name("  Coder-01  ").expect("名称应合法");

        assert_eq!(profile.as_str(), "coder-01");
    }

    #[test]
    fn accepts_default_case_insensitively() {
        let profile = normalize_profile_name(" DEFAULT ").expect("default 应合法");

        assert_eq!(profile.as_str(), "default");
    }

    #[test]
    fn rejects_empty_and_unsafe_names() {
        for value in [
            "",
            "   ",
            "-coder",
            "_coder",
            "coder/name",
            "coder name",
            "中文",
        ] {
            assert!(normalize_profile_name(value).is_err(), "{value:?} 应被拒绝");
        }
    }

    #[test]
    fn rejects_names_longer_than_sixty_four_bytes() {
        let value = "a".repeat(65);

        assert!(normalize_profile_name(&value).is_err());
    }
}
