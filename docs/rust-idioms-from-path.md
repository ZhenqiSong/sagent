# `path.rs` 中使用的 Rust 语法/惯用法总结

本文档基于 `crates/sagent-common/src/path.rs`（约 120 行非测试代码 + 140 行测试代码），逐条列出其中使用的 Rust 语法特性、标准库 API 和惯用模式。

---

## 一、源文件结构

| 语法 | 所在行 | 说明 |
|------|--------|------|
| `//!` inner doc comment | 2-7 | 模块级文档注释，`//!` 用于注释容器自身（module），会被 rustdoc 解析生成文档 |
| `///` outer doc comment | 19-20, 22-26, 33-35, 40-50, 66-83 | 函数/静态变量前的文档注释，支持 Markdown、`# Examples`、`# Panics` 等段落 |

## 二、导入系统

| 语法 | 所在行 | 说明 |
|------|--------|------|
| `use std::{env, fs, io::{self, Write}, path::PathBuf, sync::atomic::{AtomicBool, Ordering}};` | 9 | **嵌套路径导入**：花括号内可嵌套子花括号，同时导入多个路径层级 |
| `use tokio::task_local;` | 10 | 导入宏（`task_local` 是 `tokio` 提供的声明宏） |
| `use super::*;` | 115 | 在 `mod tests` 中使用 `super::*` 导入父模块所有公开/私有项 |

## 三、宏

| 宏 | 所在行 | 说明 |
|----|--------|------|
| `task_local! { static NAME: Type }` | 15-17 | `tokio` 宏，声明**协程级**（task-local）静态变量。与 `thread_local!` 类似，但作用于 tokio task 而非 OS 线程 |
| `cfg!(...)` | 54 | **编译期条件判断**：运行时返回 `bool`，不同于 `#[cfg(...)]` 属性在编译时决定代码存在与否 |
| `format!(...)` | 96-101 | **格式化字符串**：支持 `{}`（Display）、`{:?}`（Debug）、`{expr}` 等格式化参数 |
| `writeln!(io::stderr(), ...)` | 102 | 格式化写入到 `io::Stderr`，返回 `Result` 需处理 |
| `assert!(cond, "msg")` | 122, 154 | 运行时断言，支持自定义错误消息和格式化参数 |
| `assert_eq!(left, right)` | 174 | 相等断言，左右值不等时 panic 并显示值差异 |

### 属性宏

| 属性 | 所在行 | 说明 |
|------|--------|------|
| `#[cfg(test)]` | 108 | **条件编译**：仅在 `cargo test` 编译时包含此模块 |
| `#[test]` | 119, 128 | **测试函数**：标记普通同步测试 |
| `#[tokio::test]` | 168, 179, 210 | **异步测试**：由 tokio 宏展开，自动创建运行时并执行 `async` 测试函数 |
| `#[cfg(not(...))]` / `#[cfg(...)]` | 131, 138, 153 | 属性作用于块 `{}` 上，控制特定代码块在哪些平台存在 |

## 四、数据类型与结构

### 标准库类型

| 类型 | 所在行 | 说明 |
|------|--------|------|
| `PathBuf` | 9 | **所有权路径类型**：类似 `String` 之于 `str`，可修改的路径缓冲区 |
| `AtomicBool` | 9, 20 | **原子布尔类型**：无锁线程安全，多个线程可并发读写 |
| `Ordering::Relaxed` | 89 | **内存序**：最弱的内存排序，仅保证原子性，不保证顺序一致性（适合计数器/标记场景） |
| `Option<PathBuf>` | 27 | **Option 枚举**：表示可能存在或不存在的值 |
| `Result` (隐含) | 102 | 通过 `writeln!` 等返回值体现，`Result` 的 `let _ =` 模式常用于抑制警告 |

### 协程级静态变量

```rust
task_local! {
    static SAGENT_HOME_OVERRIDE: String
}
```

相当于每个 tokio task 有独立副本，`try_with(closure)` 在当前 task 中尝试读取。使用 `.ok()` 将 `Result<T, AccessError>` 转换为 `Option<T>`。

### 全局静态变量

```rust
static PROFILE_FALLBACK_WARNED: AtomicBool = AtomicBool::new(false);
```

`static` 定义全局变量，必须使用线程安全的类型（如 `AtomicBool`）才能在多个线程间安全访问。

## 五、函数定义与模式

| 语法 | 所在行 | 说明 |
|------|--------|------|
| `fn name() -> ReturnType` | 27, 36, 51, 84 | 函数声明 |
| `pub fn name() -> ReturnType` | 84 | **公开函数**：模块外部可访问 |
| `async fn` | 169, 180, 211 | **异步函数**：返回 impl Future，需使用 `.await` 调用 |
| `fn name() -> Option<PathBuf>` | 27 | 返回 Option 类型，调用方可使用模式匹配 |

## 六、控制流

| 语法 | 所在行 | 说明 |
|------|--------|------|
| `if let Some(x) = expr { ... }` | 85-87 | **if-let 模式匹配**：仅匹配 `Option::Some` 分支，简洁 | 
| `if !cond.swap(true, ...)` | 89-103 | **原子操作+条件判断**：`swap` 原子写入新值并返回旧值，一次性完成读+写 |
| `if ... { ... } else { ... }` | 95, 54-62 | 传统条件分支 |
| `return value;` | 86 | 提前返回 |

## 七、闭包与函数式链式调用

| 语法 | 所在行 | 说明 |
|------|--------|------|
| `\|v\| { v.clone().into() }` | 28-30 | **闭包**：匿名函数，`\|参数\| { 表达式 }` |
| `\|v\| v.clone().into()` | 28-30 (简写形式) | 单表达式闭包可省略花括号 |
| `\|x\|` 多形式 | 121, 133 | 简洁闭包参数 |
| `.try_with(\|v\| ...).ok()` | 28-31 | `Result → Option` 转换链 |
| `.map(PathBuf::from)` | 57 | **函数引用作为映射函数**：`PathBuf::from` 被当作 `fn(String) -> PathBuf` 传入 |
| `.map(\|s\| s.trim().to_owned())` | 93 | 闭包链式处理 |
| `.unwrap_or_else(\|\| PathBuf::from("/home"))` | 37 | **惰性求值回退**：闭包仅在需要时执行 |
| `.unwrap_or_default()` | 93 | 使用类型的 `Default::default()` 作为回退值 |
| `.unwrap_or(home.join("AppData").join("Local"))` | 58 | **急切求值回退**：参数先求值，与 `unwrap_or_else` 区别 |
| `env::var(...).map(...).unwrap_or(...).join(...)` | 56-59 | **链式调用**：连续方法调用形成管道 |

## 八、PathBuf API

| 方法 | 所在行 | 说明 |
|------|--------|------|
| `PathBuf::from(path_str)` | 37, 57 | 从字符串创建路径 |
| `path.join(component)` | 58, 59, 61, 91 | 路径拼接，自动处理分隔符 |
| `path.ends_with(suffix)` | 135 | 路径是否以某段结尾 |
| `path.starts_with(&prefix)` | 155 | 路径是否以某段开头 |
| `path.is_absolute()` | 123, 263-264 | 是否是绝对路径 |
| `path.to_string_lossy()` | 140 | 将路径转换为 `Cow<str>`（非 UTF-8 字符用 `�` 替换），比 `to_str()` 更健壮 |
| `path.as_os_str()` | 122 | 获取底层 `OsStr` 引用，跨平台兼容 |

## 九、字符串处理

| 语法 | 所在行 | 说明 |
|------|--------|------|
| `"str".to_string()` | 171 | `&str → String` |
| `s.trim()` | 93 | 去除首尾空白字符 |
| `s.to_owned()` | 93 | 将 `&str` 克隆为 `String` |
| `active.is_empty()` | 95 | 字符串判空 |
| `"str".to_lowercase()` | 140 | 转小写 |
| `format!("value is {:?}", var)` | 96-101 | 带 Debug 格式的字符串模板 |
| `{active:?}`, `{fallback_home:?}` | 97-100 | `format!` 中的命名参数 Debug 格式化 |
| `string.into()` | 29-30 | 通过 `Into<T>` trait 转换类型（`String → PathBuf`） |
| `PathBuf::from(...)` | 37, 57, 174 | 显式类型转换 |

## 十、文件 I/O

| API | 所在行 | 说明 |
|-----|--------|------|
| `fs::read_to_string(path)` | 93 | 整个文件读为 `String`，返回 `Result<String>` |
| `io::stderr()` | 102 | 获取标准错误输出句柄 |
| `writeln!(io::stderr(), ...)` | 102 | 格式化写入 stderr，自动换行 |

## 十一、env 环境变量

| API | 所在行 | 说明 |
|-----|--------|------|
| `env::var("VAR_NAME")` | 56 | 读取环境变量，返回 `Result<String, VarError>` |

## 十二、测试模式

| 模式 | 所在行 | 说明 |
|------|--------|------|
| `mod tests { use super::*; }` | 108-115 | 测试模块模式：`use super::*` 导入父模块所有私有/公开项 |
| `#[tokio::test] async fn ... { ... .await }` | 168-177 | 异步测试：`task_local!` 的 `scope()` 方法需在异步上下文中调用 |
| `var.scope(value, async { }).await` | 171-176, 182-189 | **task_local 作用域**：在 `async` 块内设置局部覆盖，结束后自动恢复 |
| 测试嵌套 `{ #[cfg(...)] assert!(...) }` | 131-145 | 条件编译测试：同一测试内对不同平台执行不同断言 |

### `task_local!` 的 `.scope()` 用法详解

```rust
SAGENT_HOME_OVERRIDE
    .scope(set_value, async {
        // 在此 async 块内，SAGENT_HOME_OVERRIDE 的值是 set_value
        // 块外恢复原始值（None）
    })
    .await;
```

核心特性：
- **协程级隔离**：只在当前 tokio task 内生效
- **自动恢复**：scope 退出后自动复原，无需手动清理
- **嵌套安全**：支持 `scope` 内再嵌套 `scope`

## 十三、`AtomicBool` 的高级用法

```rust
if !PROFILE_FALLBACK_WARNED.swap(true, Ordering::Relaxed) {
    // 只执行一次的代码
}
```

等效于：
```rust
if !PROFILE_FALLBACK_WARNED.load(Ordering::Relaxed) {
    PROFILE_FALLBACK_WARNED.store(true, Ordering::Relaxed);
    // 只执行一次的代码
}
```

差异：`swap` 是**原子单操作**（RMW, Read-Modify-Write），不会出现两个线程同时通过 `load` 检查的竞态条件。

## 十四、`let _ = expr` 惯用法

```rust
let _ = writeln!(io::stderr(), "{msg}");
```

作用：明确抑制 `writeln!` 返回的 `Result` 的未使用警告，表明"故意忽略该错误"。
不用单独 `match` 或 `.unwrap()`，因为 stderr 写入失败通常是不可恢复的或无需处理的。

## 十五、`cfg!()` 与 `#[cfg()]` 的区别

| 特性 | `cfg!()` | `#[cfg()]` |
|------|----------|------------|
| 时机 | 运行时 | 编译时 |
| 结果 | `bool` | 包含/排除代码 |
| 用途 | 运行时分支 | 条件编译 |

```rust
// 运行时判断：真假分支都在编译产物中
if cfg!(target_os = "windows") { ... } else { ... }

// 编译时判断：只有匹配平台的代码会编译
#[cfg(target_os = "windows")]
fn windows_only() { ... }
```

---

## 总结

这个文件虽小，但覆盖了 Rust 中多个关键语法领域：

- **文档系统**：`//!` 模块文档 + `///` 函数文档
- **导入语法**：嵌套路径 + `super::*`
- **宏系统**：`format!`/`writeln!` 声明宏 + `#[tokio::test]` 过程宏 + `task_local!`
- **类型系统**：`PathBuf`, `Option<T>`, `AtomicBool`, `Ordering`
- **函数式风格**：闭包 + 链式调用 + 惰性求值
- **并发原语**：`task_local!` 协程级变量 + `AtomicBool` 全局标记
- **条件编译**：`cfg!()` 运行时 + `#[cfg()]` 编译时
- **测试框架**：`#[test]` / `#[tokio::test]` + `assert!` / `assert_eq!`
- **I/O**：文件读取 + stderr 写入
- **模式匹配**：`if let` + 字符串比较
