//! Provider descriptor 解析与 OpenAI-compatible 实例化。
//!
//! 本模块把已加载的 Profile 快照解析为无秘密的 Provider 选择；配置模型、文件读取和
//! 凭据文本解析分别由相邻模块负责。只有暂时保留的 OpenAI 装配入口才读取 secret，
//! 纯 descriptor resolver 不接受路径，也不会触发文件或网络 I/O。

use anyhow::{Result, bail};
use sagent_provider::OpenAiCompatibleProvider;

use crate::{
    SagentPaths,
    credentials::read_env_value,
    profile_config::ProfileConfig,
    provider_config::{ModelSetting, UserProviderConfig},
    provider_identity::{CredentialReference, ModelId, ProviderKind},
};

const DEFAULT_PROVIDER_KIND: &str = "openai-compatible";

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
/// 该类型只包含无秘密 descriptor；credential reference 供 bootstrap/Factory 查找 secret，
/// 但不会把 secret 值带入配置快照或 Debug 输出。
pub struct ResolvedProviderConfig {
    /// 解析后的 Provider 名称。
    pub provider: ProviderKind,
    /// 解析后的模型名称。
    pub model: ModelId,
    /// 解析后的 endpoint。
    pub base_url: String,
    /// 解析后的凭据引用；这里只保存引用，不保存 secret 值。
    pub credential_reference: CredentialReference,
}

impl std::fmt::Debug for ResolvedProviderConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedProviderConfig")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("credential_reference", &"<redacted>")
            .finish()
    }
}

/// 从已经加载的 Profile 快照解析无秘密 Provider 配置，不读取文件或环境变量。
///
/// 返回值只包含已校验的 Provider/model identity、endpoint 和 credential reference。secret
/// 的读取属于 bootstrap/Factory 边界，由 `resolve_openai_provider_from_config` 暂时完成。
pub fn resolve_provider_config_from_snapshot(
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
    let default_provider = ProviderKind::try_new(DEFAULT_PROVIDER_KIND)?;
    let provider_override = provider_override.map(ProviderKind::try_new).transpose()?;
    let provider = provider_override
        .or_else(|| provider_from_model.cloned())
        .or_else(|| provider_config.provider.clone())
        .unwrap_or(default_provider);
    let custom = provider_config.providers.get(&provider);
    let endpoint = custom
        .and_then(UserProviderConfig::endpoint)
        .or(base_from_model)
        .or(provider_config.base_url.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Provider '{}' 未配置 base_url", provider))?;
    let model_override = model_override.map(ModelId::try_new).transpose()?;
    let model = model_override
        .or_else(|| custom.and_then(|value| value.model.clone()))
        .or_else(|| model_from_config.cloned())
        .ok_or_else(|| anyhow::anyhow!("Provider '{}' 未配置 model", provider))?;
    let credential_reference = custom
        .and_then(|value| value.api_key_env.clone())
        .or_else(|| key_from_model.cloned())
        .or_else(|| provider_config.api_key_env.clone())
        .ok_or_else(|| anyhow::anyhow!("Provider '{}' 未配置 api_key_env", provider))?;
    Ok(ResolvedProviderConfig {
        provider,
        model,
        base_url: endpoint.to_owned(),
        credential_reference,
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
        resolve_provider_config_from_snapshot(config, provider_override, model_override)?;
    if resolved.provider.as_str() != DEFAULT_PROVIDER_KIND
        && !config.provider.providers.contains_key(&resolved.provider)
    {
        bail!("不支持的 Provider：{}", resolved.provider);
    }
    let api_key =
        read_env_value(paths, resolved.credential_reference.as_str())?.ok_or_else(|| {
            anyhow::anyhow!("未配置 Provider API key：{}", resolved.credential_reference)
        })?;
    let client = OpenAiCompatibleProvider::from_endpoint(&resolved.base_url, api_key)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(ResolvedProvider {
        provider: resolved.provider.as_str().to_owned(),
        model: resolved.model.as_str().to_owned(),
        client,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{resolve_openai_provider_from_config, resolve_provider_config_from_snapshot};
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

        fs::remove_file(&paths.env_file).expect("纯 resolver 不应依赖 .env");
        let resolved = resolve_provider_config_from_snapshot(&config, None, None)
            .expect("配置 descriptor 应能解析");
        assert_eq!(resolved.provider.as_str(), "openai-compatible");
        assert_eq!(resolved.model.as_str(), "test-model");
        assert_eq!(resolved.base_url, "http://127.0.0.1:1/v1");
        assert_eq!(resolved.credential_reference.as_str(), "OPENAI_API_KEY");
        fs::write(&paths.env_file, "OPENAI_API_KEY=profile-key\n").expect("应能恢复凭据 fixture");
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
        fs::write(coder_home.join(".env"), "PROFILE_KEY=\n").unwrap();
        fs::write(writer_home.join(".env"), "PROFILE_KEY=writer-key\n").unwrap();
        let coder = normalize_profile_name("coder").unwrap();
        let writer = normalize_profile_name("writer").unwrap();
        let coder_paths = resolve_paths(Some(&root), Some(&coder)).unwrap();
        let writer_paths = resolve_paths(Some(&root), Some(&writer)).unwrap();
        let coder_config = load_profile_config(&coder_paths).expect("应能加载 coder 快照");
        let writer_config = load_profile_config(&writer_paths).expect("应能加载 writer 快照");
        let coder_error =
            resolve_openai_provider_from_config(&coder_paths, &coder_config, None, None)
                .expect_err("coder 的空凭据不应回退到 writer Profile");
        assert!(coder_error.to_string().contains("PROFILE_KEY"));
        resolve_openai_provider_from_config(&writer_paths, &writer_config, None, None)
            .expect("writer Profile 应读取自己的凭据");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_missing_model_endpoint_and_key() {
        let root = test_root("invalid");
        fs::write(root.join("config.yaml"), "provider: openai-compatible\n").unwrap();
        let profile = normalize_profile_name("default").unwrap();
        let paths = resolve_paths(Some(&root), Some(&profile)).unwrap();
        let config = load_profile_config(&paths).expect("应能加载不完整 Profile 快照");
        let error = resolve_provider_config_from_snapshot(&config, None, None).unwrap_err();
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
        let snapshot = resolve_provider_config_from_snapshot(&config, None, None)
            .expect("纯 resolver 应只解析 credential reference");
        assert_eq!(
            snapshot.credential_reference.as_str(),
            "MISSING_PROFILE_KEY"
        );
        let missing_key =
            resolve_openai_provider_from_config(&paths, &config, None, None).unwrap_err();
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
        let resolved = resolve_provider_config_from_snapshot(
            &config,
            Some("openai-compatible"),
            Some("cli-model"),
        )
        .unwrap();
        assert_eq!(resolved.model.as_str(), "cli-model");
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
        let result = resolve_provider_config_from_snapshot(&config, None, None).unwrap();
        assert_eq!(result.provider.as_str(), "local");
        assert_eq!(result.base_url, "http://127.0.0.1:1/v1");
        assert_eq!(result.credential_reference.as_str(), "LOCAL_KEY");
        fs::remove_dir_all(root).unwrap();
    }
}
