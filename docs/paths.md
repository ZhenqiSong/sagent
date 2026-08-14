# Sagent 路径规则

本文档定义 Sagent 的本地文件路径约定。所有路径通过 `SagentHome` 类型统一管理，使用 `PathBuf` 拼接，不硬编码路径分隔符。

## 1. Sagent Home

### 1.1 默认路径

所有平台统一使用用户 HOME 目录下的 `.sagent`：

| 平台 | 默认路径 |
| --- | --- |
| Linux | `$HOME/.sagent` |
| macOS | `$HOME/.sagent` |
| Windows | `%USERPROFILE%\.sagent` |

### 1.2 SAGENT_HOME 覆盖

可通过 `SAGENT_HOME` 环境变量覆盖默认路径。该变量用于内部/部署覆盖：

- **必须是绝对路径**：相对路径会被拒绝，返回 `PathError::RelativePath`
- **不得包含 NUL 字符**：返回 `PathError::InvalidPath`
- **空字符串或仅空白**：fallback 到平台默认路径
- **优先级**：`SAGENT_HOME` > 平台默认路径

### 1.3 Rust API

```rust
use sagent_api::paths::SagentHome;

// 发现 Sagent home（环境变量优先）
let home = SagentHome::discover()?;

// 仅从环境变量创建
if let Some(result) = SagentHome::from_env() {
    let home = result?;
}

// 测试用（直接指定路径）
let home = SagentHome::from_root(PathBuf::from("/tmp/test-sagent"));
```

## 2. 子目录结构

```text
<SAGENT_HOME>/
├── config/       # 配置文件目录
├── logs/         # 日志文件目录
├── cache/        # 缓存文件目录
└── runtime/      # 运行时文件目录（PID、sockets 等）
```

| 方法 | 路径 |
| --- | --- |
| `home.root()` | `<SAGENT_HOME>` |
| `home.config_dir()` | `<SAGENT_HOME>/config` |
| `home.logs_dir()` | `<SAGENT_HOME>/logs` |
| `home.cache_dir()` | `<SAGENT_HOME>/cache` |
| `home.runtime_dir()` | `<SAGENT_HOME>/runtime` |

## 3. Phase 0 限制

Phase 0 **不创建**以下目录或文件：

- 数据库（`state.db`）
- Session 数据
- Secrets 目录
- 任何持久化状态

目录创建采用显式初始化，不在纯路径查询函数中产生隐式副作用。

## 4. 设计原则

1. **单一入口**：`SagentHome` 是未来配置、session 和日志路径的唯一入口
2. **平台感知**：使用 `cfg!(target_os)` 条件编译，不在运行时判断平台
3. **PathBuf 拼接**：所有目录使用 `PathBuf::join()`，不拼接硬编码 `/` 或 `\`
4. **不依赖 cwd**：同一进程内重复解析得到相同路径，不因当前工作目录变化而漂移
5. **不兼容旧项目**：不调用 `hermes_constants.py`，不硬编码 `~/.hermes`
6. **环境隔离**：测试使用 `from_root()` 构造，不写入真实用户 home

## 5. 错误处理

| 条件 | 错误类型 | 行为 |
| --- | --- | --- |
| `SAGENT_HOME` 为相对路径 | `PathError::RelativePath` | 拒绝 |
| `SAGENT_HOME` 包含 NUL | `PathError::InvalidPath` | 拒绝 |
| `SAGENT_HOME` 为空 | — | fallback 到平台默认 |
| `SAGENT_HOME` 未设置 | — | 使用平台默认 |

## 6. 参考

- `crates/sagent-api/src/paths.rs`：Rust 实现
- `plans/sagent-phase0-implementation-guide.md` Step 8：原始需求
- Python 参考：`hermes_constants.py`、`tests/test_hermes_constants.py`
