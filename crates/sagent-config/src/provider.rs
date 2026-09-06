//! Profile-scoped Provider 配置与凭据解析。
//!
//! 非秘密字段来自当前 Profile 的 `config.yaml`，API key 只从当前 Profile 的 `.env`
//! 或进程环境读取。解析器不写数据库，也不把密钥放进可序列化配置结构。

use std::{collections::BTreeMap, env, fs, path::Path};

use anyhow::{Context, Result, bail};
use sagent_provider::OpenAiCompatibleProvider;
use serde::Deserialize;

use crate::SagentPaths;

/// 当前 Profile 中 Provider 配置的 YAML 表示。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProviderConfig {
    /// 例如 `openai-compatible`，也可以是 `providers` 下的自定义 key。
    pub provider: Option<String>,
    /// 可以是简单字符串，也可以是包含 `name`/`model` 等字段的对象。
    pub model: Option<ModelSetting>,
    /// 全局 OpenAI-compatible endpoint。
    pub base_url: Option<String>,
    #[serde(alias = "key_env")]
    pub api_key_env: Option<String>,
    /// 与 Python `providers:` 配置保持兼容：每个 key 描述一个自定义 OpenAI endpoint。
    #[serde(default)]
    pub providers: BTreeMap<String, UserProviderConfig>,
}

/// `model` 字段支持字符串和对象两种形态，便于兼容现有配置习惯。
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ModelSetting {
    Name(String),
    Detail(ModelDetail),
}

impl ModelSetting {
    fn values(&self) -> (Option<&str>, Option<&str>, Option<&str>, Option<&str>) {
        match self {
            Self::Name(value) => (Some(value), None, None, None),
            Self::Detail(detail) => (
                detail.name.as_deref().or(detail.model.as_deref()),
                detail.provider.as_deref(),
                detail.base_url.as_deref(),
                detail.api_key_env.as_deref().or(detail.key_env.as_deref()),
            ),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModelDetail {
    /// 模型显示名或请求名。
    pub name: Option<String>,
    /// 与 `name` 兼容的旧字段。
    pub model: Option<String>,
    /// 覆盖顶层 Provider 名称。
    pub provider: Option<String>,
    /// 覆盖顶层 endpoint。
    pub base_url: Option<String>,
    /// 凭据环境变量名。
    pub api_key_env: Option<String>,
    /// Python 配置兼容别名。
    pub key_env: Option<String>,
}

/// `providers.<name>` 中的一个用户自定义 Provider。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UserProviderConfig {
    /// UI 展示名称，不参与 endpoint 解析。
    pub name: Option<String>,
    /// Python 配置中的 API endpoint 字段。
    pub api: Option<String>,
    /// 兼容配置中的 URL 字段。
    pub url: Option<String>,
    /// OpenAI-compatible endpoint。
    pub base_url: Option<String>,
    #[serde(alias = "key_env")]
    pub api_key_env: Option<String>,
    /// Provider 使用的模型名。
    pub model: Option<String>,
}

impl UserProviderConfig {
    fn endpoint(&self) -> Option<&str> {
        self.api
            .as_deref()
            .or(self.url.as_deref())
            .or(self.base_url.as_deref())
    }
}

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

/// 读取当前 Profile 的 config.yaml。
pub fn read_provider_config(paths: &SagentPaths) -> Result<ProviderConfig> {
    let content = match fs::read_to_string(&paths.config_yaml) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("读取 Provider 配置失败：{}", paths.config_yaml.display())
            });
        }
    };
    if content.trim().is_empty() {
        return Ok(ProviderConfig::default());
    }
    serde_yaml::from_str(&content)
        .with_context(|| format!("解析 Provider 配置失败：{}", paths.config_yaml.display()))
}

/// 只解析当前 Profile 的 `.env`，并让文件值优先于同名进程环境变量。
fn read_env_value(paths: &SagentPaths, name: &str) -> Result<Option<String>> {
    if name.trim().is_empty() {
        bail!("API key 环境变量名不能为空");
    }
    if let Some(value) = read_dotenv_value(&paths.env_file, name)?
        && !value.trim().is_empty()
    {
        return Ok(Some(value));
    }
    Ok(env::var(name).ok().filter(|value| !value.trim().is_empty()))
}

fn read_dotenv_value(path: &Path, name: &str) -> Result<Option<String>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("读取凭据文件失败：{}", path.display()));
        }
    };
    for raw_line in content.lines() {
        let line = raw_line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, raw_value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != name {
            continue;
        }
        let value = raw_value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value)
            .to_owned();
        return Ok(Some(value));
    }
    Ok(None)
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

/// 解析 Profile 配置，但不创建 HTTP client。
pub fn resolve_provider_config(
    paths: &SagentPaths,
    provider_override: Option<&str>,
    model_override: Option<&str>,
) -> Result<ResolvedProviderConfig> {
    let config = read_provider_config(paths)?;
    let (model_from_config, provider_from_model, base_from_model, key_from_model) = config
        .model
        .as_ref()
        .map(ModelSetting::values)
        .unwrap_or_default();
    let provider = provider_override
        .or(provider_from_model)
        .or(config.provider.as_deref())
        .unwrap_or("openai-compatible")
        .trim()
        .to_lowercase();
    if provider.is_empty() {
        bail!("Provider 名称不能为空");
    }

    let custom = config.providers.get(&provider);
    let endpoint = custom
        .and_then(UserProviderConfig::endpoint)
        .or(base_from_model)
        .or(config.base_url.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Provider '{}' 未配置 base_url", provider))?;
    let model = model_override
        .or(custom.and_then(|value| value.model.as_deref()))
        .or(model_from_config)
        .unwrap_or("")
        .trim()
        .to_owned();
    if model.is_empty() {
        bail!("Provider '{}' 未配置 model", provider);
    }

    let key_name = custom
        .and_then(|value| value.api_key_env.as_deref())
        .or(key_from_model)
        .or(config.api_key_env.as_deref())
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

/// 将当前 Profile 配置解析为 OpenAI-compatible Provider。
pub fn resolve_openai_provider(
    paths: &SagentPaths,
    provider_override: Option<&str>,
    model_override: Option<&str>,
) -> Result<ResolvedProvider> {
    let resolved = resolve_provider_config(paths, provider_override, model_override)?;
    if resolved.provider != "openai-compatible"
        && !read_provider_config(paths)?
            .providers
            .contains_key(&resolved.provider)
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
    use super::{resolve_openai_provider, resolve_provider_config};
    use crate::{normalize_profile_name, resolve_paths};
    use std::{fs, path::PathBuf};

    fn test_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "sagent-provider-config-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试目录");
        root
    }

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

        let resolved = resolve_provider_config(&paths, None, None).expect("配置应能解析");
        assert_eq!(resolved.provider, "openai-compatible");
        assert_eq!(resolved.model, "test-model");
        assert_eq!(resolved.base_url, "http://127.0.0.1:1/v1");
        assert_eq!(resolved.api_key, "profile-key");
        let resolved = resolve_openai_provider(&paths, None, None).unwrap();
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
        assert_eq!(
            resolve_provider_config(&coder_paths, None, None)
                .unwrap()
                .api_key,
            "coder-key"
        );
        assert_eq!(
            resolve_provider_config(&writer_paths, None, None)
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
        let error = resolve_provider_config(&paths, None, None).unwrap_err();
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
        let missing_key = resolve_provider_config(&paths, None, None).unwrap_err();
        assert!(missing_key.to_string().contains("MISSING_PROFILE_KEY"));
        fs::write(root.join(".env"), "MISSING_PROFILE_KEY=key\n").unwrap();
        let invalid_url = resolve_openai_provider(&paths, None, None).unwrap_err();
        assert!(invalid_url.to_string().contains("URL"));

        fs::write(
            root.join("config.yaml"),
            "provider: unsupported\nmodel: test-model\nbase_url: http://127.0.0.1:1/v1\napi_key_env: MISSING_PROFILE_KEY\n",
        )
        .unwrap();
        let unknown = resolve_openai_provider(&paths, None, None).unwrap_err();
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
        let resolved =
            resolve_provider_config(&paths, Some("openai-compatible"), Some("cli-model")).unwrap();
        assert_eq!(resolved.model, "cli-model");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn supports_python_style_custom_provider_fields() {
        let root = test_root("custom");
        fs::write(
            root.join("config.yaml"),
            "provider: local\nmodel: local-model\nproviders:\n  local:\n    name: Local\n    api: http://127.0.0.1:1/v1\n    key_env: LOCAL_KEY\n",
        )
        .unwrap();
        fs::write(root.join(".env"), "LOCAL_KEY=local-key\n").unwrap();
        let profile = normalize_profile_name("default").unwrap();
        let paths = resolve_paths(Some(&root), Some(&profile)).unwrap();
        let result = resolve_provider_config(&paths, None, None).unwrap();
        assert_eq!(result.provider, "local");
        assert_eq!(result.base_url, "http://127.0.0.1:1/v1");
        assert_eq!(result.api_key, "local-key");
        fs::remove_dir_all(root).unwrap();
    }
}
