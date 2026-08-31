# Sagent Rust 第三阶段计划：本地 JSON-RPC 会话服务

作者：SongZQ  
状态：实施计划  
前置条件：第一、二阶段的配置、Profile、SQLite 会话存储和 CLI 会话命令已经可用。

## 1. 阶段目标

第三阶段新增 `sagent-protocol` crate 和独立的 `sagent-rpc` 进程，向后续 TUI 或其他本地客户端提供**只读**的会话访问接口。

第一版只提供：

- `gateway.ready`：服务启动完成事件；
- `gateway.ping`：连通性检查；
- `session.list`：列出当前 Profile 的会话；
- `session.resume`：读取一个会话及其可见消息，供客户端恢复界面。

传输采用标准输入、标准输出上的逐行 JSON-RPC 2.0（NDJSON）。每一行是一个完整 JSON 对象。服务启动时确定 `--home` 与 `--profile`，请求本身不能改变文件系统根目录或 Profile。

这里的 `session.resume` 只表示“读取并返回会话快照”，**不**表示启动 Agent、恢复模型上下文、建立工具环境或执行一次模型调用。

## 2. 参考 Python 实现，但不复制其运行时职责

本阶段的协议形态参考 Hermes 的 TUI 网关；实现范围只取其协议边界和持久化会话读取能力。

| 要参考的 Python 代码 | 参考内容 | Rust 第三阶段的落点 |
| --- | --- | --- |
| `D:\projects\hermes-agent\tui_gateway\server.py:387` | stdout 只写 JSON-RPC，诊断信息走 stderr | `sagent-rpc` 不允许日志、banner 或 panic 信息污染 stdout |
| `D:\projects\hermes-agent\tui_gateway\server.py:2027` | 事件信封：`method: "event"`，事件名放入 `params.type` | 发送 `gateway.ready` 事件，并为未来通知保留同一信封 |
| `D:\projects\hermes-agent\tui_gateway\server.py:2400-2404` | 成功和错误响应封装 | 实现统一的 `result` / `error` 输出函数 |
| `D:\projects\hermes-agent\tui_gateway\server.py:2419` | 请求正规化与 JSON-RPC 基础校验 | 实现请求解析、`jsonrpc`、`id`、`method`、`params` 校验 |
| `D:\projects\hermes-agent\tui_gateway\server.py:2438` | 请求分派入口 | 用表驱动或 `match` 分派协议方法，不把业务代码放进 stdio 循环 |
| `D:\projects\hermes-agent\tui_gateway\ws.py:10` | WebSocket 与 stdio 共用逐行 JSON-RPC 语义 | 先抽象协议/服务层，使后续 WebSocket 只替换 transport |
| `D:\projects\hermes-agent\hermes_state.py` 的会话查询代码 | 读取 session、message、归档状态、可见消息 | 直接复用 Sagent 第二阶段的 `sagent-store` API，不重写 SQL |

Python 的 `session.resume` 会继续进入 Agent、provider、工具和会话运行时恢复流程。Rust 当前还没有对应 Agent 核心，因此不能把该复杂行为误当作第三阶段目标；本阶段返回存储层快照即可。

## 3. 非目标与边界

本阶段明确不做：

- 不实现模型调用、流式输出、工具调用、上下文压缩或 Agent 生命周期；
- 不实现真正的写操作（创建、重命名、归档、回退仍通过现有 CLI）；
- 不实现 WebSocket、TCP 监听、认证、远程访问或多客户端会话锁；
- 不实现持续消息增量事件、事件重放或订阅；
- 不改变 SQLite schema，不为 RPC 新增状态表；
- 不让 RPC 参数接受任意 `home`、数据库路径或 Profile，以免请求越过服务启动时的作用域。

第三阶段结束时，`sagent-rpc` 是一个本地、只读、可被 TUI 对接的基础服务，而不是完整聊天后端。

## 4. crate 与模块设计

新增 workspace 成员：`crates/sagent-protocol`，输出二进制：`sagent-rpc`。

建议目录：

```text
crates/sagent-protocol/
├── Cargo.toml
└── src/
    ├── lib.rs             # 对外导出协议 DTO 与服务入口
    ├── envelope.rs        # JSON-RPC Request / Response / Error / Event
    ├── method.rs          # 方法名、参数和结果 DTO
    ├── error.rs           # 协议错误和领域错误映射
    ├── dispatch.rs        # 纯分派与参数校验
    ├── service.rs         # SessionService：配置 + Store 的只读适配
    ├── stdio.rs           # NDJSON 读取/写入循环
    └── bin/
        └── sagent-rpc.rs  # 进程入口与启动参数解析
```

依赖方向应保持单向：

```text
sagent-rpc (bin)
        ↓
sagent-protocol (stdio / dispatch / service)
        ↓                 ↓
sagent-config        sagent-store
        ↓                 ↓
                 sagent-types
```

`envelope.rs`、`method.rs` 和 `error.rs` 只依赖 `serde`，避免把 SQLite 类型泄露给未来的客户端或 transport。`service.rs` 才负责将 `sagent-store` 的领域对象转换为协议 DTO。

## 5. 协议契约

### 5.1 通用请求与响应

请求：

```json
{"jsonrpc":"2.0","id":1,"method":"gateway.ping","params":{}}
```

成功响应：

```json
{"jsonrpc":"2.0","id":1,"result":{"ok":true}}
```

错误响应：

```json
{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}
```

通知没有 `id`，服务必须执行其允许的处理，但不得写响应。第一版只接受无副作用的通知；未知通知不产生 stdout 输出，并记录到 stderr（或直接忽略）。

事件使用：

```json
{"jsonrpc":"2.0","method":"event","params":{"type":"gateway.ready","payload":{"protocol_version":1,"features":["gateway.ping","session.list","session.resume"]}}}
```

### 5.2 统一错误码

| 场景 | code | 说明 |
| --- | ---: | --- |
| JSON 无法解析 | `-32700` | Parse error |
| JSON-RPC 对象不合法 | `-32600` | Invalid Request |
| 方法不存在 | `-32601` | Method not found |
| 参数类型、范围或字段不合法 | `-32602` | Invalid params |
| 未预期的内部失败 | `-32603` | Internal error |
| Profile 或状态目录不可用 | `-32003` | 服务启动后确定的状态作用域不可访问 |
| 会话不存在 | `-32004` | `session_id` 不存在于当前 Profile |
| SQLite / FTS 无法读取 | `-32005` | Store unavailable |

错误 `message` 使用稳定的英文机器可读短语；可以在 `data` 中带安全的上下文（如 `session_id`），但不能泄露绝对密钥路径、数据库内容或内部 backtrace。

### 5.3 `gateway.ping`

请求参数为空对象或省略。返回：

```json
{"ok":true,"protocol_version":1}
```

用途是客户端在收到 `gateway.ready` 后确认请求/响应通道正常。

### 5.4 `session.list`

参数：

```json
{
  "include_archived": false,
  "limit": 50,
  "offset": 0
}
```

- `include_archived` 默认 `false`；
- `limit` 默认 `50`，最大值先定为 `200`；
- `offset` 默认 `0`；
- 结果复用第二阶段的会话摘要字段：`id`、`title`、`created_at`、`updated_at`、`archived_at`、`finished_at`、`message_count` 等已经存在且对客户端有意义的字段。

返回：

```json
{"sessions":[{"id":"...","title":"...","message_count":3}],"limit":50,"offset":0}
```

禁止在这里新增“全局所有 profile”的查询。一个 RPC 进程只服务一个由启动参数确定的 Profile。

### 5.5 `session.resume`

参数：

```json
{"session_id":"...","message_limit":100,"message_offset":0}
```

返回一个 `SessionDetail` 快照：

```json
{
  "session": {"id":"...","title":"...","message_count":3},
  "messages": [
    {"id":1,"role":"user","content":"你好","created_at":"..."}
  ],
  "message_limit":100,
  "message_offset":0
}
```

消息必须沿用第二阶段已有的“可见消息”规则：被 rewind 的消息、内部压缩消息或标记为隐藏的消息不得混入默认 UI 快照。`message_limit` 最大值同样限制为 `200`，避免单个请求无界读取数据库。

会话不存在时返回 `-32004`；它不是空数组，也不触发创建会话。

## 6. 分步实施计划

### 3.1 建立 crate 与无业务依赖的协议类型

1. 在根 `Cargo.toml` 的 workspace members 中加入 `crates/sagent-protocol`。
2. 创建 `sagent-protocol/Cargo.toml`，首先只加入 `serde`、`serde_json`、`thiserror`；服务层再显式依赖 `sagent-config`、`sagent-store`、`sagent-types`。
3. 编写 `envelope.rs`：`JsonRpcRequest`、`JsonRpcResponse`、`JsonRpcError`、`JsonRpcEvent`。
4. 编写 `method.rs`：每个方法独立的 Params/Result DTO；不要使用无约束的 `serde_json::Value` 贯穿业务层。
5. 给每个公开字段和类型写中文 rustdoc，说明字段是否面向协议稳定性。

验收：协议 DTO 可单独 `serde_json` 序列化/反序列化，且不依赖 SQLite。

### 3.2 完成请求校验、错误映射和纯分派

1. 在 `dispatch.rs` 实现 `dispatch(request, service)`；根据 `method` 路由到小型 handler。
2. 请求进入业务层前验证：`jsonrpc == "2.0"`、`method` 是非空字符串、带 `id` 的请求可返回响应、`params` 为对象或省略。
3. 使用强类型 Params 反序列化，并把字段缺失、类型不符、`limit > 200`、负 offset 等统一转换为 `-32602`。
4. 在 `error.rs` 统一定义 `ProtocolError` 与从 Config/Store 错误到 JSON-RPC code 的映射。不得在每个 handler 手写 JSON。
5. 定义服务 trait（例如 `SessionReadService`），使分派单元测试可使用内存 fake，而无需真实数据库。

验收：无效 JSON-RPC、未知方法、无效参数、通知不回包等行为都有纯单元测试。

### 3.3 实现只读 `SessionService`

1. 在 `service.rs` 创建 `SessionService`，进程启动时通过现有 `sagent-config` 的路径解析逻辑固定 `home/profile/state.db`。
2. 复用 `sagent-store` 的 `list_sessions` 查询和现有 session/message 读取 API；缺失的只读组合查询应添加在 store，而不是把 SQL 放进 protocol crate。
3. 转换 Store 领域模型为 `SessionSummaryDto`、`SessionDetailDto`、`MessageDto`，避免客户端依赖 Rust 内部 enum 或 SQLite NULL 细节。
4. 对数据库不存在、schema 不可读、FTS 不可用、Profile 不存在和会话不存在做确定的错误映射。
5. 全程以只读方式打开 Store；`session.list`、`session.resume` 不能创建 `state.db`，也不能更新 `updated_at`。

验收：针对默认和具名 Profile 的临时 fixture 数据库，list/resume 返回正确数据，且调用前后数据库字节大小与修改时间不变。

### 3.4 加入 stdio transport 与 `sagent-rpc` 二进制

1. 在 `src/bin/sagent-rpc.rs` 解析 `--home`、`--profile` 等与现有 CLI 一致的启动参数；参数在服务启动前一次性解析。
2. `stdio.rs` 从 stdin 按行读取，拒绝超过 1 MiB 的单行请求，逐行解析、分派、写出一行 JSON 响应。
3. 服务准备完成后先输出一条 `gateway.ready` 事件；之后才消费请求。
4. stdout 只能调用统一 `write_frame`；日志、诊断、panic hook 全部写 stderr。
5. stdin EOF 时正常退出，不额外写空行、文本或伪响应。

验收：用子进程测试连续发送多条 NDJSON 请求，验证响应顺序、每行可解析、stdout 无噪声，以及 EOF 后退出码为 0。

### 3.5 端到端契约测试、文档与质量门禁

1. 在 `crates/sagent-protocol/tests/` 创建 stdio 黑盒测试：以独立临时 home/profile 和 fixture `state.db` 启动 `sagent-rpc`。
2. 覆盖 `gateway.ready`、ping、list、resume、归档过滤、未知 session、未知方法、坏 JSON、坏参数、notification 无响应、无数据库等场景。
3. 用结构化断言检查 JSON 字段与错误码；仅对真正稳定的完整帧使用小型 golden fixture，避免用快照冻结时间戳。
4. 补充本文件中的“启动和交互示例”，并给 `sagent-rpc --help` 添加简洁说明。
5. 执行 `cargo fmt --check`、`cargo test --workspace --offline`、`cargo clippy --workspace --all-targets --offline -- -D warnings`；随后由 CI 的 Windows/macOS/Linux 矩阵验证。

## 7. 测试清单

| 层级 | 用例 |
| --- | --- |
| 协议 DTO | request、result、error、event 的序列化格式与可选字段 |
| 分派 | `jsonrpc` 错误、方法不存在、参数不合法、notification 不回包 |
| 服务层 | 默认/具名 Profile、归档过滤、分页、可见消息过滤、session 不存在 |
| stdio | 多行 NDJSON、响应顺序、超长行、EOF、stdout 无日志 |
| 只读保证 | RPC 前后数据库文件内容/大小不变，且不存在的 state.db 不被创建 |
| 回归 | workspace 全量 test、fmt、严格 clippy |

测试一律使用临时目录和 fixtures，不读取开发者真实的 `SAGENT_HOME` 或根目录 `state.db`。

## 8. 启动与交互示例（目标形态）

```powershell
sagent-rpc --home D:\tmp\sagent-home --profile default
```

标准输出首先出现：

```json
{"jsonrpc":"2.0","method":"event","params":{"type":"gateway.ready","payload":{"protocol_version":1,"features":["gateway.ping","session.list","session.resume"]}}}
```

然后向标准输入写入：

```json
{"jsonrpc":"2.0","id":1,"method":"session.list","params":{"limit":20}}
{"jsonrpc":"2.0","id":2,"method":"session.resume","params":{"session_id":"<session-id>","message_limit":50}}
```

每一个带 `id` 的合法请求各得到一行结果或错误响应。

## 9. 完成定义

第三阶段完成须同时满足：

- workspace 中存在经过测试的 `sagent-protocol` 与 `sagent-rpc`；
- 本地 stdio NDJSON JSON-RPC 可稳定提供 ready、ping、list、resume；
- 参数、错误码、分页和可见消息规则有明确契约与自动化测试；
- RPC 不会写入或创建 session 数据库，stdout 不会混入日志；
- 默认 Profile 与具名 Profile 都经端到端测试；
- 全 workspace 的 format、test、clippy 通过，三平台 CI 通过。

## 10. 第四阶段再处理的事项

第四阶段才评估接入 Rust TUI（建议先评估 `ratatui` + `crossterm`）、WebSocket transport、会话写操作、订阅/增量事件以及 Agent 运行时。届时 `session.resume` 是否扩展为真正的 Agent 恢复，必须在 Agent 消息模型、上下文缓存和工具运行边界已经设计完成后再决定。
