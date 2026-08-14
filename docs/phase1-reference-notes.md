# Phase 1 参考阅读记录

本文只记录从 Hermes Python 实现提取的行为契约，不迁移 Python 模块结构、旧数据库 schema
或运行时实现。

## SessionDB

- 文件：`hermes_state.py`
- 保留的行为：消息读取保持数据库插入顺序；写事务失败时不能只更新内存状态。
- 不迁移的实现：历史字段、Python mixin、旧连接管理和 FTS5。

## Schema

- 文件：`hermes_state_common.py`、`hermes_state_schema.py`
- 保留的行为：Session 与 Message 有明确所属关系；schema 版本和 migration 失败必须可诊断、可回滚。
- 不迁移的实现：旧 `SCHEMA_SQL`、旧版本号和 Hermes `state.db` 兼容层。

## Session RPC

- 文件：`tui_gateway/methods_session.py`
- 保留的行为：create/list/get/resume 区分 NotFound、非法参数和成功结果；恢复只使用已持久化历史。
- 不迁移的实现：Python handler、全局 session dict 和 transport-specific response 组装。

## RPC 与事件

- 文件：`tui_gateway/server.py`
- 保留的行为：request response 保持 request id 关联，事件使用无 id 的 notification；事件只能在状态
  成功提交后发送。
- 不迁移的实现：Python 全局锁、动态 dispatch 和事件循环桥接。

## Stdio 生命周期

- 文件：`tui_gateway/entry.py`、`tui_gateway/transport.py`
- 保留的行为：stdout 只写协议帧，EOF 和 BrokenPipe 干净退出；断开的订阅者不能阻塞其他订阅者。
- 不迁移的实现：Python transport 类层次和线程池桥接。

## 配置与测试

- 文件：`hermes_cli/config.py`、`tests/gateway/test_async_session_db.py`、`tests/tui_gateway/test_protocol.py`
- 保留的行为：配置加载产生稳定快照；真实数据库测试验证资源释放、事务边界和错误行为。
- 不迁移的实现：旧配置字段、Python async adapter 和测试 fixture 的导入方式。
