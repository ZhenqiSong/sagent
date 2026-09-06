//! RPC 服务适配层。
//!
//! transport 只负责 NDJSON、连接状态和 stdout；Profile/Store/Runtime 的生命周期由
//! 此模块管理，避免入口文件随着交互方法增加而重新变成业务逻辑聚合点。

mod runtime;

pub use runtime::RuntimeService;
