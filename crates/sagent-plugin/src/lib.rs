//! sagent-plugin — 轻量插件 SDK。
//!
//! 仅定义 `Plugin` trait 和 `PluginHook`，供第三方插件 crate 依赖。
//! 短期使用 `libloading` 加载动态库，长期考虑 WASM 隔离。
