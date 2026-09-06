//! Profile 作用域内的 RPC 服务实现。

use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use sagent_protocol::{
    GatewayPingResult, GatewayService, ProtocolError, SessionCreateParams, SessionCreateResult,
    SessionCreateService, SessionListParams, SessionListResult, SessionReadService,
    SessionResumeParams, SessionResumeResult, SessionService, SessionSummaryDto,
};
use sagent_runtime::SessionSupervisor;
use sagent_store::{NewSession, Store};
use sagent_types::SessionId;
use uuid::Uuid;

/// Profile 作用域中的 RPC 服务。
///
/// `session.create` 使用短生命周期读写 Store 创建空会话；真正的 Actor 仍由后续
/// `prompt.submit` 经 `SessionSupervisor` 启动，避免空会话占用 mailbox 或 Provider。
pub struct RuntimeService {
    sessions: SessionService,
    state_db: PathBuf,
    model: String,
    supervisor: Arc<SessionSupervisor>,
    provider_ready: bool,
}

impl RuntimeService {
    /// 将已初始化的只读服务、Actor factory 和当前模型组合为 RPC 适配层。
    pub fn new(
        sessions: SessionService,
        state_db: PathBuf,
        model: String,
        supervisor: Arc<SessionSupervisor>,
        provider_ready: bool,
    ) -> Self {
        Self {
            sessions,
            state_db,
            model,
            supervisor,
            provider_ready,
        }
    }

    /// 返回当前 Profile 的 Supervisor；后续 submit/interrupt 只能经此句柄进入 Actor。
    #[allow(dead_code)] // 步骤 4 的 prompt.submit 将通过该入口取得/启动 SessionActor。
    pub fn supervisor(&self) -> Arc<SessionSupervisor> {
        Arc::clone(&self.supervisor)
    }

    /// Provider 是否已由 bootstrap 完成安全解析。
    #[allow(dead_code)] // 步骤 4 会据此将未配置 Provider 映射为 runtime_unavailable。
    pub fn provider_ready(&self) -> bool {
        self.provider_ready
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

        // 创建空会话不触碰 Supervisor：这里的短生命周期 Store 在提交后立即释放，
        // 之后首次 prompt.submit 才由该 session 唯一 Actor 打开独占读写连接。
        let mut store = Store::open_readwrite(&self.state_db).map_err(store_error)?;
        store
            .create_session(&NewSession {
                id: session_id.clone(),
                source: Some("rpc".to_owned()),
                model: Some(self.model.clone()),
                title,
                started_at,
            })
            .map_err(store_error)?;
        let session = store
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
