//! 会话选择器的纯 Ratatui 渲染。

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

use crate::app::{AppState, Overlay};

/// 在主界面上绘制会话 picker；本函数不处理按键或发出 RPC 请求。
pub fn draw_picker(frame: &mut Frame, state: &AppState) {
    let Overlay::SessionPicker {
        selected_index,
        loading,
    } = state.overlay
    else {
        return;
    };
    let area = centered_rect(80, 70, frame.area());
    frame.render_widget(Clear, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(area);
    let items = if state.sessions.is_empty() {
        vec![ListItem::new("暂无会话；按 n 创建一个新会话")]
    } else {
        state
            .sessions
            .iter()
            .map(|session| {
                let title = session.title.as_deref().unwrap_or("未命名会话");
                let preview = session.preview.as_deref().unwrap_or("没有可显示的预览");
                let activity = session.last_active.as_deref().unwrap_or("未知时间");
                ListItem::new(vec![
                    Line::from(Span::styled(
                        title,
                        Style::default().add_modifier(Modifier::BOLD),
                    )),
                    Line::from(format!(
                        "{preview}  ·  {activity}  ·  {} 条可见消息",
                        session.message_count
                    )),
                ])
            })
            .collect()
    };
    let mut list_state = ListState::default();
    if !state.sessions.is_empty() {
        list_state.select(Some(selected_index));
    }
    let title = if loading {
        " 会话（加载中） "
    } else {
        " 会话 "
    };
    let list = List::new(items)
        .block(Block::default().title(title).borders(Borders::ALL))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, chunks[0], &mut list_state);
    let help = Paragraph::new("↑/k ↓/j 选择 · Enter 打开 · n 新建 · Esc 关闭")
        .block(Block::default().borders(Borders::TOP));
    frame.render_widget(help, chunks[1]);
}

/// 将比例尺寸限制在可见区域内，避免窄终端产生无效 Rect。
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vertical[1])[1]
}
