use std::path::PathBuf;

/// 默认的 managed 目录，相对于 sagent home。
const _DEFAULT_MANAGED_DIR: &str = "/etc/sagent";

/// 获取 managed 工具/脚本的安装目录。
///
/// 优先读取环境变量 `SAGENT_MANAGED_DIR`，未设置时使用默认值。
/// 若目录不存在则返回 `None`。
pub fn get_managed_dir() -> Option<PathBuf> {
    let override_dir = std::env::var("SAGENT_MANAGED_DIR")
        .unwrap_or_else(|_| _DEFAULT_MANAGED_DIR.to_string());
    let path = PathBuf::from(override_dir.trim());
    if path.exists() && path.is_dir() {
        Some(path)
    } else {
        None
    }
}