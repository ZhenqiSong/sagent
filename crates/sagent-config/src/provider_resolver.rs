//! Provider 配置解析与 OpenAI-compatible 实例化。
//!
//! 本模块负责把已读取的 Profile 配置和凭据引用解析为可用 Provider；配置模型、文件
//! 读取和凭据文本解析分别由相邻模块负责，解析成功后才创建 HTTP client。

use anyhow::{Result, bail};
use sagent_provider::OpenAiCompatibleProvider;

use crate::{
    SagentPaths,
    credentials::read_env_value,
    profile_config::ProfileConfig,
    provider_config::{ModelSetting, UserProviderConfig},
};

/// 已解析的模型配置和 Provider 实例。
///
/// API key 被消费进 `OpenAiCompatibleProvider`，不会作为公开字段暴露。
pub struct ResolvedProvider {
    /// 最终采用的 Provider 名称。
    pub provider: String,
    /// 最终采用的模型名称。
    pub model: String,
    /// 已绑定密钥的 HTTP 客户端。
    pub client: OpenAiCompatibleProvider,
}

impl std::fmt::Debug for ResolvedProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedProvider")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("client", &"<redacted>")
            .finish()
    }
}

/// 已解析的 Provider 配置。
///
/// `api_key` 保持私有，避免调用方意外序列化或打印密钥；只有本模块负责把它消费进
/// `OpenAiCompatibleProvider`。
pub struct ResolvedProviderConfig {
    /// 解析后的 Provider 名称。
    pub provider: String,
    /// 解析后的模型名称。
    pub model: String,
    /// 解析后的 endpoint。
    pub base_url: String,
    api_key: String,
}

impl std::fmt::Debug for ResolvedProviderConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedProviderConfig")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

/// 从已经加载的 Profile 快照解析 Provider 配置，不重新读取配置文件。
pub fn resolve_provider_config_from_config(
    paths: &SagentPaths,
    config: &ProfileConfig,
    provider_override: Option<&str>,
    model_override: Option<&str>,
) -> Result<ResolvedProviderConfig> {
    let provider_config = &config.provider;
    let (model_from_config, provider_from_model, base_from_model, key_from_model) = provider_config
        .model
        .as_ref()
        .map(ModelSetting::values)
        .unwrap_or_default();
    let provider = provider_override
        .or(provider_from_model)
        .or(provider_config
            .provider
            .as_ref()
            .map(|value| value.as_str()))
        .unwrap_or("openai-compatible")
        .trim()
        .to_lowercase();
    if provider.is_empty() {
        bail!("Provider 名称不能为空");
    }

    let custom = provider_config.providers.get(provider.as_str());
    let endpoint = custom
        .and_then(UserProviderConfig::endpoint)
        .or(base_from_model)
        .or(provider_config.base_url.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Provider '{}' 未配置 base_url", provider))?;
    let model = model_override
        .or(custom.and_then(|value| value.model.as_ref().map(|model| model.as_str())))
        .or(model_from_config)
        .unwrap_or("")
        .trim()
        .to_owned();
    if model.is_empty() {
        bail!("Provider '{}' 未配置 model", provider);
    }

    let key_name = custom
        .and_then(|value| {
            value
                .api_key_env
                .as_ref()
                .map(|reference| reference.as_str())
        })
        .or(key_from_model)
        .or(provider_config
            .api_key_env
            .as_ref()
            .map(|value| value.as_str()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Provider '{}' 未配置 api_key_env", provider))?;
    let api_key = read_env_value(paths, key_name)?
        .ok_or_else(|| anyhow::anyhow!("未配置 Provider API key：{}", key_name))?;
    Ok(ResolvedProviderConfig {
        provider,
        model,
        base_url: endpoint.to_owned(),
        api_key,
    })
}

/// 从已经加载的 Profile 快照创建 OpenAI-compatible Provider，不重新读取配置文件。
pub fn resolve_openai_provider_from_config(
    paths: &SagentPaths,
    config: &ProfileConfig,
    provider_override: Option<&str>,
    model_override: Option<&str>,
) -> Result<ResolvedProvider> {
    let resolved =
        resolve_provider_config_from_config(paths, config, provider_override, model_override)?;
    if resolved.provider != "openai-compatible"
        && !config
            .provider
            .providers
            .contains_key(resolved.provider.as_str())
    {
        bail!("不支持的 Provider：{}", resolved.provider);
    }
    let client = OpenAiCompatibleProvider::from_endpoint(&resolved.base_url, resolved.api_key)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(ResolvedProvider {
        provider: resolved.provider,
        model: resolved.model,
        client,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{resolve_openai_provider_from_config, resolve_provider_config_from_config};
    use crate::{
        load_profile_config, normalize_profile_name, resolve_paths, test_support::test_root,
    };

    #[test]
    fn resolves_default_profile_from_yaml_and_dotenv() {
        let root = test_root("default");
        fs::write(
            root.join("config.yaml"),
            "provider: openai-compatible\nmodel: test-model\nbase_url: http://127.0.0.1:1/v1\napi_key_env: OPENAI_API_KEY\n",
        )
        .expect("应能写配置");
        fs::write(root.join(".env"), "OPENAI_API_KEY=profile-key\n").expect("应能写凭据");
        let profile = normalize_profile_name("default").unwrap();
        let paths = resolve_paths(Some(&root), Some(&profile)).unwrap();
        let config = load_profile_config(&paths).expect("应能加载完整 Profile 快照");

        let resolved =
            resolve_provider_config_from_config(&paths, &config, None, None).expect("配置应能解析");
        assert_eq!(resolved.provider, "openai-compatible");
        assert_eq!(resolved.model, "test-model");
        assert_eq!(resolved.base_url, "http://127.0.0.1:1/v1");
        assert_eq!(resolved.api_key, "profile-key");
        let resolved = resolve_openai_provider_from_config(&paths, &config, None, None).unwrap();
        assert_eq!(resolved.model, "test-model");
        assert!(!format!("{resolved:?}").contains("profile-key"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn named_profiles_do_not_read_each_others_dotenv() {
        let root = test_root("isolation");
        let coder_home = root.join("profiles").join("coder");
        let writer_home = root.join("profiles").join("writer");
        fs::create_dir_all(&coder_home).unwrap();
        fs::create_dir_all(&writer_home).unwrap();
        for home in [&coder_home, &writer_home] {
            fs::write(
                home.join("config.yaml"),
                "provider: openai-compatible\nmodel: test-model\nbase_url: http://127.0.0.1:1/v1\napi_key_env: PROFILE_KEY\n",
            )
            .unwrap();
        }
        fs::write(coder_home.join(".env"), "PROFILE_KEY=coder-key\n").unwrap();
        fs::write(writer_home.join(".env"), "PROFILE_KEY=writer-key\n").unwrap();
        let coder = normalize_profile_name("coder").unwrap();
        let writer = normalize_profile_name("writer").unwrap();
        let coder_paths = resolve_paths(Some(&root), Some(&coder)).unwrap();
        let writer_paths = resolve_paths(Some(&root), Some(&writer)).unwrap();
        let coder_config = load_profile_config(&coder_paths).expect("应能加载 coder 快照");
        let writer_config = load_profile_config(&writer_paths).expect("应能加载 writer 快照");
        assert_eq!(
            resolve_provider_config_from_config(&coder_paths, &coder_config, None, None)
                .unwrap()
                .api_key,
            "coder-key"
        );
        assert_eq!(
            resolve_provider_config_from_config(&writer_paths, &writer_config, None, None)
                .unwrap()
                .api_key,
            "writer-key"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_missing_model_endpoint_and_key() {
        let root = test_root("invalid");
        fs::write(root.join("config.yaml"), "provider: openai-compatible\n").unwrap();
        let profile = normalize_profile_name("default").unwrap();
        let paths = resolve_paths(Some(&root), Some(&profile)).unwrap();
        let config = load_profile_config(&paths).expect("应能加载不完整 Profile 快照");
        let error = resolve_provider_config_from_config(&paths, &config, None, None).unwrap_err();
        assert!(error.to_string().contains("base_url"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_missing_key_invalid_url_and_unknown_provider() {
        let root = test_root("validation");
        fs::write(
            root.join("config.yaml"),
            "provider: openai-compatible\nmodel: test-model\nbase_url: not-a-url\napi_key_env: MISSING_PROFILE_KEY\n",
        )
        .unwrap();
        let profile = normalize_profile_name("default").unwrap();
        let paths = resolve_paths(Some(&root), Some(&profile)).unwrap();
        let config = load_profile_config(&paths).expect("应能加载 Provider 快照");
        let missing_key =
            resolve_provider_config_from_config(&paths, &config, None, None).unwrap_err();
        assert!(missing_key.to_string().contains("MISSING_PROFILE_KEY"));
        fs::write(root.join(".env"), "MISSING_PROFILE_KEY=key\n").unwrap();
        let invalid_url =
            resolve_openai_provider_from_config(&paths, &config, None, None).unwrap_err();
        assert!(invalid_url.to_string().contains("URL"));

        fs::write(
            root.join("config.yaml"),
            "provider: unsupported\nmodel: test-model\nbase_url: http://127.0.0.1:1/v1\napi_key_env: MISSING_PROFILE_KEY\n",
        )
        .unwrap();
        let config = load_profile_config(&paths).expect("应能加载不支持 Provider 的快照");
        let unknown = resolve_openai_provider_from_config(&paths, &config, None, None).unwrap_err();
        assert!(unknown.to_string().contains("不支持"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_provider_and_model_override_profile_defaults() {
        let root = test_root("override");
        fs::write(
            root.join("config.yaml"),
            "provider: openai-compatible\nmodel: profile-model\nbase_url: http://127.0.0.1:1/v1\napi_key_env: OVERRIDE_KEY\n",
        )
        .unwrap();
        fs::write(root.join(".env"), "OVERRIDE_KEY=key\n").unwrap();
        let profile = normalize_profile_name("default").unwrap();
        let paths = resolve_paths(Some(&root), Some(&profile)).unwrap();
        let config = load_profile_config(&paths).expect("应能加载 Override 快照");
        let resolved = resolve_provider_config_from_config(
            &paths,
            &config,
            Some("openai-compatible"),
            Some("cli-model"),
        )
        .unwrap();
        assert_eq!(resolved.model, "cli-model");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn supports_custom_provider_fields() {
        let root = test_root("custom");
        fs::write(
            root.join("config.yaml"),
            "provider: local\nmodel: local-model\nproviders:\n  local:\n    name: Local\n    api: http://127.0.0.1:1/v1\n    key_env: LOCAL_KEY\n",
        )
        .unwrap();
        fs::write(root.join(".env"), "LOCAL_KEY=local-key\n").unwrap();
        let profile = normalize_profile_name("default").unwrap();
        let paths = resolve_paths(Some(&root), Some(&profile)).unwrap();
        let config = load_profile_config(&paths).expect("应能加载自定义 Provider 快照");
        let result = resolve_provider_config_from_config(&paths, &config, None, None).unwrap();
        assert_eq!(result.provider, "local");
        assert_eq!(result.base_url, "http://127.0.0.1:1/v1");
        assert_eq!(result.api_key, "local-key");
        fs::remove_dir_all(root).unwrap();
    }
}
