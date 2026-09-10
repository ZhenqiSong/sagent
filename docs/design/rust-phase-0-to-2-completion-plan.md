# Sagent Phase 0–2 收尾计划

状态：待执行  
范围：独立 Rust 项目 `sagent`  

## 1. 目标与边界

本计划关闭总体架构中 Phase 0（契约与测量）、Phase 1（基础层）和
Phase 2（最小可用 Agent 与 TUI）的剩余工作，使 Sagent 能以单一 Provider、单一
Profile 和有限工具集稳定运行。

Sagent 是新项目。Python Hermes 代码只能作为设计和行为灵感，不构成兼容目标、测试
oracle 或发布依赖。因此本计划明确不要求：

- 读取 `~/.hermes`、复用 Hermes SQLite schema，或保持 Python RPC 方法完全一致；
- Python/Rust 双 runner、Python worker bridge，或 `hermes.experimental_runtime` 开关；
- Desktop/Web、MCP、memory、cron、delegation、多 Provider 路由或消息 Gateway。

这些能力属于后续阶段，不能借“收尾”混入 Phase 0–2。

## 2. 当前基线

已具备：workspace、Profile/config、SQLite/FTS、SessionActor、OpenAI-compatible
Provider、流式 NDJSON RPC、`read_file`、带 approval 的 `terminal`、Ratatui TUI，以及
局部的 Store/Provider/Tool/RPC fixture。

当前本地质量门禁：

```text
cargo fmt --all -- --check
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

均应在每个工作包结束时通过。所有测试使用临时 home、临时 workspace 和本地 mock
server；不得读取开发者真实的 `SAGENT_HOME`、环境凭据或用户数据。

## 3. Phase 0：契约与测量

### P0.1 建立版本化行为契约

状态：已完成（初始 contract runner 与首批 fixture）。

**目的：** 把跨 crate、跨重构仍必须成立的外部行为变成稳定数据，而不是仅靠局部单元
测试。

**交付物：**

```text
contracts/
├── README.md                 # 格式、稳定字段和脱敏规则
├── rpc/                      # request → response/event sequence
├── transcript/               # input → normalized transcript/turn outcome
├── tools/                    # schema、permission、tool result
├── store/                    # schema migration、FTS、recovery
└── config/                   # default/profile/unknown-field resolution
```

每份 JSON fixture 必须有 `contract_version`、固定时钟/ID seed 或可忽略字段声明，以及
预期的正常化结果。允许忽略临时路径、随机 ID 和时间戳；不允许忽略错误类别、事件顺序、
消息 role、tool-call 关联、持久化结果或 capability。

首批场景：

1. `client.hello`、list/resume/create/submit/interrupt/approval/events.since；
2. 正常文本、工具调用、工具失败、approval deny/timeout、interrupt、重启恢复；
3. PromptSnapshot/system/tool schema hash 稳定与显式 transition；
4. `read_file`、`terminal`、后续 `write_file`、`session_search`；
5. 默认与具名 Profile、未知 config 字段、旧 Store schema、FTS 缺失/损坏。

**验收：** 新增一个 Rust contract runner；同一 fixture 在 CI 中执行；fixture 失败信息能
指向事件序列或字段差异，而不是比较完整日志/终端快照。

**完成记录：** 新增 workspace crate `sagent-contracts` 和根目录 `contracts/`。runner 会
递归加载并逐项比较版本化 JSON fixture；失败消息同时给出 fixture 路径、预期与实际的
正常化 JSON。首批 fixture 覆盖 RPC hello/capability、tool-call transcript 顺序、工具
schema/hash、危险命令审批分类、Profile 名称规范化以及新建 Store schema。运行命令为
`cargo run -p sagent-contracts`，其测试已纳入 `cargo test --workspace`。后续工作包新增
外部行为时，必须同时新增对应 fixture 与 runner 分支。

### P0.2 建立故障与性能基线

**目的：** 留下可比较的性能和故障证据，但不在普通 CI 中用脆弱的绝对时间断言。

**交付物：**

- `benches/` 或独立 benchmark binary，覆盖冷启动、hello 到可提交、Mock SSE 首 token、
  10k/100k message 的 list/search、interrupt、terminal timeout 和 approval timeout；
- `benchmarks/baseline.json`，记录机器/OS/Rust 版本、fixture 版本和测量分位数；
- 故障矩阵：provider EOF/429/5xx、SQLite busy/corrupt、RPC EOF/超大帧、工具 spawn
  失败、取消与迟到结果竞争。

**验收：** benchmark 可离线、可重复运行；常规 CI 只验证 benchmark/fixture 能运行和关键
指标没有明显退化，发布前再人工比较数值。

## 4. Phase 1：基础服务收尾

### P1.1 三平台持续集成

**交付物：** GitHub Actions matrix（Windows、macOS、Linux），每个平台执行 format、test、
clippy 与 contract runner。Windows 需实际覆盖 Job Object 进程树清理；POSIX 覆盖 process
group 清理。

**验收：** PR 的三平台结果可见；平台专属测试不通过伪造 OS 标识来运行。

### P1.2 Transport-neutral RPC 与 WebSocket

**目的：** 在不改变业务 handler 的前提下支持 stdio 和 WebSocket。

**实施：**

1. 抽取 transport-independent connection/dispatch/event sink 接口；
2. 保持 stdio 的单 writer 和 EOF 收口语义；
3. 添加本地 loopback WebSocket server，沿用同一 JSON-RPC envelope、hello 和 capability
   gate；
4. 为断开、慢消费者、并发 response/event、非法帧和连接隔离写黑盒测试。

**验收：** 同一组 RPC contract fixtures 可在 stdio 与 WebSocket 运行；业务 service 不导入
具体 transport 类型。

### P1.3 完成只读管理面与 Store 规模验证

**交付物：**

- `config.read` RPC，返回经验证的非秘密配置、Profile 信息和 unknown-field warnings；
  不返回 API key 或 `.env` 原文；
- 明确 `session.list`、`session.resume`、`session.events.since` 的分页、排序和上限；
- 10k/100k message fixture 下的 list/search/FTS query-plan 与性能基线。

**验收：** 两个 Profile 并发访问不串配置/数据库；读取接口无写副作用；大库查询满足
P0.2 的退化阈值。

## 5. Phase 2：最小 Agent 闭环收尾

### P2.1 实现 `write_file`

**范围：** workspace 内写入、临时 sibling 文件、flush/sync、原子 rename、明确的 overwrite
policy、大小限制、审计事件和 cancellation 清理。

**安全规则：** 复用 `WorkspaceRoot` containment；拒绝 symlink/path escape；不通过 shell
写文件；文件内容和路径脱敏后才进入模型可见结果或事件。

**验收：** 新建、覆盖拒绝/允许、父目录不存在、rename 失败、取消、并发写同一路径和
workspace 逃逸均有 fixture；失败不留下半写文件或临时文件。

### P2.2 暴露 `session_search` 工具

**范围：** 只调用 Store 的 FTS repository，不执行 shell，不加载任意文件；输入包括 query、
limit 和明确的当前 Profile/权限范围。

**验收：** CJK/emoji、空 query、FTS 缺失、结果上限、跨 Profile 隔离和取消均有测试；结果
携带稳定 session/message 引用和有界 snippet，不泄漏隐藏字段。

### P2.3 补齐 Provider 与工具回环故障矩阵

补充本地 Mock SSE/Tool fixture，至少覆盖：慢首 token、重复 delta、半包 tool-call、
tool-call 后 EOF、429/5xx、取消竞争、工具超时、approval timeout、迟到 tool/provider
result。

**验收：** 每种失败只产生一个持久化终态；取消不伪造 final assistant message；工具不会被
重复执行；delta/usage 不会进入 replay。

### P2.4 完整 TUI 黑盒 E2E

以真实 `sagent-tui` 与 `sagent-rpc` 子进程、临时 Profile 和 Mock SSE 运行以下流程：

```text
hello → list/create/resume → submit → delta → complete
                              ↘ read_file / terminal → approval → complete
                              ↘ Ctrl-C → interrupted
断线 → reconnect → resume + events.since → 一致 transcript
```

测试应验证 response 先于首个 delta、TUI 不直接访问 Store、旧 turn 的 event 被忽略、
reconnect 不重复 transcript，及 stdout 始终为完整 NDJSON。

### P2.5 三平台手工 smoke 与发布记录

每个平台执行一次受控手工验证：启动/退出、panic/启动失败后的终端恢复、Ctrl-C interrupt、
approval deny、terminal timeout 和子进程树清理。将 OS、终端、Rust 版本、命令、结果和已知
限制记录在 `docs/testing/phase-2-platform-smoke.md`。

真实 Provider smoke 保持显式 opt-in，使用专用测试凭据；不得写入默认测试或日志。

## 6. 执行顺序与提交边界

按下列顺序执行，避免把架构调整、功能和 UI 改动混入同一提交：

1. P0.1（contract runner）与 P0.2（baseline harness）；
2. P1.1（CI）与 P1.3（只读面/大库 fixture）；
3. P1.2（WebSocket transport）；
4. P2.1（write_file）；
5. P2.2（session_search）；
6. P2.3（故障矩阵）；
7. P2.4（TUI E2E）；
8. P2.5（三平台 smoke）并更新完成记录。

每个工作包单独提交，提交前运行第 2 节的质量门禁；影响 RPC、Store 或工具 schema 的改动
还必须运行 contract runner。

## 7. 关闭标准

Phase 0–2 只有同时满足下列条件才可标记完成：

- 版本化 contract fixture、故障矩阵和性能基线可离线运行；
- Windows/macOS/Linux CI 全绿；
- stdio 与 WebSocket 共用同一 RPC 业务边界；
- `write_file`、`session_search`、`read_file` 和受审批 terminal 形成有界、可取消、可审计的
  工具集；
- TUI 黑盒流程覆盖普通回合、工具/审批、中断和重连恢复；
- 三平台终端与进程树 smoke 已记录；
- 无密钥、真实用户文件、开发者 home 或 Python runtime 成为测试/生产依赖。

完成后，下一步才是 Phase 3 的 MCP、memory、cron、delegation、多 Provider 路由和
Desktop/Web 能力。
