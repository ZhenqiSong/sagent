//! 终端生命周期、panic 恢复和空屏事件循环。

use std::{
    io::{self, Stdout},
    panic,
    sync::{
        Once,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, Show},
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use sagent_protocol::ApprovalDecisionDto;

use crate::{
    app::{AppAction, AppState, SelectionDirection, handle_action, handle_rpc_event, reduce},
    rpc::{ClientPoll, RpcClient},
    ui,
};

/// 确保全进程只替换一次 panic hook；hook 只能尽力恢复 terminal，绝不等待 async task。
static PANIC_HOOK_INSTALLED: Once = Once::new();
/// 标识当前是否有 TUI 正在接管终端，供 panic hook 判断是否需要恢复状态。
static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 已取得终端控制权的 guard。
///
/// raw mode 与 alternate screen 是 shell 级全局状态，不受普通 Rust 栈展开自动恢复；
/// 所有正常离开路径都依赖 Drop 按逆序回收它们。
pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    /// 进入 raw mode 与 alternate screen，并在部分初始化失败时立即回滚。
    pub fn enter() -> Result<Self> {
        enable_raw_mode().context("无法启用终端 raw mode")?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, Hide) {
            let _ = disable_raw_mode();
            return Err(error).context("无法进入 alternate screen");
        }
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(error) => {
                restore_terminal();
                return Err(error).context("无法创建 Ratatui terminal");
            }
        };
        if let Err(error) = terminal.hide_cursor() {
            restore_terminal();
            return Err(error).context("无法隐藏终端光标");
        }
        TERMINAL_ACTIVE.store(true, Ordering::Release);
        Ok(Self { terminal })
    }

    /// 仅将不可变 ViewModel 交给 UI 绘制；闭包内不得发送 RPC 或修改 AppState。
    fn draw(&mut self, state: &AppState) -> Result<()> {
        self.terminal
            .draw(|frame| ui::draw(frame, state))
            .context("TUI 绘制失败")?;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Drop 不能传播错误；panic 或启动失败期间只能尽力恢复，诊断交由原 panic hook。
        if TERMINAL_ACTIVE.swap(false, Ordering::AcqRel) {
            let _ = self.terminal.show_cursor();
            restore_terminal();
        }
    }
}

/// 安装 panic 恢复钩子，避免开发期 panic 把用户 shell 留在 raw mode。
pub fn install_panic_hook() {
    PANIC_HOOK_INSTALLED.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if TERMINAL_ACTIVE.swap(false, Ordering::AcqRel) {
                restore_terminal();
            }
            previous(info);
        }));
    });
}

/// 退出时按进入顺序的反向恢复 terminal 全局状态。
fn restore_terminal() {
    let mut stdout = io::stdout();
    let _ = execute!(stdout, Show, DisableBracketedPaste, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

/// 将键盘事件规范化为 action；终端层不读取会话状态，也不直接调用 RPC 方法。
fn key_action(event: Event, state: &AppState) -> Option<AppAction> {
    if let Event::Paste(text) = event {
        // bracketed paste 作为一个 action 插入，避免逐字符触发副作用或提交判断。
        return matches!(state.overlay, crate::app::Overlay::None)
            .then_some(AppAction::ComposerInsert(text));
    }
    let Event::Key(event) = event else {
        return None;
    };
    // Release 与 Repeat 不应造成第二次退出；只接收首次按下，后续步骤也沿用该规则。
    if event.kind != KeyEventKind::Press {
        return None;
    }
    if matches!(state.overlay, crate::app::Overlay::Approval(_)) {
        return match event.code {
            KeyCode::Char('1') => Some(AppAction::ApprovalDecisionRequested(
                ApprovalDecisionDto::Once,
            )),
            KeyCode::Char('2') => Some(AppAction::ApprovalDecisionRequested(
                ApprovalDecisionDto::Session,
            )),
            KeyCode::Char('3') => Some(AppAction::ApprovalDecisionRequested(
                ApprovalDecisionDto::Always,
            )),
            KeyCode::Char('0') | KeyCode::Esc => Some(AppAction::ApprovalDecisionRequested(
                ApprovalDecisionDto::Deny,
            )),
            _ => None,
        };
    }
    if matches!(state.overlay, crate::app::Overlay::None) {
        return match event.code {
            // VS Code 的集成终端常把 Ctrl-Enter 降级为普通 Enter，无法与换行可靠区分；
            // Ctrl-S 也可能被工作台“保存”快捷键或终端流控截获。F2 有独立的终端键码，
            // 因而作为跨终端稳定的提交键；Ctrl-Enter/Ctrl-J 仍保留为兼容映射。
            KeyCode::Enter | KeyCode::Char('j')
                if event.modifiers.contains(event::KeyModifiers::CONTROL) =>
            {
                Some(AppAction::SubmitPromptRequested)
            }
            KeyCode::F(2) => Some(AppAction::SubmitPromptRequested),
            KeyCode::Enter => Some(AppAction::ComposerInsert("\n".to_owned())),
            KeyCode::Backspace => Some(AppAction::ComposerBackspace),
            KeyCode::Delete => Some(AppAction::ComposerDelete),
            KeyCode::Left => Some(AppAction::ComposerMoveLeft),
            KeyCode::Right => Some(AppAction::ComposerMoveRight),
            KeyCode::Char('q') => Some(AppAction::QuitRequested),
            KeyCode::Char('c') if event.modifiers.contains(event::KeyModifiers::CONTROL) => {
                if state.active_turn.is_some() {
                    Some(AppAction::InterruptRequested)
                } else {
                    Some(AppAction::QuitRequested)
                }
            }
            KeyCode::Char(character) => Some(AppAction::ComposerInsert(character.to_string())),
            _ => None,
        };
    }
    match event.code {
        KeyCode::Char('q') => Some(AppAction::QuitRequested),
        KeyCode::Up | KeyCode::Char('k') => {
            Some(AppAction::MoveSessionSelection(SelectionDirection::Up))
        }
        KeyCode::Down | KeyCode::Char('j') => {
            Some(AppAction::MoveSessionSelection(SelectionDirection::Down))
        }
        KeyCode::Enter => Some(AppAction::ConfirmSessionSelection),
        KeyCode::Char('n') => Some(AppAction::CreateSessionRequested),
        KeyCode::Esc => Some(AppAction::DismissOverlay),
        KeyCode::Char('c') if event.modifiers.contains(event::KeyModifiers::CONTROL) => {
            Some(AppAction::QuitRequested)
        }
        _ => None,
    }
}

/// 运行终端事件循环；所有键盘输入先归一为 action，再由 controller 协调 RPC 副作用。
pub async fn run(mut state: AppState, client: &mut RpcClient) -> Result<TerminalExit> {
    install_panic_hook();
    let mut terminal = TerminalGuard::enter()?;
    while !state.should_quit {
        while let Some(incoming) = client.try_next_event() {
            match incoming {
                ClientPoll::Event(event) => handle_rpc_event(client, &mut state, event).await?,
                ClientPoll::Disconnected(message) => {
                    reduce(&mut state, AppAction::RpcDisconnected { message });
                    return Ok(TerminalExit::Reconnect(Box::new(state)));
                }
            }
        }
        terminal.draw(&state)?;
        if event::poll(Duration::from_millis(100)).context("读取终端事件失败")?
            && let Some(action) = key_action(event::read().context("读取键盘事件失败")?, &state)
        {
            handle_action(client, &mut state, action).await?;
        }
    }
    Ok(TerminalExit::Quit)
}

/// 终端循环的显式退出原因；重连时由入口接管子进程生命周期。
#[derive(Debug)]
pub enum TerminalExit {
    /// 用户明确退出。
    Quit,
    /// transport 已断开；包含保留 transcript/composer 的状态以便事实恢复。
    Reconnect(Box<AppState>),
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    use super::key_action;
    use crate::app::{AppAction, SelectionDirection};

    #[test]
    fn q_key_maps_to_a_reducer_action_without_touching_terminal() {
        let action = key_action(
            crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            &crate::app::AppState::default(),
        );

        assert_eq!(action, Some(AppAction::QuitRequested));
    }

    #[test]
    fn only_first_control_c_press_requests_exit() {
        let press = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let repeat = KeyEvent::new_with_kind(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            KeyEventKind::Repeat,
        );

        assert_eq!(
            key_action(
                crossterm::event::Event::Key(press),
                &crate::app::AppState::default()
            ),
            Some(AppAction::QuitRequested)
        );
        assert_eq!(
            key_action(
                crossterm::event::Event::Key(repeat),
                &crate::app::AppState::default()
            ),
            None
        );
    }

    #[test]
    fn f2_submits_while_plain_enter_keeps_multiline_composer() {
        // F2 有独立 VT 键码，不会像 Ctrl-Enter/Ctrl-S 一样在 VS Code 终端链路中被
        // 规范化或截获；普通 Enter 仍然保留为多行输入，避免改变编辑语义。
        let submit = KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE);
        let newline = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let state = crate::app::AppState::default();

        assert_eq!(
            key_action(crossterm::event::Event::Key(submit), &state),
            Some(AppAction::SubmitPromptRequested)
        );
        assert_eq!(
            key_action(crossterm::event::Event::Key(newline), &state),
            Some(AppAction::ComposerInsert("\n".to_owned()))
        );
    }

    #[test]
    fn picker_keys_are_normalized_without_a_terminal_or_rpc_client() {
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let create = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);

        assert_eq!(
            key_action(
                crossterm::event::Event::Key(down),
                &crate::app::AppState {
                    overlay: crate::app::Overlay::SessionPicker {
                        selected_index: 0,
                        loading: false
                    },
                    ..crate::app::AppState::default()
                }
            ),
            Some(AppAction::MoveSessionSelection(SelectionDirection::Down))
        );
        assert_eq!(
            key_action(
                crossterm::event::Event::Key(create),
                &crate::app::AppState {
                    overlay: crate::app::Overlay::SessionPicker {
                        selected_index: 0,
                        loading: false
                    },
                    ..crate::app::AppState::default()
                }
            ),
            Some(AppAction::CreateSessionRequested)
        );
    }
}
