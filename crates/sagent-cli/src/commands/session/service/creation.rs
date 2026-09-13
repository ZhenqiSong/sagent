//! Session 创建服务与 UTC 标识生成。
//!
//! 创建时间与随机 ID 在写入前一次性生成，避免存储操作重试时改变会话身份；存储装配
//! 由 `SessionService` 统一完成。
//!
//! 作者：SongZQ

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use sagent_store::NewSession;
use sagent_types::SessionId;
use uuid::Uuid;

use super::SessionService;

impl SessionService {
    /// 在当前命令上下文中生成身份并创建会话。
    pub(crate) fn create_session(
        &self,
        title: Option<String>,
        model: Option<String>,
    ) -> Result<SessionId> {
        let now = SystemTime::now();
        let session_id = Self::session_id_from_clock(now)?;
        let started_at = Self::rfc3339_now(now)?;
        self.create_session_with_id(session_id, title, model, started_at)
    }

    /// 在当前命令上下文中使用指定身份创建会话。
    pub(crate) fn create_session_with_id(
        &self,
        session_id: SessionId,
        title: Option<String>,
        model: Option<String>,
        started_at: String,
    ) -> Result<SessionId> {
        let mut dependencies = self.storage()?.open_write()?;
        dependencies.session_mut().create_session(&NewSession {
            id: session_id.clone(),
            source: Some("cli".to_owned()),
            model,
            title,
            started_at,
        })?;
        Ok(session_id)
    }

    /// 根据指定时刻生成兼容的会话 ID，使用完整 UUID v4 防止碰撞。
    pub(crate) fn session_id_from_clock(now: SystemTime) -> Result<SessionId> {
        let timestamp = Self::rfc3339_now(now)?;
        let (date, time_with_zone) = timestamp
            .split_once('T')
            .context("无法生成会话 ID 时间前缀")?;
        let time = time_with_zone
            .get(..8)
            .context("无法读取会话 ID 的时间部分")?;
        let prefix = format!("{}_{}", date.replace('-', ""), time.replace(':', ""));
        Ok(SessionId::new(format!(
            "{}_{}",
            prefix,
            Uuid::new_v4().simple()
        )))
    }

    /// 生成指定时刻的 UTC RFC 3339 毫秒时间戳。
    pub(crate) fn rfc3339_now(now: SystemTime) -> Result<String> {
        let duration = now
            .duration_since(UNIX_EPOCH)
            .context("系统时间早于 Unix epoch，无法创建会话")?;
        let seconds = i64::try_from(duration.as_secs()).context("系统时间超出可表示范围")?;
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
}

/// 把 Unix 秒数转换为 UTC 公历日期。
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
