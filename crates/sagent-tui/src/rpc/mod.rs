//! TUI 对本地 `sagent-rpc` 子进程的 NDJSON 客户端。
//!
//! 此模块拥有子进程与协议字节流；应用层只能接收已规范化的连接 action，不能直接碰
//! stdin/stdout。这样未来替换 transport 时不会让 UI 绕过 reducer 或 Profile 边界。

mod client;
mod codec;

pub use client::{ClientPoll, RpcClient};
