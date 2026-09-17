//! 面向 RPC/UI 的非秘密配置摘要。
//!
//! 本模块只把配置转换为可公开的摘要并报告未知顶层字段；endpoint、凭据引用和值均不
//! 进入 DTO，配置读取也不会修改原文件或触发基础设施创建。

use anyhow::Result;
use serde::Serialize;

use crate::{SagentPaths, profile_config::ProfileConfig, provider_config::ModelSetting};

/// 可经 RPC 返回的非秘密配置摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicConfig {
    /// 启动时固定的 Profile 名称；帮助客户端标示数据隔离边界而不暴露本地路径。
    pub profile: String,
    /// 已配置的 Provider 名称；未设置时为 `null`。
    pub provider: Option<String>,
    /// 用户选择的模型显示名；复杂 model 设置会归一为 name/model 字段。
    pub model: Option<String>,
    /// 配置中声明的自定义 Provider 名称，按字典序排列。
    pub provider_names: Vec<String>,
    /// 不被当前版本识别的顶层 YAML 字段；读取不会删除它们。
    pub unknown_fields: Vec<String>,
}

/// 从已经加载的 Profile 快照生成公开摘要，不重新读取配置文件。
pub fn read_public_config_from_config(
    paths: &SagentPaths,
    config: &ProfileConfig,
) -> Result<PublicConfig> {
    Ok(PublicConfig {
        profile: paths.profile.clone(),
        provider: config
            .provider
            .provider
            .as_ref()
            .map(|provider| provider.as_str().to_owned()),
        model: config
            .provider
            .model
            .as_ref()
            .and_then(ModelSetting::display_name)
            .map(str::to_owned),
        provider_names: config
            .provider
            .providers
            .keys()
            .map(|provider| provider.as_str().to_owned())
            .collect(),
        unknown_fields: config.unknown_fields.clone(),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::read_public_config_from_config;
    use crate::{
        load_profile_config, normalize_profile_name, resolve_paths, test_support::test_root,
    };

    #[test]
    fn public_config_exposes_model_state_without_endpoint_or_credential_metadata() {
        // 配置读取是 GUI 的诊断接口，不得把本地部署地址、凭据变量名或 .env 内容
        // 通过 JSON-RPC 反射给连接到 daemon 的客户端。
        let root = test_root("public-summary");
        fs::write(
            root.join("config.yaml"),
            "provider: local\nmodel:\n  name: local-model\nbase_url: http://private.example/v1\napi_key_env: PRIVATE_KEY\nproviders:\n  backup:\n    api: http://backup.example/v1\ndisplay_theme: dark\n",
        )
        .unwrap();
        fs::write(root.join(".env"), "PRIVATE_KEY=must-not-leak\n").unwrap();
        let profile = normalize_profile_name("default").unwrap();
        let paths = resolve_paths(Some(&root), Some(&profile)).unwrap();
        let config = load_profile_config(&paths).expect("应能加载公开摘要快照");

        let public =
            read_public_config_from_config(&paths, &config).expect("公开摘要应能从快照生成");

        assert_eq!(public.profile, "default");
        assert_eq!(public.provider.as_deref(), Some("local"));
        assert_eq!(public.model.as_deref(), Some("local-model"));
        assert_eq!(public.provider_names, ["backup"]);
        assert_eq!(public.unknown_fields, ["display_theme"]);
        let encoded = serde_json::to_string(&public).unwrap();
        assert!(!encoded.contains("private.example"));
        assert!(!encoded.contains("PRIVATE_KEY"));
        assert!(!encoded.contains("must-not-leak"));
        fs::remove_dir_all(root).unwrap();
    }
}
