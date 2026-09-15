# 已完成计划归档

本目录保存已经结项或被后续计划替代的设计与执行记录。归档是可恢复的整理操作，
不代表删除；当前执行中的计划仍保留在上一级 `docs/design/` 目录。

## 归档清单

| 文件 | 归档原因 |
| --- | --- |
| `rust-phase-0-to-2-completion-plan.md` | Phase 0–2 已全部关闭；作为当前 Phase 3 的历史前置记录。 |
| `rust-second-phase-plan.md` | 第二阶段已完成，内容已由 Phase 0–2 收尾计划汇总。 |
| `rust-third-phase-plan.md` | 旧第三阶段 RPC 计划已完成，现由当前 `rust-phase-3-plan.md` 替代。 |
| `rust-fourth-phase-4.1-plan.md` | 纯 Agent 状态机与 PromptSnapshot 已完成。 |
| `rust-fourth-phase-4.2-plan.md` | Turn、Generation 与事件持久化已完成。 |
| `rust-fourth-phase-4.3-plan.md` | SessionActor、并发边界与取消已完成。 |
| `rust-fourth-phase-4.4-plan.md` | Provider 与流式模型接入已完成。 |
| `rust-fourth-phase-4.5-plan.md` | 最小工具、Terminal 与 Approval 闭环已完成。 |
| `rust-storage-manager-execution-plan.md` | StorageManager M0–M8、R3.5 已结项。 |
| `rust-storage-manager-m0-baseline.md` | StorageManager 已完成计划的配套基线记录。 |

## 当前仍在执行的计划

- `../rust-code-quality-and-architecture-optimization-plan.md`：R4–R8；
- `../rust-phase-3-plan.md`：Phase 3 能力扩展与本地多客户端接入；
- `../rust-fourth-phase-plan.md`：第四阶段总览，4.6/4.7 仍在进行；
- `../rust-fourth-phase-4.6-plan.md`：交互 RPC、事件流与 Headless E2E；
- `../rust-fourth-phase-4.7-plan.md`：Ratatui 薄客户端。

规范、重构基线和行为契约不是计划书，因此继续保留在上一级目录。
