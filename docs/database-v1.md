# Sagent SQLite 数据库 v1

Phase 1 使用全新的 Sagent SQLite schema，不兼容旧 Hermes `state.db`。数据库初始化由
`sagent-session::DatabaseConnection` 完成，Repository 和 Runtime 不直接复制初始化逻辑。

## 路径和生命周期

- `database.path` 为绝对路径时直接使用。
- `database.path` 为相对路径时由 `sagent-config` 解析为相对于 `SAGENT_HOME` 的路径。
- `database.path: null` 使用 `<SAGENT_HOME>/state.db`。
- 首次打开会创建父目录和数据库文件；纯路径查询不会创建这些资源。
- 初始化顺序为：创建父目录、打开文件、设置 PRAGMA、执行 migration、校验 schema，全部成功后
  才允许 Runtime 接收 Session 请求。

## Schema

schema version 使用 `schema_meta` 表中的 `current_version` key，当前版本为 `2`。

- migration `0001_initial` 创建 `schema_meta`、`sessions` 和 `messages`。
- migration `0002_indexes` 创建 Session 更新时间和 Message 顺序索引。
- migration 内容不可变，版本号必须严格递增。
- 所有待执行 migration 和版本更新在同一个 `BEGIN IMMEDIATE` 事务中完成。
- migration 失败会 rollback SQL 和版本更新，不忽略错误继续启动。
- 数据库存在用户表但缺少 Sagent `schema_meta`，或版本高于当前支持版本时，返回
  `DatabaseSchemaUnsupported` 语义错误且不修改旧数据库。

## 关键表

`messages.session_id` 使用 SQLite foreign key 引用 `sessions(id)`，并以
`UNIQUE(session_id, sequence)` 保证 Session 内顺序不重复。`message_count` 和 `revision` 是
Session 持久化投影，后续 Repository 必须在同一写事务中更新。

## PRAGMA

每个连接初始化时设置并测试实际值：

- `foreign_keys = ON`
- `journal_mode = WAL`
- `busy_timeout` 使用配置的 milliseconds，默认 `5000`
- `synchronous` 默认 `FULL`，支持 `NORMAL` 和显式 `OFF`

WAL 或 schema 校验不满足要求时初始化失败，不降级为未声明的数据库行为。

## 失败和恢复

Step 3 已使用真实 SQLite 文件测试首次创建、重复打开、foreign key、PRAGMA、损坏 migration
rollback、旧数据库拒绝和 future schema 拒绝。Repository 的事务性消息写入、并发 append 和
Session 生命周期留到 Step 4。
