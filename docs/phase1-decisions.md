# Phase 1 数据模型决策

本文记录 Step 2 的 Session 和 Message 数据模型决策。模型是 Sagent 新定义的持久化契约，
不兼容旧 Hermes SQLite schema 或 Python 对象结构。

## Session

`Session` 包含服务端生成的 `SessionId`、来源、标题、创建/更新时间、生命周期状态、可选 cwd、
metadata、已提交消息数量和 revision。

`SessionStatus` 只允许 `active`、`closed`、`recovering`。它描述持久化生命周期，不描述 Agent
执行状态；Phase 1 不加入 `thinking`、`waiting_for_tool` 或 `compressing`。

`message_count` 要求与已提交消息数量严格一致。`revision` 只在对应数据库事务成功后递增，
`after_message_commit` 和 `after_close_commit` 仅用于构造成功提交后的内存投影。

## Message

Message 使用现有 Phase 0 wire 字段 `message_id`，不在此阶段将其改名为 `id`，以避免破坏已经
发布的 Message fixture 和公共协议字段。Phase 1 新增：

- `session_id`：所属 Session，反序列化时必填。
- `sequence`：Session 内插入顺序，从 1 开始，不依赖 wall-clock timestamp。
- `metadata`：JSON object 扩展字段，默认空对象，不能用于运行时控制或 secret。

`role`、`content`、工具调用和工具调用关联字段继续复用 Phase 0 的 `sagent-types` 类型。删除
和修改消息不属于 Phase 1 能力，不实现 soft-delete。

## 校验边界

- ID newtype 保持相互不可赋值，`SessionId`、`MessageId` 和 `ToolCallId` 不做字符串别名。
- serde 负责必填字段、类型和枚举值校验。
- `Session::validate` 和 `Message::validate` 负责空 ID、空时间和 sequence 边界等持久化不变量。
- Message 必须引用存在的 Session，这一外键关系由后续 SQLite Repository 保证，类型层只保存
  `session_id`。
- 时间戳在 wire 模型中保持 RFC 3339 UTC 字符串；数据库存储转换留给 Step 3/4。
