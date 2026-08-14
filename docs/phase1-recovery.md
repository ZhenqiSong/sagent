# Phase 1 Runtime Recovery

Phase 1 的 Session Actor 只恢复 SQLite 中已经提交的 Session 和 Message。Actor 快照在
Repository 写事务成功后更新；失败事务不会发布成功事件，也不会改变内存快照。

## Session Actor

- 每个 Actor 拥有一个有界 command mailbox，同一 Session 的命令按 mailbox 顺序执行。
- SQLite 同步操作在 Tokio `spawn_blocking` 边界执行，不阻塞 async worker。
- `message.appended` 和 `session.closed` 只在 Repository commit 成功并更新快照后发布。
- live subscription 不补发订阅前历史事件；订阅者 receiver 关闭或缓冲区满时会被清理。
- event sequence 从 1 开始，按单个 Session 的 Actor 生命周期递增。
- mailbox 满时调用者收到 `MailboxFull`；Actor shutdown 后新命令收到 `Shutdown`。
- shutdown 命令完成后，已在 mailbox 中但尚未处理的命令统一收到 `Shutdown`。

## 恢复边界

Step 5 的 `SessionActor::spawn` 接收已由 Repository 恢复的快照。它不创建 Session row，
不补发历史 `message.appended` 事件，也不把未提交的调用方 draft 放入内存。

Runtime Supervisor 在后续 Step 负责启动顺序、live Actor registry、`session.recovered` 事件、
进程重启和数据库所有权。Actor 本身不负责跨 Session registry、RPC 或 CLI。
