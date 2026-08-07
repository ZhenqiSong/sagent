# Phase 0 明确不做

以下功能必须留到后续 Phase，不得在 Phase 0 中引入：

## 模型与网络
- Provider HTTP 请求、SSE 流、API key 管理
- 模型 fallback、retry、rate limiting
- 任何模型 SDK（OpenAI、Anthropic 等）

## Agent 核心
- Agent Loop、Turn 执行、Session Actor
- Context Budget、上下文压缩
- Memory 三层模型（Transcript/Local/External）

## 持久化
- SQLite 数据库、Session Store
- 消息持久化、FTS5 全文搜索
- 数据库 migration

## 工具系统
- Terminal、File、Browser 或任何实际工具执行
- Tool Registry 运行时发现和动态加载
- MCP 协议适配

## 接入层
- HTTP、WebSocket 传输
- TUI、Desktop、Web UI
- Gateway 多平台消息路由
- Scheduler 定时任务

## 插件与扩展
- 插件运行时（外部进程/WASM）
- Skills 加载和执行

## 兼容性
- Python 代码调用、Python ABI 兼容
- 旧 SQLite schema 兼容
- 旧插件协议兼容
- 以当前 Python 模块名为 Rust 模块名的逐文件翻译
