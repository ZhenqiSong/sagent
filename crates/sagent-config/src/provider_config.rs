//! Profile 配置的原始 YAML 模型与规范化 Provider descriptor。
//!
//! 本模块刻意把 serde 输入模型和运行时配置意图分开：`RawProviderConfig` 及其子类型
//! 只存在于 parser 边界并承载兼容 alias；`ProviderDescriptor` 只保存归一化后的字段，
//! 不读取文件、环境变量或网络，也不创建 Provider 实例。

use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Result, bail};
use serde::Deserialize;

use crate::storage::StorageDescriptor;

/// parser 层使用的 Profile `config.yaml` 原始文档模型。
///
/// 该类型及其字段不对 crate 外公开，避免 alias、YAML 字段名和运行时 descriptor
/// 混入同一个公共契约。转换为 descriptor 时会一次性完成字段归一化。
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct RawProviderConfig {
    /// Profile 的持久化后端描述；缺省时由 StorageDescriptor 使用本地 SQLite。
    #[serde(default)]
    pub(crate) storage: Option<StorageDescriptor>,
    /// 例如 `openai-compatible`，也可以是 `providers` 下的自定义 key。
    pub(crate) provider: Option<String>,
    /// 可以是简单字符串，也可以是包含 `name`/`model` 等字段的对象。
    pub(crate) model: Option<RawModelSetting>,
    /// 全局 OpenAI-compatible endpoint。
    pub(crate) base_url: Option<String>,
    /// parser 层将兼容的 `key_env` 归一化为 canonical credential reference。
    #[serde(alias = "key_env")]
    pub(crate) api_key_env: Option<String>,
    /// 每个 key 描述一个自定义 OpenAI endpoint；子字段 alias 只在 parser 层存在。
    #[serde(default)]
    pub(crate) providers: BTreeMap<String, RawUserProviderConfig>,
    /// 工具可访问的 workspace 根；相对路径锚定当前 Profile。
    pub(crate) workspace: Option<PathBuf>,
}

impl RawProviderConfig {
    /// 将原始 YAML 一次性转换为存储、Provider 和 workspace 三个规范化 descriptor。
    pub(crate) fn into_descriptors(
        self,
    ) -> Result<(StorageDescriptor, ProviderDescriptor, WorkspaceDescriptor)> {
        let provider = ProviderDescriptor::from_raw(&self)?;
        Ok((
            self.storage.unwrap_or_default(),
            provider,
            WorkspaceDescriptor {
                path: self.workspace,
            },
        ))
    }
}

/// 原始 YAML 中的 `model` 字段形态；只在 parser 边界使用。
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum RawModelSetting {
    /// 直接使用的模型名。
    Name(String),
    /// 包含 Provider、endpoint 或凭据覆盖项的详细模型设置。
    Detail(RawModelDetail),
}

/// 原始 YAML 的模型对象，兼容字段只保留在 parser 类型上。
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct RawModelDetail {
    /// 模型显示名或请求名。
    pub(crate) name: Option<String>,
    /// 与 `name` 兼容的旧字段。
    pub(crate) model: Option<String>,
    /// 覆盖顶层 Provider 名称。
    pub(crate) provider: Option<String>,
    /// 覆盖顶层 endpoint。
    pub(crate) base_url: Option<String>,
    /// parser 层将 `key_env` 归一化为同一个 credential reference 字段。
    #[serde(alias = "key_env")]
    pub(crate) api_key_env: Option<String>,
}

/// 原始 YAML 的 `providers.<name>` 项，endpoint alias 只在 parser 层使用。
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct RawUserProviderConfig {
    /// UI 展示名称，不参与 endpoint 解析。
    pub(crate) name: Option<String>,
    /// 兼容配置中的 API endpoint 字段。
    pub(crate) api: Option<String>,
    /// 兼容配置中的 URL 字段。
    pub(crate) url: Option<String>,
    /// OpenAI-compatible endpoint。
    pub(crate) base_url: Option<String>,
    /// parser 层将兼容的 `key_env` 归一化为 canonical credential reference。
    #[serde(alias = "key_env")]
    pub(crate) api_key_env: Option<String>,
    /// Provider 使用的模型名。
    pub(crate) model: Option<String>,
}

/// 已归一化并完成结构校验的 Provider 意图描述。
///
/// 该类型不保留 YAML alias，也不携带 API key 值；Factory/Resolver 只能依据这些不可变
/// 字段创建运行时 Provider。字段完整性在 parser 转换边界检查，Provider 可用性仍由
/// 具体 resolver 根据选择的 Provider 决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDescriptor {
    /// 例如 `openai-compatible`，也可以是 `providers` 下的自定义 key。
    pub provider: Option<String>,
    /// 模型名称或模型级覆盖配置。
    pub model: Option<ModelSetting>,
    /// 归一化后的全局 OpenAI-compatible endpoint。
    pub base_url: Option<String>,
    /// API key 所在的环境变量名；只保存引用，不保存值。
    pub api_key_env: Option<String>,
    /// 按 provider 名称索引的归一化自定义 Provider 配置。
    pub providers: BTreeMap<String, UserProviderConfig>,
}

impl ProviderDescriptor {
    /// 从 parser 原始模型创建规范化 descriptor；所有 alias 在此边界被消费。
    fn from_raw(raw: &RawProviderConfig) -> Result<Self> {
        let provider = normalize_optional(raw.provider.clone(), "provider")?;
        let model = raw.model.clone().map(ModelSetting::from_raw).transpose()?;
        let base_url = normalize_optional(raw.base_url.clone(), "base_url")?;
        let api_key_env = normalize_optional(raw.api_key_env.clone(), "api_key_env")?;
        let providers = normalize_provider_map(raw.providers.clone())?;

        let descriptor = Self {
            provider,
            model,
            base_url,
            api_key_env,
            providers,
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    /// 检查 descriptor 已经完成文本归一化，避免空值在 resolver 深处才暴露。
    pub(crate) fn validate(&self) -> Result<()> {
        if self.provider.as_deref().is_some_and(str::is_empty) {
            bail!("Provider 名称不能为空");
        }
        if self.base_url.as_deref().is_some_and(str::is_empty) {
            bail!("Provider base_url 不能为空");
        }
        if self.api_key_env.as_deref().is_some_and(str::is_empty) {
            bail!("Provider api_key_env 不能为空");
        }
        if let Some(model) = &self.model {
            model.validate()?;
        }
        for (name, provider) in &self.providers {
            if name.is_empty() {
                bail!("自定义 Provider 名称不能为空");
            }
            provider.validate(name)?;
        }
        Ok(())
    }
}

/// 已归一化的 workspace 意图描述。
#[derive(Debug, Clone, Default)]
pub struct WorkspaceDescriptor {
    /// workspace 路径；相对路径由 resolver 锚定到当前 Profile。
    pub path: Option<PathBuf>,
}

/// `model` 字段的规范化形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSetting {
    /// 直接使用的模型名。
    Name(String),
    /// 包含可选覆盖项的模型设置。
    Detail(ModelDetail),
}

impl ModelSetting {
    /// 将 parser 原始模型转换为只含 canonical 字段的模型设置。
    fn from_raw(raw: RawModelSetting) -> Result<Self> {
        match raw {
            RawModelSetting::Name(value) => Ok(Self::Name(normalize_required(value, "model")?)),
            RawModelSetting::Detail(detail) => Ok(Self::Detail(ModelDetail {
                model: normalize_optional(detail.name.or(detail.model), "model")?,
                provider: normalize_optional(detail.provider, "model.provider")?,
                base_url: normalize_optional(detail.base_url, "model.base_url")?,
                api_key_env: normalize_optional(detail.api_key_env, "model.api_key_env")?,
            })),
        }
    }

    /// 验证模型设置中的可选文本已经完成空白归一化。
    fn validate(&self) -> Result<()> {
        if let Self::Detail(detail) = self {
            if detail.model.is_none() {
                // 允许空 detail 继续进入 resolver，以保持旧配置的延迟错误类别；其余
                // 字段仍在 parser 边界完成归一化，R4.2 再收紧 Provider 可用性校验。
                return Ok(());
            }
            if detail.model.as_deref().is_some_and(str::is_empty) {
                bail!("model 名称不能为空");
            }
        }
        Ok(())
    }

    /// 返回 resolver 所需的模型、Provider、endpoint 和 credential reference。
    pub(crate) fn values(&self) -> (Option<&str>, Option<&str>, Option<&str>, Option<&str>) {
        match self {
            Self::Name(value) => (Some(value), None, None, None),
            Self::Detail(detail) => (
                detail.model.as_deref(),
                detail.provider.as_deref(),
                detail.base_url.as_deref(),
                detail.api_key_env.as_deref(),
            ),
        }
    }

    /// 返回可展示的模型名，不暴露 Provider endpoint 或 credential 关联信息。
    pub fn display_name(&self) -> Option<&str> {
        self.values().0
    }
}

/// `model` 对象的规范化覆盖字段。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelDetail {
    /// parser 已将 `name`/`model` 两个 YAML 形态合并后的模型名。
    pub model: Option<String>,
    /// 覆盖顶层 Provider 名称。
    pub provider: Option<String>,
    /// 覆盖顶层 endpoint。
    pub base_url: Option<String>,
    /// canonical credential reference。
    pub api_key_env: Option<String>,
}

/// `providers.<name>` 的规范化 Provider 配置。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserProviderConfig {
    /// UI 展示名称，不参与 endpoint 解析。
    pub name: Option<String>,
    /// 归一化后的 OpenAI-compatible endpoint。
    pub base_url: Option<String>,
    /// credential reference，不保存 secret 值。
    pub api_key_env: Option<String>,
    /// Provider 使用的模型名。
    pub model: Option<String>,
}

impl UserProviderConfig {
    /// 按 parser 已确定的优先级返回 endpoint，不再暴露 YAML alias。
    pub(crate) fn endpoint(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    /// 检查自定义 Provider 的规范化字段，保留缺失 endpoint 交给 resolver 分类错误。
    fn validate(&self, name: &str) -> Result<()> {
        if self.name.as_deref().is_some_and(str::is_empty) {
            bail!("自定义 Provider '{name}' 的 name 不能为空");
        }
        if self.base_url.as_deref().is_some_and(str::is_empty) {
            bail!("自定义 Provider '{name}' 的 base_url 不能为空");
        }
        if self.api_key_env.as_deref().is_some_and(str::is_empty) {
            bail!("自定义 Provider '{name}' 的 api_key_env 不能为空");
        }
        if self.model.as_deref().is_some_and(str::is_empty) {
            bail!("自定义 Provider '{name}' 的 model 不能为空");
        }
        Ok(())
    }
}

fn normalize_provider_map(
    providers: BTreeMap<String, RawUserProviderConfig>,
) -> Result<BTreeMap<String, UserProviderConfig>> {
    let mut normalized = BTreeMap::new();
    for (name, provider) in providers {
        let name = normalize_required(name, "providers name")?;
        let endpoint = provider.api.or(provider.url).or(provider.base_url);
        let canonical = UserProviderConfig {
            name: normalize_optional(provider.name, "providers.name")?,
            base_url: normalize_optional(endpoint, "providers.base_url")?,
            api_key_env: normalize_optional(provider.api_key_env, "providers.api_key_env")?,
            model: normalize_optional(provider.model, "providers.model")?,
        };
        canonical.validate(&name)?;
        if normalized.insert(name.clone(), canonical).is_some() {
            bail!("重复的自定义 Provider 名称：{name}");
        }
    }
    Ok(normalized)
}

fn normalize_required(value: String, field: &str) -> Result<String> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        bail!("{field} 不能为空");
    }
    Ok(value)
}

fn normalize_optional(value: Option<String>, field: &str) -> Result<Option<String>> {
    value
        .map(|value| normalize_required(value, field))
        .transpose()
}
