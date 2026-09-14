# StorageManager M0：行为契约与迁移边界基线

作者：SongZQ  
日期：2026-09-13  
状态：已完成  
对应计划：[StorageManager 重构执行计划](rust-storage-manager-execution-plan.md) 的 M0

## 1. 范围与审计方法

本基线只冻结 R3.5 迁移前已经存在的持久化行为，不改变公开架构、数据库 schema 或
SessionActor 状态机。审计生产代码时使用：

```powershell
rg -n "StorageFactory|StorageDependencies|Store::open|SqliteStorage" \
  crates/sagent-rpc/src crates/sagent-runtime/src crates/sagent-cli/src crates/sagent-tools/src \
  -g '*.rs'
```

测试模块中的 `Store` 直接使用是 fixture 或事务契约验证的一部分，不属于上层生产依赖。
迁移过程中，任何新增的 Runtime、RPC、CLI 或工具生产路径都不得新增 `Store`、
`rusqlite`、SQLite 路径或 `StorageKind` 分支。

## 2. 生产调用路径映射

| 当前调用方 | 当前依赖与行为 | R3.5 目标 | 对应工作包 |
|---|---|---|---|
| `sagent-rpc/bootstrap/runtime.rs` | 保存 `Arc<dyn StorageFactory>`；启动时 `create()` 初始化 SQLite；为 Runtime、工具和 RPC Service 传播 Factory | selector 返回 `Arc<dyn StorageManager>`；bootstrap 只管理后端生命周期和初始化策略 | M4、M5 |
| `sagent-rpc/bootstrap/storage_factory.rs` | 根据 descriptor 选择 `SqliteStorageFactory` | 统一为 `create_storage_manager(descriptor)`；不支持的后端明确失败 | M4 |
| `sagent-runtime/supervisor_dependencies.rs` | 以 Factory 闭包为每个 Actor 创建 `StorageDependencies` | 持有 Manager；Actor 申请独占 `Storage` | M5 |
| `sagent-runtime/actor/session.rs` | 解构 `StorageDependencies` 为写、查询、搜索端口 | 只持有业务 `storage.session` 外观；保留 Actor 为唯一写入者 | M5 |
| `sagent-rpc/service/runtime.rs` | 为创建和查询操作保存 Factory | 分别申请 `WriteStorage`、`ReadStorage`，不识别后端 | M5、M6 |
| `sagent-runtime/worker/tool.rs` | 将 Factory 注入会话搜索工具 | 注入受限的搜索/只读业务 Storage | M6 |
| `sagent-tools/session_search.rs` | 搜索时调用 `StorageFactory::create()` 再取搜索端口 | 仅申请 `ReadStorage.session.search(...)`，绝不打开写入端口 | M6 |
| `sagent-cli/commands/storage.rs` | CLI 自己选择 `SqliteStorageFactory`，向命令提供读写依赖 | CLI Context 保存 Manager，只暴露按命令需要申请的 `Storage` 对象 | M4、M6 |
| `sagent-cli/commands/session/service/*` | Session 命令从 `CliStorageContext` 取得底层端口 | 统一使用 `storage.session` | M6 |
| `sagent-cli/commands/profile/service.rs` | Profile 创建经 CLI 存储上下文初始化 | 使用 Manager 的窄初始化/写入入口 | M6 |

`sagent-store` 内的 `Store`、`SqliteStorageManager`、`SqliteStorageFactory` 和端口适配器
是后端实现边界，M1--M3 会重组它们，但不允许向上层传播。

## 3. 已冻结的 Session 事务契约

`SessionActor` 是单个 Session 活动状态的唯一写入者。它在启动时执行恢复计划；运行中由
mailbox 串行处理 start、工具结果提交、complete 与 interrupt。M5 迁移时必须保持以下
高层原子边界，不能拆成多次上层读写调用：

| 业务行为 | 既有契约测试 | 必须保持的关系 |
|---|---|---|
| 开始 Turn | `sagent-store/src/store_tests/durability.rs::begin_turn_atomically_persists_user_message_turn_and_events` | 用户消息、running Turn、计数和事件作为一个事务落库 |
| 提交工具结果 | `sagent-store/src/store_tests/durability.rs::commit_tool_result_persists_message_and_events_atomically` | 工具结果消息与相应事件不会部分提交 |
| 完成/中断 Turn | `sagent-runtime/src/actor/tests.rs` 的完成、中断生命周期测试 | Actor 状态、持久化 Turn 状态和 runtime 事件顺序一致 |
| 启动恢复 | `sagent-runtime/src/actor_support/recovery.rs` 与 `sagent-runtime/src/actor/tests.rs` 的恢复测试 | 遗留 running Turn 会安全收口，恢复阶段不会重启工具或 Provider |
| 分支、回退与压缩 | `sagent-store/src/store_tests/branch.rs` | 审计历史保留，显示与搜索只看到允许可见的消息 |
| Profile 隔离 | `sagent-store/src/store_tests/durability.rs::event_queries_are_isolated_between_profile_databases` | 不同 Profile 的会话和事件绝不串库 |

## 4. 只读与初始化边界

以下规则是迁移的强制约束：

1. `Store::open_readonly`、`StorageFactory::create_readonly` 和未来
   `StorageManager::open_read_storage` 不创建数据库、不执行 migration、不修改数据；
   `sagent-store/src/store_tests/core.rs::opens_existing_database_in_readonly_mode` 与
   `sagent-store/src/sqlite/manager.rs::readonly_manager_path_does_not_create_missing_database`
   已覆盖底层边界。
2. CLI `session list` 等查询经 `CliStorageContext::open_read` 进入只读 Factory。新增
   `session/handler.rs::readonly_session_list_does_not_create_missing_database` 覆盖实际命令
   服务路径：缺失 `state.db` 时查询失败，但不会留下数据库文件。
3. 当前 `SessionSearchService` 名义上是只读服务，却调用 `StorageFactory::create()`。
   这不是可接受的最终实现：它在 M6 必须改为只获取搜索或只读 Storage，并追加同类
   “缺失数据库不创建文件”的端到端契约测试。
4. `RuntimeBootstrap::from_paths` 在 daemon 启动阶段主动调用 `StorageFactory::create()`，
   用于初始化 migration 和连接检查。因此“启动 RPC daemon 不创建数据库”不是当前契约，
   也不能作为 M0 回归测试。M4 必须将初始化策略明确成 Manager 的生命周期入口；M5 迁移
   时不得把普通 RPC 查询误接到该写入初始化路径。

## 5. 新旧对象最小契约

| 当前对象 | 迁移后的对象 | 不可改变的行为 |
|---|---|---|
| `StorageFactory::create()` | `StorageManager::open_actor_storage()` / `open_write_storage()` | 返回独立的写入能力；不得共享 Actor 的可变写入句柄 |
| `StorageFactory::create_readonly()` | `StorageManager::open_read_storage()` | 只读、无 migration、缺失库不创建文件 |
| `StorageDependencies` | `Storage` | 上层通过 `storage.session` 执行 Session 领域动作；端口组合留在 adapter |
| `StorageReadDependencies` | `ReadStorage` | 只暴露读取和搜索，编译期不提供写入方法 |
| 独立 `SearchStorage` | `ReadStorage.session` 或受限搜索对象 | 搜索不能借由能力对象获得写入权限 |
| `SqliteStorageManager` | `SqliteStorageManager`（重组后） | SQLite 文件、连接和 migration 细节仍只在 adapter 内部 |

## 6. M1 的进入条件

M0 结束时，迁移边界和现有行为已经有可追溯的基线。M1 只能新增 `Storage`、
`ReadStorage`、`WriteStorage` 及 Session 领域外观；不得迁移 Runtime、RPC、CLI 或工具
调用方，也不得改变上述事务和只读规则。
