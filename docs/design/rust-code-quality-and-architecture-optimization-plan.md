# Sagent 代码可读性、职责边界与可替换性优化计划

状态：执行中（R0–R3.5 已结项；下一工作包为 R4–R8）
范围：独立 Rust 项目 `sagent` 的既有 Phase 0–2 实现  
前置：Phase 0–2 已完成；本计划是进入 Phase 3 前的结构治理，不交付 MCP、memory、cron、delegation、Desktop/Web 或新 Provider 功能。

## 1. 目的

Sagent 的 Phase 0–2 已具备 SQLite/FTS、Profile、单 Provider、SessionActor、受审批工具、
stdio/WebSocket RPC 与 TUI 闭环，并已通过三平台 CI。本计划不否定这些实现，也不以
Python 项目为兼容目标。

当前主要问题是读者在一条核心路径中同时需要理解状态转换、数据库事务、Provider、工具、
异步任务、配置和 RPC。随着 Phase 3 增加多 Provider、MCP、记忆和调度，这些边界会迅速
膨胀。本计划的目标是：

1. 让每个 crate、模块、对象和函数有单一、可一句话描述的职责；
2. 将 SQLite、OpenAI-compatible、stdio/WebSocket 等具体实现留在基础设施边界之后；
3. 让新实现可由配置描述、selector 创建为运行时 Manager，再由领域接口消费，
   而非在 Runtime 中增加后端分支；
4. 保持现有协议、事务、事件顺序、审批和取消行为不变；
5. 缩短首次阅读核心代码所需的上下文，并让测试直接表达行为契约。

## 2. 范围与非目标

### 2.1 本计划包含

- 补齐模块职责说明、公共 API 中文 Rustdoc、语义化常量和领域错误映射；
- 按职责整理 Runtime、Store、配置、Provider、工具、Protocol、RPC 与测试；
- 引入持久化、Provider、工具、能力解析和 transport 的最小可替换边界；
- 以 SQLite 和 OpenAI-compatible 作为第一实现，建立可验证的第二实现接入路径；
- 保留或补充 contract、集成、真实进程和三平台测试。

### 2.2 本计划明确不包含

- 将所有模块预先 trait 化，或建立没有第二调用方的插件框架；
- 用 Python/Hermes 代码、数据库或配置作为兼容目标；
- 在本轮直接实现 PostgreSQL、远程数据库、第二个云 Provider、MCP server 或 Scheduler；
- 改变 RPC 方法、SQLite 数据语义、工具权限、审批规则或默认行为；
- 以降低行数为目的拆散事务、Actor 串行状态机或进程清理边界。

远程数据库、第二 Provider、MCP 等是本计划用来验证边界的目标，不是本计划默认交付物。
若要实现其中任一项，必须另立功能工作包并使用本计划产出的接口。

## 3. 当前基线与问题清单

### 3.1 代码体量

当前约有 138 个 Rust 文件、32,055 行 Rust 代码。大部分文件和函数仍在可维护范围，但下列
位置已达到结构审查阈值：

| 位置 | 现状 | 主要问题 |
|---|---:|---|
| `sagent-runtime/src/actor/worker.rs::handle_worker_event` | 约 168 行 | 流事件、状态机、持久化、工具回环与事件发布混杂 |
| `sagent-tools/src/terminal.rs::execute_with_approval` | 约 160 行 | 校验、审批、spawn、取消、输出收集和清理混杂 |
| `sagent-tui/src/app/reducer.rs::reduce` | 约 155 行 | 多个 UI 领域 action 聚集在一个 reducer |
| `sagent-runtime/src/actor.rs::submit_prompt` | 约 117 行 | prompt、generation、Store、worker 和事件一次完成 |
| `sagent-store/src/sqlite/session/turns.rs` | 约 623 行 | 一条 turn 生命周期的多个事务集中 |
| `sagent-rpc/src/transport/stdio.rs` | 约 806 行（生产约 650 行） | framing、连接任务、dispatch、event bridge 与测试混合 |
| `sagent-runtime/tests/tool_actor.rs` | 约 994 行 | 多个工具/审批/取消场景共用重复搭建 |

行数不是缺陷本身。这里的判断依据是一个文件或函数是否混合了不随同一类变化而变化的职责。

### 3.2 边界问题

| 领域 | 当前现实 | 需要建立的边界 |
|---|---|---|
| Storage | `Store` 持有 `rusqlite::Connection`，Runtime/CLI/工具直接依赖具体 Store | StorageManager（由共享 selector 创建）+ 小型领域存储接口 |
| Provider | config 解析时创建 `OpenAiCompatibleProvider`；Supervisor 保存全局 Provider | ProviderDescriptor + ProviderFactory + CapabilityResolver |
| Tools | schema registry 与 `ToolWorker` 的硬编码执行分支分离 | ToolCatalog + ToolExecutor + ToolExecutionContext |
| Runtime | Supervisor 同时管理 actor 生命周期、provider、模型、工具、超时 | SessionSupervisor + SessionSupervisorDependencies + GenerationResolution |
| Protocol | protocol crate 中含 Store 驱动的 `SessionService` | 纯协议 DTO/trait 与 RPC/Runtime adapter 分离 |
| Transport | stdio/WebSocket 仍各自维护连接任务和关闭逻辑 | ConnectionRuntime + 各自 framing adapter |
| Config | 路径、YAML、密钥读取、Provider 创建部分混合 | Profile/Storage/Provider/Workspace/Policy descriptors |

### 3.3 质量基线

- 已有约 359 个 Rust 测试，含 contract、集成、TUI 黑盒、真实 PTY 与三平台 CI；
- 当前公共声明 Rustdoc 覆盖率约 90%，仍有约 56 个公开声明需要补齐中文文档；
- CI 已执行格式化、工作区测试、Clippy、contract runner、依赖审计和三平台原生终端验证；
- 曾出现一次 macOS 原生终端监督失败，之后最新三平台 CI 已全绿。重构前需增加失败诊断，
  不能只将它标记为 flaky。

## 4. 目标架构

```text
config.yaml / .env
        │
        ▼
Config Resolver ──► ProfileDescriptor / StorageDescriptor / ProviderDescriptor / PolicyDescriptor
        │
        ▼
Bootstrap / Factories
        │
        ├──► StorageManager ──► SessionStorage / QueryStorage / SearchStorage
        │       （由共享 selector 创建；SQLite/PG 只在 manager 内选择连接策略）
        ├──► ProviderFactory ─► ModelProvider
        └──► ToolCatalog + ToolExecutorRegistry
                                      │
                                      ▼
CapabilityResolver ──► immutable GenerationResolution（每个 Turn）
                                      │
                                      ▼
SessionSupervisor ──► SessionActor ──► Provider / ToolExecutor / Storage
                                      │
                                      ▼
Protocol DTO/traits ◄── RPC service adapters ◄── ConnectionRuntime ◄── stdio/WebSocket framing
                                      │
                                      ▼
TUI / future Desktop/Web client
```

### 4.1 不变量

1. 同一 Session 在一个运行时内仍只有一个 Actor 写入其活动状态；
2. `start_turn`、工具结果提交、完成和中断仍是高层原子操作；
3. 每个 Turn 开始时固定 Provider、模型、工具 schema、system prompt 和策略 revision；
4. 配置变化只影响之后新建的 resolution，不能改变正在运行的 Turn；
5. 工具 worker、Provider worker 和 transport task 都不能绕过 Actor 写 Store；
6. Protocol 公开错误不得泄漏 SQLite、HTTP、文件路径、endpoint 或凭据；
7. TUI/客户端不得直接访问 Storage、Provider 或本地文件系统。

## 5. 执行工作包

每个工作包先写或确认行为契约，再做无行为重构；不要将多个工作包合并成一个超大 PR。

### R0：建立重构护栏与可读性基线

状态：已完成（2026-09-13）。基线记录见
[重构护栏与可读性基线](rust-refactor-baseline.md)；CI 已加入 workspace rustdoc 构建，
原生 terminal 测试会输出不含命令/环境变量的进程监督诊断。

**目的：** 让后续拆分能证明不改变可观察行为。

**工作：**

1. 记录当前 crate 依赖图、生产/测试文件行数、超过 100 行函数和公开 Rustdoc 缺口；
2. 为下列路径补齐或确认 contract：提交回合、工具调用/审批、interrupt、重启恢复、
   `events.since`、stdio/WebSocket 事件顺序、Profile 隔离；
3. 为原生 terminal 监督测试加入失败诊断：平台、命令、子进程 PID/组、取消原因、耗时与
   清理结果；禁止记录命令内容之外的秘密或用户文件内容；
4. 在 CI 加入 `cargo doc --workspace --no-deps`；存量 Rustdoc 修复完成前不全局启用
   `deny(missing_docs)`；
5. 定义架构指标仅作 review 信号：生产文件 >500/>700 行、测试文件 >600 行、函数
   >100/>120 行、crate 反向依赖和未文档化 public item 数。

**验收：** 基线数据可复现；现有 contract 与三平台 CI 均通过；失败诊断不泄密。

### R1：模块地图、公开文档与错误语义

状态：已完成（2026-09-13）。已补齐当前识别出的公开 DTO、Store event/turn、terminal/
process、Provider 配置与 Runtime tool-call 边界 Rustdoc；并为 Actor、Store event/turn、
terminal 与 stdio transport 补充“负责/不负责”的模块地图。既有 `RuntimeError`、
`ProviderError`、`ProtocolError`、`WorkspaceError` 与工具错误保持稳定公开类别，未改变
任何 RPC 或持久化错误语义。

**目的：** 不改变任何依赖结构，先降低阅读门槛。

**工作：**

1. 在每个 crate `lib.rs` 添加或修订模块级说明：职责、不负责事项、主要入口和相邻边界；
2. 为 `sagent-types` 的存储 DTO、`sagent-store` 的 event/turn 类型、`sagent-agent` 的
   transcript/prompt API、`sagent-tools` 的 terminal/process API 补齐中文 `///`；
3. 为超过 300 行的生产模块补充“职责、关键不变量、协作对象”模块注释；
4. 将跨 crate 错误收敛成 `StorageError`、`ProviderError`、`ToolError`、`RuntimeError`、
   `ProtocolError` 等稳定类别；内部仍保留上下文链；
5. 收集并命名 mailbox 容量、事件容量、审批 timeout、最大工具轮次、revision 等策略值，
   不在多处复制数字和字符串。

**不做：** 不重命名公开 RPC 字段；不因为文档任务大面积移动实现。

**验收：** 新改动无未文档化 public item；错误公开映射有单元测试；读每个 crate 根可在
不打开所有实现文件的前提下理解入口与边界。

### R2：Runtime 流程与对象职责整理

状态：已完成（2026-09-13）。

**完成记录：** `SessionSupervisor` 现在主要负责 SessionActor 的生命周期、mailbox、事件
订阅和收口，并作为冻结存储依赖向 RPC transport 提供连接级查询端口；`SessionSupervisorDependencies` 聚合存储、模型、工具和策略依赖，并通过枚举消除
Provider/worker/tool 的非法 `Option` 组合。`submit_prompt` 已按校验、计划、持久化、worker
启动、active state 安装和事件发布拆分；`handle_worker_event` 已按流式增量、用量、工具调用、
工具结果和终态/失败拆分。Store 提交后发布事件、取消先传播 token 再收口任务、Actor 单写
以及迟到事件丢弃等不变量均保留，Actor/Provider/tool/fault-matrix/recovery 测试和工作区
格式、Clippy、Rustdoc 检查全部通过。

本阶段的 resolution 使用启动时冻结的模型/工具运行模式和 generation 校验；更完整的
`CapabilityResolver`、`GenerationResolution` 显式解析对象按计划移交 R4，避免在 R2 与能力
解析重构重复建模。

**目的：** 使 Runtime 的主路径可按业务步骤阅读，同时保持 Actor 单写和取消语义。

**工作：**

1. 将 `SessionSupervisor` 限定为 SessionActor 生命周期、mailbox、事件订阅、收口，以及
   通过冻结依赖向 transport 提供连接级查询端口；
2. 提取 `SessionSupervisorDependencies`，集中 StorageManager、Clock、CapabilityResolver 等完整依赖；
3. 以 enum 或已验证的运行模式替代 `provider`、`worker_factory`、`tool_dispatcher`、
   `tool_worker` 的松散 `Option` 组合；
4. 将 `submit_prompt` 拆为：输入/会话校验、resolution 获取、Turn 计划准备、原子持久化、
   worker 启动、active state 安装与事件发布；
5. 将 `handle_worker_event` 拆为：流式增量、用量、工具调用、工具结果、终态/失败；
6. 在每个拆分点明确“Store 成功提交后才发布事件”“取消先传播 token 后收口任务”的不变量；
7. 保留 Actor 为唯一状态推进者，worker 只能投递事实，不能直接写 Store 或发布最终事件。

**验收：** 现有 Actor/Provider/tool/fault-matrix/recovery tests 不变或增强；重复终态、
迟到工具结果、interrupt 竞争、worker panic 和恢复路径仍有契约覆盖；核心编排函数不超过
100 行，必要例外写明原子性理由。

### R3：持久化端口与 SQLite 实现隔离

状态：已完成（R3.1–R3.5）

当前进度：R3.1 `StorageDescriptor`、R3.2 `provider.rs` 配置职责拆分、SQLite 默认路径
降级、R3.3 最小领域存储端口与首个 SQLite adapter 已完成；R3.4 Runtime/RPC/工具和
CLI 迁移已完成；R3.5 `StorageManager` 框架、调用方迁移、Factory 过渡路径清理和综合
验收已完成。R3.5 的分阶段执行拆分见
[StorageManager 重构执行计划](archive/rust-storage-manager-execution-plan.md)。
Runtime Actor、SessionSupervisorDependencies、RPC SessionService、RuntimeService、`session_search`
以及 CLI 会话/Profile 管理命令均已通过工厂申请领域端口，不再直接依赖 `Store`。

**R3.2 完成记录：** 配置读取、Profile 聚合快照、Provider 数据模型、Provider resolver、
凭据读取、workspace 解析和公开配置摘要分别位于独立 sibling；`lib.rs` 直接公开这些稳定
API，不再保留只做转发的 `provider.rs`。`config_reader` 与 `ProfileConfig` 只负责 YAML
读取、反序列化和不可变意图组合，不创建数据库、HTTP client 或后台任务。当前
`resolve_openai_provider_from_config` 只接受已加载快照，仍暂时在 config crate 中实例化
OpenAI-compatible client；后续 R4 的 ProviderFactory 将把该副作用移到 bootstrap。
Profile 目录索引的 `Profile`/`ProfileInfo` 也位于 `sagent-config`，只保存按名称索引的
快照和 active-profile 状态操作；CLI 的命令解析、输出及存储初始化不下沉到配置层。
`ProfileConfig::get_storage_descriptor` 以及各主题的 `*_from_config` API 提供无 I/O 的已
加载配置提取路径，供 bootstrap 复用同一份配置快照；`load_profile_config` 是当前唯一的
`config.yaml` 文件读取入口。

**SQLite 默认路径降级记录：** `SagentPaths` 不再携带数据库字段；`storage` 模块仅在
`StorageKind::Sqlite` 且未配置路径时使用 `state.db` 默认文件名。调用方先从同一
`ProfileConfig::get_storage_descriptor` 提取 `StorageDescriptor`，再由
`StorageDescriptor::resolve_sqlite_database_path`
解析自定义 SQLite 路径，Runtime、CLI 和会话搜索均使用解析后的路径。远程、schema/namespace
和只读策略在 StorageManager 接入前继续 fail-closed，不会静默
回退到默认文件。

**R3.3 初始定义记录：** `sagent-store::ports` 按职责拆分为 `SessionStorage`、
`SessionQueryStorage` 和 `SearchStorage`，并以业务 `Storage` 外观聚合领域能力。Manager
负责按已冻结的 descriptor 创建独立存储对象，保证每个 Actor 的写入端口不共享。端口只依赖业务 DTO 和强类型 ID，不暴露 SQLite 路径、连接、连接池或 `Store`；Turn 开始、
工具结果提交、完成、中断和失败等方法保持高层原子操作。当前接口先保留同步调用模型以
保持 Actor 单写和事务期间无 `await`，R3 后续技术 spike 再决定远程后端的异步适配方式。

**SQLite adapter 初始实现记录：** `SqliteStorageManager` 绑定已解析的绝对路径；每次
申请都重新打开一个 Store，并以受保护的共享句柄包装为业务端口，保证同一 Actor 内写入、
查询和搜索看到一致连接状态，同时不在 Actor 或连接之间共享 Store。路径和具体 Store
只存在于 Manager/adapter 内部；远程后端尚未注册时由 selector 明确拒绝。

**R3.5 `StorageManager` 设计记录（新增）：** 后端选择只发生在 selector，Profile 作用域的
`StorageManager` 由 bootstrap 创建一次，适合作为 `RuntimeService`、`ToolWorker` 和
其它业务对象的长期依赖。bootstrap 按
`StorageDescriptor` 创建一次并持有后端资源：SQLite manager 可以持有数据库路径或连接
池，PostgreSQL manager 可以持有 `PgPool`。manager 负责连接/池的生命周期、健康检查、
migration 和依赖申请，不负责具体 session/turn 业务规则。

manager 只提供按职责拆分的申请入口，不变成包含所有 CRUD 的万能 `Storage` 对象：

- `open_actor_storage()`：为一个 `SessionActor` 返回独占写入端口和配套查询端口；
- `open_read_storage()`：为 RPC 查询、事件补读和搜索返回只读端口集合；
- `open_write_storage()`：为 `session.create` 等短操作返回写入端口；
- 搜索通过 `ReadStorage.session.search_messages(...)` 提供，避免工具获得不需要的写入能力。

完整 `Storage` 不能简单全局复用：其中 `SessionStorage` 使用 `&mut self`，并且 Actor
是会话状态的唯一写入者。因此 manager 可以被 `Arc` 共享，但每个 Actor 仍须申请独立的
业务存储；只读和搜索端口则可根据后端能力使用连接池中的轻量句柄。SQLite
实现可在 manager 内部继续按次打开 Store，PG 实现则可从池中申请连接或事务，Runtime、
RPC、CLI 和工具均不感知这种差异。

迁移完成后，后端构造器不再作为独立 Factory 暴露；`RuntimeBootstrap`、`SessionSupervisor`、
`RuntimeService` 和 `ToolWorker` 不再直接传递数据库路径、连接或连接池，而是持有 manager
或更窄的领域能力。manager
方法如果需要异步获取连接，应在 R3 的同步/异步技术 spike 中统一决定，不能由各调用方
自行 `spawn_blocking` 或复制连接生命周期。

**R3.5 框架实现记录（本次）：** `sagent-store::ports` 新增 `StorageManager` 管理端口，
统一提供 `open_actor_storage`、`open_read_storage`、`open_write_storage` 申请入口；
`WriteStorage` 将短操作的写入能力与查询、搜索能力隔离。新增 `SqliteStorageManager` 作为首个实现，持有经过校验的绝对数据库路径，
并由 selector 直接创建 `SqliteStorageManager`。当前 manager
仍按次打开 SQLite Store，以保持既有事务和连接行为；Runtime、RPC、CLI 和工具的持有对象
已迁移到 Manager，连接池和远程后端的资源生命周期留待后续工作包。

**R3.4 Runtime/RPC/工具/CLI 迁移记录（已完成）：** SessionActor 的写入、查询和恢复路径已改为分别依赖
`SessionStorage`/`SessionQueryStorage`；协议层 `SessionService` 只持有查询端口。CLI 的 Profile
创建、会话创建、列表、详情、搜索及生命周期管理命令通过 `StorageManager` 申请
`WriteStorage`/`ReadStorage`，Runtime、RPC 和工具的生产路径也已切换到 Manager 或窄领域端口，
不再直接导入 `Store`、`rusqlite` 或数据库连接。`RuntimeBootstrap` 只保存 Profile 级 Manager，
`RuntimeService` 和 `RuntimePromptContext` 通过 Manager 获取短生命周期端口，`session_search`
只接收拆出的 `SearchStorage`。CLI 的 Profile 索引由配置层 `Profile` 提供，目录和存储初始化由
独立的 `ProfileService` 承担；CLI 通过 `HandlerFactory` 为每条顶层命令创建领域 handler，
`SessionHandler` 由 `SessionService` 持有 `CommandContext` 并复用惰性 `CliStorageContext`。
Factory 构造方法和旧端口聚合已在 R3.5 M7 删除；SQLite adapter 仅保留从
`SqliteDatabase` 到业务 `Storage`/`ReadStorage` 的装配边界，生产装配不传播该类型。

**目的：** 使所有业务持久化经由统一边界，并为本地/远程后端配置切换建立真实路径。

**设计决定：** `Storage` 是依赖聚合或入口名，不是包含所有业务读写方法的万能对象。接口按
事务和查询领域拆分，DTO 继续属于 `sagent-types`。

**Bootstrap 边界（新增决定）：** `RuntimeBootstrap::from_paths` 是装配入口，不是数据库适配器。
它可以读取固定路径并加载一次 `ProfileConfig`，然后把 `StorageDescriptor` 交给 selector
创建 `StorageManager`；manager 再按调用边界提供 `Storage`/`ReadStorage`/`WriteStorage`。
Bootstrap 的签名、
字段和返回对象不得暴露 SQLite/PG 的文件路径、连接、连接池、`rusqlite::Connection`、
`Store` 或远程连接字符串等后端细节。Runtime、RPC、Actor 和工具只接收按领域拆分的存储
端口或 manager 能力，后端分支、连接生命周期和事务实现均封装在 manager/adapter 内。
切换 `storage.kind` 时只应更换 manager 的实现或配置，不应修改上层编排；尚未支持的远程
后端必须在 selector/manager 层明确失败，不能进入 SQLite 专用路径或静默回退到默认数据库。

**工作：**

1. 定义 `StorageDescriptor`：至少支持 `sqlite` 与未来远程后端所需的 kind、连接引用、
   schema/namespace、只读策略；秘密只能引用环境变量或秘密提供者；
2. 将原 `sagent-config/src/provider.rs` 的混合职责拆分，保持行为不变并保留中文 Rustdoc：
   `config_reader.rs` 负责 YAML 反序列化，`provider_resolver.rs` 负责 Provider 解析与实例化，
   `credentials.rs` 负责 `.env`/环境变量读取，`workspace.rs` 负责 workspace 路径解析，
   `storage.rs` 负责 `StorageDescriptor`，公开配置摘要只保留在独立的 `public_config.rs`；
   配置读取和 descriptor 组合不得打开数据库、创建 Provider/HTTP 客户端或启动后台任务；
   现有 Provider 实例化兼容入口暂由 resolver 保留，后续 R4 迁移到 `ProviderFactory`；
3. 移除 `SagentPaths` 上的数据库字段；仅当选择 SQLite 且未指定路径时使用 storage 模块的
   `state.db` 默认文件名。实际路径由 `StorageDescriptor::resolve_sqlite_database_path` 根据
   已解析的 descriptor 计算，
   Runtime/CLI 不得绕过该解析；
4. 设计 `Storage`、`SessionStorage`、`SessionQueryStorage`、`SearchStorage` 的最小领域 API。
   selector 根据 `StorageDescriptor` 创建 Manager，
   `RuntimeBootstrap::from_paths` 只负责传入已加载的 Profile 快照并接收该聚合，不保存或
   转发任何后端连接/路径。`start_turn`、`commit_tool_result`、`complete_turn`、
   `interrupt_turn` 等必须保持单个高层原子操作；
5. 引入 Profile 作用域的 `StorageManager`。定义 `open_actor_storage`、`open_read_storage`、`open_write_storage`
   和可选 `open_search_storage` 等窄入口；Runtime、RPC、CLI、工具只持有 manager 或已申请
   的领域端口，不再传递原始 Factory。验收必须证明每个 Actor 的写入端口仍独占，RPC/工具
   不获得超出职责的写入能力，且 SQLite 与未来 PG 的资源模型可以在 manager 内替换；
6. 先做技术 spike 决定 manager/端口的同步/异步模型：远程后端需要 async；SQLite 实现必须在不破坏
   Actor 单写和事务期间无 await 的前提下适配。spike 记录线程安全、连接生命周期、取消和
   transaction boundary 的选择；
7. 将现有 `Store` SQLite 逻辑迁为第一实现。可保留 `sagent-store` 作为端口 crate 并新增
   `sagent-store-sqlite`，或将端口与实现置于清晰子模块；选择以依赖图最小、无循环依赖为准；
8. **已完成：** Runtime Actor、SessionSupervisorDependencies、RPC session read service、事件补读、
    空会话创建、`session_search` 和 CLI 管理命令均已迁移到领域端口，并禁止新的上层代码
    直接导入 SQLite 类型；只读路径通过 `StorageManager::open_read_storage` 获取查询与
    搜索端口，可写命令通过 `StorageManager::open_write_storage` 获取会话写入端口；
9. 以第二个测试实现验证边界：内存/recording storage 或独立 fake；它必须验证事务调用的
   原子语义，而不是模拟 SQL 细节；
10. 单独制定远程后端功能计划，涵盖 migration、全文搜索能力差异、连接池、重试、并发写、
   session lease/乐观版本和数据导入；本工作包不承诺实现该后端。

**验收：** Runtime、CLI、RPC 和工具不再直接依赖 `rusqlite` 或 `Store` 具体实现；
`RuntimeBootstrap::from_paths`、`SessionSupervisorDependencies` 和 RuntimeService 不暴露数据库路径、
连接、连接池或具体 Store；切换 SQLite/远程后端只需替换 manager/adapter，未支持后端能明确
失败且不会回退到 SQLite。SQLite 行为契约不变；将 fake/recording storage 注入 actor 可覆盖
start/commit/complete/interrupt；`storage.kind = sqlite` 保持当前默认行为；配置解析、Provider
实例化、凭据读取、workspace 解析和公开配置摘要均能从独立模块按职责定位，且配置解析不产生
基础设施副作用。

**R3.5 专项验收：** 单个 Profile 只创建一个 `StorageManager`；Actor、RPC 查询、写入
和搜索均通过 manager 申请与其职责匹配的依赖。代码中除 selector、manager 构造和 adapter
外，不再出现后端构造器的长期持有或传播；SQLite 使用本地 Store、PG 使用连接池
时，上层调用路径和领域端口签名保持不变。

**R3.5 结项记录（2026-09-15）：**

- [x] `StorageManager` 管理端口提供按职责拆分的 actor/read/write 申请入口；
- [x] Runtime、RPC、CLI 和工具只持有 manager 或窄领域端口，不再传播原始 Factory；
- [x] `SessionStorage`、`SessionQueryStorage`、`SearchStorage` 的职责边界保持独立，
  Actor 写入端口不会被 RPC/工具复用；
- [x] SQLite 路径、Store/连接生命周期和后端 selector 均封装在 manager/adapter 内，
  未支持的远程后端 fail-closed，不回退到 SQLite；
- [x] StorageManager 分阶段执行记录已归档至
  [`rust-storage-manager-execution-plan.md`](archive/rust-storage-manager-execution-plan.md)。

**R3.5 质量门禁（2026-09-15，Windows）：**

- [x] `cargo fmt --all -- --check`；
- [x] `cargo test --workspace --offline`；
- [x] `cargo clippy --workspace --all-targets --offline -- -D warnings`；
- [x] `git diff --check`；
- [x] 本地 Markdown 相对链接检查。

R3.5 已完成并关闭。后续如增加 PostgreSQL 或其他持久化后端，只能在既有
`StorageManager`/adapter 边界内扩展，不重新向 Runtime、RPC、CLI 或工具暴露数据库细节。

### R4：配置、Provider 与每回合能力快照

**目的：** 配置只描述意图，bootstrap 创建实现；能力选择从全局 Supervisor 移到每个 Turn。

R4 详细执行步骤见[《配置、Provider 与每回合能力快照》](rust-r4-provider-capability-plan.md)。

当前进度：R4.0 的行为契约与 fixture 基线已完成；R4.1 已完成 raw/canonical descriptor
分离、alias 归一化、provider identity 类型化和纯 descriptor resolver；尚未进入
ProviderFactory 和 Runtime 生产路径迁移。

**工作：**

1. 将当前 Provider YAML 解析分为 `ProviderDescriptor`、模型选择、凭据引用和公开摘要；
2. 移除 config crate 对 `OpenAiCompatibleProvider` 的直接依赖，将实例创建移到
   `ProviderFactory`；
3. 定义 `CapabilityResolver` 输入：Profile descriptor、Session identity、请求选项和 policy；
   输出 `GenerationResolution`；
4. `GenerationResolution` 至少包含 provider/model identity、不可变 provider handle 或 factory
   结果、tool schema hash/revision、prompt revision、policy revision、workspace/approval policy；
5. 在 `submit_prompt` 之前一次解析并持久化可审计 revision；worker 只使用该 snapshot；
6. 将当前 Supervisor 的 `with_provider`、model、profile revision、工具 dispatcher/worker 等
   builder 迁为完整的 `SessionSupervisorDependencies` 与 resolver；测试注入走同一边界；
7. 明确 Python 风格配置别名的策略：若不再是正式用户兼容承诺，在一次配置版本迁移中标记
   deprecated 并移除；若保留，则文档化为 Sagent 自身支持字段并写解析契约。

**验收：** config crate 不创建 HTTP client/Provider；配置读取不会访问网络；同一运行时可用
不同 resolver 结果启动不同 Session/Turn；运行中的 Turn 不受后续配置变更影响；密钥不进入
`config.read`、日志、Debug 或 RPC。

### R5：工具目录、执行器与终端编排整理

**目的：** 让“可被模型调用的工具”与“真实执行能力”使用同一明确边界，并降低安全代码阅读负担。

**工作：**

1. 定义 `ToolCatalog`/`ToolDefinition` 的责任：schema、版本、风险、availability；
2. 定义 `ToolExecutor` 的责任：执行已验证的 ToolCall 并返回 ToolResult；
3. 定义显式 `ToolExecutionContext`：session/turn/tool call identity、workspace、approval 结果、
   cancellation、输出限制与审计 sink；禁止从全局环境或静态变量隐式读取；
4. 将 `ToolWorker` 从工具名 `match` 分支改为 executor lookup；内建 read/write/terminal/search
   都以同一接口注册；
5. 保持审批决策在 Actor/Policy 边界，具体工具仍负责本地安全校验；
6. 将 `execute_with_approval` 拆为输入校验、授权解析、进程创建/注册、等待/取消、结果归一化；
   进程树清理必须保持一个明确资源所有者；
7. 为 catalog-definition-executor 一致性写行为测试：每个可见工具均可验证和执行；不可用
   工具 fail-closed；工具 schema revision 与实际 executor 一致。

**验收：** 新增内建工具不需要修改 Runtime 核心 `match`；工具取消、审批、workspace escape、
timeout、迟到结果和审计事件契约不变；终端原生三平台测试全绿且失败诊断可用。

### R6：Protocol、RPC 服务和传输生命周期收口

**目的：** 让客户端依赖稳定协议，而不同 transport 只负责帧和连接。

**工作：**

1. 审核 `sagent-protocol` 导出项；保留 DTO、方法名、错误 DTO、分页规则和 service trait；
2. 将 Store 驱动的 `SessionService` 实现迁到 RPC/service adapter 或独立 service crate；
3. 定义 transport-neutral `ConnectionRuntime`：请求入站、dispatch、响应/事件出站、慢消费者、
   cancellation、任务收口与错误映射；
4. 将 stdio/WebSocket 保留为 framing adapter：NDJSON 和 WebSocket text frame 的编解码、EOF/
   close 信号转换；不得复制业务 dispatch；
5. 固化 response 与 event 的顺序契约、连接隔离、未经认证的监听边界和 stdout 只输出 NDJSON；
6. 为未来 Desktop/Web 定义本地 attach 接口，但不在本工作包创建 UI 或网络监听器。

**验收：** protocol crate 不依赖 SQLite；相同 contract 可在 stdio/WebSocket 运行；连接关闭会
取消并 join 所有受监管任务；TUI 仍只依赖 protocol/types，不依赖 Runtime/Storage。

### R7：TUI、CLI 与测试可读性整理

**目的：** 让入口层保持薄，让测试正文直接表达行为。

**工作：**

1. 将 TUI reducer 按 connection/session、composer、turn stream、approval/tool 分为私有 handler；
   保留唯一纯 `reduce` 入口；
2. CLI 命令只解析参数、调用 service、映射展示错误；Store/Provider 创建必须统一经 bootstrap；
3. 创建小型 `RuntimeHarness`、`RpcHarness`、`ToolHarness`，每个只拥有一种 fixture 职责；
4. 将 `tool_actor.rs`、`stdio_integration.rs` 等长测试按场景拆分；共享 fixture 不得隐藏默认
   权限、时间或 Provider 响应；
5. 所有测试命名为可观察契约，例如 `interrupt_persists_once_after_tool_cancellation`；
6. 平台行为继续使用真实平台 marker/CI，不伪造 OS；测试 fixture 永远使用临时 home、SQLite、
   workspace 和 loopback server。

**验收：** TUI 不新增 Runtime/Store 依赖；最长测试文件不超过 600 行或附有合理例外；测试
正文不超过少量 setup 即能看出输入、动作和断言的状态契约。

### R8：集成切换、删除旧路径与架构验收

**目的：** 防止新旧两套路径长期并存。

**工作：**

1. 从 bootstrap 开始切换到 descriptor → factory → domain port；
2. 确认文件读取式 Provider 入口已删除，并继续移除具体 Store factory、工具硬编码分支和重复连接
   lifecycle 代码；不保留仅供内部使用的 re-export shim；
3. 更新 Phase 3 计划、架构文档、配置示例、contract fixture 和 crate 文档；
4. 在干净环境运行全量格式化、测试、Clippy、contract runner、三平台 CI 与 native terminal
   检查；
5. 复测基线：依赖图无反向依赖、没有上层直连 SQLite、没有 config 创建 client、没有 protocol
   依赖 Store、没有未受监管的 `tokio::spawn`。

**验收：** 第 4 节不变量全部满足；R0 指标未恶化；旧实现入口已删除；下一项 Phase 3 功能
可以只通过 resolver、storage port、tool executor 或 connection runtime 的扩展完成。

## 6. 推荐顺序与依赖

```text
R0 ─► R1 ─► R2 ──────────────┐
            │                │
            ├──► R3 ─► R4 ───┼──► R8
            │                │
            ├──► R5 ─────────┤
            │                │
            └──► R6 ─► R7 ───┘
```

建议执行顺序：R0、R1、R2、R3、R4、R5、R6、R7、R8。

- R2 先减少 Runtime 复杂度，避免在状态机仍混杂时引入 Storage/Resolver；
- R3 在 R4 之前完成，因为能力 resolution 必须能引用统一 storage descriptor；
- R4 在 Phase 3 多 Provider 前完成；
- R5 在 MCP 接入前完成；
- R6 在 Desktop/Web attach 前完成；
- R7 可以与 R5/R6 的纯测试整理并行，但不与同一文件的行为改动混合。

## 7. 提交、测试与回滚策略

### 7.1 单个工作包的提交边界

每个工作包至少分为：

1. 行为契约/测试或文档基线；
2. 无行为实现重构；
3. 删除旧路径和文档更新。

接口迁移采用短暂双实现时，只允许 factory/adaptor 层双路；Runtime 业务逻辑、协议和工具
不得长期同时维护旧新两套分支。切换完成后立即删除旧路径。

### 7.2 必跑验证

每次生产代码变更均运行：

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

影响 Store、Protocol、Tool schema、Provider 或 RPC 的改动还必须运行：

```powershell
cargo run -p sagent-contracts
```

影响终端、PTY、子进程、WebSocket 或 TUI 的改动必须触发三平台 CI；不得用当前 Windows
环境模拟 macOS/Linux 结果。

### 7.3 回滚准则

出现以下任一情况时，停止当前工作包并先回退到最近可验证边界：

- 事务原子性、事件顺序、取消或恢复 contract 改变；
- 新 abstraction 迫使业务代码出现更多 `match backend`/`if kind` 分支；
- 新接口无法用 SQLite 与 fake/recording implementation 同时实现；
- 为保留旧 API 而在内部增加 re-export、全局状态或隐式 fallback；
- 测试只能依靠 mock 通过，无法覆盖真实 Store、真实 transport 或真实进程路径。

## 8. 完成标准

本计划完成时，必须同时满足：

- 所有新增和修改的生产代码遵守 `AGENTS.md` 中的单一职责、注释、规模、异步任务和可替换性约束；
- 核心 Runtime、Store、工具和 RPC 的主要流程能按模块文档和具名步骤阅读；
- 所有持久化经由领域 storage port，SQLite 仅是其中一个实现；
- 配置只输出 descriptors，Provider/Storage 由 factory/bootstrap 创建；
- 每个 Turn 有不可变 `GenerationResolution`，Supervisor 不再承担全局能力配置；
- 所有可见工具都有定义与执行器，执行上下文显式传递；
- protocol crate 不依赖 SQLite，stdio/WebSocket 共用连接生命周期；
- 测试 fixture 按领域拆分，核心 contract、三平台 CI 与原生 terminal 测试全绿；
- 没有为了抽象或降低行数而损失事务、取消、权限、事件顺序或可观察行为。

完成后再开始 [Phase 3：能力扩展与本地多客户端接入](rust-phase-3-plan.md) 的功能工作。
