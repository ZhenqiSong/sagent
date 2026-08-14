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

## Repository 事务

`sagent-session::Repository` 持有已初始化的数据库连接，只暴露 typed Session/Message API，不
暴露 SQLite connection 或 SQL。其写入边界如下：

- `create_session` 在单事务中写入 Session 和初始 metadata。
- `append_message` 使用 `BEGIN IMMEDIATE`，校验 Session 状态，按 `MAX(sequence) + 1` 分配顺序，
  插入 Message，并在同一事务中更新 `message_count`、`updated_at` 和 `revision`。
- `close_session` 在单事务中更新状态、时间和 revision；重复关闭幂等。
- 任一步失败都会 rollback；成功 commit 前不会返回成功 Message 或更新投影。
- Session list 按 `updated_at DESC, id ASC` 稳定排序；Message 按 `sequence ASC` 排序，所有读取
  都有上限。
- `resume_session` 校验 message_count、最后 sequence 和实际消息窗口的一致性；不一致时 fail
  closed，不静默截断历史。
- 多个 Repository 实例可以通过 SQLite WAL 和 busy timeout 并发写入；同一 Session 的 sequence
  由数据库事务分配，不会重复或丢失。

## 失败和恢复

Step 3 已使用真实 SQLite 文件测试首次创建、重复打开、foreign key、PRAGMA、损坏 migration
rollback、旧数据库拒绝和 future schema 拒绝。Step 4 已补充 Repository 的事务性消息写入、并发
append 和 Session 生命周期测试；Step 5 的 Actor 在 commit 后更新快照并发布 live event。
