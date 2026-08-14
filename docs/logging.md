# sagent 日志系统

## 概述

sagent 使用 `tracing` + `tracing-subscriber` 作为日志框架。所有日志写 stderr，绝不污染 stdout JSON-RPC 协议通道。

## 核心规则

1. **日志通道隔离**：所有日志写 stderr，stdout 只用于 JSON-RPC 协议帧。
2. **默认日志级别**：`info`，通过 `RUST_LOG` 环境变量覆盖。
3. **结构化日志**：每个 RPC request 携带 `request_id` tracing span 字段。
4. **敏感数据保护**：日志中不打印 secret、API key、完整 request params 或未经裁剪的用户内容。
5. **幂等初始化**：日志初始化可被多次调用，不会 panic 或添加重复 subscriber。
6. **启动信息**：server 启动时记录 protocol version、runtime version 和 enabled capabilities。

## 使用方式

### 初始化

```rust
// 在 main 函数开始时调用（幂等，可多次调用）
sagent_api::logging::init();

// 或指定自定义默认级别
sagent_api::logging::init_with_level("debug");
```

### Request Span

每个 RPC request 使用 `request_span` 包裹处理逻辑，实现请求级别的日志关联：

```rust
let span = sagent_api::logging::request_span("req-001");
let _guard = span.enter();
info!("processing request");
// 所有日志自动携带 request_id=req-001
```

### 敏感数据过滤

使用 `redact_sensitive` 对 JSON params 进行脱敏：

```rust
let params = serde_json::json!({
    "api_key": "sk-secret-key",
    "name": "test"
});
let safe = sagent_api::logging::redact_sensitive(&params);
// api_key 被替换为 ***REDACTED***，name 保持不变
```

敏感字段关键词（大小写不敏感匹配）：
- `token`
- `secret`
- `password`
- `api_key`
- `apikey`
- `authorization`
- `credential`
- `private_key`
- `access_key`

## 日志级别配置

```bash
# 默认 info 级别
cargo run --bin sagent -- rpc stdio

# debug 级别（所有模块）
RUST_LOG=debug cargo run --bin sagent -- rpc stdio

# 只对特定模块
RUST_LOG=sagent=debug cargo run --bin sagent -- rpc stdio

# 静默模式（仅 error）
RUST_LOG=error cargo run --bin sagent -- rpc stdio
```

## 结构化字段

每条日志包含以下字段：

| 字段 | 说明 | 示例 |
| --- | --- | --- |
| `timestamp` | ISO 8601 时间戳 | 自动 |
| `level` | 日志级别 | `INFO` / `WARN` / `ERROR` |
| `target` | 模块路径 | `sagent::dispatcher` |
| `message` | 日志消息 | `请求处理成功` |
| `request_id` | 请求 ID（span 内） | `req-001` |
| `method` | RPC 方法名 | `rpc.echo` |
| `error_code` | 错误码（错误日志） | `-32601` |

## 验收标准

- [x] 执行 stdio server 时 stdout 每一行都能被 JSON parser 解析
- [x] 设置 `RUST_LOG=debug` 只增加 stderr 日志，不改变 stdout response
- [x] 错误 response 包含稳定 code，stderr 日志包含相同 code 或 request_id
- [x] request params 中包含 token 等敏感字段时，日志不会原样输出该值
- [x] 重复调用日志初始化不会出现重复行或 panic
- [x] 日志写入失败不会破坏 RPC response 的 schema

## 参考

- `hermes_logging.py`：session context、组件日志和 stderr 容错（Python 参考）
- `tui_gateway/entry.py`：stdout 协议通道和 stderr crash/exit 诊断（Python 参考）
- `tests/test_hermes_logging.py`：日志初始化行为测试（Python 参考）
