# Sagent Rust 第二阶段：可审计的会话生命周期 CLI

状态：实施计划  
作者：SongZQ

## 1. 阶段目标

第一阶段已经建立了 workspace、profile 隔离、SQLite/FTS5、会话读取与基础 CLI。本阶段把
已经存在于 `sagent-store` 的**安全写能力**以可审计、可脚本化的方式暴露给 CLI。

完成后，用户可以不启动模型、不启动 TUI，就能管理本地会话的标题、状态和消息分支：

```text
sagent session list [--include-archived]
sagent session show <SESSION_ID>
sagent session rename <SESSION_ID> <TITLE>
sagent session archive <SESSION_ID>
sagent session unarchive <SESSION_ID>
sagent session finish <SESSION_ID> --reason <REASON>
sagent session rewind <SESSION_ID> <MESSAGE_ID>
sagent session restore <SESSION_ID> <MESSAGE_ID>
```

本阶段的核心原则是：**所有修改必须可解释、可验证，并且不破坏消息审计历史。**

## 2. 明确不在本阶段做的事情

- 不调用模型 API，不实现 provider、Agent loop 或 tool execution；
- 不实现 TUI、JSON-RPC、gateway 或多设备同步；
- 不提供 `session retry`：它需要模型重新生成回复，应留到 Agent 阶段；
- 不提供 `session compact`：压缩摘要需要可信的摘要生成策略，应留到 Agent/上下文阶段；
- 不删除 sessions/messages；归档、隐藏、回退都必须保持审计数据；
- 不修改现有 profile/config 格式，也不读取任何 API key。

## 3. 现有实现与 Python 行为参考

第二阶段应以 Sagent 已有 Store 行为为唯一业务来源，不重新发明 SQL。

| 能力 | 现有 Rust 文件 | 关键接口 |
|---|---|---|
| 命令分组与根分发 | `crates/sagent-cli/src/commands/mod.rs` | `Command::execute` |
| session CLI | `crates/sagent-cli/src/commands/session.rs` | `SessionCommand::execute` |
| JSON/text 输出 | `crates/sagent-cli/src/output.rs` | `print_output` |
| 会话读取 | `crates/sagent-store/src/session.rs` | `list_sessions`、`get_session` |
| 消息读取 | `crates/sagent-store/src/message.rs` | `get_messages_for_display` |
| 写事务 | `crates/sagent-store/src/write.rs` | `update_session_title`、`finish_session`、`set_session_archived`、`rewind_to_message`、`restore_rewound` |
| 现有事务测试 | `crates/sagent-store/src/lib.rs` | archive、rewind、retry、compression 测试 |

### 3.1 必须阅读的 Python 实现

Sagent 不复刻 Hermes 的命令名称、数据库结构或 ID 格式，但本阶段的状态转换必须参考下列
Python 实现，避免把复杂的会话语义错误地简化成普通 SQL 更新：

| Sagent 功能 | Python 参考 | 必须继承的行为 |
|---|---|---|
| 创建 session | `hermes_state.py:create_session()`（约 5770 行） | 创建与写入分离；创建本身不隐式产生消息 |
| archive/unarchive | `hermes_state.py:set_session_archived()`（约 9361 行） | 归档是软隐藏，不删除 transcript；compression parent/child 存在时必须整体处理 lineage |
| rewind 基础事务 | `hermes_state.py:rewind_to_message()`（约 12379 行） | 目标 user 行及后续活动行一起变 inactive；计数在同一事务重算；`rewind_count` 每次操作递增 |
| 恢复回退 | `hermes_state.py:restore_rewound()`（约 12553 行） | 只重新激活指定 message id 之后的 inactive 行；它是 undo 基础设施，不是任意历史合并 |
| `/retry` | `cli.py:retry_last()`（约 10280 行） | 重试先持久化回退，再改内存历史；只针对真正的用户回合，不能误处理 timeline/压缩 handoff 行 |
| `/undo` | `cli.py:undo_last()`（约 10342 行） | 按用户回合而非按物理消息回退；回退后将用户文本预填到输入框 |
| compression | `hermes_state.py:archive_and_compact()`（约 11194 行） | 压缩保留软归档历史，并用 watermark 防止并发追加丢失消息 |

### 3.2 Sagent 与 Python 的阶段性差异

当前 Sagent 的 schema 尚未实现 Hermes 的 compression lineage、跨进程 turn lease、活动消息
快照或复合 handoff carrier。因此第二阶段不能声称完全兼容 Python；应采用以下边界：

1. archive/unarchive 先只处理 Sagent 当前的单一 session 行；引入 `parent_session_id` 与压缩
   continuation 后，必须升级为 Python `set_session_archived()` 的递归 lineage 语义；
2. `session rewind` 只接收物理 `MESSAGE_ID`，这是 Python `rewind_to_message()` 的低层基础；
   用户友好的按回合 `/undo N` 延后到拥有消息投影和输入预填的 TUI/Agent 阶段；
3. `session restore` 必须比 Python 的低层 `restore_rewound()` 更保守：若回退后已经有新的活动
   分支，则拒绝恢复，避免无提示地合并两条分支；
4. retry 与 compact 继续排除在第二阶段之外，因为它们分别依赖 Agent 重发与摘要模型调用。

## 4. 命令与数据契约

### 4.1 全局参数

继续沿用第一阶段的参数：

```text
--home <ABSOLUTE_PATH>
--profile <NAME>
--format text|json
```

所有修改命令的 JSON stdout 必须是单个对象；成功时 stderr 为空。建议统一返回：

```json
{
  "operation": "archive",
  "session_id": "20260831_...",
  "changed": true,
  "updated_at": "2026-08-31T12:00:00.000Z"
}
```

`changed: false` 表示目标会话不存在或状态本来就相同；具体语义应在每个命令中固定，并以
集成测试锁定，不能由调用方猜测。

### 4.2 `session rename`

```text
sagent session rename <SESSION_ID> <TITLE>
```

- 使用 `Store::update_session_title`；
- CLI 层拒绝空白标题，限制最大 UTF-8 字节数（建议 256）；
- 更新 `updated_at`，不改消息、`started_at`、`ended_at`；
- JSON 返回新标题与 `changed`。

### 4.3 `session archive` / `unarchive`

```text
sagent session archive <SESSION_ID>
sagent session unarchive <SESSION_ID>
```

- 使用 `Store::set_session_archived(session_id, true/false, now)`；
- 归档只影响默认 `session list` 的可见性，`session show <ID>` 仍可精确读取；
- 增加 `session list --include-archived`，避免归档后用户无法发现或恢复会话；
- 不使用 `hidden` 承担归档语义，两者应保持分离。
- 当前仅更新指定 session；将来存在 compression lineage 时，必须升级为 Python
  `set_session_archived()` 的 ancestor/descendant 整体归档，不能只改当前 tip。

### 4.4 `session finish`

```text
sagent session finish <SESSION_ID> --reason <REASON>
```

- 使用 `Store::finish_session`；
- `reason` 必填、去除首尾空白后不得为空；
- 仅写 `ended_at`、`end_reason`、`updated_at`；
- 完成会话不等于归档，仍保持当前可见性。

### 4.5 `session rewind`

```text
sagent session rewind <SESSION_ID> <MESSAGE_ID>
```

- `<MESSAGE_ID>` 必须解析为 `i64`，再构造 `MessageId`；不可接受负数或零；
- 只允许目标为该 session 的 `user` 消息；
- 直接调用 `Store::rewind_to_message`，由 Store 事务内完成软删除、`message_count` 更新和
  `rewind_count + 1`；
- JSON 返回 `rewound_count`、`new_head_id`、`target_message_id`；
- 命令绝不能尝试自行 UPDATE messages，避免绕过 Store 的事务边界。

### 4.6 `session restore`

`Store::restore_rewound` 目前接受进程内的 `RewindCheckpoint`，而 CLI 的每次调用都是新进程，
不能直接把 checkpoint 放在内存中。因此第二阶段应先增加一个明确的 Store 门面：

```rust
pub fn restore_rewound_from(
    &mut self,
    session_id: &SessionId,
    target_message_id: MessageId,
    updated_at: &str,
) -> Result<u64>;
```

该方法必须在同一个事务中：

1. 读取当前活动消息头；
2. 验证从 `target_message_id` 开始存在可恢复的 inactive 消息；
3. 验证回退后的活动头没有被新的消息推进；
4. 恢复目标及后续 inactive 消息；
5. 重新计算 `message_count` 并提交。

不能以“把 checkpoint 序列化到临时文件”的方式实现，这会引入过期文件、跨 profile 混用和
崩溃恢复不一致的问题。它以 Python `restore_rewound(session_id, since_message_id)` 的物理
消息范围为基础，但额外加入 Sagent 的分支安全校验。

## 5. 实施步骤

### 步骤 2.1：建立 CLI 运行上下文和错误类别

目标：避免每个新增命令继续携带 `home/profile/format` 三个参数，并统一退出状态。

1. 在 `sagent-cli/src/commands/mod.rs` 新增 `CommandContext`：
   `home: Option<PathBuf>`、`profile: Option<ProfileName>`、`format: OutputFormat`；
2. `main.rs` 只构造一次 Context 并调用 `command.execute(&context)`；
3. 将 `anyhow` 边界错误映射为 CLI 错误类别：
   - `2`：参数/输入错误；
   - `3`：profile 或 state.db 不存在；
   - `4`：数据库、schema、事务错误；
   - `1`：未分类内部错误；
4. stdout 始终只承载成功结果；错误诊断只写 stderr。

验收：新增 CLI 集成测试覆盖错误类别与 stdout/stderr 分离。

### 步骤 2.2：完善读取侧可见性

目标：归档命令落地前，先让用户能找到归档的 session。

1. 在 `sagent-store/src/session.rs` 为 list 查询增加 `SessionListQuery`，包含
   `include_archived`、`include_hidden`、`limit`、`offset`；
2. 保留当前 `list_sessions(limit, offset)` 作为默认可见性包装，避免破坏第一阶段调用方；
3. 在 `commands/session.rs` 为 `List` 新增 `--include-archived`；
4. 为默认列表、归档列表、精确 show 建立 Store 与 CLI 集成测试。

### 步骤 2.3：实现 rename、archive、unarchive、finish

目标：先落地无消息分支风险的生命周期操作。

1. 在 `SessionCommand` 增加四个变体；
2. 增加 `current_writable_store` 私有辅助函数，复用 profile 路径解析并使用
   `Store::open_readwrite`；
3. 在一个 `now_rfc3339()` 辅助函数中生成 UTC 毫秒时间，禁止各命令各自格式化时间；
4. 每个命令只调用一个 `Store` 写接口，并输出固定 JSON DTO；
5. 先测试不存在 ID、重复 archive/unarchive、空 reason/标题、profile 隔离，再测试成功路径。

### 步骤 2.4：实现 rewind

目标：将 Python `rewind_to_message()` 的低层、物理消息回退能力作为显式、有审计记录的 CLI
操作；这不是 `/undo N` 的替代品。

1. 在 `sagent-types` 或 `sagent-cli` 增加严格的 `parse_message_id`；
2. 先补齐 Python 的关键前置校验：目标必须仍是 active 的 `user` 消息；事务内重新读取活动
   消息头，防止 CLI 读取消息后被另一个写进程改变；
3. `session rewind` 调用 Store，输出 `RewindResult` 的稳定 DTO，避免直接暴露未来可能改变的
   内部 checkpoint；
4. 集成测试必须确认：
   - 目标 user 消息和后续消息不再默认显示；
   - `rewind_count` 增加；
   - 审计查询仍可看到软删除消息；
   - 错误目标不会写入任何数据。

### 步骤 2.5：实现跨进程 restore

目标：提供安全撤销回退，而不是不受约束地重新激活历史。

1. 先为 `restore_rewound_from` 添加 Store 单元测试，并对照 Python
   `restore_rewound(session_id, since_message_id)` 的范围语义；
2. 新活动消息出现时必须拒绝 restore；
3. `session restore` 只接收 session/message ID，不接受 `--force`；
4. JSON 返回 `restored_count` 和恢复后的活动消息头；
5. CLI 集成测试用两个独立进程分别执行 rewind 与 restore，证明它不依赖内存 checkpoint。

### 步骤 2.6：补全 fixture 与发布验证

1. 在 `crates/sagent-store/tests/fixtures/` 保存最小脱敏 SQLite fixture：
   空库、v1 schema、含 FTS5、含中文/emoji、损坏文件；
2. 锁库测试在 Windows 和 POSIX 行为不同，必须采用平台条件测试或 CI 适配，不能假设一个
   进程锁策略通用；
3. GitHub Actions 添加 Windows、macOS、Linux matrix，执行：
   `cargo fmt --check`、`cargo clippy -- -D warnings`、`cargo test --workspace`；
4. 在 CI 保存失败时的测试输出，不上传真实 state.db。

## 6. 测试矩阵

| 层级 | 覆盖内容 |
|---|---|
| `sagent-types` | ID 解析、写操作 JSON DTO 序列化 |
| `sagent-store` | 每个写操作的事务性、无副作用失败、可见性、restore 分支保护 |
| `sagent-cli` 单元测试 | Clap 参数、参数边界、Context 构造 |
| `sagent-cli` 集成测试 | 真实二进制、JSON stdout、stderr、退出码、profile 隔离、跨进程 rewind/restore |
| CI | Windows/macOS/Linux、fixture、fmt/clippy/test |

所有测试使用系统临时目录或仓库 fixture，禁止访问开发者的 `SAGENT_HOME`。

## 7. 完成定义

第二阶段只有在以下条件全部满足时完成：

1. rename/archive/unarchive/finish/rewind/restore 均可通过 CLI 执行；
2. 每个成功修改返回稳定 JSON，stdout 不混入诊断；
3. 每个失败写操作都不会留下部分状态；
4. 归档不删除数据，默认列表隐藏但 `--include-archived` 可发现；
5. rewind/restore 跨 CLI 进程可用，并正确拒绝已有新分支的 restore；
6. JSON、文本输出、错误退出码有集成测试；
7. CJK/emoji、损坏数据库、旧 schema fixture 均有测试；
8. Windows、macOS、Linux CI 通过 fmt、clippy、test；
9. 无模型 API、TUI 或 gateway 依赖被引入。

## 8. 第二阶段后的下一步

第三阶段再引入 `sagent-protocol`：先做本地 JSON-RPC 的只读 `session.list` / `session.resume`，
再将生命周期命令映射到 RPC。只有协议与 session service 稳定后，才开始 SessionActor、
provider 和 Ratatui TUI。
