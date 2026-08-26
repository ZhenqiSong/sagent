//! 环境变量工具模块。
//!
//! 提供环境变量的安全默认值设置辅助函数。
//!
//! @author   songzq
//! @created  2026-08-26

/// 仅当环境变量尚未设置时写入默认值。
///
/// 若 `key` 已存在于环境中，则保持原值不变；否则将其设置为 `value`。
///
/// 参数:
/// - `key`: 环境变量名
/// - `value`: 未设置时的默认值
///
/// 示例:
/// ```
/// sagent_core::env::set_env_default("SAGENT_HOME", "/tmp/sagent");
/// assert_eq!(std::env::var("SAGENT_HOME").unwrap(), "/tmp/sagent");
/// ```
pub fn set_env_default(key: &str, value: &str) {
    if std::env::var_os(key).is_none() {
        std::env::set_var(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::set_env_default;

    /// 生成测试专用的唯一环境变量名，避免并行测试互相干扰。
    fn unique_key(prefix: &str) -> String {
        format!(
            "SAGENT_TEST_{}_{}_{}",
            prefix,
            std::process::id(),
            thread_unique_suffix()
        )
    }

    /// 生成当前线程内唯一的后缀（基于时间与线程名）。
    fn thread_unique_suffix() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        format!("{}", nanos)
    }

    /// 测试：当变量未设置时，set_env_default 会写入默认值。
    #[test]
    fn test_sets_value_when_unset() {
        let key = unique_key("UNSET");
        set_env_default(&key, "default-value");
        assert_eq!(
            std::env::var(&key).unwrap(),
            "default-value",
            "未设置的变量应被写入默认值"
        );
        // 清理，避免污染其他测试
        std::env::remove_var(&key);
    }

    /// 测试：当变量已设置时，set_env_default 保持原值不变。
    #[test]
    fn test_keeps_existing_value() {
        let key = unique_key("SET");
        std::env::set_var(&key, "original-value");
        set_env_default(&key, "default-value");
        assert_eq!(
            std::env::var(&key).unwrap(),
            "original-value",
            "已设置的变量不应被默认值覆盖"
        );
        // 清理
        std::env::remove_var(&key);
    }

    /// 测试：变量值被设置为空字符串时视为已设置，不写入默认值。
    #[test]
    fn test_empty_existing_value_is_preserved() {
        let key = unique_key("EMPTY");
        std::env::set_var(&key, "");
        set_env_default(&key, "default-value");
        assert_eq!(
            std::env::var(&key).unwrap(),
            "",
            "空字符串也是已设置状态，不应覆盖"
        );
        std::env::remove_var(&key);
    }

    /// 测试：重复调用对已设置的变量是幂等的。
    #[test]
    fn test_idempotent_when_called_multiple_times() {
        let key = unique_key("IDEMPOTENT");
        set_env_default(&key, "v1");
        set_env_default(&key, "v2");
        assert_eq!(std::env::var(&key).unwrap(), "v1", "首次写入的值应保持不变");
        std::env::remove_var(&key);
    }
}
