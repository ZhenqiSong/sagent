//! Profile 作用域内的 RPC 服务实现。

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use sagent_protocol::{
    ConfigReadParams, ConfigReadResult, ConfigReadService, GatewayPingResult, GatewayService,
    ProtocolError, SessionCreateParams, SessionCreateResult, SessionCreateService, SessionEventDto,
    SessionEventsSinceParams, SessionEventsSinceResult, SessionListParams, SessionListResult,
    SessionReadService, SessionResumeParams, SessionResumeResult, SessionService,
    SessionSummaryDto,
};
use sagent_runtime::SessionSupervisor;
use sagent_store::{EventQuery, NewSession, StorageManager};
use sagent_types::SessionId;
use uuid::Uuid;

/// Profile 作用域中的 RPC 服务。
///
/// `session.create` 使用 Manager 创建短生命周期写入端口；真正的 Actor 仍由后续
/// `prompt.submit` 经 `SessionSupervisor` 启动，避免空会话占用 mailbox 或 Provider。
pub struct RuntimeService {
    sessions: SessionService,
    storage_manager: Arc<dyn StorageManager>,
    model: String,
    supervisor: Arc<SessionSupervisor>,
    provider_ready: bool,
    /// 启动时读取的非秘密配置快照；请求不能以参数切换其 Profile。
    public_config: ConfigReadResult,
}

/// `prompt.submit` 所需的可跨 await 使用的运行时快照。
///
/// 它刻意不携带 `SessionService` 的查询端口：dispatcher 不能在 await Actor mailbox
/// 时借用它。会话存在性检查和事件补读都通过 Manager 创建短生命周期端口，Actor 则
/// 始终通过 Supervisor 获取自己的独占端口集合。
#[derive(Clone)]
pub struct RuntimePromptContext {
    storage_manager: Arc<dyn StorageManager>,
    supervisor: Arc<SessionSupervisor>,
    provider_ready: bool,
}

impl RuntimeService {
    /// 将已初始化的只读服务、StorageManager 和当前模型组合为 RPC 适配层。
    pub fn new(
        sessions: SessionService,
        storage_manager: Arc<dyn StorageManager>,
        model: String,
        supervisor: Arc<SessionSupervisor>,
        provider_ready: bool,
        public_config: ConfigReadResult,
    ) -> Self {
        Self {
            sessions,
            storage_manager,
            model,
            supervisor,
            provider_ready,
            public_config,
        }
    }

    /// 返回当前 Profile 的 Supervisor；后续 submit/interrupt 只能经此句柄进入 Actor。
    pub fn supervisor(&self) -> Arc<SessionSupervisor> {
        Arc::clone(&self.supervisor)
    }

    /// 提取不含共享数据库连接的 prompt 运行时快照。
    pub fn prompt_context(&self) -> RuntimePromptContext {
        RuntimePromptContext {
            storage_manager: Arc::clone(&self.storage_manager),
            supervisor: self.supervisor(),
            provider_ready: self.provider_ready,
        }
    }
}

impl ConfigReadService for RuntimeService {
    /// 直接克隆启动快照，避免 RPC handler 重新读取 YAML 或 `.env` 并造成配置漂移。
    fn read_config(&self, _: &ConfigReadParams) -> Result<ConfigReadResult, ProtocolError> {
        Ok(self.public_config.clone())
    }
}

impl RuntimePromptContext {
    /// Provider 是否已在启动时通过安全的 Profile 配置解析。
    pub fn provider_ready(&self) -> bool {
        self.provider_ready
    }

    /// 返回 SessionActor 的唯一入口。
    pub fn supervisor(&self) -> Arc<SessionSupervisor> {
        Arc::clone(&self.supervisor)
    }

    /// 在启动 Actor 前验证会话已持久化到当前 Profile。
    pub fn require_session(&self, session_id: &SessionId) -> Result<(), ProtocolError> {
        let storage = self
            .storage_manager
            .open_read_storage()
            .map_err(store_error)?;
        if storage
            .session
            .get_session(session_id)
            .map_err(store_error)?
            .is_some()
        {
            Ok(())
        } else {
            Err(ProtocolError::SessionNotFound(
                session_id.as_str().to_owned(),
            ))
        }
    }

    /// 从当前 Profile 的持久化事件日志补读一个稳定页面。
    ///
    /// 运行时 delta 从不写入 daemon_events，因而这里天然只会返回可恢复事实。sequence
    /// 是全库递增的游标而非 session 内连续计数，`has_more` 必须与本 session 的最新值
    /// 比较，不能假定下一条序号相邻。
    pub fn events_since(
        &self,
        params: &SessionEventsSinceParams,
    ) -> Result<SessionEventsSinceResult, ProtocolError> {
        const DEFAULT_LIMIT: u32 = 50;
        const MAX_LIMIT: u32 = 200;

        let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let storage = self
            .storage_manager
            .open_read_storage()
            .map_err(store_error)?;
        let query = &storage.session;
        if query
            .get_session(&params.session_id)
            .map_err(store_error)?
            .is_none()
        {
            return Err(ProtocolError::SessionNotFound(
                params.session_id.as_str().to_owned(),
            ));
        }
        let events = query
            .events_since(&EventQuery {
                session_id: params.session_id.clone(),
                after_sequence: params.after_sequence,
                limit: i64::from(limit),
            })
            .map_err(store_error)?;
        let latest_sequence = query
            .latest_event_sequence(&params.session_id)
            .map_err(store_error)?
            .unwrap_or_default();
        let last_sequence = events
            .last()
            .map(|event| event.sequence)
            .unwrap_or(params.after_sequence);

        Ok(SessionEventsSinceResult {
            // DTO 映射隔离存储实现的内部记录，后续调整 SQLite 行结构不会改变线上协议。
            events: events.into_iter().map(event_dto).collect(),
            has_more: latest_sequence > last_sequence,
            latest_sequence,
        })
    }
}

/// 将已提交的 daemon event 变为协议事件，而不是嵌套另一层 JSON-RPC envelope。
fn event_dto(event: sagent_store::StoredDaemonEvent) -> SessionEventDto {
    SessionEventDto {
        sequence: event.sequence,
        session_id: event.session_id,
        turn_id: event.turn_id,
        event_type: event.event_type,
        payload: event.payload,
        created_at: event.created_at,
    }
}

impl GatewayService for RuntimeService {
    fn ping(&self) -> GatewayPingResult {
        GatewayPingResult {
            ok: true,
            protocol_version: sagent_protocol::PROTOCOL_VERSION,
        }
    }
}

impl SessionReadService for RuntimeService {
    fn list_sessions(
        &self,
        params: &SessionListParams,
    ) -> Result<SessionListResult, ProtocolError> {
        self.sessions.list_sessions(params)
    }

    fn resume_session(
        &self,
        params: &SessionResumeParams,
    ) -> Result<SessionResumeResult, ProtocolError> {
        self.sessions.resume_session(params)
    }
}

impl SessionCreateService for RuntimeService {
    fn create_session(
        &self,
        params: &SessionCreateParams,
    ) -> Result<SessionCreateResult, ProtocolError> {
        let title = validate_title(params.title.as_deref())?;
        let started_at = rfc3339_now(SystemTime::now())
            .map_err(|_| ProtocolError::RuntimeUnavailable("clock unavailable".to_owned()))?;
        let session_id = SessionId::new(format!("rpc_{}", Uuid::new_v4().simple()));

        // 创建空会话不触碰 Supervisor：这里先申请最小写端口并在作用域结束时释放，
        // 之后首次 prompt.submit 才由该 session 唯一 Actor 获取独占端口集合。
        let mut storage = self
            .storage_manager
            .open_write_storage()
            .map_err(store_error)?;
        storage
            .session
            .create_session(&NewSession {
                id: session_id.clone(),
                source: Some("rpc".to_owned()),
                model: Some(self.model.clone()),
                title,
                started_at,
            })
            .map_err(store_error)?;
        drop(storage);

        // 写入已提交后再通过只读入口读取完整摘要，保持 RPC 的读写能力边界清晰。
        let read_storage = self
            .storage_manager
            .open_read_storage()
            .map_err(store_error)?;
        let session = read_storage
            .session
            .get_session(&session_id)
            .map_err(store_error)?
            .ok_or_else(|| ProtocolError::Internal("created session was not found".to_owned()))?;

        Ok(SessionCreateResult {
            session_id,
            session: SessionSummaryDto::from(session),
        })
    }
}

fn validate_title(title: Option<&str>) -> Result<Option<String>, ProtocolError> {
    let Some(title) = title else {
        return Ok(None);
    };
    let title = title.trim();
    if title.is_empty() {
        return Err(ProtocolError::InvalidParams(
            "title must not be blank".to_owned(),
        ));
    }
    Ok(Some(title.to_owned()))
}

fn store_error(error: anyhow::Error) -> ProtocolError {
    // SQLite 路径、系统用户名等诊断只留在调用方 stderr，协议错误保持稳定且无环境细节。
    ProtocolError::StoreUnavailable(error.to_string())
}

/// 生成用于会话元数据的 UTC RFC 3339 毫秒时间。
fn rfc3339_now(now: SystemTime) -> Result<String, std::time::SystemTimeError> {
    let duration = now.duration_since(UNIX_EPOCH)?;
    let seconds = duration.as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let seconds_in_day = seconds.rem_euclid(86_400);
    let (year, month, day) = utc_date_from_days(days);
    let hour = seconds_in_day / 3_600;
    let minute = seconds_in_day % 3_600 / 60;
    let second = seconds_in_day % 60;
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        duration.subsec_millis()
    ))
}

fn utc_date_from_days(days_since_unix_epoch: i64) -> (i64, u32, u32) {
    let days = days_since_unix_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    (year, month as u32, day as u32)
}
