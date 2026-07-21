
use std::path::Path;
use std::fs::OpenOptions;

use fd_lock::RwLock;

/// 基于文件锁的互斥保护。
///
/// 使用 fd_lock（flock 系统调用）提供进程间文件级互斥。
/// 锁的生命周期受 `run_with_lock` 闭包控制，闭包结束时自动释放。
pub struct FileLock {
    path: String,
}

impl FileLock {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }

    /// 加锁并执行回调，回调结束后自动解锁。
    ///
    /// 阻塞等待直到锁可用。返回 `Err` 仅在文件无法创建/打开时。
    ///
    /// # 示例
    ///
    /// ```ignore
    /// let lock = FileLock::new("/tmp/sagent.lock");
    /// lock.run_with_lock(|| {
    ///     // 临界区代码...
    ///     Ok(())
    /// })?;
    /// ```
    pub fn run_with_lock<F, T>(&self, f: F) -> anyhow::Result<T>
    where
        F: FnOnce() -> anyhow::Result<T>,
    {
        let p = Path::new(&self.path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(p)?;

        let mut rw_lock = RwLock::new(&file);
        let _guard = rw_lock.write()?;  // 阻塞等待排他锁
        f()  // guard 在此作用域结束时自动 drop → 释放锁
    }
}