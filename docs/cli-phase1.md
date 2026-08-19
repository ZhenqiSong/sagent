<!--
  sagent Phase 1 基础 CLI。

  @author   songzq
  @created  2026-08-18
  @change   2026-08-18 初始版本：Session、health 和 protocol CLI
-->

# 基础 CLI

基础 CLI 通过 Runtime service/API 访问 Session，不直接调用 SQLite Repository。

## 命令

```text
sagent session create [--title TITLE] [--source SOURCE] [--cwd PATH] [--json]
sagent session list [--limit N] [--source SOURCE] [--status STATUS] [--json]
sagent session get SESSION_ID [--limit N] [--after-sequence N] [--json]
sagent session resume SESSION_ID [--json]
sagent health [--json]
sagent protocol describe [--json]
```

`session create` 默认使用 `cli` 作为来源，不会隐式保存当前工作目录。只有显式传入
`--cwd` 时，工作目录才会写入 Session。

`--json` 输出稳定 JSON 到 stdout；诊断错误写入 stderr。找不到 Session 或参数无效时返回非零
退出码。CLI 每次调用结束都会关闭 Runtime，确保数据库连接和 Actor task 被释放。
