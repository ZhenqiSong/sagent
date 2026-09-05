//! 真实 Provider smoke test。
//!
//! 这些测试默认不会运行，也不会访问网络。只有显式设置
//! `SAGENT_RUN_LIVE_TESTS=1` 并提供隔离的 `SAGENT_SMOKE_HOME` 后，才会读取
//! 测试 Profile、调用真实 endpoint。API key 仍由 Profile 的 `.env` 和
//! `api_key_env` 解析，测试代码不会读取或打印密钥。

use std::{
    env,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, bail};
use sagent_agent::{RequestId, UserInput};
use sagent_config::{normalize_profile_name, resolve_openai_provider, resolve_paths};
use sagent_runtime::{
    RuntimeEventKind, RuntimeEventSubscription, SessionHandle, SessionSupervisor,
};
use sagent_store::{MessageQuery, NewSession, Store};
use sagent_types::SessionId;

const LIVE_SWITCH: &str = "SAGENT_RUN_LIVE_TESTS";
const LIVE_HOME: &str = "SAGENT_SMOKE_HOME";
const LIVE_PROFILE: &str = "SAGENT_SMOKE_PROFILE";
static LIVE_SESSION_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static LIVE_TEST_LOCK: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();

struct LiveSession {
    state_db: PathBuf,
    session_id: SessionId,
    model: String,
    profile_revision: String,
    supervisor: SessionSupervisor,
    handle: SessionHandle,
    // 多个 ignored test 可能同时运行；同一个 Profile 只能串行写 state.db。
    _lock: tokio::sync::OwnedMutexGuard<()>,
}

/// 准备一次真实 smoke test 所需的隔离 Profile。
///
/// `None` 表示没有显式启用 live test；这条路径不读取配置、不打开数据库、
/// 不创建 HTTP client，从而保证普通离线测试不会触碰网络或用户数据。
async fn prepare_live_session() -> Result<Option<LiveSession>> {
    if env::var(LIVE_SWITCH).as_deref() != Ok("1") {
        eprintln!("跳过真实 Provider smoke test；设置 {LIVE_SWITCH}=1 后才会运行。");
        return Ok(None);
    }

    let lock = LIVE_TEST_LOCK
        .get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
        .lock_owned()
        .await;

    let home = env::var_os(LIVE_HOME).ok_or_else(|| {
        anyhow::anyhow!("运行真实 smoke test 还需要 {LIVE_HOME}，它必须指向隔离的测试 Sagent Home")
    })?;
    let home = PathBuf::from(home);
    let profile_name = env::var(LIVE_PROFILE).unwrap_or_else(|_| "default".into());
    let profile = normalize_profile_name(&profile_name)?;
    let paths = resolve_paths(Some(&home), Some(&profile))?;
    let resolved = resolve_openai_provider(&paths, None, None)?;
    let model = resolved.model.clone();
    let profile_revision = format!("live:{}", profile.as_str());

    let session_id = SessionId::new(format!(
        "live-smoke-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        LIVE_SESSION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut store = Store::open_readwrite(&paths.state_db)?;
    store.create_session(&NewSession {
        id: session_id.clone(),
        source: Some("live-provider-smoke".into()),
        model: Some(model.clone()),
        title: Some("live provider smoke test".into()),
        started_at: "2026-09-05T00:00:00Z".into(),
    })?;
    drop(store);

    let state_db = paths.state_db.clone();
    let provider = Arc::new(resolved.client);
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&state_db).map_err(|error| error.to_string())
    })
    .with_provider(provider, model.clone(), profile_revision.clone());
    let handle = supervisor.get_or_start(session_id.clone()).await?;

    Ok(Some(LiveSession {
        state_db: paths.state_db,
        session_id,
        model,
        profile_revision,
        supervisor,
        handle,
        _lock: lock,
    }))
}

async fn wait_for_completion(events: &mut RuntimeEventSubscription) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(120), async {
        let mut saw_delta = false;
        loop {
            let event = events.recv().await?;
            match event.kind {
                RuntimeEventKind::ModelTextDelta { text } => {
                    if !text.is_empty() {
                        saw_delta = true;
                    }
                }
                RuntimeEventKind::TurnCompleted => {
                    if !saw_delta {
                        bail!("真实 Provider 未产生任何文本 delta");
                    }
                    return Ok::<(), anyhow::Error>(());
                }
                RuntimeEventKind::TurnFailed { reason } => {
                    bail!("真实 Provider 回合失败：{reason}");
                }
                RuntimeEventKind::TurnInterrupted => bail!("真实 Provider 回合被意外中断"),
                _ => {}
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("等待真实 Provider 回合完成超时"))??;
    Ok(())
}

#[tokio::test]
#[ignore = "需要显式测试 Profile 和真实 API 凭据"]
async fn live_provider_round_trip_persists_assistant_message() -> Result<()> {
    let Some(live) = prepare_live_session().await? else {
        return Ok(());
    };
    let mut events = live.handle.subscribe();
    live.handle
        .submit(
            RequestId::new(),
            UserInput::new("只回答 OK，不要添加其它内容")?,
        )
        .await?;
    wait_for_completion(&mut events).await?;

    let store = Store::open_readonly(&live.state_db)?;
    let messages = store.get_messages_for_display(&live.session_id, &MessageQuery::default())?;
    let assistant = messages
        .last()
        .filter(|message| message.role == "assistant")
        .ok_or_else(|| anyhow::anyhow!("真实 Provider 完成后没有 assistant 消息"))?;
    if assistant.content.trim().is_empty() {
        bail!("真实 Provider 产生了空 assistant 消息");
    }
    let generation = store
        .get_generation(&live.session_id, 0)?
        .ok_or_else(|| anyhow::anyhow!("真实 Provider 完成后没有 generation"))?;
    assert_eq!(generation.model_id, live.model);
    assert_eq!(generation.profile_revision, live.profile_revision);

    live.handle.close().await?;
    live.supervisor.remove(&live.session_id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "需要显式测试 Profile 和真实 API 凭据"]
async fn live_provider_cancel_stops_active_turn() -> Result<()> {
    let Some(live) = prepare_live_session().await? else {
        return Ok(());
    };
    let mut events = live.handle.subscribe();
    live.handle
        .submit(
            RequestId::new(),
            UserInput::new("请输出一篇很长的文章，尽量持续生成内容")?,
        )
        .await?;
    live.handle.interrupt(RequestId::new()).await?;

    let interrupted = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let event = events.recv().await?;
            match event.kind {
                RuntimeEventKind::TurnInterrupted => return Ok::<bool, anyhow::Error>(true),
                RuntimeEventKind::TurnCompleted => return Ok(false),
                RuntimeEventKind::TurnFailed { reason } => {
                    bail!("取消测试中的 Provider 失败：{reason}")
                }
                _ => {}
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("等待真实 Provider 取消结果超时"))??;
    if !interrupted {
        bail!("真实 Provider 在取消请求前已经完成回合，无法验证取消路径");
    }

    let store = Store::open_readonly(&live.state_db)?;
    let messages = store.get_messages_for_display(&live.session_id, &MessageQuery::default())?;
    assert!(messages.iter().all(|message| message.role != "assistant"));

    live.handle.close().await?;
    live.supervisor.remove(&live.session_id).await?;
    Ok(())
}
