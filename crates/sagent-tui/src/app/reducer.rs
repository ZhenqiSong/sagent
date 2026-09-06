//! `AppAction` 到 `AppState` 的确定性归约。

use super::{
    ActiveTurnView, AppAction, AppState, ApprovalView, Overlay, SelectionDirection, SessionView,
    ToolActivityStatus, ToolActivityView, TranscriptEntry,
};

/// 将一个外部动作归约到当前 ViewModel。
///
/// reducer 不执行 I/O，也不返回 future；因此同一输入序列的状态变化可用纯单元测试验证。
/// Quit 是单调状态：一旦设为 true，重复动作不会让应用回到运行状态。
pub fn reduce(state: &mut AppState, action: AppAction) {
    match action {
        AppAction::QuitRequested => state.should_quit = true,
        AppAction::RpcStarting => state.status = super::ConnectionStatus::Starting,
        AppAction::RpcWaitingForReady => state.status = super::ConnectionStatus::WaitingForReady,
        AppAction::GatewayReady => state.status = super::ConnectionStatus::Handshaking,
        AppAction::HelloSucceeded => state.status = super::ConnectionStatus::Connected,
        AppAction::RpcDisconnected { message } => {
            // delta 未持久化，断线必须丢弃；旧 transcript 与 composer 可以安全保留。
            state.active_turn = None;
            state.status = super::ConnectionStatus::Disconnected {
                message,
                retry_after_secs: 1,
            }
        }
        AppAction::OpenSessionPicker => {
            state.overlay = Overlay::SessionPicker {
                selected_index: 0,
                loading: true,
            };
            state.status_message = None;
        }
        AppAction::MoveSessionSelection(direction) => move_selection(state, direction),
        // 此 action 表达用户意图；真正的 RPC I/O 只能由 controller 执行，完成后再派发
        // SessionsLoaded/SessionResumed，避免 reducer 直接依赖 transport。
        AppAction::ConfirmSessionSelection | AppAction::CreateSessionRequested => {}
        AppAction::DismissOverlay => state.overlay = Overlay::None,
        AppAction::SessionsLoaded(result) => {
            state.sessions = result.sessions;
            state.status_message = None;
            state.overlay = Overlay::SessionPicker {
                selected_index: 0,
                loading: false,
            };
        }
        AppAction::SessionResumed(result) => {
            state.active_session = Some(SessionView {
                summary: result.detail.session,
                transcript: result
                    .detail
                    .messages
                    .into_iter()
                    .map(|message| TranscriptEntry {
                        message_id: message.id,
                        role: message.role,
                        content: message.content,
                        timestamp: message.timestamp,
                        display_kind: message.display_kind,
                    })
                    .collect(),
            });
            state.overlay = Overlay::None;
            state.status_message = None;
            // resume 是持久化事实快照；成功替换 transcript 后，旧 delta buffer 已无意义，
            // 也必须清除 active turn 以免随后的 turn.completed 再触发一次 resume。
            state.active_turn = None;
        }
        AppAction::SessionOperationFailed { message } => {
            state.status_message = Some(message);
            if let Overlay::SessionPicker { loading, .. } = &mut state.overlay {
                *loading = false;
            }
        }
        AppAction::ComposerInsert(text) => insert_composer_text(state, &text),
        AppAction::ComposerBackspace => remove_composer_character(state, true),
        AppAction::ComposerDelete => remove_composer_character(state, false),
        AppAction::ComposerMoveLeft => {
            state.composer.cursor_char_index = state.composer.cursor_char_index.saturating_sub(1)
        }
        AppAction::ComposerMoveRight => {
            state.composer.cursor_char_index =
                (state.composer.cursor_char_index + 1).min(state.composer.text.chars().count())
        }
        AppAction::SubmitPromptRequested => {}
        AppAction::PromptSubmitted(result) => {
            let Some(session) = state.active_session.as_ref() else {
                return;
            };
            state.active_turn = Some(ActiveTurnView {
                session_id: session.summary.id.clone(),
                turn_id: result.turn_id.as_uuid().to_string(),
                stream_text: String::new(),
                interrupt_pending: false,
                tool_activity: None,
            });
            state.composer.text.clear();
            state.composer.cursor_char_index = 0;
            state.status_message = None;
        }
        AppAction::StreamDeltaReceived {
            session_id,
            turn_id,
            text,
        } => {
            if let Some(active) = state.active_turn.as_mut()
                && active.session_id == session_id
                && active.turn_id == turn_id
            {
                active.stream_text.push_str(&text);
            }
        }
        // controller 收到此 action 后请求 session.resume；只有 resume 成功才会覆盖/清除
        // 临时 stream_text，避免终态事件后的瞬态内容被错误当作持久化历史。
        AppAction::ActiveTurnFinished { .. } => {}
        AppAction::InterruptRequested => {}
        AppAction::InterruptAccepted => {
            if let Some(active) = state.active_turn.as_mut() {
                active.interrupt_pending = true;
            }
        }
        AppAction::ApprovalRequested {
            approval_id,
            session_id,
            turn_id,
            tool_name,
            summary,
            expires_at,
        } => {
            state.overlay = Overlay::Approval(ApprovalView {
                approval_id,
                session_id,
                turn_id,
                tool_name,
                summary,
                expires_at,
                submitting: false,
                error: None,
            });
        }
        AppAction::ApprovalDecisionRequested(_) => {
            if let Overlay::Approval(approval) = &mut state.overlay {
                approval.submitting = true;
                approval.error = None;
            }
        }
        AppAction::ApprovalResponseAccepted => {}
        AppAction::ToolActivityReceived {
            call_id,
            tool_name,
            running,
        } => {
            if let Some(active) = state.active_turn.as_mut() {
                active.tool_activity = Some(ToolActivityView {
                    call_id,
                    tool_name,
                    status: match running {
                        Some(true) => ToolActivityStatus::Running,
                        Some(false) => ToolActivityStatus::Completed { ok: false },
                        None => ToolActivityStatus::Requested,
                    },
                });
            }
        }
    }
}

/// 将字符索引转换为 byte offset，保证所有字符串切片都落在 UTF-8 边界。
fn composer_byte_index(text: &str, character_index: usize) -> usize {
    text.char_indices()
        .nth(character_index)
        .map_or(text.len(), |(index, _)| index)
}

/// 在光标所在字符边界插入文本，粘贴内容也只经过一次调用。
fn insert_composer_text(state: &mut AppState, text: &str) {
    let byte_index = composer_byte_index(&state.composer.text, state.composer.cursor_char_index);
    state.composer.text.insert_str(byte_index, text);
    state.composer.cursor_char_index += text.chars().count();
}

/// 删除光标前或后的一个 Unicode 字符，不能用 byte 下标破坏中文或 emoji。
fn remove_composer_character(state: &mut AppState, before_cursor: bool) {
    let count = state.composer.text.chars().count();
    if before_cursor && state.composer.cursor_char_index > 0 {
        let end = composer_byte_index(&state.composer.text, state.composer.cursor_char_index);
        let start = composer_byte_index(&state.composer.text, state.composer.cursor_char_index - 1);
        state.composer.text.replace_range(start..end, "");
        state.composer.cursor_char_index -= 1;
    } else if !before_cursor && state.composer.cursor_char_index < count {
        let start = composer_byte_index(&state.composer.text, state.composer.cursor_char_index);
        let end = composer_byte_index(&state.composer.text, state.composer.cursor_char_index + 1);
        state.composer.text.replace_range(start..end, "");
    }
}

/// 仅在 picker 打开且有项目时改变索引；空列表与边界移动都保持状态不变。
fn move_selection(state: &mut AppState, direction: SelectionDirection) {
    let Overlay::SessionPicker {
        selected_index,
        loading,
    } = &mut state.overlay
    else {
        return;
    };
    if *loading || state.sessions.is_empty() {
        return;
    }
    *selected_index = match direction {
        SelectionDirection::Up => selected_index.saturating_sub(1),
        SelectionDirection::Down => (*selected_index + 1).min(state.sessions.len() - 1),
    };
}

#[cfg(test)]
mod tests {
    use sagent_protocol::{
        SessionDetailDto, SessionMessageDto, SessionResumeResult, SessionSummaryDto,
    };

    use super::reduce;
    use crate::app::state::ConnectionStatus;
    use crate::app::{AppAction, AppState};

    #[test]
    fn default_state_is_running_without_a_rpc_connection() {
        let state = AppState::default();

        assert!(!state.should_quit);
        assert_eq!(state.status, ConnectionStatus::NotStarted);
    }

    #[test]
    fn quit_action_requests_a_clean_main_loop_exit() {
        let mut state = AppState::default();

        reduce(&mut state, AppAction::QuitRequested);

        assert!(state.should_quit);
    }

    #[test]
    fn repeated_quit_actions_are_idempotent() {
        let mut state = AppState::default();

        reduce(&mut state, AppAction::QuitRequested);
        reduce(&mut state, AppAction::QuitRequested);

        assert!(state.should_quit, "退出请求不能被重复输入反转");
    }

    #[test]
    fn handshake_actions_expose_only_the_completed_connection_state() {
        let mut state = AppState::default();

        reduce(&mut state, AppAction::RpcStarting);
        assert_eq!(state.status, ConnectionStatus::Starting);
        reduce(&mut state, AppAction::RpcWaitingForReady);
        assert_eq!(state.status, ConnectionStatus::WaitingForReady);
        reduce(&mut state, AppAction::GatewayReady);
        assert_eq!(state.status, ConnectionStatus::Handshaking);
        reduce(&mut state, AppAction::HelloSucceeded);

        assert_eq!(state.status, ConnectionStatus::Connected);
    }

    #[test]
    fn picker_selection_is_clamped_and_empty_picker_is_safe() {
        let mut state = AppState::default();
        reduce(&mut state, AppAction::OpenSessionPicker);
        reduce(
            &mut state,
            AppAction::MoveSessionSelection(crate::app::SelectionDirection::Down),
        );
        assert!(matches!(
            state.overlay,
            crate::app::Overlay::SessionPicker {
                selected_index: 0,
                ..
            }
        ));
    }

    #[test]
    fn resume_snapshot_replaces_transcript_with_server_visible_messages_in_order() {
        let mut state = AppState::default();
        let result = SessionResumeResult {
            detail: SessionDetailDto {
                session: SessionSummaryDto {
                    id: "server-session".to_owned(),
                    source: None,
                    model: None,
                    title: Some("服务端会话".to_owned()),
                    started_at: None,
                    ended_at: None,
                    end_reason: None,
                    last_active: None,
                    preview: None,
                    message_count: 2,
                },
                messages: vec![
                    SessionMessageDto {
                        id: 10,
                        session_id: "server-session".to_owned(),
                        role: "user".to_owned(),
                        content: "你好".to_owned(),
                        timestamp: None,
                        tool_call_id: None,
                        tool_name: None,
                        tool_calls: None,
                        reasoning: None,
                        finish_reason: None,
                        display_kind: None,
                        display_metadata: None,
                    },
                    SessionMessageDto {
                        id: 11,
                        session_id: "server-session".to_owned(),
                        role: "assistant".to_owned(),
                        content: "你好！".to_owned(),
                        timestamp: None,
                        tool_call_id: None,
                        tool_name: None,
                        tool_calls: None,
                        reasoning: None,
                        finish_reason: None,
                        display_kind: None,
                        display_metadata: None,
                    },
                ],
            },
            message_limit: 200,
            message_offset: 0,
        };

        reduce(&mut state, AppAction::SessionResumed(Box::new(result)));

        let active = state.active_session.expect("resume 成功应写入活动会话");
        assert_eq!(active.summary.id, "server-session");
        assert_eq!(
            active
                .transcript
                .iter()
                .map(|message| (message.message_id, message.content.as_str()))
                .collect::<Vec<_>>(),
            [(10, "你好"), (11, "你好！")]
        );
        assert_eq!(state.overlay, crate::app::Overlay::None);
    }

    #[test]
    fn composer_keeps_unicode_boundaries_and_ignores_old_turn_deltas() {
        let mut state = AppState::default();
        reduce(&mut state, AppAction::ComposerInsert("你🙂".to_owned()));
        reduce(&mut state, AppAction::ComposerMoveLeft);
        reduce(&mut state, AppAction::ComposerBackspace);
        assert_eq!(state.composer.text, "🙂");

        state.active_turn = Some(crate::app::ActiveTurnView {
            session_id: "session-a".to_owned(),
            turn_id: "turn-new".to_owned(),
            stream_text: String::new(),
            interrupt_pending: false,
            tool_activity: None,
        });
        reduce(
            &mut state,
            AppAction::StreamDeltaReceived {
                session_id: "session-a".to_owned(),
                turn_id: "turn-old".to_owned(),
                text: "忽略".to_owned(),
            },
        );
        reduce(
            &mut state,
            AppAction::StreamDeltaReceived {
                session_id: "session-a".to_owned(),
                turn_id: "turn-new".to_owned(),
                text: "保留".to_owned(),
            },
        );
        reduce(
            &mut state,
            AppAction::ActiveTurnFinished {
                session_id: "session-a".to_owned(),
                turn_id: "turn-new".to_owned(),
            },
        );

        assert_eq!(
            state
                .active_turn
                .as_ref()
                .map(|turn| turn.stream_text.as_str()),
            Some("保留")
        );
    }

    #[test]
    fn disconnect_discards_transient_turn_but_preserves_composer_and_transcript() {
        let mut state = AppState::default();
        state.composer.text = "未发送的内容".to_owned();
        state.composer.cursor_char_index = state.composer.text.chars().count();
        state.active_turn = Some(crate::app::ActiveTurnView {
            session_id: "session-a".to_owned(),
            turn_id: "turn-a".to_owned(),
            stream_text: "未持久化 delta".to_owned(),
            interrupt_pending: false,
            tool_activity: None,
        });

        reduce(
            &mut state,
            AppAction::RpcDisconnected {
                message: "stdout EOF".to_owned(),
            },
        );

        assert!(state.active_turn.is_none());
        assert_eq!(state.composer.text, "未发送的内容");
        assert!(matches!(
            state.status,
            ConnectionStatus::Disconnected {
                retry_after_secs: 1,
                ..
            }
        ));
    }
}
