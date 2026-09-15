# Sagent Rust 代码规范

作者：SongZQ  
适用范围：`sagent` 全部 Rust crate、生产代码、测试和文档  
关联计划：[StorageManager 重构执行计划](archive/rust-storage-manager-execution-plan.md)

## 1. 总原则

### 1.1 单一职责优先

每个 crate、模块、struct、enum、trait、service 和函数都必须能用一句话说明主要职责，
并且只因为同一类业务变化而修改。

一个对象不得同时承担以下多项职责：

- 配置读取和业务决策；
- 状态管理和数据库事务；
- 数据库访问和 CLI/RPC 输出；
- Provider 创建和 Turn 编排；
- 工具授权和进程执行；
- 连接生命周期和具体业务 CRUD。

当一个对象同时出现两项以上职责时，应先按变化原因拆分，再继续增加功能。

### 1.2 数据、编排和副作用分离

代码按以下顺序组织：

```text
领域类型/DTO → 业务 Service/Actor → 领域 Storage → 后端 Adapter → 外部资源
```

- 领域类型只表达数据、不变量和稳定错误；
- Service/Actor 负责业务决策、状态推进和流程编排；
- Storage 负责持久化操作和事务；
- Adapter 负责 SQLite、PostgreSQL、HTTP、文件或进程等具体 I/O；
- CLI/RPC/TUI 只负责参数、协议和展示适配。

入口层不得堆积业务规则，领域对象不得直接打开数据库、读取配置或操作终端，除非它的
唯一职责就是该基础设施适配。

### 1.3 先满足真实变化点，再引入抽象

trait、泛型、注册表和 Manager 只有在存在明确调用方、替换目标或已知扩展需求时引入。
不得为了“未来可能需要”创建空接口或无消费者的扩展点。抽象必须说明：

1. 当前调用方是谁；
2. 要替换的实现是什么；
3. 两种实现必须保持什么行为契约。

## 2. 分层和依赖方向

推荐依赖方向：

```text
sagent-config  →  descriptors
sagent-store   →  domain storage ports + adapters
sagent-runtime →  services/actors
sagent-rpc     →  protocol adapters/bootstrap
sagent-cli     →  commands/handlers
sagent-tui     →  protocol/types
```

低层 crate 不得反向依赖 CLI、RPC、TUI 或具体 Runtime。协议层只能包含 DTO、方法定义、
稳定错误语义和 service trait，不得依赖 SQLite、`Store` 或 CLI。

## 3. 持久化代码规范

### 3.1 三层 Storage 模型

持久化必须区分三个概念：

```text
StorageManager  →  管理 Profile 级后端资源和生命周期
Storage         →  对外暴露的抽象业务存储对象
SessionStorage  →  负责 Session 领域的业务操作（可跨多个表）、查询和事务
```

`StorageManager` 只负责：

- 保存已验证的 `StorageDescriptor` 或后端资源句柄；
- 连接、连接池、文件句柄的生命周期；
- 健康检查和 migration；
- 创建 Actor、只读、写入和搜索所需的抽象 Storage；
- 将后端错误映射为稳定的 `StorageError`。

`StorageManager` 不得负责 Session/Turn 业务判断、CLI/RPC 参数校验或业务 CRUD。

### 3.2 业务域通过 Storage 字段访问

业务操作按领域组织，不按数据库表组织：

```rust
let mut storage = manager.open_actor_storage()?;
storage.session.start_turn(...)?;
storage.session.complete_turn(...)?;
storage.session.search(...)?;
```

涉及 Session 的操作必须通过 `storage.session`，不得在上层分别保存 query、search、
SQLite Store 或数据库连接。

未来只有在出现真实业务调用方后，才增加 `storage.event`、`storage.cron`、`storage.memory`
等领域字段。不得提前创建空的万能 Storage。

### 3.3 领域 Storage 与表 Model 分离

`Storage` 和具体表 Model 必须处于不同层次。`Storage` 表达业务领域能力，不表达数据库
表结构；一个业务 Storage 可以跨越多个表，只要这些表属于同一领域的一致性边界。

以 Session 为例：

```text
SessionStorage                  ← Session 领域业务外观
├── sessions                    ← 会话状态
├── messages                    ← 消息历史
├── turns                       ← Turn 生命周期
├── generations                 ← Generation 快照
└── daemon_events               ← Session 审计/恢复事件
```

必须遵守以下边界：

- `SessionStorage` 负责 Session 领域的业务操作、查询和事务边界，不对应单一数据库表；
- `Storage` 只聚合 `session`、`memory`、`cron` 等真实业务领域对象，不直接聚合表对象；
- `SessionRow`、`TurnRow`、`MessageRow`、`EventRow` 等表模型只能位于具体持久化 Adapter
  （如 `sagent-store` 的 SQLite/PG 实现）内部，默认使用私有或 `pub(crate)` 可见性；
- Row Model 只表达列、外键、索引和数据库编码，不得被 Runtime、RPC、CLI、工具或协议层
  引用；
- 领域模型和端口 DTO 只表达业务含义、不变量和稳定错误，不携带 SQL、连接、表名或
  数据库专属字段；
- Adapter 负责 Row Model ↔ 领域模型的转换、SQL 和连接细节；业务 Storage 不直接依赖
  `rusqlite::Row`、`Connection` 或 PostgreSQL 行类型。

当一次业务操作需要更新多个表时，必须由对应领域 Storage 提供一个有业务含义的高层
原子方法，例如 `start_turn`、`complete_turn` 或 `rewind_to_message`。Service/Actor 负责
校验和编排，不能将多个表更新拆成多次 CRUD 调用；顶层 `Storage` 和 `StorageManager`
不能因为“方便调用”而承载这些表操作。

如果操作跨越多个业务领域且确实要求原子提交，应定义明确的跨领域业务操作或应用级
事务边界；禁止引入暴露数据库连接的通用事务闭包，也禁止把所有领域方法塞进万能
`Storage`。只有出现真实一致性需求时才建立该边界，并配套行为契约测试。

当前迁移期间，`NewSession`、`NewMessage`、`StoredMessage` 等可作为端口输入/输出 DTO
暂留在 `sagent-store`；新增的数据库列映射类型不得沿用这些 DTO 的名义泄漏到上层。
M3 整理 SQLite adapter 时，应优先将表模型收回 adapter，并保持领域 Storage 方法和
错误语义不变。

### 3.4 读写权限最小化

不同调用边界使用不同能力对象：

```rust
pub struct Storage { /* Actor 的完整 Session 能力 */ }
pub struct ReadStorage { /* 只有只读业务能力 */ }
pub struct WriteStorage { /* 只有短操作写入能力 */ }
```

- Actor 获得独占写入能力和配套读取能力；
- RPC 查询、事件补读和列表命令只能获得 `ReadStorage`；
- 短写命令只能获得 `WriteStorage`；
- 搜索工具不能因为查询而获得写入能力；
- 每个 Actor 的可变写入端口必须独占，不能在 Actor 之间共享。

### 3.5 事务和状态不变量

跨表业务操作必须由一个高层 Storage 方法表达，不得在上层拆成多次写调用：

- `start_turn`；
- `commit_tool_result`；
- `complete_turn`；
- `interrupt_turn`；
- `rewind`/`restore`。

Storage 成功提交后才允许发布对应事件。worker、Provider 和工具执行器不得绕过 Actor
直接写 Storage 或发布最终状态事件。

### 3.6 后端隔离

只有后端 Adapter 可以依赖以下类型：

- `rusqlite::Connection`；
- `Store`；
- SQLite 路径；
- PostgreSQL 连接池；
- SQL 字符串和 migration 细节。

Runtime、RPC、CLI、工具和协议层不得出现 `Store::open_*`、SQL 或 `match StorageKind`。
SQLite 和 PostgreSQL 对外必须提供相同的领域 Storage 方法和错误语义。

## 4. 配置和 Bootstrap 规范

配置层只负责：

- 读取一次配置文件；
- 反序列化和字段校验；
- 组合不可变的 `ProfileConfig`、`StorageDescriptor`、`ProviderDescriptor` 和策略描述符。

配置层不得：

- 打开数据库；
- 创建 Store、连接池或 HTTP client；
- 读取并传播 API key；
- 启动后台任务；
- 根据配置执行业务操作。

Bootstrap/Selector 负责把 descriptor 转换为具体 Manager：

```text
config.yaml → ProfileConfig → StorageDescriptor → selector → StorageManager
```

同一个 Profile 只创建一个长期 `StorageManager`。配置不得在 Service、Handler、Actor 或
每次查询中重复读取。

后端构造只允许位于 bootstrap/selector 和具体 Manager adapter 边界；业务对象不得长期持有
或传播数据库构造器。

## 5. Service、Handler 和 Actor 规范

### 5.1 Service

Service 负责一个业务领域的决策和编排，不负责 CLI 展示、协议解析或后端选择。

Service 应通过构造函数接收完整依赖：

```rust
pub struct SessionService {
    storage: Arc<dyn StorageManager>,
}
```

Service 不得在方法内部重新读取配置或创建 Factory。方法名必须表达业务意图，例如
`create_session`、`complete_turn`、`search_messages`，不得使用含义模糊的 `process`、
`handle` 或 `run` 掩盖多种职责。

### 5.2 Handler

Handler 只负责：

1. 接收解析后的命令；
2. 做 CLI 输入校验；
3. 调用对应 Service；
4. 将领域结果转换为文本或 JSON。

Handler 不得直接打开数据库、访问 SQLite、执行 SQL 或保存待执行的命令值。命令通过
`execute(command)` 显式传入；上下文由 Handler 持有的 Service 统一管理。

### 5.3 Actor 和异步任务

- Actor 是 Session 活动状态的唯一写入者；
- worker 只能投递事实，不能直接写 Storage；
- 每个 `tokio::spawn` 必须有明确所有者、取消来源、等待/join 或 abort 策略；
- 取消必须先传播 `CancellationToken`，再收口任务和持久化终态；
- 不得在业务对象中隐藏不可取消的后台任务。

## 6. 模块、文件和命名规范

### 6.1 模块职责

每个模块只处理一个主题。对外 facade 只保留稳定入口和组合逻辑，具体实现进入按主题
命名的 sibling/module：

```text
session/
├── mod.rs       # 模块声明和稳定导出
├── command.rs   # 命令协议
├── handler.rs   # CLI 参数校验和输出适配
└── service/     # Session 领域业务操作
```

`mod.rs` 不得重新聚合大量业务实现，也不得通过无意义 re-export 掩盖循环依赖。

### 6.2 文件和函数规模

- 生产 Rust 文件目标：200–450 行；
- 超过 500 行必须进行职责审查；
- 超过 700 行必须拆分为同主题子模块或 facade + siblings；
- 普通函数目标不超过 60 行；
- 状态机、事务和 I/O 编排函数目标不超过 100 行；
- 超过 120 行必须提取具名的校验、决策、准备、执行或收尾步骤；
- 测试文件目标不超过 600 行，重复 fixture 应抽成透明的领域 harness。

规模是设计审查信号，不得为缩短函数破坏事务原子性、Actor 串行性或资源清理边界。

### 6.3 命名

- 类型名体现领域概念：`StorageManager`、`SessionStorage`、`GenerationResolution`；
- 方法名体现业务意图：`open_read_storage`、`commit_tool_result`；
- `Context` 只表示共享调用上下文，不得成为万能 Service；
- 不使用 `Manager`、`Service`、`Handler` 作为没有领域限定的万能容器；
- 不使用按字符串写的四分支以上 `if/else`，使用表驱动或 handler registry。

## 7. 错误、注释和安全规范

### 7.1 错误

- 跨 crate 和 RPC 边界使用稳定领域错误类别；
- 内部错误使用 `anyhow` 添加上下文，但不泄漏 SQLite、HTTP、路径或凭据细节；
- 禁止用 `unwrap`/`expect` 掩盖可恢复的运行时错误；
- 不得使用 `try/except: pass` 等静默吞错模式；
- 未支持的后端必须明确失败，不能静默回退到 SQLite。

### 7.2 Rustdoc 和注释

新增或修改公共 `struct`、`enum`、`trait`、函数、字段和枚举变体时，必须同步补充准确
的中文 `///`，说明用途、生命周期、取值范围和错误语义。

关键逻辑的注释优先说明：

- 为什么采用该边界；
- 状态转换和合法顺序；
- 事务、幂等、回滚和可见性规则；
- 并发、取消和资源释放原因；
- Profile 隔离、配置覆盖和后端替换约束。

注释不得机械复述代码，不得保留过时说明，不得包含 API key、令牌或真实敏感数据。

## 8. 测试规范

### 8.1 测试行为而不是源码形状

测试必须验证行为契约，不得读取 `.rs`、`.ts` 或其它源文件文本来断言实现结构。
不要测试硬编码枚举数量、模型快照或当前版本常量；应测试数据之间的关系和不变量。

### 8.2 持久化测试

至少覆盖：

- SQLite 和 fake/recording Storage 都能实现同一领域接口；
- Actor 写入端口独占；
- 只读路径不创建数据库、不执行 migration；
- `start/commit/complete/interrupt` 的原子调用顺序；
- Session/Profile 隔离；
- 搜索、恢复、归档和可见性规则；
- 未支持后端的 fail-closed 错误。

测试必须使用临时 `home`、Profile 和数据库，不能写入用户真实目录。

### 8.3 测试可读性

测试名称要表达可观察行为，例如：

```text
interrupt_persists_once_after_tool_cancellation
readonly_storage_does_not_create_missing_database
session_search_hides_rewound_messages
```

重复的目录、Store、Provider、RPC 启动过程应抽成小型 fixture，但 fixture 不得隐藏业务
默认值、权限或时间语义。

## 9. Review 检查清单

提交前逐项检查：

1. 每个新增对象是否只有一个主要职责？
2. Manager 是否只管理资源，没有承载业务 CRUD？
3. Session 操作是否统一通过 `storage.session`？
4. Storage 是否按业务领域组织，而不是按数据库表组织？
5. 表 Row Model 是否只存在于具体 Adapter，未泄漏到上层？
6. 上层是否完全隐藏 `Store`、SQL、数据库路径和连接池？
7. 是否只在 bootstrap/selector 读取配置和创建 Manager？
8. 读写权限是否按最小能力分离？
9. 跨表操作是否仍是一个高层原子 Storage 方法？
10. 是否删除了重复的后端构造器、配置读取和自由函数路径？
11. 文件、函数、测试文件是否超过规模阈值？
12. 新增 public item 是否有中文 Rustdoc？
13. 测试是否验证行为契约而非源码形状？
14. 是否运行格式化、工作区测试、Clippy 和 diff 检查？

## 10. 必跑命令

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

影响存储边界时，额外检查上层是否仍依赖具体实现：

```powershell
rg -n "Store::|use rusqlite|StorageFactory" crates/sagent-runtime/src crates/sagent-rpc/src crates/sagent-cli/src crates/sagent-tools/src -g '*.rs'
```
