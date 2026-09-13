//! Profile 级不可变配置快照。
//!
//! `ProfileConfig` 是一次配置读取后的组合结果，负责把各领域 descriptor 放在同一个
//! 生命周期边界内。它只保存非秘密意图和诊断信息，不创建数据库、Provider、HTTP client
//! 或后台任务；Bootstrap 将它交给各自 Factory/Resolver 创建运行时实现。

use crate::{
    provider_config::{ProviderConfig, ProviderDescriptor, WorkspaceDescriptor},
    storage::StorageDescriptor,
};

/// 当前 Profile 的完整配置快照。
///
/// 快照在 Runtime/Bootstrap 生命周期内保持不变。配置文件变更只会影响下一次加载，避免
/// 运行中的 Session/Turn 在中途更换 Provider、工具边界或持久化策略。
#[derive(Debug, Clone)]
pub struct ProfileConfig {
    /// 已校验的存储后端意图；缺省为本地 SQLite。
    pub storage: StorageDescriptor,
    /// 已提取的 Provider 选择和非秘密连接引用。
    pub provider: ProviderDescriptor,
    /// 已提取的 workspace 路径意图。
    pub workspace: WorkspaceDescriptor,
    /// 原始 YAML 中未知的顶层字段，供公开诊断显示。
    pub unknown_fields: Vec<String>,
}

impl ProfileConfig {
    /// 获取当前 Profile 已校验的存储 descriptor。
    ///
    /// 返回不可变借用，确保所有使用方都读取同一份启动期快照；该方法不重新读取
    /// `config.yaml`，也不创建数据库、连接或其它基础设施。
    pub fn get_storage_descriptor(&self) -> &StorageDescriptor {
        &self.storage
    }

    /// 从一次完整文档解析结果组合 Profile 快照。
    pub(crate) fn from_document(document: ProviderConfig, unknown_fields: Vec<String>) -> Self {
        Self {
            storage: document.storage.clone().unwrap_or_default(),
            provider: document.provider_descriptor(),
            workspace: document.workspace_descriptor(),
            unknown_fields,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::{
        load_profile_config, read_public_config_from_config, resolve_openai_provider_from_config,
        resolve_paths, resolve_workspace_from_config, test_support::test_root,
    };

    #[test]
    fn snapshot_reuses_one_config_read_for_all_resolvers() {
        let root = test_root("snapshot");
        fs::create_dir_all(root.join("workspace")).expect("应能创建 workspace fixture");
        fs::write(
            root.join("config.yaml"),
            "storage:\n  kind: sqlite\nprovider: openai-compatible\nmodel: test-model\nbase_url: http://127.0.0.1:1/v1\napi_key_env: SNAPSHOT_KEY\nworkspace: workspace\n",
        )
        .expect("应能写入 Profile 配置");
        fs::write(root.join(".env"), "SNAPSHOT_KEY=profile-key\n").expect("应能写入凭据 fixture");
        let paths = resolve_paths(Some(&root), None).expect("应能解析 Profile 路径");

        let config = load_profile_config(&paths).expect("应能加载一次 Profile 快照");
        // 删除配置文件后继续使用快照；任何 resolver 若重新读取 YAML，此测试都会失败。
        fs::remove_file(&paths.config_yaml).expect("应能删除配置文件 fixture");

        let storage = config.get_storage_descriptor();
        let workspace = resolve_workspace_from_config(&paths, &config).expect("workspace 应能解析");
        let public = read_public_config_from_config(&paths, &config).expect("摘要应能解析");
        let provider = resolve_openai_provider_from_config(&paths, &config, None, None)
            .expect("Provider 应能解析");

        assert_eq!(storage.kind, crate::StorageKind::Sqlite);
        assert_eq!(workspace, fs::canonicalize(root.join("workspace")).unwrap());
        assert_eq!(public.model.as_deref(), Some("test-model"));
        assert_eq!(provider.model, "test-model");
        fs::remove_dir_all(root).expect("应能清理 snapshot fixture");
    }
}
