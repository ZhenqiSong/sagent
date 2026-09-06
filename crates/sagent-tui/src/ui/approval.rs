//! 工具审批弹层的无副作用渲染。

use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::app::{AppState, Overlay};

/// 显示 Runtime 已脱敏的审批摘要；绝不从 TUI 重新读取工具参数或执行任何命令。
pub fn draw_approval(frame: &mut Frame, state: &AppState) {
    let Overlay::Approval(approval) = &state.overlay else {
        return;
    };
    let area = Layout::vertical([
        Constraint::Percentage(25),
        Constraint::Length(12),
        Constraint::Percentage(25),
    ])
    .split(frame.area())[1];
    frame.render_widget(Clear, area);
    let submission = if approval.submitting {
        "\n正在提交决定…"
    } else {
        ""
    };
    let error = approval
        .error
        .as_deref()
        .map_or(String::new(), |error| format!("\n错误：{error}"));
    let text = format!(
        "工具：{}\n摘要：{}\n过期时间：{}\n\n1 仅本次允许  2 本会话允许  3 始终允许  0/Esc 拒绝{}{}",
        approval.tool_name, approval.summary, approval.expires_at, submission, error,
    );
    frame.render_widget(
        Paragraph::new(text).block(Block::default().title(" 工具审批 ").borders(Borders::ALL)),
        area,
    );
}
