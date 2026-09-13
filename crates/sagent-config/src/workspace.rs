//! Profile workspace 路径解析。
//!
//! 本模块只把配置中的 workspace 路径解析为已经存在且可规范化的目录；它不创建目录、
//! 不启动工具，也不决定 Provider 或存储后端。

use std::{fs, path::PathBuf};

use anyhow::{Context, Result, bail};

use crate::{SagentPaths, profile_config::ProfileConfig};

/// 从已经加载的 Profile 快照解析 workspace，不重新读取配置文件。
pub fn resolve_workspace_from_config(
    paths: &SagentPaths,
    config: &ProfileConfig,
) -> Result<PathBuf> {
    let configured = config
        .workspace
        .path
        .clone()
        .unwrap_or_else(|| PathBuf::from("workspace"));
    let candidate = if configured.is_absolute() {
        configured
    } else {
        paths.sagent_home.join(configured)
    };
    let metadata = fs::metadata(&candidate)
        .with_context(|| format!("workspace 不可用：{}", candidate.display()))?;
    if !metadata.is_dir() {
        bail!("workspace 不是目录：{}", candidate.display());
    }
    fs::canonicalize(&candidate)
        .with_context(|| format!("无法规范化 workspace：{}", candidate.display()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::resolve_workspace_from_config;
    use crate::{
        load_profile_config, read_public_config_from_config, resolve_paths, test_support::test_root,
    };

    #[test]
    fn workspace_is_profile_anchored_and_not_reported_as_unknown() {
        let root = test_root("workspace");
        let workspace = root.join("project");
        fs::create_dir_all(&workspace).expect("应能创建 workspace fixture");
        fs::write(
            root.join("config.yaml"),
            "workspace: project\nprovider: openai-compatible\n",
        )
        .expect("应能写入 workspace 配置");
        let paths = resolve_paths(Some(&root), None).expect("应能解析 Profile 路径");
        let config = load_profile_config(&paths).expect("应能加载 workspace 快照");

        assert_eq!(
            resolve_workspace_from_config(&paths, &config).expect("相对 workspace 应锚定 Profile"),
            fs::canonicalize(workspace).expect("fixture workspace 应可 canonicalize")
        );
        assert!(
            read_public_config_from_config(&paths, &config)
                .expect("公开配置应可读取")
                .unknown_fields
                .is_empty(),
            "workspace 是已知配置字段，不应误报 unknown"
        );
        fs::remove_dir_all(root).expect("应能清理 workspace fixture");
    }
}
