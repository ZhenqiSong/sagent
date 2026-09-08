# Sagent

Sagent 是一个使用 Rust 构建的本地 AI Agent 原型，提供会话持久化、Profile 隔离、OpenAI 兼容 Provider、JSON-RPC 本地服务，以及基于 Ratatui 的终端交互界面（TUI）。

当前项目处于持续开发阶段，命令行、RPC、Runtime 和 TUI 已拆分为独立 crate。

## 特性

- SQLite 会话存储与 FTS5 消息全文搜索
- `default` 和具名 Profile 隔离
- OpenAI-compatible HTTP Provider 与流式响应
- Session Actor / Runtime 生命周期管理
- 工具执行与审批流程
- 标准输入输出上的 NDJSON JSON-RPC
- 基于 Ratatui + Crossterm 的 macOS/Linux/Windows TUI
- 文本和 JSON 两种 CLI 输出格式

## 项目结构

```text
crates/
├── sagent-types/       # 公共 ID、消息、会话和领域类型
├── sagent-config/      # Home、Profile、Provider 配置解析
├── sagent-store/       # SQLite 存储、迁移和 FTS5 搜索
├── sagent-agent/       # Agent 状态与消息转换
├── sagent-provider/    # OpenAI-compatible Provider
├── sagent-tools/       # Terminal、文件等工具和审批策略
├── sagent-runtime/     # Session Actor、Turn 和工具/Provider Worker
├── sagent-protocol/    # JSON-RPC DTO、方法和服务分发
├── sagent-rpc/         # 本地 RPC 服务端
├── sagent-tui/         # Ratatui 终端客户端
└── sagent-cli/         # Profile 和 Session 管理 CLI
```

TUI 不直接访问数据库或 Provider，而是启动一个受控的 `sagent-rpc` 子进程，通过 stdin/stdout 上的 JSON-RPC 与 Runtime 通信：

```text
sagent-tui ── NDJSON/stdin/stdout ──> sagent-rpc ──> Runtime / Store / Provider
```

## 环境要求

- Rust `1.88` 或更高版本
- macOS、Linux 或 Windows
- 能访问所配置 Provider 的网络

检查 Rust 版本：

```bash
rustc --version
cargo --version
```

## macOS 构建和启动 TUI

在仓库根目录执行：

```bash
cd /Users/songzq/develop/projects/llm/sagent

cargo build --release -p sagent-rpc -p sagent-tui

./target/release/sagent-tui \
  --rpc-bin ./target/release/sagent-rpc
```

开发调试时可以使用 debug 构建：

```bash
cargo build -p sagent-rpc -p sagent-tui

./target/debug/sagent-tui \
  --rpc-bin ./target/debug/sagent-rpc
```

也可以指定数据目录和 Profile：

```bash
./target/debug/sagent-tui \
  --home "$HOME/.sagent" \
  --profile default \
  --rpc-bin ./target/debug/sagent-rpc
```

其中 `--rpc-bin` 是 TUI 启动的 RPC 子进程路径。若 `sagent-rpc` 已安装到 `PATH`，可以省略该参数。

TUI 输入快捷键：普通 `Enter` 提交消息，`Ctrl+Enter` 插入换行；在部分 macOS 终端中，`Ctrl+J` 也作为换行处理。`F2` 保留为备用提交键。

## 数据目录和 Profile

macOS/Linux 默认使用：

```text
~/.sagent/
```

默认 Profile 直接使用该目录；具名 Profile 位于：

```text
~/.sagent/profiles/<profile-name>/
```

每个 Profile 主要包含：

```text
config.yaml   # 非秘密配置
.env          # API Key 等秘密
state.db      # SQLite 会话数据库
```

也可以通过 `SAGENT_HOME` 指定根目录，命令行的 `--home` 优先级更高：

```bash
export SAGENT_HOME="$HOME/.sagent"
```

## Provider 配置

Provider 的非秘密配置放在当前 Profile 的 `config.yaml`。例如：

```yaml
provider: openai-compatible
model: your-model-name
base_url: https://api.example.com/v1/chat/completions
api_key_env: OPENAI_API_KEY
```

API Key 放在当前 Profile 的 `.env`，不要提交到 Git：

```dotenv
OPENAI_API_KEY=your-api-key
```

配置按 Profile 独立解析，不会跨 Profile 读取凭据。

## CLI 用法

运行 CLI：

```bash
cargo run -p sagent-cli -- --help
```

Profile 管理：

```bash
cargo run -p sagent-cli -- profile list
cargo run -p sagent-cli -- profile create coder
cargo run -p sagent-cli -- profile use coder
```

Session 管理：

```bash
cargo run -p sagent-cli -- session create --title "第一次会话"
cargo run -p sagent-cli -- session list
cargo run -p sagent-cli -- session show <SESSION_ID>
cargo run -p sagent-cli -- session search "关键词"
cargo run -p sagent-cli -- session rename <SESSION_ID> "新标题"
cargo run -p sagent-cli -- session archive <SESSION_ID>
cargo run -p sagent-cli -- session unarchive <SESSION_ID>
```

脚本或其他客户端可以使用 JSON 输出：

```bash
cargo run -p sagent-cli -- --format json session list
```

全局参数可以放在命令前：

```bash
cargo run -p sagent-cli -- \
  --home "$HOME/.sagent" \
  --profile coder \
  session list
```

## JSON-RPC 服务

单独启动 RPC 服务：

```bash
cargo run -p sagent-rpc -- --home "$HOME/.sagent" --profile default
```

RPC 使用 stdin/stdout 传输逐行 JSON（NDJSON）；诊断信息写入 stderr，避免污染协议流。正常使用时通常由 `sagent-tui` 自动启动，不需要手动运行。

## VS Code 调试

仓库提供 macOS 调试配置：

1. 打开项目根目录。
2. 在 Run and Debug 中选择“调试 TUI（${workspaceFolder}/tmp）”。
3. VS Code 会通过 `tasks.json` 先构建 `sagent-rpc`，再启动 TUI。

也可以选择“调试 RPC 服务端”或对应的测试配置。

## 开发命令

```bash
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

只测试某个 crate：

```bash
cargo test -p sagent-tui
cargo test -p sagent-rpc
```

## 许可证

MIT License。
