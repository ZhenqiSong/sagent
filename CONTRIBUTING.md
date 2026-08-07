# Contributing to Sagent

## 本地开发环境

### 前置依赖

- Rust 稳定版工具链（见 `rust-toolchain.toml`）
- `cargo-deny`：`cargo install cargo-deny --locked`
- `cargo-audit`：`cargo install cargo-audit --locked`

### 本地验收命令

提交 PR 前，在仓库根目录依次执行以下命令，确保全部通过：

```bash
# 1. 代码格式化检查
cargo fmt --all -- --check

# 2. 编译检查（全 workspace、全 target）
cargo check --workspace --all-targets

# 3. 运行所有测试
cargo test --workspace

# 4. Clippy 静态检查（零警告）
cargo clippy --workspace --all-targets --all-features -- -D warnings

# 5. 依赖审计
cargo deny check

# 6. 安全漏洞扫描
cargo audit
```

### CI 流水线

PR 提交后，GitHub Actions 会自动执行以下检查：

| Job | 内容 | 平台 |
|-----|------|------|
| Quality | fmt → check → test → clippy | Linux / macOS / Windows |
| Cross Check | 跨平台基础编译 | x86_64-linux / x86_64-darwin / x86_64-windows |
| Deny | 许可证合规 + 依赖禁止 + 源码来源 | Linux |
| Audit | 安全漏洞扫描 | Linux |

**任一步失败都会导致 CI 失败**，没有 `continue-on-error` 豁免。

### 代码风格

- 使用 `cargo fmt` 默认风格（配置见 `.rustfmt.toml`）
- 所有公共 API 必须有文档注释
- 新增类型必须有序列化 round-trip 测试
- 禁止在 PR 中引入 `#[allow(clippy::*)]` 而不附带理由

### 提交信息

- 使用英文撰写 commit message
- 格式：`<type>: <简短描述>`
- 类型：`feat`、`fix`、`docs`、`test`、`refactor`、`chore`、`ci`

### 依赖管理

- Phase 0 最小依赖原则：仅引入当前阶段必需的 crate
- 新增依赖必须通过 `cargo deny check` 和 `cargo audit`
- 不允许使用 git 依赖或非 crates.io 来源
- 不允许使用通配符版本依赖（`*`）
- 同一 crate 的多个版本不允许共存

### 协议与类型

- `sagent-types` 是窄腰层，不得引入 IO / HTTP / 数据库依赖
- 协议变更必须同步更新 schema、fixture 和文档
- 新增错误码必须在 `sagent-api/src/error.rs` 中统一定义
