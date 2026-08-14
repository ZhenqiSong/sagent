# Sagent 配置 v1

Phase 1 的配置文件路径为 `<SAGENT_HOME>/config.yaml`。`SAGENT_HOME` 未设置时使用平台默认的
`~/.sagent`；配置文件不存在时使用 Rust 定义的完整默认值，不自动创建目录或文件。

## Schema

```yaml
version: 1
runtime:
  shutdown_timeout_ms: 5000
  max_live_sessions: 128
  actor_mailbox_capacity: 256
  event_buffer_capacity: 256
database:
  path: null
  busy_timeout_ms: 5000
  synchronous: full
rpc:
  max_line_bytes: 1048576
  max_response_bytes: 4194304
logging:
  level: info
```

所有字段都有默认值。`database.path` 为 `null` 时保留为未指定；非空相对路径相对于
`SAGENT_HOME` 解析，不相对于当前工作目录解析。绝对路径按原值使用。

## 校验和安全

- 当前只支持配置版本 `1`。
- 顶层和嵌套未知字段均拒绝，错误只包含 key path。
- 数值必须为非零整数，并受 Runtime、RPC 和超时上限约束。
- `database.synchronous` 支持 `full`、`normal` 和 `off`，默认是 `full`。
- `logging.level` 支持 `trace`、`debug`、`info`、`warn` 和 `error`。
- 配置结构不包含 API key、token 或其他 secret，也不读取 `.env`。
- YAML 类型错误只返回字段路径和期望类型，不把完整文件内容写入日志。
- 加载结果是独立配置快照；文件后续变化不会修改已创建的 Runtime 配置。

## API

```rust
use sagent_config::{ConfigLoader, ConfigPaths};

let paths = ConfigPaths::from_root("/tmp/sagent");
let config = ConfigLoader::new(paths).load()?;
```

配置 crate 不依赖 `sagent-session`、`sagent-runtime` 或 CLI，不负责创建 SQLite、目录或 secrets
文件。
