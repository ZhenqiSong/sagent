//! 初始空屏布局。

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
};

use crate::{
    app::{AppState, ConnectionStatus},
    ui::{draw_approval, draw_picker, wrap_display_width},
};

/// 随终端宽度降级的纯布局选择；composer 始终保留，次要信息才允许隐藏。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayoutMode {
    /// 是否展示完整快捷键帮助。
    pub show_key_help: bool,
    /// 状态栏是否应使用短文本。
    pub compact_status: bool,
}

/// 根据当前列宽选择信息密度；断点与 widget 无关，因而可用纯测试覆盖。
pub fn layout_mode(width: u16) -> LayoutMode {
    match width {
        0..40 => LayoutMode {
            show_key_help: false,
            compact_status: true,
        },
        _ => LayoutMode {
            show_key_help: true,
            compact_status: false,
        },
    }
}

/// 绘制最小 TUI shell；本函数没有 I/O 和状态变更，因此可被任意重绘安全调用。
pub fn draw(frame: &mut Frame, state: &AppState) {
    let mode = layout_mode(frame.area().width);
    let areas = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(4),
        Constraint::Length(3),
    ])
    .split(frame.area());
    let connection = match &state.status {
        ConnectionStatus::NotStarted => "尚未连接 RPC".to_owned(),
        ConnectionStatus::Starting => "正在启动 RPC".to_owned(),
        ConnectionStatus::WaitingForReady => "等待 RPC 就绪".to_owned(),
        ConnectionStatus::Handshaking => "正在协商 RPC 能力".to_owned(),
        ConnectionStatus::Connected => "已连接 RPC".to_owned(),
        ConnectionStatus::Disconnected {
            message,
            retry_after_secs,
        } => return_disconnected_label(message, *retry_after_secs),
    };
    let mut transcript = state.active_session.as_ref().map_or_else(
        || "尚未选择会话\n\n按 n 创建会话，或按 Esc 关闭选择器".to_owned(),
        |session| {
            if session.transcript.is_empty() {
                format!(
                    "{}\n\n（此会话暂无可见消息）",
                    session.summary.title.as_deref().unwrap_or("未命名会话")
                )
            } else {
                session
                    .transcript
                    .iter()
                    .rev()
                    .take(100)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .map(|message| format!("{}：{}", message.role, message.content))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            }
        },
    );
    if let Some(turn) = &state.active_turn {
        transcript.push_str(&format!("\n\nassistant（生成中）：{}", turn.stream_text));
    }
    let body_width = usize::from(areas[0].width.saturating_sub(2)).max(1);
    let body = Paragraph::new(wrap_display_width(&transcript, body_width).join("\n"))
        .alignment(Alignment::Center)
        .block(Block::default().title(" Sagent TUI ").borders(Borders::ALL));
    let status_label = state.status_message.as_ref().map_or_else(
        || {
            if mode.compact_status {
                connection.clone()
            } else {
                format!("状态：{connection}")
            }
        },
        |message| format!("状态：{connection}；{message}"),
    );
    let status = Paragraph::new(status_label)
        .style(Style::default().fg(Color::Cyan))
        .block(Block::default().borders(Borders::TOP));

    let composer = Paragraph::new(if state.composer.text.is_empty() {
        if mode.show_key_help {
            "输入消息；Enter 提交（Ctrl-Enter 换行）".to_owned()
        } else {
            "输入".to_owned()
        }
    } else {
        state.composer.text.clone()
    })
    .block(Block::default().title(" 输入 ").borders(Borders::ALL));
    frame.render_widget(body, areas[0]);
    frame.render_widget(composer, areas[1]);
    frame.render_widget(status, areas[2]);
    draw_picker(frame, state);
    draw_approval(frame, state);
}

/// 断线状态保留退避信息，避免用户误以为 TUI 已永久卡死。
fn return_disconnected_label(message: &str, retry_after_secs: u32) -> String {
    format!("{message}；{retry_after_secs} 秒后重试（r 立即重试）")
}

#[cfg(test)]
mod tests {
    use super::{LayoutMode, layout_mode};

    #[test]
    fn narrow_layout_hides_help_but_never_removes_composer_area() {
        assert_eq!(
            layout_mode(39),
            LayoutMode {
                show_key_help: false,
                compact_status: true
            }
        );
        assert_eq!(
            layout_mode(80),
            LayoutMode {
                show_key_help: true,
                compact_status: false
            }
        );
    }
}
