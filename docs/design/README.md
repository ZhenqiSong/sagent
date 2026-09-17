# Sagent 设计与阶段计划

这里保留当前仍需要指导开发的计划、规范和架构基线。已经结项或被替代的计划请从
[已完成计划归档](archive/README.md)进入，不再与执行中的文件混放。

## 当前执行入口

| 文档 | 用途 | 状态 |
| --- | --- | --- |
| [`rust-phase-3-plan.md`](rust-phase-3-plan.md) | Phase 3 能力扩展与本地多客户端接入 | 规划中 |
| [`rust-code-quality-and-architecture-optimization-plan.md`](rust-code-quality-and-architecture-optimization-plan.md) | R4–R8 代码治理与架构优化 | 执行中 |
| [`rust-r4-provider-capability-plan.md`](rust-r4-provider-capability-plan.md) | R4 配置、Provider 与每回合能力快照 | R4.0 已完成，R4.1 待执行 |
| [`rust-fourth-phase-plan.md`](rust-fourth-phase-plan.md) | 第四阶段总览 | 4.6/4.7 进行中 |
| [`rust-fourth-phase-4.6-plan.md`](rust-fourth-phase-4.6-plan.md) | 交互 RPC、事件流与 Headless E2E | 执行中 |
| [`rust-fourth-phase-4.7-plan.md`](rust-fourth-phase-4.7-plan.md) | Ratatui 薄客户端 | 待执行 |

## 配套文档

- [`rust-code-standards.md`](rust-code-standards.md)：跨 crate 的 Rust 代码规范；
- [`rust-refactor-baseline.md`](rust-refactor-baseline.md)：重构前行为与体量基线；
- [`rust-fourth-phase-4.5-tool-behavior.md`](rust-fourth-phase-4.5-tool-behavior.md)：工具与审批行为契约；
- [`sagent-actor-flow-ima-note.md`](sagent-actor-flow-ima-note.md)：Actor 生命周期说明。

## 维护规则

1. 新的执行计划放在本目录，并在本文件登记状态；
2. 整份计划完成后移入 `archive/`，保留历史内容和归档原因；
3. 规范、基线、行为契约不是阶段计划，不因阶段完成而删除；
4. 移动文档时同步更新仓库内的相对链接。
