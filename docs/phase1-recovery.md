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

Runtime Supervisor 负责启动顺序、live Actor registry、进程重启和数据库所有权。Step 6 的
Supervisor 在启动时完成数据库初始化后才接受请求，
registry 只保存 live handle 和 task；`get/list` 在 shutdown 开始后拒绝新请求。Actor 本身不负责
跨 Session registry、RPC 或 CLI。

## Runtime Shutdown

- shutdown 首先将 Runtime 标记为不再接受请求。
- 之后逐个向 live Actor 发送 shutdown，等待其已接受命令完成并回收 task。
- 每个等待都受 `runtime.shutdown_timeout_ms` 的总体 deadline 约束；超时会 abort Actor task，
  然后释放 Supervisor Repository。
- 重复 shutdown 是幂等的；数据库 connection 在所有 Actor task 释放后才关闭。
