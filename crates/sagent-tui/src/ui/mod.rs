//! Ratatui 的纯渲染模块。
//!
//! 所有 widget 只读取 `AppState`；键盘、RPC 或终端副作用必须由外层事件循环产生
//! action 后再进入 reducer，避免 draw 被帧率重复调用时改变业务状态。

mod approval;
mod layout;
mod sessions;
mod text;

pub use approval::draw_approval;
pub use layout::draw;
pub use sessions::draw_picker;
pub use text::wrap_display_width;
