# Sagent Rust 第四阶段 4.4 计划：Provider 与流式模型接入

作者：SongZQ  
状态：规划中  
前置条件：4.1 已完成 Agent 状态机与 PromptSnapshot；4.2 已完成 generation、Turn、消息和事件持久化；4.3 已完成 SessionActor、Supervisor、取消和生命周期管理。

## 1. 阶段目标

4.4 的目标是让 4.3 的 fake worker 替换为可测试的 Provider worker，并完成一条最小的真实模型调用链：

```text
SessionActor
    │ ProviderRequest + CancellationToken
    ▼
ModelProvider
    │ HTTP/SSE
    ▼
OpenAI-compatible endpoint
    │ delta / finish / usage / error
    ▼
ProviderEvent
    │ 转换为模型无关 WorkerEvent
    ▼
SessionActor
    │ Store 事务 + RuntimeEvent
    ▼
TUI / RPC / 历史查询
```

完成后，用户可以在一个 Profile 中发送普通文本 prompt，看到流式输出，完成后重新打开数据库仍能恢复完整 user/assistant 消息；取消、EOF、网络错误和 Provider 失败也具有确定的 Turn 终态。

## 2. Python 参考代码

| Python 文件 | 重点参考内容 | Rust 落点 |
| --- | --- | --- |
| `D:\projects\hermes-agent\agent\transports\chat_completions.py` | Chat Completions 请求体、SSE 行解析、delta、finish_reason、usage | `sagent-provider` 的 OpenAI adapter |
| `D:\projects\hermes-agent\run_agent.py` | 模型调用循环、最终响应收口、取消和错误传播 | Provider worker 与 `SessionActor` 的边界 |
| `D:\projects\hermes-agent\tui_gateway\server.py` | `_start_inflight_turn`、`_run_prompt_submit`、流式事件发送 | WorkerEvent 到 RuntimeEvent 的转换 |
| `D:\projects\hermes-agent\hermes_state.py` | user/assistant 消息写入、短事务和历史可见性 | 继续调用 `sagent-store`，Provider 不写 SQL |
| `D:\projects\hermes-agent\hermes_constants.py` | profile/home 路径解析 | `sagent-config` 的 Profile resolver |
| `D:\projects\hermes-agent\agent\providers` | endpoint、model、credential 的配置习惯 | Rust Profile-scoped provider config |

Python 代码只作为行为参考，不直接复制共享状态、线程或异常层次。Rust 必须保持 4.3 的单一写者约束：Provider 只能向 Actor 发送事件。

## 3. 范围与非目标

### 3.1 本阶段实现

- 新增独立 `sagent-provider` crate；
- 定义 provider-neutral `ModelProvider` trait 和 DTO；
- 实现 test-only Mock Provider/Mock SSE Server；
- 实现 OpenAI-compatible Chat Completions HTTP/SSE adapter；
- 支持文本 delta、正常结束、usage、tool-call 原始片段的中性表示、EOF、429、5xx、解析错误和取消；
- 从当前 Profile 配置解析 endpoint、model 和 API key；
- 将 Provider 输出接入 4.3 `SessionActor`；
- 添加 fixture、单元测试、Provider-Actor 集成测试和真实 endpoint 可选 smoke test。

### 3.2 本阶段不实现

- 多 Provider fallback、路由、credential pool；
- 完整工具执行、approval、terminal、read_file；属于 4.5；
- RPC 写方法、WebSocket 和 ratatui TUI；属于 4.6/4.7；
- compression、fork、retry 的 generation 迁移；
- Provider 直接写 SQLite、直接广播 RuntimeEvent 或直接打印 stdout；
- 把 API key 放入数据库、PromptSnapshot、事件 payload 或错误文本。

## 4. 架构和依赖边界

新增目录：

```text
crates/sagent-provider/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── error.rs          # ProviderError 与错误分类
    ├── types.rs          # ProviderRequest、ProviderEvent、ProviderFinish
    ├── trait.rs          # ModelProvider、ProviderEventSink
    ├── mock.rs           # test-only provider
    ├── sse.rs            # SSE framing/parser
    └── openai.rs         # OpenAI-compatible adapter
```

依赖方向：

```text
sagent-types ───────┐
sagent-config ──────┼──> sagent-provider ───> sagent-runtime
serde/serde_json ───┤
reqwest/tokio -------┘
```

Provider 不依赖 `sagent-store`。Runtime 可以依赖 Provider，但 Provider 不能反向依赖 Runtime，避免循环依赖。若 `WorkerEvent` 当前定义在 runtime 内，应增加一个窄的转换层，而不是让 Provider 引用 Actor 私有类型。

## 5. 稳定的 Provider API

建议先定义以下模型无关接口，具体字段可根据现有 `sagent-agent` 类型调整：

```rust
#[async_trait::async_trait]
pub trait ModelProvider: Send + Sync {
    async fn stream(
        &self,
        request: ProviderRequest,
        sink: &mut dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ProviderFinish, ProviderError>;
}

pub trait ProviderEventSink {
    fn emit(&mut self, event: ProviderEvent) -> Result<(), ProviderError>;
}
```

`ProviderRequest` 至少包含：

```text
profile / provider_id / model
messages（来自 PromptSnapshot，不是数据库连接）
temperature 等已确认的模型参数
request_id / turn_id（仅用于关联和日志）
```

`ProviderEvent` 至少包含：

```text
TextDelta(String)
ToolCallDelta（先保留中性结构）
Usage(TokenUsage)
Finished { reason: StopReason }
```

Provider 返回 `ProviderFinish`，但不得自行决定 Turn 的最终持久化状态。Actor 根据事件和错误调用 `complete_turn`、`fail_turn` 或 `interrupt_turn`。

## 6. 错误分类

错误分类必须稳定，不能把完整响应体或密钥放进错误：

| 类型 | 示例 | Actor 行为 |
| --- | --- | --- |
| Configuration | endpoint/model/key 缺失 | `fail_turn`，提示配置错误 |
| Authentication | 401/403 | `fail_turn`，不自动重试 |
| RateLimited | 429 | `fail_turn`，保留 retry-after 元数据 |
| RemoteServer | 5xx | `fail_turn` |
| Transport | DNS、连接断开、超时 | `fail_turn` |
| Protocol | SSE/JSON 解析失败 | `fail_turn` |
| Cancelled | CancellationToken 已取消 | `interrupt_turn` |
| IncompleteStream | EOF 前没有 finish | `fail_turn`，不伪造 assistant final |
| EventSinkClosed | Actor 已停止 | 停止 Provider，不再写库 |

错误中允许保留 status code、分类、request_id 和安全摘要；禁止保留 API key、Authorization header 和完整 provider 原始响应。

## 7. 分步执行计划

### 步骤 0：基线、契约和 Python 行为清单

1. 运行 `cargo fmt --all -- --check`、`cargo test --workspace --offline` 和 clippy；
2. 阅读并记录 Python Chat Completions 的请求字段、SSE 事件格式和异常分类；
3. 核对 `sagent-agent::PromptSnapshot`、消息 role、generation 和 4.3 `WorkerEvent`；
4. 决定 ProviderEvent 到 WorkerEvent 的映射，不引入 tool 执行；
5. 建立一份行为清单：delta 顺序、finish 顺序、EOF、usage、取消和错误。

完成条件：Provider API 设计不会泄露 HTTP/SSE 专属类型，也不会改变 4.3 Store schema。

### 步骤 1：创建 `sagent-provider` crate

1. 新建 crate 并加入 workspace；
2. 添加 `serde`、`serde_json`、`thiserror`、`tokio`、`tokio-util` 和 HTTP 客户端的最小依赖；
3. 创建 `lib.rs`、`error.rs`、`types.rs`、`trait.rs`；
4. 只导出 trait、DTO、错误和安全的 usage/finish 类型；
5. 为 DTO 做 JSON round-trip 和敏感字段不序列化测试。

完成条件：`cargo check -p sagent-provider --offline` 通过，Provider crate 不依赖 Store/Runtime。

### 步骤 2：实现 Mock Provider 和 Mock SSE Server

1. 使用本地 HTTP server 返回固定 SSE fixture；
2. 支持 `delta → delta → finish` 正常序列；
3. 支持半包 JSON 和多条 SSE event 粘连；
4. 支持无 finish 的 EOF；
5. 支持 429、500、连接延迟和主动断开；
6. 支持 CancellationToken，取消后停止读取并返回 `Cancelled`；
7. 测试 Provider 不写 Store、不打印 stdout，只向 sink 发事件。

建议 fixture：

```text
tests/fixtures/provider/
├── normal_text.sse
├── split_json.sse
├── usage.sse
├── eof_without_finish.sse
├── rate_limited.json
└── server_error.json
```

完成条件：不访问真实网络即可覆盖所有协议边界。

### 步骤 3：实现 SSE framing 和 JSON parser

1. 按空行分隔 SSE event；
2. 支持多行 `data:` 合并；
3. 忽略 comment/keep-alive 行；
4. 正确处理 CRLF、LF 和半包边界；
5. 对 `[DONE]` 映射为 finish/EOF 语义；
6. 对非法 JSON 返回 Protocol 错误，并包含 event 序号；
7. 解析文本 delta、finish_reason、usage 和中性 tool-call 片段；
8. 禁止把原始 provider JSON 传到 RuntimeEvent。

完成条件：parser 是纯函数或独立异步 reader，可用 fixture 逐条断言，不依赖 Actor。

### 步骤 4：实现 OpenAI-compatible adapter

1. 根据 Profile 配置组装 endpoint、Authorization、Content-Type 和 model；
2. 根据 PromptSnapshot 生成 Chat Completions 请求体；
3. 默认启用 stream；
4. 通过 SSE parser 读取响应并产生 ProviderEvent；
5. 按 status code 分类 401、403、429、4xx、5xx；
6. 支持连接超时、读取超时和取消；
7. 记录安全的 request metadata，不记录密钥和完整 prompt；
8. 将 finish_reason、usage、provider request id 放入 `ProviderFinish` 的安全字段。

完成条件：Mock endpoint 能完整模拟一个普通文本回合，adapter 不依赖 Store 或 Runtime。

### 步骤 5：Profile-scoped credential resolver

1. 明确 endpoint/model/provider 的 config 字段；
2. 从当前 Profile 路径解析配置；
3. API key 只从现有 secret resolver 或 `.env` 读取；
4. 不新增非秘密 `HERMES_*`/`SAGENT_*` 行为环境变量；
5. 缺失、空值、非法 URL 和不支持 provider 返回 Configuration 错误；
6. 测试 default 与命名 profile 的配置隔离；
7. 测试错误和日志中不出现 API key。

完成条件：同一 SessionId 在不同 Profile 中使用不同 endpoint/key，且不会互相污染。

### 步骤 6：接入 SessionActor

1. 将 runtime 的 fake worker factory 抽象为 Provider worker factory；
2. Actor 从 PromptSnapshot 构造 ProviderRequest；
3. ProviderEvent::TextDelta 转换为 `WorkerEvent::TextDelta`；
4. Provider 完成时发送 `FinalText`，由 Actor 调用 `complete_turn`；
5. Provider 错误发送 `Failed`，由 Actor 调用 `fail_turn`；
6. CancellationToken 取消时发送 `Cancelled`，由 Actor 调用 `interrupt_turn`；
7. provider worker 不持有 Store，不直接广播 RuntimeEvent；
8. 保持 final/interrupt/failure 的单一终态竞争规则；
9. Provider 迟到事件必须按 turn_id 和 active 状态丢弃。

完成条件：4.3 的 fake worker 测试仍通过，同时新增 Provider worker 可驱动同一套持久化收口。

### 步骤 7：端到端集成测试

至少覆盖：

1. submit → Mock SSE delta → finish → assistant message 恢复；
2. 多个 delta 的顺序和完整拼接；
3. usage 出现在完成结果，但不污染消息正文；
4. 半包 JSON 和多行 SSE；
5. `[DONE]` 正常完成；
6. EOF 无 finish → failed，且没有 assistant 空消息；
7. 401/403/429/5xx/网络断开；
8. submit 后立即 cancel；
9. delta 过程中 cancel；
10. final 与 cancel 竞态只有一个 Turn 终态；
11. provider panic/JoinError；
12. 两个 Session 并行且事件隔离；
13. 两个 Profile 的 endpoint/credential 隔离；
14. 对照 Python fixture 验证 transcript 顺序。

### 步骤 8：真实 endpoint smoke test（可选）

1. 只在显式提供测试 Profile 和凭据时运行；
2. 默认不加入离线 CI；
3. 发送最小、无敏感内容的 prompt；
4. 验证 delta、finish、assistant 持久化和 cancel；
5. 测试失败时不输出 API key 或完整响应；
6. 在文档中记录 endpoint 兼容性和已知差异。

### 步骤 9：质量门禁和提交边界

每个提交只改变一个行为边界，建议：

```text
feat(provider): add provider-neutral request and event types
test(provider): add mock SSE fixtures and parser coverage
feat(provider): implement OpenAI-compatible streaming adapter
feat(runtime): connect provider worker to SessionActor
test(runtime): cover provider persistence and cancellation races
```

每次提交执行：

```text
cargo fmt --all -- --check
cargo test -p sagent-provider --offline
cargo test -p sagent-runtime --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
git diff --check
```

最终执行 `cargo test --workspace --offline`。

## 8. 数据和事件时序

普通文本回合必须保持以下顺序：

```text
1  Store::begin_turn
2  TurnStarted / UserMessagePersisted
3  Provider 请求发送
4  ModelTextDelta（0..N 次，仅广播）
5  Provider Finished
6  Store::complete_turn（assistant + completed + daemon events）
7  FinalMessagePersisted
8  TurnCompleted
```

取消时：

```text
1  Client Interrupt
2  Actor cancellation.cancel()
3  Provider 停止读取 SSE
4  Store::interrupt_turn
5  TurnInterrupted
```

Provider 错误时：

```text
1  ProviderError
2  Actor 转为 WorkerEvent::Failed
3  Store::fail_turn
4  TurnFailed
```

禁止以下错误顺序：

```text
先广播 TurnCompleted，再写 assistant；
Provider 直接写 messages；
取消后伪造空 assistant final；
EOF 无 finish 却标记 completed。
```

## 9. 验收清单

- [ ] `sagent-provider` 独立 crate 创建并加入 workspace；
- [ ] Provider trait 与 DTO 不泄露 HTTP/SSE 类型；
- [ ] Mock SSE 覆盖正常、半包、EOF、429、5xx 和取消；
- [ ] OpenAI-compatible adapter 可以流式输出普通文本；
- [ ] Profile-scoped resolver 正确读取 endpoint/model/key；
- [ ] API key 不进入日志、消息、事件和错误正文；
- [ ] Provider 不依赖 Store，不直接写数据库；
- [ ] Actor 仍是唯一 Turn/Message 写入者；
- [ ] delta 只作为瞬态事件；
- [ ] final/failure/cancel 都有唯一终态；
- [ ] 完成事件发布前 assistant 已可从 Store 读取；
- [ ] EOF、网络错误、取消不会伪造 assistant final；
- [ ] 普通回合退出后可以重新打开并恢复；
- [ ] workspace fmt/test/clippy 全部通过。

## 10. 风险和取舍

### SSE parser 与 HTTP client 解耦

先把 parser 写成独立模块，避免网络重试、缓冲区和 JSON 解析问题同时出现在 Actor 测试中。

### Provider 重试暂不自动实现

429/5xx 的自动重试会影响 Turn 的取消、generation 和 request_id 幂等语义。4.4 先分类并失败收口，重试作为后续独立能力实现。

### tool call 只做协议保留

4.4 可以解析并传递中性的 tool-call delta，但不执行工具；工具生命周期和 approval 留到 4.5。

### 真实 smoke test 不进入默认 CI

真实 endpoint 依赖外部网络和凭据，默认 CI 使用 Mock SSE；真实测试必须显式启用并使用隔离 Profile。

## 11. 4.4 完成后的交付边界

4.4 完成后：

```text
普通 prompt
  → Profile 配置解析
  → OpenAI-compatible SSE
  → ProviderEvent
  → WorkerEvent
  → SessionActor
  → Store 事务
  → RuntimeEvent
  → 可恢复 assistant 历史
```

4.4 仍不提供工具执行、approval UI、公开交互 RPC 和 TUI。下一阶段 4.5 在此基础上增加 `read_file`、terminal、approval 和工具结果持久化，但必须继续遵守本计划定义的 Provider/Actor 边界。

## 12. 步骤 0 执行记录

执行日期：2026-09-04  
状态：契约核对完成；测试门禁受依赖缓存和网络环境阻塞

### 12.1 基线门禁

- `cargo fmt --all -- --check`：通过；
- `cargo test --workspace --offline`：未执行到编译阶段，原因是本机离线 Cargo 索引缺少 `anyhow`；
- `cargo clippy --workspace --all-targets --offline -- -D warnings`：同样因离线索引缺少 `anyhow` 无法解析依赖；
- `cargo test --workspace`：尝试在线更新 crates.io 索引，但当前环境无法连接 `index.crates.io:443`。

上述失败属于依赖缓存/网络环境问题，不是 Rust 源码编译错误。待依赖缓存可用后，应重新执行完整门禁。

### 12.2 Python 行为清单

已核对：

- `agent/transports/chat_completions.py` 的消息转换、请求参数、stream、finish_reason、usage 和响应规范化；
- `tui_gateway/server.py` 的 `_start_inflight_turn`、`_run_prompt_submit`、`_interrupt_session_turn` 及 `message.delta`/`message.complete` 发送路径；
- `run_agent.py` 的模型循环、流式回调、Provider 错误归类、usage 统计和 `interrupt`；
- `hermes_state.py` 的短事务消息写入和会话历史持久化边界。

得到的 4.4 行为约束：

```text
Provider response delta       → 只产生流式增量，不直接写 transcript
finish_reason=stop            → 产生完整 assistant 文本
usage                         → 作为结构化元数据，不拼进消息正文
SSE [DONE]                    → 正常结束信号
EOF（没有 finish）             → 不完整流，不能伪造 final
429                           → RateLimited
5xx                           → RemoteServer
连接/读取异常                  → Transport
JSON/SSE 解析异常              → Protocol
取消                          → Cancelled，由 Actor 持久化 interrupted
```

Python 的多 Provider fallback、线程共享 session 字典、工具执行和复杂重试不纳入 4.4 第一版。

### 12.3 Rust 现有契约核对

已确认当前 Rust 已具备：

- `sagent-agent::PromptSnapshot`、`PromptMessage`、role 和 tool-call 关联校验；
- `SessionCommand::SubmitPrompt` 与 `Interrupt`，以及稳定的 `RequestId`；
- `sagent-runtime::WorkerEvent` 的 `TextDelta`、`FinalText`、`Failed`、`Cancelled`；
- `RuntimeEvent` 的 session/turn/request 归属、delta、终态和订阅 lag 语义；
- `CancellationToken`、ActiveTurn 和 Actor 的唯一 Store 写入边界。

Provider 到 Runtime 的唯一允许映射为：

```text
Provider TextDelta  → WorkerEvent::TextDelta
Provider Finished   → WorkerEvent::FinalText
Provider Error      → WorkerEvent::Failed
Provider Cancelled  → WorkerEvent::Cancelled
```

### 12.4 步骤 0 结论

Provider-neutral 契约已经明确，可以进入步骤 1。步骤 1 应只创建 `sagent-provider` crate，定义 trait、DTO 和错误类型；暂不实现真实 HTTP、SSE 或 Actor 接入。依赖缓存恢复后，先补跑本记录中的 workspace 测试和 clippy，再提交步骤 1。
