//! Profile `config.yaml` 的共享读取层。
//!
//! 本模块只负责文件读取、空文档默认值和 YAML 反序列化；它不解析 Provider 运行策略、
//! 不读取 `.env`、不创建 HTTP client，也不访问数据库。各主题模块通过它复用同一套
//! 文件错误上下文，避免重复实现读取边界。

use std::{fs, path::Path};

use anyhow::{Context, Result};

use crate::{SagentPaths, profile_config::ProfileConfig, provider_config::RawProviderConfig};

/// 读取当前 Profile 的配置文本；缺失文件按空配置处理。
pub(crate) fn read_config_yaml(paths: &SagentPaths, purpose: &str) -> Result<String> {
    match fs::read_to_string(&paths.config_yaml) {
        Ok(content) => Ok(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error)
            .with_context(|| format!("读取 {purpose} 配置失败：{}", paths.config_yaml.display())),
    }
}

/// 一次读取并组合当前 Profile 的完整配置快照。
pub fn load_profile_config(paths: &SagentPaths) -> Result<ProfileConfig> {
    let content = read_config_yaml(paths, "Profile")?;
    let document = parse_provider_config(&content, &paths.config_yaml)?;
    let unknown_fields = collect_unknown_fields(&content, &paths.config_yaml)?;
    let config = ProfileConfig::from_document(document, unknown_fields)?;
    config.get_storage_descriptor().validate()?;
    Ok(config)
}

/// 将已经读取的 YAML 转为 Provider 配置，供公开摘要复用严格反序列化结果。
pub(crate) fn parse_provider_config(content: &str, path: &Path) -> Result<RawProviderConfig> {
    if content.trim().is_empty() {
        return Ok(RawProviderConfig::default());
    }
    serde_yaml::from_str(content)
        .with_context(|| format!("解析 Provider 配置失败：{}", path.display()))
}

/// 收集当前版本不认识的顶层字段，供 `PublicConfig` 诊断使用。
pub(crate) fn collect_unknown_fields(content: &str, path: &Path) -> Result<Vec<String>> {
    let mut unknown_fields = Vec::new();
    if !content.trim().is_empty() {
        let document: serde_yaml::Value = serde_yaml::from_str(content)
            .with_context(|| format!("解析公开配置失败：{}", path.display()))?;
        if let Some(mapping) = document.as_mapping() {
            for key in mapping.keys().filter_map(serde_yaml::Value::as_str) {
                if !is_known_field(key) {
                    unknown_fields.push(key.to_owned());
                }
            }
        }
    }
    unknown_fields.sort();
    Ok(unknown_fields)
}

/// 判断顶层字段是否已纳入当前 Profile 配置契约。
fn is_known_field(key: &str) -> bool {
    matches!(
        key,
        "provider"
            | "storage"
            | "model"
            | "base_url"
            | "api_key_env"
            | "key_env"
            | "providers"
            | "workspace"
    )
}
