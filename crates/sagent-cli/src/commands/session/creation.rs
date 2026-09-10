//! CLI session 的创建和 UTC 标识生成。
//!
//! 创建时间与随机 ID 在写入前一次性生成，避免 Store 事务重试时改变会话身份；此模块不
//! 读取其他 Profile，路径解析仍统一经过 session 父模块。

use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use sagent_store::{NewSession, Store};
use sagent_types::SessionId;
use uuid::Uuid;

use super::current_paths;
/// 创建会话并返回写入数据库的 ID。
pub fn create(
    home: Option<&Path>,
    profile_override: Option<&str>,
    title: Option<String>,
    model: Option<String>,
) -> Result<SessionId> {
    let now = SystemTime::now();
    let session_id = session_id_from_clock(now)?;
    let started_at = rfc3339_now(now)?;
    create_with_id(home, profile_override, session_id, title, model, started_at)
}

/// 使用调用方指定的 ID 与时间创建会话，供生产编排和确定性测试复用。
pub fn create_with_id(
    home: Option<&Path>,
    profile_override: Option<&str>,
    session_id: SessionId,
    title: Option<String>,
    model: Option<String>,
    started_at: String,
) -> Result<SessionId> {
    let paths = current_paths(home, profile_override)?;
    let mut store = Store::open_readwrite(&paths.state_db)
        .with_context(|| format!("打开当前 profile 数据库失败：{}", paths.state_db.display()))?;
    store.create_session(&NewSession {
        id: session_id.clone(),
        source: Some("cli".to_owned()),
        model,
        title,
        started_at,
    })?;
    Ok(session_id)
}

/// 生成与 Python Hermes 兼容的时间前缀，并使用完整 UUID v4 防止碰撞。
pub(super) fn session_id_from_clock(now: SystemTime) -> Result<SessionId> {
    let timestamp = rfc3339_now(now)?;
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

/// 生成当前 UTC 的 RFC 3339 毫秒时间戳。
pub(super) fn rfc3339_now(now: SystemTime) -> Result<String> {
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
