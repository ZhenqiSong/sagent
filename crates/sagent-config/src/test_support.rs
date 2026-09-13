//! `sagent-config` 单元测试共用的临时 Profile fixture。

use std::{fs, path::PathBuf};

/// 创建一个按测试进程隔离的临时 Profile 根目录。
pub(crate) fn test_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "sagent-config-refactor-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("应能创建测试目录");
    root
}
