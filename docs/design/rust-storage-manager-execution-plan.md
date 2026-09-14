# Sagent StorageManager 重构执行计划

作者：SongZQ  
状态：M0、M1、M2、M3 已完成；M4.1 selector 已定义但尚未接入实际启动链，M4.2 及后续迁移仍未完成
范围：R3.5 StorageManager 领域存储聚合与后端隔离

## 1. 背景与问题

当前 `sagent-store` 已有 `StorageFactory`、旧依赖聚合和
`SqliteStorageManager` 的过渡实现，但依赖聚合仍主要围绕 Session：

- 旧 `StorageDependencies` 暴露写端口、查询端口和搜索端口；
- `StorageManager` 的申请入口直接返回这些 Session 端口；
- Runtime、RPC、CLI 和工具仍有长期持有 `StorageFactory` 的路径；
- SQLite 的具体数据库对象虽然位于 `sagent-store`，但业务调用方仍按底层端口组织代码。

这使“存储管理”和“Session 业务存储”两个层次混在一起。目标不是建立一个包含所有 CRUD
的万能 `Storage`，而是建立清晰的三层边界：

1. `StorageManager` 管理 Profile 级后端资源和生命周期；
2. `Storage` 是上层持有的抽象存储对象，按业务域暴露 `storage.session` 等能力；
3. `SessionStorage`、`EventStorage` 等业务存储负责各自的表操作、查询和事务规则。

## 2. 目标与非目标

### 2.1 目标

- 每个 Profile 只创建一个长期存活的 `StorageManager`；
- Runtime、RPC、CLI 和工具不持有数据库路径、连接、连接池、`SqliteDatabase` 或原始 Factory；
- 上层通过稳定的抽象对象访问业务域，例如 `storage.session.search(...)`；
- SQLite、PostgreSQL 或其它后端只在 Manager/adapter 内部选择资源和连接策略；
- 业务 Storage 负责领域表操作和事务边界，Manager 不承载 Session/Turn 业务规则；
- 读、写、Actor 和搜索能力使用最小权限的存储对象；
- 现有 Session、Turn、Message、Event 的协议、事务、事件顺序和错误语义保持不变；
- 新增未来领域（Cron、Memory 等）时，不需要修改 Runtime 的后端分支。

### 2.2 非目标

- 本计划不实现 PostgreSQL、远程数据库或连接池本身；
- 不把所有领域方法塞进一个万能 `Storage` trait；
- 不在没有实际调用方时预先创建 Cron、Memory 等空实现；
- 不改变数据库 schema、SessionActor 状态机、RPC 方法或 CLI 参数；
- 不在 Manager 中编排跨领域业务流程；跨表原子操作仍由对应业务 Storage 提供高层方法。

## 3. 目标架构

```text
ProfileConfig / StorageDescriptor
                │
                ▼
       create_storage_manager()
                │
                ▼
        Arc<dyn StorageManager>
                │
       ┌────────┴────────┐
       │                 │
       ▼                 ▼
  open_actor_storage  open_read_storage
       │                 │
       ▼                 ▼
    Storage          ReadStorage
       │                 │
       ├── session      └── session (只读)
       ├── event
       └── ...未来业务域
                │
                ▼
    SqliteSessionStorage / SqliteEventStorage
                │
                ▼
      SqliteDatabase / SQLite
```

Manager 只负责 Profile 级资源和能力装配；具体 Session 表、Message 表、Turn 表和搜索
索引的读写由 `SessionStorage` 完成。`SqliteDatabase` 只属于 SQLite adapter，用于连接、
事务和 schema 管理。上层代码不应出现 `match StorageKind`、`SqliteDatabase::open_*` 或 SQL。

## 4. 职责边界

### 4.1 `StorageManager`

唯一职责是管理一个 Profile 的持久化后端资源，并按访问边界创建抽象业务存储对象。
它是长期存在的 Profile 级管理器，不是某个数据库连接，也不是 Session 业务 Storage。

负责：

- 保存已验证的 `StorageDescriptor` 或后端资源句柄；
- 连接、连接池、文件句柄的生命周期；
- 健康检查和 migration 入口；
- 创建 Actor、只读和短写所需的 `Storage` 对象；搜索能力随 `ReadStorage.session` 提供；
- 将后端错误映射为稳定的 `StorageError`。

不负责：

- Session/Turn 的业务判断；
- CLI/RPC 参数校验；
- 组织跨命令的业务流程；
- 直接暴露具体数据库类型。

### 4.2 抽象 `Storage` 对象

`Storage` 是 Manager 返回给上层的能力聚合。字段按业务域组织，而不是按数据库表组织：

```rust
pub struct Storage {
    pub session: SessionStorage,
    // 只有出现真实调用方后，才增加 event、cron、memory 等字段。
}
```

因此 Session 相关操作统一写成：

```rust
let mut storage = manager.open_actor_storage()?;
storage.session.start_turn(...)?;
storage.session.complete_turn(...)?;
storage.session.search(...)?;
```

上层只认识 `Storage` 和业务 Storage，不认识 SQLite/PG 的连接模型。

### 4.3 业务 Storage

`SessionStorage` 是 Session 领域的业务存储外观，负责：

- Session 元数据和 Message 查询；
- Turn 生命周期和恢复操作；
- Session 范围的全文搜索；
- 需要跨多张表的高层原子事务。

它内部组合写入端口和统一的 `ReadOnlySessionStorage`，后者再收纳查询与搜索端口；这些
底层端口不再向 Runtime、RPC、CLI 或工具扩散。SQLite 的 `SqliteSessionStorage` 可以使用
`SqliteDatabase`，PG 的实现可以使用连接池，二者对外保持同一业务方法和错误语义。
`SqliteDatabase` 只负责 SQLite 资源，不再承载对外的 Session 业务 API。

### 4.4 权限聚合

不同调用边界使用不同的抽象对象：

```rust
pub struct ReadStorage {
    pub session: ReadOnlySessionStorage,
}

pub struct WriteStorage {
    pub session: WriteOnlySessionStorage,
}
```

- `open_actor_storage()` 返回完整的 `Storage`，但每个 Actor 必须独占自己的写入能力；
- `open_read_storage()` 返回 `ReadStorage`，不包含任何写入方法；
- `open_write_storage()` 返回 `WriteStorage`，不包含查询和搜索能力；
- 搜索工具从 `ReadStorage.session` 申请搜索能力；`open_search_storage()` 不作为长期
  Manager 接口，迁移期如保留只能是只读兼容入口。

读写对象可以共享 Manager 内部的连接池或后端资源，但不能共享可变的业务写入句柄。

## 5. 建议的核心接口

### 5.1 Manager 接口

```rust
pub trait StorageManager: Send + Sync {
    fn open_actor_storage(&self) -> StorageResult<Storage>;
    fn open_read_storage(&self) -> StorageResult<ReadStorage>;
    fn open_write_storage(&self) -> StorageResult<WriteStorage>;
    fn initialize(&self) -> StorageResult<()>;
    fn health_check(&self) -> StorageResult<()>;
}
```

其中 `StorageResult` 是稳定的存储错误结果；接口签名不得出现 `SqliteDatabase`、数据库路径、
`rusqlite::Connection`、连接池或远程连接字符串。

### 5.2 Session 外观

当前的底层端口建议按以下方式收敛：

```text
SessionStorage（对外业务外观）
    ├── SessionWriteStorage（内部写入能力）
    └── ReadOnlySessionStorage（查询 + 搜索能力）
```

对外方法使用业务意图命名，例如 `create_session`、`list_sessions`、`search_messages`、
`start_turn`、`complete_turn`；底层端口只在 adapter 和 Storage 组装边界出现。

## 6. 分阶段执行计划

### M0：冻结行为契约和迁移边界

工作：

1. 盘点所有 `StorageFactory`、`StorageDependencies`、`Store` 和 SQLite 类型的生产引用；
2. 确认 SessionActor 的 start/commit/complete/interrupt/recovery 事务契约；
3. 确认只读 RPC、CLI 查询和全文搜索不能创建数据库或执行 migration；
4. 为新旧对象关系写最小 contract，先锁定行为再迁移结构。

完成条件：旧实现和新目标接口的映射表明确，测试在迁移前全绿。

**执行记录（2026-09-13）：** 已完成生产引用、事务契约和只读边界审计，详见
[`rust-storage-manager-m0-baseline.md`](rust-storage-manager-m0-baseline.md)。同时新增 CLI
实际查询路径的“缺失数据库不创建文件”回归测试。RPC bootstrap 当前会显式初始化已选
Profile 的存储，此行为不属于只读 RPC 查询契约；M4/M5 必须在保留或显式调整该初始化
策略后，才能迁移 bootstrap。

### M1：定义业务 Storage 外观

工作：

1. 在 `sagent-store` 定义 `Storage`、`ReadStorage`、`WriteStorage`；
2. 将当前 Session 查询、写入和搜索端口隐藏在 `SessionStorage` 组装边界内；
3. 确定 `storage.session` 的读写方法和错误语义；
4. 不改变 SQLite adapter 的行为，只增加适配层。

完成条件：可以用 fake/recording Session Storage 构造 `Storage`，不需要 SQLite 类型。

**执行记录（2026-09-13）：** 已新增 `Storage`、`ReadStorage`、`WriteStorage` 以及按
职责拆分的 `SessionStorage`、`ReadOnlySessionStorage`、`WriteOnlySessionStorage`。完整
外观统一通过 `storage.session` 暴露业务方法。只有 `SessionStorage` 负责组合 Session
底层端口，顶层 `Storage`/`ReadStorage`/`WriteStorage` 只接收已组装的业务对象；窄对象在类型层面隔离读写能力；兼容期的
`StorageDependencies` 仍保留为过渡适配层。新增不依赖 SQLite 的 recording 构造测试，
验证三种聚合均可由领域端口装配。Runtime、RPC、CLI 和工具迁移留给 M5/M6。

### M2：重构 Manager 框架

工作：

1. 将 `StorageManager` 改为返回抽象 `Storage`/`ReadStorage`/`WriteStorage`；
2. 明确 Manager 是每个 Profile 一个的长期资源管理器；它只保存已验证 descriptor 或
   后端资源句柄，不保存 Session/Turn 业务状态；
3. 让 `SqliteStorageManager` 管理或申请 `SqliteDatabase`，并在 `open_*` 中组装业务
   Storage；Manager 本身不执行 Session 表操作；
4. 增加 `initialize` 和 `health_check` 两个明确入口：前者允许创建数据库和执行 migration，
   后者必须只读、无副作用，不创建数据库、不执行 migration；
5. 保留 `StorageFactory` 仅作为构造边界，禁止在 Runtime/RPC/CLI 中长期持有；
6. 删除长期的 `open_search_storage` 平级入口，搜索统一通过 `ReadStorage.session`；
7. 记录同步/异步模型、线程安全、连接生命周期和取消策略，并明确每次申请的资源释放
   和 Actor 独占写入语义。

完成条件：Manager 接口不出现 Session CRUD，也不泄漏数据库实现细节。

**执行记录（2026-09-13）：** `StorageManager` 已改为返回 `Storage`、`ReadStorage` 和
`WriteStorage`，并新增 `initialize`/`health_check` 生命周期入口。`SqliteStorageManager`
现在在 Manager 边界组装业务聚合；`StorageFactory` 仅在兼容层将新聚合拆回旧依赖，未向
Runtime/RPC/CLI 扩散。当前接口是同步阻塞模型：Manager 为 `Send + Sync`，每次 `open_*`
重新申请独立 SQLite 数据库句柄，Actor 之间不共享可变写句柄；没有隐藏后台任务，取消由调用方
的任务边界负责。`health_check` 只读且不会创建文件，`initialize` 明确表示允许创建和
迁移数据库。新增 Manager 的完整存储、窄读写、初始化和缺失库健康检查测试。

### M3：实现 SQLite 业务 Storage

工作：

1. 将当前 SQLite 端口实现整理为 `SqliteSessionStorage`；
2. 将当前 `Store` 重命名并收窄为 adapter 内部的 `SqliteDatabase`，只保留连接、事务、
   schema/migration 和底层执行职责，不向业务调用方导出；迁移期仅允许 fixture/兼容适配器
   使用临时根导出；
3. 将 Session、Message、Turn 和搜索操作集中到 `SqliteSessionStorage` 的清晰子模块，
   `SessionRow`、`TurnRow`、`MessageRow`、`EventRow` 等表 Model 只存在于 adapter 内部；
4. 保持 `complete_turn`、`commit_tool_result` 等高层原子操作，由业务 Storage 负责跨表
   事务边界，不能退化为上层多次 CRUD；
5. Manager 负责创建资源和组装 `SqliteSessionStorage`，不直接执行表操作；
6. 保证只读对象不会获得 SQLite 写入句柄。

完成条件：SQLite 与 fake 实现都能构造同一抽象 `Storage`，既有 `SqliteDatabase` 行为契约
不变。

**执行记录（2026-09-14）：** 已将原 `Store` 重命名为 `SqliteDatabase`，并把连接打开、
访问模式、migration、健康检查和只读写入保护从 `lib.rs` 拆到独立的
`sqlite/database.rs`。所有 SQLite 实现现已收拢到 `src/sqlite/` 包，其中 `session/` 按
会话领域继续划分为消息、查询、搜索、Turn、Event、事务和端口适配子模块。`SqliteStorageFactory`
只保留兼容构造职责，`SqliteStorageManager` 直接
通过 adapter 组装 `Storage`、`ReadStorage` 和 `WriteStorage`，不再经由旧依赖聚合创建新对象。
Session、Message、Turn、Event 和 FTS 的 SQL 实现模块已收回 crate 内部可见性，外部只看到
领域 DTO 与业务 Storage 外观；既有高层原子操作和只读边界保持不变。`SqliteDatabase` 根导出
暂为 fixture/兼容适配器保留，待 M5/M6 完成上层测试与工具迁移后删除该过渡导出。

### M4：建立唯一 selector/bootstrap 入口

工作：

1. 新增 `create_storage_manager(ProfileConfig/StorageDescriptor)`；
2. SQLite 由 selector 创建 `SqliteStorageManager`，未支持后端明确失败；
3. 配置只读取一次，Manager 构造后不重复读取 `config.yaml`；
4. `SqliteStorageFactory::into_manager` 仅作为过渡，最终由 selector 直接返回 Manager；
5. `RuntimeBootstrap::from_paths` 只接收抽象 Manager，不暴露数据库细节。

完成条件：SQLite/未来 PG 只替换 selector 和 adapter，业务调用路径不变。

**执行记录（2026-09-14，M4.1 selector 定义）：** 已在 `sagent-rpc` bootstrap selector 中新增
`create_storage_manager(paths, descriptor)`，但尚未替换 `RuntimeBootstrap` 当前的 Factory
依赖。该入口复用已加载的 `StorageDescriptor`，将
SQLite 相对路径或默认文件名解析为 Profile 作用域的绝对路径后创建
`Arc<dyn StorageManager>`；Remote 和当前 SQLite 不支持的 schema、namespace、只读组合
均 fail-closed。迁移期 `create_storage_factory` 仍保留，并与 Manager selector 共用同一校验
和路径解析逻辑，避免两条入口产生不同后端语义。RuntimeBootstrap 的实际切换留在后续步骤，
因此当前运行时的实际依赖仍是 `StorageFactory`。

### M5：迁移 Runtime 和 Supervisor

工作：

1. `SessionSupervisorDependencies` 持有 `Arc<dyn StorageManager>`，不再持有原始 Factory；
2. 每个 SessionActor 通过 `open_actor_storage()` 获得独占 `Storage`；
3. RPC 查询通过 `open_read_storage()` 获取 `ReadStorage`；
4. Supervisor 不保存路径、连接或 `SqliteDatabase`；
5. Actor 仍是 Session 状态唯一写入者，worker 不得绕过 `storage.session` 写库。

完成条件：Runtime 的 Session、Turn、恢复和事件测试全部通过，且无生产代码 Factory 传播。

### M6：迁移 RPC、CLI 和工具

工作：

1. RPC `SessionService` 只依赖 `ReadStorage.session`；
2. CLI `SessionService` 只依赖 `Storage`/`ReadStorage`/`WriteStorage`，命令 handler 不接触
   `SqliteDatabase` 或 Factory；
3. `session_search` 使用 `storage.session.search(...)`，不单独打开 SQLite；
4. Profile 创建和其它命令都通过 Manager 的窄入口申请能力；
5. 工具只获得自身职责需要的读、写或搜索能力。

完成条件：Runtime、RPC、CLI 和工具的生产路径不再导入 `SqliteDatabase` 或 `rusqlite`。

### M7：删除过渡路径

工作：

1. 删除上层对 `StorageFactory` 的长期字段和闭包传播；
2. 删除仅为兼容旧调用方保留的 Session 端口 re-export 和自由函数；
3. 删除 CLI/RPC 各自的后端 selector，统一使用 `create_storage_manager`；
4. 更新 crate 文档、R3 计划和架构图；
5. 检查依赖图，避免 Manager 反向依赖 CLI、RPC 或 Runtime。

完成条件：代码中只有 selector、Manager 构造和 adapter 可以看到具体后端类型。

### M8：综合验证与 R3.5 结项

工作：

1. 增加 fake/recording Manager，验证 Actor 的独占写端口和事务调用顺序；
2. 验证不同 Profile 的 Manager 和 `storage.session` 不串库；
3. 验证只读对象无法执行写入，搜索对象无法获得写入能力；
4. 验证 SQLite 缺失文件、schema 错误、FTS 错误和未支持后端的稳定错误映射；
5. 运行全量格式化、测试、Clippy、contract 和文档检查。

完成条件：R3.5 专项验收全部满足，R3.4 的已有行为无回归。

## 7. 不变量与风险控制

### 7.1 必须保持的不变量

- 一个 Profile 只创建一个长期 `StorageManager`；
- 一个 Session 的活动状态仍只有一个 Actor 写入；
- `start_turn`、工具结果提交、完成、中断和恢复保持单次高层原子操作；
- 只读路径不创建数据库、不执行 migration、不修改 `updated_at`；
- 后端切换不改变业务 Storage 的方法和错误语义；
- Manager 不读取业务参数、不输出 CLI 文本、不发布 Runtime 事件。

### 7.2 主要风险

| 风险 | 控制措施 |
|---|---|
| 为统一 API 把所有业务方法塞进 Manager | Manager 只创建领域 Storage，CRUD 留在 `storage.session` |
| 读写对象意外共享可变端口 | 使用 `ReadStorage`/`WriteStorage` 和 Actor 独占依赖 |
| SQLite 实现细节泄漏到上层 | 在 adapter 组装边界封装 `SqliteDatabase`，contract 检查上层依赖 |
| Factory 与 Manager 长期并存 | 先 selector 切换，再删除上层 Factory 字段和闭包 |
| 为了抽象破坏事务边界 | 以高层业务方法为原子单元，禁止拆成多次上层调用 |
| 未来领域尚未存在却提前建空模块 | 只有出现真实调用方时才增加 Event/Cron/Memory Storage |

## 8. 验证命令

每个工作包完成后至少执行：

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

影响 Store、Runtime、RPC、CLI 或工具边界时，还必须检查：

```powershell
rg -n "Store::|use rusqlite|StorageFactory" crates/sagent-runtime crates/sagent-rpc crates/sagent-cli/src crates/sagent-tools -g '*.rs'
```

最终目标是：上层只看到 `StorageManager`、`Storage` 和业务 Storage，Session 操作统一通过
`storage.session`，不同持久化后端的差异完全留在 Manager 和 adapter 内部。

## 9. 与现有 R3 计划的关系

- R3.4 仍保持“Runtime/RPC/工具/CLI 已迁移到领域端口”的完成状态；
- 本计划是 R3.5 的具体执行拆分；
- 当前已存在的 `StorageManager` 和 `SqliteStorageManager` 只作为过渡骨架，需按本计划的
  `Storage`/业务 Storage 分层重新整理；
- M0–M4 完成后才开始 Runtime、RPC、CLI 和工具的长期依赖迁移；
- M8 完成后，R3.5 才能标记为完成。
