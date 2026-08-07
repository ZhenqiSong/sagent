# Python 参考代码阅读记录

本文档记录对 Python 版 Hermes Agent 参考代码的行为分析，用于指导 Sagent Rust 重写。
每个参考文件只记录三项：保留的行为、不迁移的实现、Rust 实现建议。

## run_agent.py

文件：`run_agent.py`
保留的行为：
- 消息角色：`system`、`user`、`assistant`、`tool` 四种角色，对应 OpenAI Chat Completions 格式
- 工具调用结构：`id`（唯一标识）、`name`（工具名）、`arguments`（JSON 参数）
- 工具调用结果回填：`role: "tool"` 的消息通过 `tool_call_id` 关联到对应的 assistant 工具调用
- 消息交替不变量：不能出现连续两条同角色消息
- 每轮 Turn 的迭代次数受 `max_iterations` 控制

不迁移的实现：
- Python `openai.OpenAI` 客户端、全局 session dict、`threading.Lock` 并发控制
- `AIAgent.__init__` 中的全局状态初始化和动态 import
- `fire` CLI 框架的使用

Rust 实现建议：
- `Message` 使用 enum `Role { System, User, Assistant, Tool }`
- `ToolCall` 包含 `id: ToolCallId`、`name: String`、`arguments: serde_json::Map<String, Value>`
- 消息序列验证在 `Session` 层面做，不在类型定义层

## model_tools.py

文件：`model_tools.py`
保留的行为：
- 工具定义的公开形状：`{"type": "function", "function": {"name": "...", "description": "...", "parameters": {...}}}`
- 工具调用分发：通过 `function_name` 查找对应的 handler 执行
- 工具 schema 与 handler 分离：registry 持有 schema，handler 是独立的可调用对象
- 工具参数类型强制转换（coerce_tool_args）：LLM 常返回字符串形式的数字/布尔值
- 工具错误信息消毒（_sanitize_tool_error）：移除 XML 标签、代码围栏、CDATA，限制长度

不迁移的实现：
- 全局 `_tool_defs_cache` 和 `_last_resolved_tool_names` 进程级缓存
- `_run_async` 的线程池/事件循环桥接
- `_AGENT_LOOP_TOOLS` 硬编码列表
- 模块级 `discover_builtin_tools()` 副作用导入
- Python callable handler 和动态 dispatch
- Toolset 的 `enabled_toolsets`/`disabled_toolsets` 过滤逻辑

Rust 实现建议：
- `ToolDefinition` 包含 `name`、`description`、`input_schema`（JSON Schema）
- 参数类型强制转换在 Rust 中使用 serde_json 的 `Value` 类型判断实现
- 工具注册通过显式函数调用而非模块导入副作用

## tools/registry.py

文件：`tools/registry.py`
保留的行为：
- 工具注册包含：name、schema、handler、toolset 归属、availability check
- `check_fn` 用于运行时工具可用性检查（环境依赖、权限等）
- registry 是单一真源，所有工具查询走 registry

不迁移的实现：
- 基于 AST 解析的 `_module_registers_tools()` 自动发现
- 基于 `importlib.import_module` 的动态模块加载
- `_generation` 计数器用于缓存失效
- Python 文件系统扫描 `tools/*.py`

Rust 实现建议：
- `ToolRegistry` 使用显式注册（`register(Arc<dyn Tool>)`），不扫描文件系统
- `Tool` trait 定义：`name()`、`definition()`、`execute()`、`check_availability()`
- 不实现 Phase 0

## tui_gateway/server.py

文件：`tui_gateway/server.py`
保留的行为：
- JSON-RPC 2.0 协议：request 必须带 `jsonrpc`、`id`、`method`；response 带 `id` + `result` 或 `error`
- event/notification 不带 `id`，使用 `method` 字段标识事件类型
- stdout 是协议通道，stderr 是日志通道
- `_stdout_lock` 确保 stdout 写入串行化
- `_methods` dict 用于 method 到 handler 的 dispatch

不迁移的实现：
- 全局 `_sessions` dict（Python 字典存所有 session 状态）
- 全局 `_pending`、`_answers`、`_db` 模块级变量
- `ThreadPoolExecutor` 工作线程池
- Python `threading.Lock` / `threading.RLock` 并发原语
- 全局 `write_json` 函数直接操作 `_real_stdout`
- `_panic_hook` 和 `_thread_panic_hook` 的 crash 日志

Rust 实现建议：
- `Request` / `Response` / `Notification` 三个独立类型，不使用万能结构体
- `Dispatcher` 使用 `HashMap<&str, Handler>` 注册 method
- stdout 写入使用 `BufWriter<Stdout>` + `serde_json::to_writer`

## tui_gateway/transport.py

文件：`tui_gateway/transport.py`
保留的行为：
- Transport 抽象：`write(obj: dict) -> bool`、`close()`
- stdout 是协议数据通道，不能写日志
- 写入需要串行化（stdout lock）
- BrokenPipe / peer disconnect 时干净退出（不 panic）
- `_PEER_GONE_ERRNOS` 用于区分"对端断开"和"真正的 I/O 错误"

不迁移的实现：
- Python `contextvars.ContextVar` 绑定 transport
- `Protocol` 类型（Python duck typing）
- `_DISABLE_FLUSH` 环境变量开关
- `TeeTransport` 多路输出

Rust 实现建议：
- `Transport` trait：`write(&self, frame: &JsonRpcFrame) -> Result<(), TransportError>`
- `StdioTransport` 使用 `BufWriter<Stdout>` + `Mutex`
- `TransportError` 区分 `PeerGone` 和 `IoError`

## tui_gateway/entry.py

文件：`tui_gateway/entry.py`
保留的行为：
- stdin 逐行读取 JSON-RPC request
- stdout 逐行写入 JSON-RPC response
- stderr 用于日志和诊断
- stdin EOF 时正常退出（返回码 0）
- 信号处理：SIGTERM/SIGINT 触发有序关闭

不迁移的实现：
- `hermes_bootstrap.harden_import_path()` 路径加固
- `_install_sidecar_publisher()` WebSocket sidecar
- MCP 工具发现线程
- `_panic_hook` crash 日志

Rust 实现建议：
- 使用 `BufRead::lines()` 逐行读取 stdin
- 使用 `serde_json::from_str` 解析每行
- 使用 `tokio::signal` 处理信号（Phase 0 不需要）

## hermes_constants.py

文件：`hermes_constants.py`
保留的行为：
- 平台默认 home 路径：Linux `~/.hermes`、macOS `~/Library/Application Support/hermes`、Windows `%LOCALAPPDATA%\hermes`
- `HERMES_HOME` 环境变量覆盖默认路径
- `ContextVar` 实现 per-task profile 隔离

不迁移的实现：
- Sagent 使用 `~/.sagent` 而非 `~/.hermes`，环境变量使用 `SAGENT_HOME`
- 不实现 `ContextVar` per-task override（Phase 0 不需要）
- 不实现 Node.js 管理、WSL 检测、容器检测
- 不实现 `get_config_path()`、`get_skills_dir()` 等（Phase 0 不需要）

Rust 实现建议：
- 使用 `dirs` crate 获取平台默认路径
- `SagentHome` 结构体提供 `discover()`、`from_env()`、`config_dir()`、`logs_dir()` 等方法
- 使用 `SAGENT_HOME` 环境变量覆盖
- 路径解析使用 `PathBuf`，不拼接硬编码 `/`

## hermes_logging.py

文件：`hermes_logging.py`
保留的行为：
- 日志写 stderr 或文件，绝不写 stdout
- 通过 `RUST_LOG` / `HERMES_LOG_LEVEL` 环境变量控制日志级别
- Session context：`set_session_context(session_id)` 在日志中注入 session 关联
- 日志初始化幂等（重复调用不添加重复 handler）
- Secret redaction：`RedactingFormatter` 防止密钥写入日志

不迁移的实现：
- Python `logging` 模块的 `RotatingFileHandler`、`QueueHandler`、`QueueListener`
- `_ManagedRotatingFileHandler` 的 NixOS managed mode 支持
- 多文件日志（agent.log、errors.log、gateway.log、gui.log）
- `RedactingFormatter` 的完整实现（Phase 0 仅需要基本 redact 逻辑）
- 异步日志队列（Phase 0 stdio server 无需文件日志轮转）

Rust 实现建议：
- 使用 `tracing` + `tracing-subscriber` 替代 Python logging
- stderr layer 使用 `tracing_subscriber::fmt::Layer` with `with_writer(std::io::stderr)`
- 通过 `RUST_LOG` 控制级别
- `request_id` 和 `session_id` 通过 `tracing::Span` 传递
- 日志初始化使用 `tracing_subscriber::registry()` 确保幂等

## tests/tui_gateway/test_protocol.py

文件：`tests/tui_gateway/test_protocol.py`
保留的行为：
- 测试 JSON-RPC request/response 的格式正确性
- 测试非法 JSON 输入的错误处理
- 测试 method dispatch 的正确性
- 使用 mock 隔离外部依赖

不迁移的实现：
- `unittest.mock.MagicMock` 和 `patch` 的使用
- `pytest.fixture` 的 server fixture 设置/清理模式
- `io.StringIO` 捕获 stdout

Rust 实现建议：
- 使用 Rust 的 `#[test]` 函数测试
- stdio 测试通过启动子进程（`std::process::Command`）进行端到端验证
- 使用临时目录和环境变量隔离测试环境

## 其他重要观察

### 消息交替不变量（run_agent.py）
Python 实现中通过消息列表追加隐式保证角色交替。Rust 中应显式验证：不允许连续两条同角色消息。

### 工具调用配对（run_agent.py）
assistant 消息可以包含多个 `tool_calls`，每个 `tool_call` 需要对应的 `tool` 角色消息回填结果。这构成了不可分割的 pair。

### JSON-RPC 协议约定（tui_gateway）
- request `id` 支持 string 和 number
- notification 不带 `id`，不返回 response
- 每行一个完整的 JSON-RPC 消息（newline-delimited JSON）
- stdout flush 保证即时响应

### Python 测试场景映射（test_protocol.py）
Python 协议测试覆盖的场景和对应的 Rust conformance 方向：
- 合法 request/response 格式校验 → valid fixtures + schema 验证
- 非法 JSON 输入（语法错误） → `-32700` ParseError
- 缺少 method 的 request → `-32600` InvalidRequest
- 未知 method → `-32601` MethodNotFound
- params 类型错误 → `-32602` InvalidParams
- notification 不返回 response → conformance 测试验证
- 连续多 request 的顺序保证 → stdio 进程测试
- 这些场景已映射到 `protocols/fixtures/invalid/` 中的对应 fixture

### 完成原因分析（run_agent.py / conversation_loop.py）
Python 实现中 Turn 的完成条件：
1. `finish_reason == "stop"` — 正常完成，无 tool call
2. `finish_reason == "tool_calls"` — 有 tool call，执行后继续循环
3. `finish_reason == "length"` — 达到 token 限制，触发 continuation
4. `agent._interrupt_requested` — 用户中断
5. `iteration_budget.consume()` 返回 False — 预算耗尽
6. `api_call_count >= agent.max_iterations` — 超过最大迭代次数
这些已记录在 `protocols/protocol-decisions.md` 的 Decision #14 中。
