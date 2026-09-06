//! `AppAction` 到 `AppState` 的确定性归约。

use super::{AppAction, AppState};

/// 将一个外部动作归约到当前 ViewModel。
///
/// reducer 不执行 I/O，也不返回 future；因此同一输入序列的状态变化可用纯单元测试验证。
/// Quit 是单调状态：一旦设为 true，重复动作不会让应用回到运行状态。
#[allow(dead_code)] // 步骤 1 的主事件循环会调用 reducer；步骤 0 保持 main 无终端副作用。
pub fn reduce(state: &mut AppState, action: AppAction) {
    match action {
        AppAction::QuitRequested => state.should_quit = true,
    }
}

#[cfg(test)]
mod tests {
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
}
