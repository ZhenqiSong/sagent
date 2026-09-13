//! Profile 配置文档中的 Provider 数据模型。
//!
//! 本模块只表达 YAML 字段和字段归一化规则，不读取文件、环境变量或网络，也不创建
//! Provider 实例；这些副作用分别由配置读取器、凭据模块和 Provider resolver 负责。

use std::{collections::BTreeMap, path::PathBuf};

use serde::Deserialize;

use crate::storage::StorageDescriptor;

/// 当前 Profile `config.yaml` 的原始文档模型。
///
/// 该类型保留 YAML 字段形态，供兼容性读取 API 使用；运行时应优先使用已经完成组合
/// 和默认值填充的 [`crate::ProfileConfig`]，避免各主题 resolver 重复读取文档。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProviderConfig {
    /// Profile 的持久化后端描述；缺省时由 StorageDescriptor 使用本地 SQLite。
    #[serde(default)]
    pub storage: Option<StorageDescriptor>,
    /// 例如 `openai-compatible`，也可以是 `providers` 下的自定义 key。
    pub provider: Option<String>,
    /// 可以是简单字符串，也可以是包含 `name`/`model` 等字段的对象。
    pub model: Option<ModelSetting>,
    /// 全局 OpenAI-compatible endpoint。
    pub base_url: Option<String>,
    #[serde(alias = "key_env")]
    pub api_key_env: Option<String>,
    /// 与现有配置保持兼容：每个 key 描述一个自定义 OpenAI endpoint。
    #[serde(default)]
    pub providers: BTreeMap<String, UserProviderConfig>,
    /// 工具可访问的 workspace 根；相对路径锚定当前 Profile，而不是进程当前目录。
    pub workspace: Option<PathBuf>,
}

/// 已从 Profile 文档中提取的 Provider 意图描述。
///
/// 该类型只包含 Provider 选择和非秘密连接引用；API key 仍由 Provider resolver 在
/// 创建实例时读取，不会进入配置快照。
#[derive(Debug, Clone)]
pub struct ProviderDescriptor {
    /// 例如 `openai-compatible`，也可以是 `providers` 下的自定义 key。
    pub provider: Option<String>,
    /// 模型名称或模型级覆盖配置。
    pub model: Option<ModelSetting>,
    /// 全局 OpenAI-compatible endpoint。
    pub base_url: Option<String>,
    /// API key 所在的环境变量名；只保存引用，不保存值。
    pub api_key_env: Option<String>,
    /// 用户自定义 Provider 配置。
    pub providers: BTreeMap<String, UserProviderConfig>,
}

/// 已从 Profile 文档中提取的 workspace 意图描述。
#[derive(Debug, Clone, Default)]
pub struct WorkspaceDescriptor {
    /// workspace 路径；相对路径由 resolver 锚定到当前 Profile。
    pub path: Option<PathBuf>,
}

impl ProviderConfig {
    /// 将原始文档中的 Provider 字段提取为独立 descriptor。
    pub(crate) fn provider_descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            provider: self.provider.clone(),
            model: self.model.clone(),
            base_url: self.base_url.clone(),
            api_key_env: self.api_key_env.clone(),
            providers: self.providers.clone(),
        }
    }

    /// 将原始文档中的 workspace 字段提取为独立 descriptor。
    pub(crate) fn workspace_descriptor(&self) -> WorkspaceDescriptor {
        WorkspaceDescriptor {
            path: self.workspace.clone(),
        }
    }
}

/// `model` 字段支持字符串和对象两种形态，便于兼容现有配置习惯。
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ModelSetting {
    /// 直接使用的模型名。
    Name(String),
    /// 包含 Provider、endpoint 或凭据覆盖项的详细模型设置。
    Detail(ModelDetail),
}

impl ModelSetting {
    /// 返回归一化后的模型、Provider、endpoint 和凭据环境变量引用。
    pub(crate) fn values(&self) -> (Option<&str>, Option<&str>, Option<&str>, Option<&str>) {
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

    /// 返回可展示的模型名，不暴露 Provider endpoint 或 credential 关联信息。
    pub fn display_name(&self) -> Option<&str> {
        self.values().0
    }
}

/// `model` 对象形式的可选覆盖字段。
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
    /// 兼容配置中的凭据环境变量别名。
    pub key_env: Option<String>,
}

/// `providers.<name>` 中的一个用户自定义 Provider。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UserProviderConfig {
    /// UI 展示名称，不参与 endpoint 解析。
    pub name: Option<String>,
    /// 兼容配置中的 API endpoint 字段。
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
    /// 按兼容字段优先级返回 endpoint，不读取网络或验证 URL。
    pub(crate) fn endpoint(&self) -> Option<&str> {
        self.api
            .as_deref()
            .or(self.url.as_deref())
            .or(self.base_url.as_deref())
    }
}
