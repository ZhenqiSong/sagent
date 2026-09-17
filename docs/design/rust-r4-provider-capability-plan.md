# Sagent 代码质量优化 R4 详细计划：配置、Provider 与每回合能力快照

作者：SongZQ  
日期：2026-09-17  
状态：R4.0 已完成；R4.1 执行中；R4.2–R4.8 待执行
所属总计划：[代码可读性、职责边界与可替换性优化计划](rust-code-quality-and-architecture-optimization-plan.md)  
前置条件：R0–R3.5 已结项；StorageManager 已成为唯一持久化装配边界。  
关联阶段：[Phase 3：能力扩展与本地多客户端接入](rust-phase-3-plan.md)

## 1. 计划目标

R4 的目标不是新增 Provider 数量，也不是提前实现 Phase 3 的 MCP、memory 或多 Provider
fallback，而是先把“配置意图 → Provider 实例 → 每回合能力快照”的边界固定下来。完成后，
后续能力可以在 generation 边界接入，不需要继续扩张 RuntimeBootstrap、
SessionSupervisorDependencies 或 SessionActor 的职责。

R4 必须同时解决以下问题：

1. 配置 crate 只负责读取、校验和组合不可变 descriptor，不创建 HTTP client、Provider 或
   后台任务；
2. Provider 实例只由组合根/Factory 创建，API key 只在创建时读取，不能进入配置快照、日志、
   Debug、RPC 或持久化记录；
3. 一个 Turn 开始前只解析一次 GenerationResolution，worker、工具回环和恢复路径只使用
   该快照；
4. 配置文件、环境变量、工具注册表或客户端能力在运行中变化时，不会静默修改当前 Turn 的
   model、system prompt、tool schema 或审批策略；
5. 测试替身与生产 Provider 经过同一个 Capability/Factory 边界注入，不能继续依赖
   with_provider 等只适用于当前单 Provider 的特殊 builder；
6. 旧的 ResolvedProvider、resolve_openai_provider_from_config 等“配置层直接创建
   Provider”的路径在 R4 结束前删除，不保留内部 re-export shim。

## 2. 明确范围与非目标

### 2.1 本计划包含

- Provider descriptor、secret reference、模型选择和配置 revision 的纯数据契约；
- ProviderFactory 与首个 OpenAI-compatible 实现的组合边界；
- CapabilityResolver、GenerationResolution 和 generation 持久化记录；
- SessionSupervisorDependencies、SessionActor 和 Provider worker 的依赖迁移；
- Profile/客户端 capability、workspace、approval policy 的快照化；
- v3 → R4 加性 migration、旧数据库读取和错误映射；
- contract、单元、集成、恢复和多 Profile 隔离测试；
- 配置、Provider、Runtime 和 RPC 文档的同步更新。

### 2.2 本计划不包含

- 不实现多个 Provider 的自动 fallback、credential pool、负载均衡或健康探测路由；
- 不实现 MCP、Skills、memory、scheduler、delegation 或 browser broker；这些属于
  Phase 3 的独立工作包；
- 不读取、迁移或依赖 Python/Hermes 运行时；Python 只能作为行为参考；
- 不改变现有 RPC 方法名、stdio/WebSocket transport 或 TUI 交互流程；
- 不把完整 prompt、API key、endpoint、绝对 workspace 路径或 MCP 命令写入数据库；
- 不把 CapabilityResolver 做成全局可变配置中心，也不允许 Turn 中途 reload；
- 不因为抽象需要而新增空的插件系统、通用 hook 或独立 bootstrap crate。

R4 可以为未来多 Provider route 保留明确的 descriptor 字段和 Factory 扩展点，但只有一个
真实的 OpenAI-compatible 实现；第二个 Provider 和 fallback 必须在 Phase 3 P3.1 另行设计。

## 3. 当前基线与需要移除的耦合

| 位置 | 当前问题 | R4 目标 |
| --- | --- | --- |
| sagent-config::provider_resolver | 同时解析配置、读取凭据、创建 OpenAiCompatibleProvider | 只输出已校验的 Provider descriptor；实例创建移到 Factory |
| sagent-config/Cargo.toml | config 直接依赖 sagent-provider | config 不依赖具体 Provider crate |
| RuntimeBootstrap::from_paths | 直接调用 resolve_openai_provider_from_config，保存 model/ready 两套状态 | 读取一次 ProfileConfig，交给 Factory/Resolver，保存不可变能力装配结果 |
| SessionSupervisorDependencies | ModelRuntime + with_provider 只表达静态单 Provider | 持有 CapabilityResolver 或等价的窄能力端口 |
| SessionActor::submit_prompt | prompt、工具 schema、generation 与静态 model 分散计算 | 先得到完整 GenerationResolution，再原子持久化和启动 worker |
| start_next_provider_round | 从 Actor 的 model runtime 重新取 Provider/model | 从当前 ActiveTurn 复用同一个 resolution |
| session_generations | 只有 system/tool/model/profile 基础字段 | 增加无秘密的 capability/revision 记录，支持恢复和审计 |
| PublicConfig/RPC | 需要继续保证不泄漏 endpoint、secret reference 和路径 | 只显示 provider/model 名称及必要的非秘密状态 |

现有 ProfileConfig 已经提供一次配置读取快照，这是 R4 的起点。R4 不得恢复各 resolver
各自读取 config.yaml 的旧模式。

## 4. 目标架构与依赖方向

~~~text
Profile config.yaml + profile .env
              │  只读一次
              ▼
       sagent-config
   validated descriptors
              │  composition root 映射
              ▼
       ProviderFactory ──────► sagent-provider
              │                    │
              │                    └─ ModelProvider / OpenAI adapter
              ▼
     CapabilityCatalog (immutable)
              │
              ▼
       CapabilityResolver
              │  每个 Turn 解析一次
              ▼
      GenerationResolution
       ┌───────────────┐
       │ runtime handle│  Arc<ModelProvider>, Tool snapshot, policy
       │ durable record│  ids, hashes, revisions; 无秘密
       └───────────────┘
              │
              ▼
      SessionActor / Worker ───► StorageManager / Store
~~~

依赖方向固定为：

- sagent-config 不依赖 sagent-provider、sagent-runtime 或 sagent-rpc；
- sagent-provider 只定义 Provider-neutral DTO、Factory、错误和具体 adapter，不读取
  ProfileConfig；
- sagent-runtime 只依赖已经装配的 Provider、Tool、Policy 和 Storage 能力，不读取文件或
  环境变量；
- sagent-rpc 的 bootstrap 是当前组合根，负责把 config descriptor 映射为 Factory 输入；
- sagent-store 只保存可审计的 generation record，不保存运行时 trait object 或秘密；
- sagent-protocol 仍只包含 DTO、方法和稳定错误，不依赖 Provider、Runtime 或 Store。

当 CLI 成为第二个需要 Provider 装配的生产入口时，再提取共享组合模块；R4 不提前创建
没有第二调用方的 sagent-bootstrap crate。

## 5. 核心数据契约

### 5.1 配置层 descriptor

保留当前 YAML 的兼容读取形态，但在 ProfileConfig::from_document 后立即归一化为以下
非秘密结构：

~~~text
ProviderDescriptor
├── provider_id / kind
├── model_id
├── endpoint (validated URL, optional for local/future kinds)
├── credential_ref (environment/keychain reference, never value)
└── descriptor_revision
~~~

要求：

- 原始 provider、model、base_url、api_key_env、key_env、providers.<name>.api/url
  只在 config parser 出现一次；之后统一使用规范化字段；
- 现有 Sagent alias 继续支持一个配置版本，alias 归一化后不能导致不同 revision；不额外
  复制 Python 的字段兼容；
- 空 provider、空 model、无效 URL、空 secret reference、重复 provider 名称和不支持的
  kind 在快照构造阶段失败；不拖到第一次 prompt；
- Debug、Serialize 和公开配置摘要都不包含 secret value；credential reference 是否
  对外显示由公开摘要策略决定，默认不显示；
- ProfileConfig 仍是一次性、不可变组合对象；所有 resolver 接受 &ProfileConfig 或
  其中的 descriptor，不接受路径并自行读取文件。

建议类型分工：

| 类型 | 所属 | 职责 |
| --- | --- | --- |
| ProviderConfig | sagent-config 内部 | serde YAML 原始形态 |
| ProviderDescriptor | sagent-config | 已归一化的非秘密配置意图 |
| CredentialReference | config/provider-neutral 边界 | 表示如何取得 secret，不携带值 |
| ProviderSpec | sagent-provider | Factory 可消费的 Provider-neutral 输入 |
| CapabilityDescriptor | sagent-runtime | Provider、工具、workspace、approval 的不可变组合 |
| GenerationResolution | sagent-runtime | 当前 Turn 的运行时快照，允许持有 trait object |
| PersistedGenerationResolution | sagent-store/types | 可序列化、无秘密的审计记录 |

### 5.2 ProviderFactory

ProviderFactory 放在 sagent-provider 的独立模块中，但不引用 sagent-config。其输入是
ProviderSpec，由组合根从 ProviderDescriptor 映射得到；这样具体 Provider crate 不会反向
依赖配置读取器。

Factory 必须满足：

1. create(spec, secret_resolver) 只在 bootstrap 或显式 generation refresh 时调用；
2. secret_resolver 返回带 Debug 脱敏语义的 secret material，Factory 不保存 credential
   reference 以外的可序列化配置；
3. openai-compatible 由 OpenAiProviderBuilder 创建；未知 kind 返回稳定的
   UnsupportedProvider，不得静默使用 SQLite 或默认 Provider；
4. Factory 输出 ProviderHandle/Arc<dyn ModelProvider> 和非秘密 ProviderIdentity，
   不输出带 key 的 ResolvedProvider；
5. HTTP client、timeout、SSE parser 和 API key 生命周期全部停留在 Provider adapter；
   Runtime 只调用 ModelProvider::stream；
6. 认证失败、配置错误、限流、远端错误、协议错误和取消保留稳定错误类别，错误文本不得
   包含 endpoint、header、request body 或 key。

Factory 内部允许使用 kind → builder 的表驱动注册，但 R4 只注册一个真实 builder。不能
为了“未来插件化”增加动态加载、全局注册表或无调用方的泛型框架。

### 5.3 CapabilityResolver

CapabilityResolver 属于 sagent-runtime::capabilities，只消费启动时装配好的
CapabilityCatalog：

~~~text
ResolutionInput
├── session_id / turn_id
├── prompt input
├── client capabilities snapshot
├── profile/config revision
└── current policy revision
~~~

输出 GenerationResolution，至少包含：

- provider_id、model_id 和 Arc<dyn ModelProvider>；
- PromptSnapshot 或稳定的 system prompt parts/hash；
- canonical tool schema、tool schema hash/revision；
- workspace policy、approval policy 和客户端 capability 的有效交集；
- config_revision、prompt_revision、policy_revision、tool_schema_revision；
- 用于持久化的 PersistedGenerationResolution，不包含运行时 handle、secret、endpoint 和
  绝对路径。

Resolver 只做“从已装配能力中选择和冻结”，不在每次 Turn 中读取 YAML、.env、网络健康
状态或数据库 schema。当前 Turn 建立后，后续 tool round、重试收口和 worker 事件都只能
读取 ActiveTurn 保存的 resolution。

### 5.4 Revision 与 hash

- config_revision：规范化、非秘密 Profile descriptor 的 canonical JSON SHA-256；
- provider_revision：provider kind、provider id、model 和 endpoint 的非秘密摘要；
- prompt_revision：system prompt parts 的 hash，复用 PromptSnapshot 的 canonical 规则；
- tool_schema_revision：ToolRegistry canonical schema hash；
- policy_revision：approval、workspace、client capability 有效策略的 canonical hash；
- generation_revision：以上 revision 加 session/generation 号的组合标识。

所有 hash 输入必须排序稳定、拒绝秘密字段和机器绝对路径。环境变量值改变不会修改已经
持久化的 generation；只有显式重新 bootstrap 或 capability refresh 才能创建新 revision。

### 5.5 持久化记录

R4 采用加性 schema migration，不修改旧字段含义：

~~~text
generation_resolutions
├── session_id + generation (unique/FK)
├── provider_id / model_id
├── config_revision / provider_revision
├── prompt_revision / system_hash
├── tool_schema_revision / tool_schema_hash
├── policy_revision / workspace_policy_hash / approval_policy_hash
├── client_capability_hash
└── created_at
~~~

禁止写入：API key、secret reference 的值、endpoint、绝对路径、完整 system prompt、完整
tool arguments、HTTP 响应和临时 delta。

现有 v3 数据库读取策略：旧 session_generations 行没有 resolution record 时返回显式的
LegacyGeneration 状态；不得伪造新的 Provider 或把旧数据升级成当前配置。新 generation
必须在同一个高层 Store 操作中写入基础 generation、resolution record、Turn 和 user message，
以避免“Turn 已开始但 capability 没有审计记录”。

## 6. 分阶段执行计划

### R4.0：冻结契约与行为基线

**目标：** 在改动 config/provider/runtime 前锁定现有单 Provider、prompt cache、工具权限、
取消和错误行为。

**范围：** 只新增 contract fixture、测试辅助和本计划引用，不修改生产行为。

**主要文件：**

- crates/sagent-contracts/：新增 capability/generation contract fixture；
- crates/sagent-runtime/tests/：补充当前 Provider、工具回环和取消顺序的行为测试；
- docs/design/rust-code-quality-and-architecture-optimization-plan.md：链接本计划。

**必须锁定的行为：**

1. Provider 尚未配置时只读 RPC、空会话创建仍可用，prompt 失败映射稳定；
2. Provider 已产生 delta、tool-call fragment 或 usage 后，不允许改变当前调用来源；
3. tool approval、workspace、client interactive capability 的有效权限不在 Turn 中途变化；
4. system_hash、tool_schema_hash 和 assistant transcript 顺序保持不变；
5. 两个 Profile 的 config、secret source、workspace 和 Store 完全隔离。

**退出条件：** fixture 在当前实现上通过；每条测试说明输入、动作、可观察结果和保护的
不变量，不读取源码文本，不固定易变的模型目录数量。

### R4.1：配置 descriptor 纯化与 alias 收口

**目标：** 让 sagent-config 输出一次加载、一次校验、无副作用的 Profile snapshot。

**主要文件：**

- crates/sagent-config/src/provider_config.rs；
- crates/sagent-config/src/profile_config.rs；
- crates/sagent-config/src/provider_resolver.rs；
- crates/sagent-config/src/credentials.rs；
- crates/sagent-config/src/lib.rs、Cargo.toml；
- crates/sagent-config/src/public_config.rs 及对应测试。

**执行步骤：**

1. 将 raw YAML model 与已校验 ProviderDescriptor 分开，所有 alias 只在 parser 层归一化；
2. 为 provider kind、model id、credential reference、descriptor revision 定义显式类型或
   构造校验函数；
3. 把 resolve_provider_config_from_config 改为纯 descriptor 解析；名称应表达“已加载
   快照输入”，不得再接受路径或隐式读取配置；
4. 删除 ResolvedProvider、ResolvedProviderConfig 中的 OpenAiCompatibleProvider 字段；
5. 将凭据读取收口为只供 bootstrap/Factory 使用的窄入口，禁止 ProfileConfig 保存 secret；
6. 删除 config crate 对 sagent-provider 的生产依赖，更新 workspace 依赖图和公开导出；
7. 更新 PublicConfig 测试，保证 endpoint、credential reference、secret value 和路径均不出现在
   JSON 或 Debug。

**已完成子项（2026-09-17）：**

- [x] 新增 parser 专用 `RawProviderConfig`、`RawModelSetting` 和
  `RawUserProviderConfig`，兼容 alias 不再进入运行时 descriptor；
- [x] `ProviderDescriptor`、`ModelSetting`、`ModelDetail` 和 `UserProviderConfig` 只保存
  归一化字段，`name/model`、`api/url/base_url` 和 `key_env/api_key_env` 在 parser 边界合并；
- [x] 增加 alias 归一化行为测试，确认 ProfileConfig 对外只暴露 canonical 字段。
- [x] 新增 `ProviderKind`、`ModelId`、`CredentialReference`、`DescriptorRevision` 显式值对象；
  构造时校验空白和格式，descriptor revision 基于归一化且不含 secret 的 canonical JSON
  计算 SHA-256。

**验收：** 删除/暂时屏蔽 config.yaml 与 .env 后，已经创建的 ProfileConfig 仍能完成
descriptor/public summary 相关纯操作；config crate 编译不需要 Provider adapter；无 HTTP、
SQLite、tokio task 或网络副作用。

### R4.2：ProviderFactory 与密钥边界

**目标：** 把 Provider 实例化从 config 移到可测试的 Factory/组合根，并保留现有 OpenAI SSE
行为。

**主要文件：**

- crates/sagent-provider/src/factory.rs（新）；
- crates/sagent-provider/src/lib.rs、error.rs、openai.rs；
- crates/sagent-rpc/src/bootstrap/；
- crates/sagent-config/src/credentials.rs；
- provider mock/adapter 集成测试。

**执行步骤：**

1. 定义 ProviderSpec、ProviderIdentity、CredentialReference、SecretResolver 和
   ProviderFactory 的最小公共契约；
2. 用 OpenAiProviderBuilder 封装 endpoint 校验、HTTP client 创建和 secret consumption；
3. 在 RPC bootstrap 中将 ProviderDescriptor 映射为 ProviderSpec，Factory 失败时保存
   可分类的 ProviderAvailability，不泄漏具体错误；
4. 将 secret resolver 做成一次性、profile-scoped 的 adapter；禁止把 secret 放进
   ProviderSpec、ProviderIdentity、RuntimeBootstrap Debug 或 RPC DTO；
5. 让 mock Provider 也由同一个 Factory/Resolver boundary 创建，测试不再绕过生产装配；
6. 保持当前“Provider 不可用时只读可用”的兼容行为，prompt 边界返回稳定 runtime error。

**验收：**

- 缺失 secret、无效 endpoint、未知 kind、HTTP client 创建失败都有稳定分类；
- format!("{factory:?}")、format!("{identity:?}")、错误和日志不含 key/endpoint；
- OpenAI SSE、半包 JSON、取消、429/5xx 和 usage 现有测试不回归；
- Factory 不依赖 Store、Runtime、RPC 或 Profile 文件路径。

### R4.3：CapabilityCatalog 与 GenerationResolution

**目标：** 把 Provider、工具、workspace、approval 和 client capability 组合为不可变能力目录，
并定义每个 Turn 的 resolution。

**主要文件：**

- crates/sagent-runtime/src/capabilities/mod.rs（新）；
- catalog.rs、resolver.rs、revision.rs、policy.rs（按职责拆分）；
- crates/sagent-runtime/src/supervisor_dependencies.rs；
- crates/sagent-agent/src/prompt.rs（仅在需要复用 hash 契约时修改）；
- crates/sagent-types/src/ 中的稳定 ID/摘要类型（确有跨 crate 使用时才新增）。

**执行步骤：**

1. 定义 CapabilityCatalog：Provider handles、ToolRegistry snapshot、workspace policy、
   approval policy 和配置 revision；构造后只读；
2. 定义 ResolutionInput 与 CapabilityResolver，输入 session/turn/client/prompt，输出
   GenerationResolution；
3. 将 prompt system parts、tool schema、model/provider identity 和策略 hash 统一通过
   canonical serializer 计算；
4. 把当前单 Provider 选择写成明确的 SingleProviderSelection，未知/缺失 Provider fail-closed；
5. 将 ClientCapabilities 转换成 approval policy 的输入快照；Resume 只影响下一个 Turn，
   不修改当前 active Turn；
6. 将 ProviderRouter、自动 fallback 和多候选 route 留到 Phase 3 P3.1，不在 R4 复制；
7. 为 GenerationResolution 提供脱敏 Debug 和仅包含非秘密字段的持久化投影。

**验收：** 相同输入得到相同 revision；修改配置文件、环境变量、tool registry 原对象或
client capability 后，已创建 resolution 不变；新输入得到新 resolution；两个 Profile 不
共享 capability object、workspace 或 secret source。

### R4.4：Generation resolution 加性持久化

**目标：** 在数据库中留下足以恢复和审计的 resolution record，但不把运行时对象或秘密写入 Store。

**主要文件：**

- crates/sagent-store/src/sqlite/migration.rs；
- crates/sagent-store/src/sqlite/session/turns.rs；
- crates/sagent-store/src/ports/ 的 Session/Turn 端口；
- crates/sagent-store/src/store_tests/ 与 v3 fixture；
- crates/sagent-runtime/src/actor/generation.rs。

**执行步骤：**

1. 新增 v4 加性 migration 和 generation_resolutions 表，唯一键为 session + generation；
2. 定义 PersistedGenerationResolution 与 row mapping，字段只包含 ID、hash、revision、时间；
3. 扩展高层 Store 操作，使 generation、resolution、Turn、user message 和 started event 的
   业务一致性边界清晰；不得由 Actor 分别拼 SQL；
4. v3 旧数据读取返回明确 legacy 状态；不伪造当前配置、不覆盖既有 hash、不静默执行额外 migration；
5. 加入 v3→v4、重复 migration、损坏 schema、回滚和 profile isolation 测试；
6. get_generation/恢复 API 同时返回基础 generation 与可选 resolution，调用方不能依赖 SQLite row。

**验收：** 新 Turn 不可能出现“有 user message/started event 但没有 resolution record”；旧 v3
数据库仍能只读恢复；迁移失败不留下半套表；resolution 查询无副作用；对外错误不泄漏 SQL、路径
或连接信息。

### R4.5：SessionSupervisor 与 SessionActor 迁移

**目标：** 移除静态 model/provider builder，让 Actor 在 Turn 开始时解析一次，并在整个 Turn 内复用。

**主要文件：**

- crates/sagent-runtime/src/supervisor_dependencies.rs；
- crates/sagent-runtime/src/actor/model_runtime.rs、session.rs、prompt.rs、generation.rs；
- crates/sagent-runtime/src/active_turn.rs、actor_support/input.rs；
- crates/sagent-runtime/src/worker/provider.rs 与 recovery/tool round 代码；
- 现有 runtime actor/provider/recovery/fault-matrix 测试。

**执行步骤：**

1. SessionSupervisorDependencies 持有 Arc<dyn CapabilityResolver> 或等价窄接口，删除
   生产路径的 with_provider；
2. SessionActor::submit_prompt 按“校验 → resolve → prompt/tool 准备 → 原子持久化 → 启动
   worker → active 安装 → 发布事件”顺序执行；
3. ActiveTurn 保存完整 GenerationResolution 或其不可变运行时引用；
4. start_next_provider_round 只从 ActiveTurn 读取 provider/model/tool/policy，不再次 resolver、
   读取配置或读取环境变量；
5. worker 只接收 resolution 投影出的 request，不直接访问 Catalog、ProfileConfig 或 secret source；
6. 测试 FakeResolver 与 FakeProvider 走相同入口，删除“测试直接注入静态 model runtime”的旁路；
7. 保持 Actor 单写、先持久化后完成、取消 token 传播、迟到事件和 worker panic 收口语义。

**验收：** 同一个 Turn 的所有 provider round 使用相同 provider/model、tool schema 和 policy hash；
配置 reload 不影响活动 Turn；中断、超时、worker panic 和重启恢复不会重新创建或切换 Provider；
提交前 resolver/Store 失败不会启动 worker。

### R4.6：Bootstrap、RPC 与公开边界迁移

**目标：** 让当前 RPC daemon 成为正确的组合根，并移除旧 resolver 入口。

**主要文件：**

- crates/sagent-rpc/src/bootstrap/runtime.rs；
- crates/sagent-rpc/src/service/runtime.rs；
- crates/sagent-rpc/src/transport/connection.rs、stdio.rs、websocket.rs；
- crates/sagent-protocol/src/method/、dispatch.rs（仅在 DTO/错误确需同步时修改）；
- crates/sagent-cli/ 中任何直接调用旧 Provider resolver 的路径。

**执行步骤：**

1. RuntimeBootstrap::from_paths 只读取一次 ProfileConfig，创建 StorageManager、Provider
   Factory 和 CapabilityCatalog；不暴露 endpoint、路径、连接或 secret；
2. 将 provider_ready: bool 替换或包装为稳定的 ProviderAvailability，保留只读 RPC/空会话
   兼容行为；
3. open_service 继续从 Supervisor 获取查询端口，不重新解析配置或创建第二份 provider；
4. config.read/gateway.ready 只公开 provider/model 名称和安全状态；不得回显 credential
   reference、endpoint、workspace 绝对路径、HTTP 错误或 hash 原文；
5. 删除 resolve_openai_provider_from_config、旧 ResolvedProvider 导出和调用点；用一次
   grep 检查生产路径不再引用它们；
6. 保持 protocol JSON 字段兼容；如必须新增 availability/revision 字段，使用可选、可拒绝未知
   字段的契约测试，不改变现有请求方法语义。

**验收：** RPC 子进程可以在无 Provider 配置时启动并提供只读功能；配置正确时 prompt 通过
Resolver；错误时返回稳定错误且无 secret；stdout 仍为合法 NDJSON，WebSocket 与 stdio
共用同一 Runtime 依赖快照。

### R4.7：删除旧路径、文档和可读性收口

**目标：** 防止新旧两套装配路径长期并存，并让读者能沿一条路径理解 R4。

**执行内容：**

- 删除 config crate 到 Provider adapter 的依赖和无调用方 re-export；
- 删除旧 ResolvedProvider、with_provider、路径读取 resolver 和重复 secret 读取；
- 检查 lib.rs/mod.rs 只导出稳定 API，内部 Factory/adapter 保持最小可见性；
- 按主题拆分超过 500 行的新增/修改 Rust 文件；超过 100 行的编排函数按具名阶段拆分；
- 为公共类型、Factory、Resolver、revision 和 migration 补充中文 Rustdoc，说明秘密、生命周期、
  取消和事务边界；
- 更新 rust-code-quality-and-architecture-optimization-plan.md、rust-phase-3-plan.md、
  docs/design/README.md、crate README/模块级 Rustdoc；
- 在 R4 结束时将本计划标记为已完成或记录未完成项，不能只修改顶层状态。

### R4.8：综合验证与结项

**必须执行：**

~~~powershell
cargo fmt --all -- --check
cargo test --workspace --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo run -p sagent-contracts
git diff --check
~~~

影响真实 transport、子进程或终端时，额外执行项目约定的 native Windows 测试，并在 macOS/Linux
CI 上验证，不允许通过修改 sys.platform 或伪造环境变量代替原生验证。

## 7. 测试矩阵

| 层次 | 必须覆盖的行为 |
| --- | --- |
| config unit | alias 归一化、descriptor 校验、canonical revision、未知 kind、缺少 secret reference |
| provider unit | Factory builder、secret 脱敏、错误分类、OpenAI request/SSE/取消/usage |
| capability unit | 同输入同 revision、client capability 有效交集、配置变化不污染旧 resolution |
| store unit | v3→v4 migration、resolution 原子写入、旧数据 legacy、重复/回滚/损坏数据库 |
| runtime unit | Resolver 失败不启动 worker、ActiveTurn 复用 resolution、tool round 不重新解析 |
| runtime integration | fake resolver/provider 的完整 prompt→persist→worker→final/interrupt 回合 |
| RPC integration | 无 Provider 只读启动、配置 Provider 启动、secret 不出 stdout/JSON/错误 |
| profile isolation | 两个 Profile 的 config revision、Provider spec、workspace、Store 不串线 |
| contract/E2E | reload 不改变活动 generation；显式 refresh 才产生新 generation；重启可恢复非秘密记录 |

测试约束：fixture 使用临时 Profile home、loopback mock 和内存/recording resolver；不写开发者
home，不使用真实 API key，不读取源文件文本，不锁定模型目录数量等易变快照。

## 8. 提交边界与回滚策略

建议拆为以下可独立审阅的提交：

1. test: freeze R4 provider and generation contracts：R4.0 fixture/基线；
2. refactor(config): produce pure provider descriptors：R4.1；
3. feat(provider): add factory and redacted secret boundary：R4.2；
4. feat(runtime): resolve immutable generation capabilities：R4.3；
5. feat(store): persist generation resolution additively：R4.4；
6. refactor(runtime): consume resolver from actor turn boundary：R4.5；
7. refactor(rpc): assemble provider and capability catalog in bootstrap：R4.6；
8. chore: remove legacy provider resolution paths and update docs：R4.7/R4.8。

任一步骤如果出现以下情况，停止并回到最近一个绿色边界：

- 当前 Turn 的 provider/model/tool schema/policy 发生隐式变化；
- Provider 或 secret 进入 Debug、RPC、Store、日志或错误；
- config 重新依赖具体 Provider，或 Runtime 重新读取文件/环境变量；
- Actor 单写、事务原子性、取消、迟到事件或恢复顺序改变；
- 为通过单测而绕过 Factory/Resolver，生产和测试出现两条装配路径；
- 新增接口没有真实调用方，或出现万能 Context/Manager/Resolver 收纳无关依赖。

## 9. R4 完成定义

只有全部满足以下条件，R4 才能标记完成：

- sagent-config 不依赖具体 Provider adapter，配置读取只产生已校验 descriptor；
- ProviderFactory 是唯一生产 Provider 创建入口，secret 只在 Factory 创建边界消费；
- CapabilityResolver 能从不可变 Catalog 为每个 Turn 生成 GenerationResolution；
- Active Turn 的 worker 和工具回环只使用原 resolution，不响应中途配置/环境变化；
- generation resolution 的持久化记录无秘密、可恢复、可审计，v3 数据库可安全读取；
- Runtime、RPC、CLI 和工具不再持有或传播 ResolvedProvider、具体 HTTP client 或配置路径；
- Provider 不可用时只读 RPC/空会话兼容行为保持，prompt 失败为稳定错误；
- 现有 Provider、工具、approval、cancel、recovery、RPC/TUI contract 全部通过；
- cargo fmt、workspace test、严格 Clippy、contracts、diff 检查和适用的原生 CI 全绿；
- 计划、架构文档、代码约束和 README 已同步，R4 旧路径已删除而不是仅标记 deprecated。

R4 完成后，才进入 Phase 3 P3.0/P3.1 的多 Provider route、CapabilityResolver 扩展和 MCP
能力接入。R4 不为这些功能预先实现业务分支，只保证它们有稳定的 generation 边界和可替换装配点。

## 10. 首个执行步骤

R4.0 执行记录（2026-09-17）：

- [x] 新增 `provider_config_snapshot` fixture，验证 Profile 配置只读取一次、删除源文件后
  快照仍可生成公开摘要，并验证 Provider 调试输出不包含 fixture secret；
- [x] 新增 `generation_record` fixture，验证 generation 的 system/tool hash、model 和
  profile revision 原值写入并读回；
- [x] 保留并纳入现有 `prompt_submit_validates_input_and_hides_unconfigured_provider_details`、
  `tool_results_are_replayed_to_the_next_provider_round_before_final_text`、
  `cancellation_during_provider_worker_does_not_create_assistant_message` 和
  `separate_profile_databases_do_not_share_actor_data` 行为测试；这些测试继续锁定输入、
  动作、可观察结果和不变量；
- [x] `cargo run -p sagent-contracts`、受影响 crate 测试、workspace 测试、严格 Clippy、
  格式化和 diff 检查通过。

R4.0 的 fixture 只增加行为基线，不改变 Provider、config 或 Runtime 生产路径。下一步执行
R4.1：将配置层的 Provider 解析收敛为纯 descriptor，并移除 config crate 对具体 Provider
adapter 的创建耦合。
