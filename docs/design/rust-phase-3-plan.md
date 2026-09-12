# Sagent Phase 3：能力扩展与本地多客户端接入计划

状态：规划中
范围：独立 Rust 项目 `sagent`
前置条件：Phase 0–2 已关闭；三平台 CI 与三平台手工 smoke 已完成。

## 1. 定位、命名与目标

本计划是 [Phase 0–2 收尾计划](rust-phase-0-to-2-completion-plan.md) 所称的“下一步
Phase 3”。仓库中较早的 `rust-third-phase-plan.md` 与 `rust-fourth-phase-*.md` 记录的是
已经实施过的 RPC/最小 Agent 垂直切片的历史拆分；它们不定义本阶段范围。新工作必须以
本文件的工作包、边界和退出条件为准。

Phase 3 的目标不是堆叠更多工具，而是把 Phase 2 的单 Provider、固定工具集、本地 TUI
闭环扩展成一套可控的能力解析与运行时编排机制：

```text
Profile config + secrets
          │
          ▼
Capability resolver ──► immutable GenerationResolution
          │                         │
          ├── Provider router       ├── prompt / cache boundary
          ├── MCP tool catalog      ├── tool schema hash
          ├── Skills + memory       └── audit / recovery metadata
          └── policy + client caps
                    │
                    ▼
SessionSupervisor / SessionActor ──► Store
          │             │              │
          │             ├── cron       ├── memory records
          │             ├── delegation └── scheduled/delegated runs
          │             └── browser broker
          ▼
CLI / TUI / local Web / Desktop clients
```

阶段结束时，一个 Profile 可安全使用多 Provider 路由、MCP、Skills/本地 memory、持久化
cron 和受预算限制的子代理；Desktop/Web 客户端可通过认证的本地连接附着到同一个 Rust
daemon。所有能力都必须在 session/generation 边界解析，不能中途改变缓存前缀或工具 schema。

## 2. 适用约束与明确非目标

### 2.1 不变量

| 不变量 | Phase 3 的落实方式 |
| --- | --- |
| Prompt cache 稳定 | Provider 路由、模型、system prompt、Skills、memory policy、工具 schema 均冻结到 `GenerationResolution`；reload 只影响新 session 或显式 generation transition。 |
| 单一写者 | 只有 `SessionActor` 可以提交 parent session 的 turn/message/event；MCP、provider、cron、child actor、browser controller 只能发送受类型约束的结果。 |
| Profile 隔离 | 每个 resolver、scheduler、memory store、MCP credential 和 child session 都从启动时固定的 `SagentPaths` 派生；RPC 参数不得传入任意 home、数据库或 profile。 |
| 最小权限 | 工具权限由已解析的 session policy 与客户端 capability 决定；不可信/无交互客户端默认没有 terminal、write、browser 控制等能力。 |
| 取消有归属 | 每个 MCP call、provider attempt、scheduled run、child session 与 controller request 都有父 `CancellationToken`、deadline 和 join record；禁止 detached task。 |
| 可恢复且可审计 | 先持久化可恢复事实，再通知客户端；token delta、临时 browser 画面和 MCP stderr 不进入 transcript 真相。 |

### 2.2 本阶段不做

- 不读取、迁移或依赖 Python/Hermes 运行时、数据库、插件或 worker；Python 只能是设计参考，**不实现 Python bridge**；
- 不迁移 Telegram、Discord、Slack、Webhook 或其他消息 Gateway；这属于后续 Gateway 阶段；
- 不开放公网 HTTP/RPC，也不以 loopback 监听替代认证；
- 不发布 WASM/native plugin SDK、第三方 marketplace 或通用扩展 hook；MCP 是本阶段唯一的外部工具集成标准；
- 不实现完整浏览器自动化内核、截图管线或远程浏览器服务；仅建立已认证 client controller 的 broker 边界；
- 不引入 credential pool、对已产生可见输出的自动 provider failover，或把真实凭据加入默认测试；
- 不在本阶段重做完整桌面产品或公共 Web 产品；交付的是本地 attach 协议和最小可用客户端壳层。

## 3. 当前基线与架构落点

当前 workspace 已有 `sagent-config`、`sagent-store`、`sagent-agent`、`sagent-provider`、
`sagent-tools`、`sagent-runtime`、`sagent-protocol`、`sagent-rpc` 与 `sagent-tui`。已有
OpenAI-compatible 流、`SessionActor`、generation/tool schema hash、`read_file`、
`write_file`、受审批 terminal、FTS session search、stdio/loopback WebSocket RPC。

Phase 3 只在下列职责明确时增加 crate：

```text
sagent-types
  ├── sagent-config            # Profile 配置、secret reference、capability 输入
  ├── sagent-store             # 仅 Sagent 自有的加性 SQLite migration
  ├── sagent-agent             # 纯 Prompt/turn 不变量
  ├── sagent-provider          # provider trait、router、attempt 分类
  ├── sagent-tools             # 内置工具与不可变 registry
  ├── sagent-mcp               # MCP transport、server lifecycle、schema translation
  ├── sagent-memory            # memory 查询、来源标注与本地存储接口
  └── sagent-scheduler         # occurrence 计算、claim、worker supervision
              │
              ▼
       sagent-runtime           # resolver 消费者、actor/supervisor、delegation/browser broker
              │
              ▼
 sagent-protocol / sagent-rpc / sagent-cli / sagent-tui / apps/*
```

`sagent-mcp`、`sagent-memory`、`sagent-scheduler` 不得依赖 `sagent-runtime` 或
`sagent-protocol`；Runtime 负责把它们装配成一次 session 的能力。这样不会形成循环依赖，
也能用真实 fixture 单独验证其 I/O 与恢复语义。

## 4. 共同地基：P3.0 能力解析与 generation 边界

### 4.1 目的

在接入任何动态能力前，先建立单一 `CapabilityResolver`。当前直接在 bootstrap 里创建
Provider/ToolRegistry 的方式不能扩展为“在回合中 reload MCP 或切换模型”，否则会破坏
`session_generations` 记录的 `system_hash` 与 `tool_schema_hash`。

### 4.2 实现项

1. 在 `sagent-config` 定义非秘密的 `CapabilityConfig`：provider route、MCP server 声明、
   Skills/memory policy、scheduler 默认值和 workspace policy；所有行为开关写入
   `config.yaml`，不得用新的 `SAGENT_*` 环境变量。
2. 在 `sagent-runtime` 新建窄的 `capabilities` 模块，输入固定 Profile paths、已认证
   `ClientCapabilities` 和配置 revision，输出 `GenerationResolution`：
   - `SystemPromptParts` 的稳定输入与 hash；
   - canonical `ToolRegistry` 与 tool schema hash；
   - provider route snapshot/hash 与首选 provider/model；
   - 已激活 Skills 的内容 hash；
   - memory policy/hash；
   - 允许的 approval/browser/controller capability。
3. 为 `session_generations` 增加一个加性 resolution 记录（建议独立
   `generation_resolutions` 表），只保存 hash、provider ID、配置 revision、非秘密
   capability 摘要和创建时间；不保存 API key、MCP token、MCP stderr 或完整 prompt。
4. 定义唯一的 transition：`session.refresh_capabilities` 只能在没有 running turn 时创建
   新 generation；普通 config/MCP/Skill reload 只返回“对新会话生效”。
5. `gateway.ready`/`config.read` 仅公开可展示的能力名称和 revision，不泄露 endpoint、
   credential reference、本地绝对路径或 MCP command line。

### 4.3 验收

- 同一个 generation 中配置文件改变、MCP health 改变或 memory 新增，均不能改变其
  prompt/system/tool schema hash；
- 显式 refresh 后才创建新 generation，旧 transcript 仍可按原 resolution 恢复；
- 两个 Profile 的 capability snapshot、workspace、secret reference 和 Store 完全隔离；
- migration 从当前 schema v3 及本阶段每个中间版本可重复升级，且降级二进制会因未知
  schema 版本 fail closed；
- contract fixture 覆盖“reload 不影响当前 generation”和“capability 不随环境变量漂移”。

## 5. P3.1 多 Provider 路由

### 5.1 交付范围

先支持多个 OpenAI-compatible provider 的命名配置和确定性路由，再保留其他 provider
adapter 的扩展位。路由是 Runtime 的依赖，不是模型可见工具。

建议配置形态（字段名在实现前以 serde contract 固化）：

```yaml
providers:
  primary:
    kind: openai-compatible
    base_url: https://example.invalid/v1
    api_key_env: SAGENT_PRIMARY_KEY
    model: model-a
  backup:
    kind: openai-compatible
    base_url: https://example.invalid/v1
    api_key_env: SAGENT_BACKUP_KEY
    model: model-b
routes:
  default:
    candidates: [primary, backup]
    retry_before_first_event: 1
```

`api_key_env` 仍只表示 secret 的来源名；路由顺序、重试数、超时、模型选择等行为配置
必须写入 YAML。公开 RPC 配置摘要不得回显 endpoint 或变量名。

### 5.2 实现项

1. 将 `sagent-config::ResolvedProvider` 拆成不含 secret 的 `ProviderDescriptor` 与仅在
   构造 client 时消费 secret 的 factory；校验候选名称重复、循环引用、空 route 和未知模型。
2. 在 `sagent-provider` 实现 `ProviderRouter` 与 `ProviderAttempt`：按 generation 已冻结的
   route 依次选择 provider，记录 attempt 序号、稳定 failure kind、开始/结束时间和结果。
3. 自动 fallback 只允许发生在**首个文本 delta、tool-call fragment 或 usage 之前**。任一
   可见/可持久化 provider event 出现后，失败必须结束当前 turn；用户可显式 retry，不能
   静默换 provider 造成重复工具调用或不同模型的半段回复。
4. 把选中的 provider/model 与每个 attempt 的无秘密摘要写入 Store outcome/event，供恢复、
   CLI 诊断与错误分析；不写 HTTP header、request body 或 credential。
5. 为路由增加 provider-local timeout、cancellation 传播和不可重试分类（配置错误、认证
   拒绝、参数错误）；429/可恢复网络错误仅在未产生事件时按 route policy 重试。

### 5.3 验收与测试

- Mock provider fixture 验证 primary 在首事件前失败时只调用 backup 一次；
- primary 已发送 delta 或 tool fragment 后失败时，backup 绝不启动；
- interrupt 会停止当前 attempt，且不会尝试下一个候选；
- 同一 generation 的 route hash 和 provider 选择不随后续 YAML 改动漂移；
- Profile A 不能引用或诊断 Profile B 的 provider/secret；真实 endpoint 只允许显式 opt-in
  smoke，默认 CI 使用 loopback Mock SSE。

## 6. P3.2 MCP：受控的外部工具目录

### 6.1 分两步交付

| 子项 | 内容 | 前置条件 |
| --- | --- | --- |
| P3.2a | stdio MCP client、生命周期、`initialize`、`tools/list`、`tools/call`、取消、timeout、health | P3.0 |
| P3.2b | streamable HTTP/remote transport、OAuth credential reference、reconnect/backoff | P3.2a 的 contract 与安全审计通过 |

### 6.2 配置与命名

MCP server 按 Profile 配置。stdio command 必须是显式 argv 数组，禁止经 shell 拼接；远程
endpoint 与 OAuth reference 只在 server 侧解析。

```yaml
mcp:
  servers:
    files:
      transport: stdio
      command: ["sagent-mcp-files", "--root", "workspace"]
      timeout_ms: 30000
      enabled: true
```

内部名称固定为 `mcp.<server_id>.<tool_id>`。在构建 `ToolRegistry` 前执行规范化、重复名
检查和 schema 校验；不能把两个不同 server 的同名工具偷偷覆盖。模型可见名称与内部名称
必须有可逆映射，并写入 generation resolution。

### 6.3 实现项

1. 新建 `sagent-mcp`：transport 只负责 framing、request ID、MCP lifecycle、server
   health 和 stderr ring buffer；它不依赖 Store、Prompt 或 Runtime。
2. Runtime 在 resolver 阶段向每个启用 server 执行有限生命周期探测，成功的 `tools/list`
   结果转换为 immutable `ToolDefinition`；失败 server 对新 session 标为 unavailable，
   不影响内置工具和其他 server。
3. `ToolWorker` 为 MCP call 建立一个 invocation，传入 cancellation/deadline，限制输入、
   输出和 stderr 大小；结构化 content 转换为有界 `ToolResult`，完整 artifact 另存受控
   引用而不是塞进模型上下文。
4. `mcp.reload` 只更新 resolver cache/revision；已有 generation 不增删 schema。需要新工具
   的用户必须创建新 session 或执行 P3.0 的显式 refresh。
5. OAuth token 只通过 Profile-scoped secret reference/keychain 获取；token refresh 失败只
   影响该 server health，不写日志值，也不回传给客户端。

### 6.4 验收与测试

- 用真实子进程 fixture 覆盖 initialize/list/call、超时、取消、异常退出、stderr 截断、
  非法 schema、同名 collision 和重启；
- HTTP fixture 覆盖断线、重连、401、OAuth refresh failure；
- server 不可用时现有 session 的 schema hash 不变，新 session 只少该 server 工具并获得
  明确 health 状态；
- MCP tool 的权限不高于内置工具：无 interactive approval capability 时危险调用 fail closed；
- Windows/macOS/Linux 均使用宿主真实子进程，而不伪造 OS 或 mock process tree。

## 7. P3.3 Skills 与本地 Memory

### 7.1 Skills

Skills 保持文件资产，不作为可执行插件。解析顺序为 Profile skills、用户显式配置的 skills、
项目 skills；每个路径必须 canonicalize 并限制在声明根目录。完整内容在激活时读取，不给
模型提供可只读“第一页”的 offset/limit 接口。

- `/skill <name>` 作为 normal user message/command event 进入 transcript，不能直接改写
  已缓存 system prompt；
- 安装、删除、编辑 Skill 默认只影响新 session；`--now` 必须创建显式 generation transition
  并向客户端显示 cache invalidation；
- 记录 activated skill 名称、内容 hash 和来源，不持久化无关私有文件内容。

### 7.2 Memory MVP

Phase 3 只实现 Profile-scoped 本地 memory，不接第三方 memory provider。memory 条目必须
具备 `id`、`profile_id`、`content`、`source`、`created_at`、`updated_at`、`visibility`、
`superseded_by` 与 FTS 索引。初始写入来源仅允许：

1. 用户显式的 CLI/RPC memory save/update/delete；
2. 模型提出的候选记忆，经 interactive approval 后提交。

自动从任意对话静默抽取并永久写入 memory 不属于本阶段，避免 prompt injection 和错误事实
污染长期状态。

### 7.3 实现项

1. 新建 `sagent-memory`，定义 `MemoryStore`/`MemoryRetriever` 的窄接口和输入/输出 token
   预算；检索使用本地 SQLite FTS，稳定排序为 relevance、更新时间、ID。
2. 在 `sagent-store` 增加 memory 表、FTS 表及可逆的 tombstone/supersession 规则；写入必须
   走 transaction 并追加可审计 event。
3. resolver 把 memory policy（是否启用、最大条数、最大 token、允许 visibility）冻结到
   generation；每个 turn prefetch 的条目作为明确的 prompt input，带来源标记和总 token。
4. memory 内容必须与普通用户消息以不可伪造的 section boundary 渲染；不允许 memory 自己
   声称拥有系统权限或重新定义 tool policy。
5. 提供 CLI/RPC 的受权限保护管理面：list/search/save/update/delete；Desktop/Web 仅在能力
   协商后显示该管理入口。

### 7.4 验收与测试

- memory/Skills 的新增、编辑、删除不改变现有 generation，refresh 后才生效；
- 相同检索输入产生确定顺序和不超过 token/条数上限的结果；
- Profile、visibility 和 tombstone 不串读；被删除条目不再 prefetch；
- candidate memory 未批准时零写入；批准、取消、崩溃恢复均不会出现半条记录；
- fixture 覆盖 CJK/emoji、长文本截断、注入式内容、FTS 损坏与只读 Store。

## 8. P3.4 持久化 Cron 与后台任务

### 8.1 范围

Cron 是 Profile-scoped 持久化调度器，不是内存 timer。初版支持 cron expression、固定间隔、
一次性任务和 heartbeat；交付目标是向本地 runtime 创建受控 turn，并将结果记录在本地。
不发送 Telegram/Email/Webhook，也不把“进程还活着”视为任务成功。

### 8.2 数据与执行模型

新增 `scheduled_jobs`、`scheduled_runs` 和必要索引。job 至少保存：稳定 ID、Profile、
schedule/timezone、payload、enabled、misfire policy、最大并发、last scheduled time；run 至少
保存：job ID、不可变 occurrence timestamp、claim owner/lease、started/finished、outcome、
child session/turn reference、output artifact reference。

`(job_id, occurrence_at)` 是幂等键。执行前在单个 SQLite claim transaction 中插入/取得 run
lease；两个 daemon 同时 tick 时只有一个可以执行。lease 过期后的恢复必须先检查已持久化
终态，再决定 skip/retry，不能盲目重放。

### 8.3 实现项

1. 新建 `sagent-scheduler`：纯 occurrence 计算与 `SchedulerService` 分离；时间计算接受
   `Clock`，测试不能依赖 sleep。
2. 在 `sagent-runtime` 增加有 owner 的 task supervisor：deadline、并发上限、取消、join status
   与 structured error；scheduler 只通过它提交一次 job run。
3. job payload 解析为受限的 `ScheduledPrompt`，继承 job Profile/workspace/policy，绝不接受
   payload 覆盖任意 filesystem root、provider secret 或扩大 toolset。
4. CLI/RPC 提供 create/list/show/pause/resume/delete/run-history；管理操作进入 audit event。
5. health 同时报告 scheduler tick heartbeat、上次成功 tick、已到期未 claim 数、失败 run 数；
   不因 daemon 存活而报告 healthy。

### 8.4 验收与测试

- DST 前后、重启、misfire（skip/run-once/bounded-catch-up）、重复 tick、两个 scheduler、
  lease 失效、取消和 Profile 隔离均有 SQLite integration test；
- 进程崩溃于 claim 后、turn 前、turn 中、turn 后分别有确定恢复语义；
- 同一 occurrence 至多创建一个 active run/turn；
- scheduler 的 job 不读取开发者 home、真实凭据或 wall clock；
- TUI/CLI/本地 Web 对 run history 看到的是已持久化 outcome，不是临时 task 状态。

## 9. P3.5 Delegation：子会话而非共享 transcript

### 9.1 模型

`delegate` 是一个需要 policy/预算检查的工具。它创建新的 child `SessionId` 和独立
`SessionActor`，而不是让多个 worker 共享 parent actor 或直接写 parent transcript。

新增 `delegated_sessions`（parent/child session、parent turn/tool call、budget、deadline、
status、created/finished）及必要的 event relation。child 继承 parent 的 Profile、workspace
与 capability **上限**，但可被 policy 收窄；永远不能借 delegation 获取 parent 没有的
工具、secret、browser 或 approval 权限。

### 9.2 实现项

1. 在 `sagent-types` 定义 `DelegationId`、budget、status 与 cancellation lineage；在
   `sagent-runtime` 维护 parent→child supervisor relation 和每 Profile 并发/深度上限。
2. child 生成自己的 immutable `GenerationResolution`。parent 只获得结构化 progress、
   final summary、artifact references 或 failure；child token/event 不直接写进 parent transcript。
3. parent interrupt/cancel 级联取消 child；child 自身失败/超时必须收敛为 parent 的单次
   `ToolResult`，不能无限 retry 或留下 orphan actor。
4. 预算至少限制 child depth、并发数、最大 turn 数、deadline 和 provider token/attempt
   预算；超过任一限制在启动前 fail closed。
5. CLI/RPC 提供 delegation status/inspect/cancel，且只允许同一 Profile、具备父关系或管理
   权限的客户端查看。

### 9.3 验收与测试

- parent 与 child 同时工作时 transcript、Store 写入和 event sequence 不交叉；
- parent cancel、daemon restart、child panic、budget exhausted 均无 orphan task/lease；
- child 没有 interactive approval capability 时不能把请求转嫁给 parent UI；
- 结构化 child result 在 parent tool loop 中只提交一次，且可从 Store 恢复关系；
- 两个 Profile 之间无法查看、取消或引用彼此 child session。

## 10. P3.6 Browser controller broker

Browser controller 是 client 提供的会话能力，不是 daemon 根据 `SAGENT_DESKTOP`、argv 或
机器环境猜测出的进程身份。Phase 3 仅定义 daemon 与已认证 client 之间的 broker。

1. 扩展 `client.hello` capability，例如 `browser_control`、`artifact_open`、
   `interactive_approval`；方法 registry 依 capability gating，而不是 process-wide `check_fn`。
2. controller attach 时绑定 `ClientId`、connection lifetime、允许的 workspace/profile 与
   request concurrency；browser tool call 通过 broker 发送结构化 request，等待带 request ID
   的 response。
3. controller disconnect、deadline、session interrupt 或 daemon shutdown 必须取消在途
   request；不能让 actor 永久等待，也不能把 controller response 投递给另一个 session。
4. browser capability 不存在时，工具不进入该 generation 的 `ToolRegistry`；调用方获得
   stable `not_supported`/`capability_unavailable`，不能伪造成功。
5. 使用 fake controller + 真实 RPC connection 验证 attach/detach、乱序 response、重复
   response、断线、取消、并发 session 与 Profile 隔离。真实浏览器自动化不进入默认 CI。

## 11. P3.7 本地 Desktop/Web attach

### 11.1 服务端协议与认证

现有 loopback WebSocket 只适合开发调试。供 Desktop/Web attach 的本地 daemon 必须增加每次
启动随机的 connection token 或等价本机 IPC credential；token 只通过受控 launch/IPC 传给
客户端，绝不出现在 URL、日志、Store、prompt 或 RPC error 中。认证成功前仅允许
`client.hello`，并限制尝试次数与连接大小。

协议保持 JSON-RPC envelope 与 version/capability negotiation：

- client 请求声明 surface、协议版本与支持的 streaming/approval/browser/artifact capability；
- server 返回允许的 methods/features、Profile 公共摘要、connection/session policy；
- 不支持的 feature 在协商后隐藏或返回明确 `not_supported`，绝不返回空成功结果；
- TUI stdio 路径继续可用，且不因 Desktop/Web attach 改变其 toolset。

### 11.2 客户端交付边界

1. 先交付一个本地 Web reference client：连接、hello、session list/resume、submit、stream、
   interrupt、approval、reconnect/event replay 和 capability-gated browser/memory/cron 入口。
2. Desktop 壳层复用同一 Web client 与受控 daemon launcher；具体壳技术在实现前记录 ADR，
   但不得让 UI 用环境变量决定 daemon 的会话能力。
3. UI 只消费 RPC，不直连 SQLite、不加载 Profile `.env`、不启动 provider/MCP 子进程；
   daemon 仍是唯一 capability resolver 和 Store 写入方。
4. 以 mock provider、mock MCP、fake browser controller 启动真实 daemon，覆盖多 client
   attach、refresh/reconnect、orphan session reap、approval owner disconnect 和长 transcript。

### 11.3 验收

- 未携带本机 connection credential 的 loopback WebSocket 不能读取 session 或调用方法；
- Desktop/Web/TUI 同时连接时，只能由 session policy 允许的 client 响应 approval/controller
  request；
- Web refresh/reconnect 不重复 transcript 或重放已完成的 tool/cron/delegation side effect；
- UI 不可用时 core CLI/TUI/cron 仍可独立工作；
- 真实浏览器 smoke 是显式 opt-in，默认 CI 使用本地 headless fixture。

## 12. P3.8 交叉硬化、文档与发布门禁

### 12.1 版本化契约

扩展 `sagent-contracts`，新增但不依赖 Python 的 fixture：

```text
contracts/
  capabilities/       # resolution、reload、generation hash、capability gating
  providers/          # route/fallback/attempt outcome
  mcp/                # schema、name mapping、health、reload
  memory/             # visibility、prefetch、token budget
  scheduler/          # occurrence、misfire、claim/recovery
  delegation/         # budget、lineage、parent result
  clients/            # hello/auth/capability/replay
```

fixture 只断言 Sagent 的归一化输入/输出关系；不冻结模型目录长度、时间戳、随机 UUID、
provider endpoint、源代码文本或 Python 行为。

### 12.2 质量门禁

每个工作包单独提交，并至少运行：

```text
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo run -p sagent-contracts
git diff --check
```

涉及 MCP 子进程、cron lease、provider router、browser broker、Desktop/Web RPC 或文件/网络
I/O 的变更，必须有真实 import/真实 loopback/临时 `SAGENT_HOME` 的 E2E；单元 mock 不能
替代该路径。OS 特异进程行为仅在对应 Windows/macOS/Linux host 通过 marker 验证，不伪造
`sys.platform` 或等价运行时平台标志。

### 12.3 性能与安全基线

- 对 capability resolve、MCP cold/warm call、memory prefetch、scheduler recovery、
  delegation startup、provider fallback 追加离线 benchmark；
- 为每个新持久化表提供 migration fixture、损坏/锁定数据库 fixture 和恢复断言；
- 全量日志、错误、event、artifact、公开 config DTO 做 secret-redaction test；
- 对 path traversal、MCP command injection、schema collision、OAuth refresh、loopback token
  泄露、cross-profile query、orphan child/task 写攻击测试；
- 文档同步更新 `README.md`、配置参考、RPC protocol、故障排查和三平台 smoke 记录。

## 13. 推荐实施顺序与提交边界

| 顺序 | 工作包 | 依赖 | 完成后的可独立交付 |
| --- | --- | --- | --- |
| 1 | P3.0 能力解析与 generation resolution | Phase 0–2 | 动态能力不会破坏缓存/工具 schema |
| 2 | P3.1 Provider router | P3.0 | 受控的多 Provider 路由与安全 fallback |
| 3 | P3.2a MCP stdio | P3.0 | 可取消、可审计的外部工具 |
| 4 | P3.3 Skills + local memory | P3.0 | 可控长期上下文与完整 Skill 读取 |
| 5 | P3.4 Scheduler | P3.0、Runtime task supervisor | 持久化本地 cron/heartbeat |
| 6 | P3.5 Delegation | P3.0、P3.1 | 有预算、可取消的 child session |
| 7 | P3.6 Browser broker | P3.0 | 会话能力驱动的 controller 边界 |
| 8 | P3.2b MCP remote/OAuth | P3.2a、secret policy | 远程 MCP，不影响既有 session |
| 9 | P3.7 Desktop/Web attach | P3.6、protocol hardening | 认证本地多客户端连接 |
| 10 | P3.8 综合硬化与发布 | 全部 | 可回归、可诊断、可发布 |

禁止为了并行而让后续工作包绕过 P3.0。P3.1、P3.2a、P3.3 可以在 P3.0 contract 冻结后
并行设计，但只有一个变更负责修改 generation resolution 和 Store migration；其余分支必须
rebase 到该公共提交后再合并。

## 14. Phase 3 关闭标准与回滚

只有同时满足下列条件，Phase 3 才能标记完成：

- MCP、Skills/local memory、cron、delegation、provider routing 与 browser broker 均经
  capability resolver 在 generation 边界装配；
- config/MCP/Skill/memory/route reload 不会静默改变活跃 generation 的 system prompt、
  model 或 tool schema；
- 多 Provider fallback、MCP/tool、cron occurrence、child session 均具备取消、deadline、
  持久化 outcome 和跨进程恢复规则；
- Desktop/Web 通过认证本地连接与 capability negotiation 使用 Rust daemon，TUI/CLI 回归
  保持可用；
- 三平台 CI、原生 process/PTY smoke、contracts、migration/recovery E2E 全绿；
- 默认测试、默认运行时、配置和生产路径均不依赖 Python、真实用户数据或真实凭据。

回滚依赖加性 schema 和 feature-gated capability：禁用某项能力后，既有 session、turn、
memory、scheduled run、delegation relation 仍可只读诊断；新 daemon 必须把不支持的 feature
显式报为 unavailable，而不是删除历史或尝试重放副作用。Phase 4 才开始外部消息 Gateway，
Phase 5 才评估插件 SDK 与更广泛的第三方扩展生态。
