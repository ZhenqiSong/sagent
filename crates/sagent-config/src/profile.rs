//! Profile 名称的规范化与安全校验。
//!
//! 作者：SongZQ
//! 创建日期：2026-08-29

use std::{fs, path::Path};

use anyhow::{Context, Result, bail};

/// 已规范化、可安全用于 Profile 路径解析的名称。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileName(String);

impl ProfileName {
    /// 返回内部字符串的只读引用（零拷贝，不转移所有权）。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 返回指定 Sagent 根目录中可用的 profile。
///
/// default 永远存在于结果首位，它直接对应根目录；命名 profile 则来自
/// \`<root>/profiles/\` 下名称合法的直接子目录。无效目录不会作为 profile
/// 暴露给调用方，避免历史残留或手工创建的路径绕过名称校验。
pub fn list_profile_names(root: &Path) -> Result<Vec<ProfileName>> {
    if !root.is_absolute() {
        bail!("Sagent 根目录必须是绝对路径");
    }

    let mut profiles = vec![ProfileName("default".to_owned())];
    let profiles_dir = root.join("profiles");
    if !profiles_dir.exists() {
        return Ok(profiles);
    }

    let entries = fs::read_dir(&profiles_dir)
        .with_context(|| format!("读取 profile 目录失败：{}", profiles_dir.display()))?;
    for entry in entries {
        let entry = entry.context("读取 profile 目录项失败")?;
        if !entry
            .file_type()
            .context("读取 profile 目录项类型失败")?
            .is_dir()
        {
            continue;
        }

        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(profile) = normalize_profile_name(&name) else {
            continue;
        };
        if profile.as_str() != "default" {
            profiles.push(profile);
        }
    }

    profiles[1..].sort_by(|left, right| left.as_str().cmp(right.as_str()));
    Ok(profiles)
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
    use std::fs;

    use super::{list_profile_names, normalize_profile_name};

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

    #[test]
    fn lists_default_and_valid_named_profile_directories() {
        let root = std::env::temp_dir().join(format!("sagent-profile-list-{}", std::process::id()));
        let profiles = root.join("profiles");
        fs::create_dir_all(profiles.join("Zebra")).expect("应能创建 profile 目录");
        fs::create_dir_all(profiles.join("coder")).expect("应能创建 profile 目录");
        fs::create_dir_all(profiles.join("-unsafe")).expect("应能创建无效目录");
        fs::write(profiles.join("not-a-profile"), "file").expect("应能创建普通文件");

        let names = list_profile_names(&root).expect("应能列出 profile");

        assert_eq!(
            names.iter().map(|name| name.as_str()).collect::<Vec<_>>(),
            vec!["default", "coder", "zebra"]
        );
        fs::remove_dir_all(root).expect("应能清理 profile 测试目录");
    }

    #[test]
    fn listing_a_new_root_still_returns_default_profile() {
        let root =
            std::env::temp_dir().join(format!("sagent-profile-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");

        let names = list_profile_names(&root).expect("应能列出默认 profile");

        assert_eq!(
            names.iter().map(|name| name.as_str()).collect::<Vec<_>>(),
            vec!["default"]
        );
        fs::remove_dir_all(root).expect("应能清理 profile 测试目录");
    }
}
