use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

use crate::redact::redact_str;

// ── 日志模式 ──

/// 日志运行模式，影响日志输出的目标与格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogMode {
    /// 开发模式：输出到 stderr，人类可读格式，DEBUG 级别
    Development,
    /// 生产模式：输出到文件，JSON 格式，INFO 级别
    Production,
    /// Gateway 模式：输出到文件，JSON 格式，INFO 级别，额外 gateway 日志
    Gateway,
}

/// 存储 setup_logging 时传入的 sagent_home 路径。
static SAGENT_HOME: OnceLock<String> = OnceLock::new();

// ── RedactingWriter ──

/// 包装 `io::Write` 的写入器，在每次 `write()` 调用时对输出内容进行敏感信息脱敏。
///
/// 核心思路：tracing-subscriber 的 `fmt::Layer` 最终通过 `io::Write` 写入格式化后的日志行。
/// 我们在写入器层面按行拦截，对 JWT/Slack token/API key/手机号/邮箱/UUID 做正则脱敏后再写入底层 writer。
#[derive(Debug)]
pub struct RedactingWriter<W> {
    inner: W,
    /// 缓冲区，用于累积不完整的行（跨 write 调用）
    buffer: Arc<Mutex<Vec<u8>>>,
}

// 手动实现 Clone（buffer 用 Arc 共享）
impl<W: Clone> Clone for RedactingWriter<W> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            buffer: Arc::clone(&self.buffer),
        }
    }
}

impl<W: io::Write> RedactingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl<W: io::Write> io::Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut buffer = self.buffer.lock().unwrap();
        buffer.extend_from_slice(buf);

        while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
            let line_bytes = &buffer[..=pos];
            let line = String::from_utf8_lossy(line_bytes);
            let redacted = redact_str(&line);
            self.inner.write_all(redacted.as_bytes())?;
            buffer.drain(..=pos);
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut buffer = self.buffer.lock().unwrap();
        if !buffer.is_empty() {
            let line = String::from_utf8_lossy(&buffer);
            let redacted = redact_str(&line);
            self.inner.write_all(redacted.as_bytes())?;
            buffer.clear();
        }
        self.inner.flush()
    }
}

// ── 公共入口 ──

/// 初始化全局日志系统。
///
/// # 参数
/// - `mode`: 日志运行模式（Development / Production / Gateway）。
/// - `sagent_home`: sagent 主目录路径，日志文件将写入 `<sagent_home>/logs/`。
///
/// # 日志文件
/// - Development: 仅 stderr，不写文件，DEBUG 级别。
/// - Production: `<sagent_home>/logs/agent.log` + `<sagent_home>/logs/errors.log`（仅 ERROR），INFO 级别。
/// - Gateway: 额外 `<sagent_home>/logs/gateway.log`，INFO 级别。
///
/// # 脱敏
/// 所有输出（stderr 和文件）均通过 `RedactingWriter` 进行敏感信息脱敏。
///
/// # 错误
/// 如果日志目录创建失败或日志文件无法打开，返回 `anyhow::Error`。
/// 如果 tracing subscriber 已被初始化（重复调用），返回错误。
pub fn setup_logging(mode: LogMode, sagent_home: &Path) -> anyhow::Result<()> {
    SAGENT_HOME
        .set(sagent_home.to_string_lossy().into_owned())
        .ok();

    let logs_dir = sagent_home.join("logs");
    std::fs::create_dir_all(&logs_dir)?;

    match mode {
        LogMode::Development => setup_development()?,
        LogMode::Production => setup_production(&logs_dir, false)?,
        LogMode::Gateway => setup_production(&logs_dir, true)?,
    }

    tracing::info!(
        mode = ?mode,
        sagent_home = %sagent_home.display(),
        "日志系统初始化完成"
    );

    Ok(())
}

// ── 内部实现 ──

fn setup_development() -> anyhow::Result<()> {
    // stderr 通过闭包每次创建新的 RedactingWriter（Stderr 不实现 Clone）
    let make_writer = || RedactingWriter::new(io::stderr());

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(make_writer)
        .with_target(true)
        .with_thread_ids(true)
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
            !metadata.target().starts_with("tracing_appender")
        }));

    tracing_subscriber::registry()
        .with(stderr_layer)
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("debug")),
        )
        .try_init()
        .map_err(|e| anyhow::anyhow!("日志系统重复初始化：{e}"))?;

    Ok(())
}

fn setup_production(logs_dir: &Path, is_gateway: bool) -> anyhow::Result<()> {
    // agent.log — 非阻塞写入 + 脱敏
    let agent_file = tracing_appender::rolling::never(logs_dir, "agent.log");
    let (agent_writer, _guard_agent) = tracing_appender::non_blocking(agent_file);
    // NonBlocking 实现了 Clone，闭包每次创建带脱敏的新 writer
    let make_agent = { let w = agent_writer.clone(); move || RedactingWriter::new(w.clone()) };

    let agent_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(make_agent)
        .with_target(true)
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
            !metadata.target().starts_with("tracing_appender")
        }));

    // errors.log — 仅 ERROR 级别 + 脱敏
    let errors_file = tracing_appender::rolling::never(logs_dir, "errors.log");
    let (errors_writer, _guard_errors) = tracing_appender::non_blocking(errors_file);
    let make_errors = { let w = errors_writer.clone(); move || RedactingWriter::new(w.clone()) };

    let errors_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(make_errors)
        .with_target(true)
        .with_filter(tracing_subscriber::filter::LevelFilter::ERROR);

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    if is_gateway {
        // gateway.log — 额外日志 + 脱敏
        let gateway_file = tracing_appender::rolling::never(logs_dir, "gateway.log");
        let (gateway_writer, _guard_gateway) = tracing_appender::non_blocking(gateway_file);
        let make_gateway = { let w = gateway_writer.clone(); move || RedactingWriter::new(w.clone()) };

        let gateway_layer = tracing_subscriber::fmt::layer()
            .json()
            .with_writer(make_gateway)
            .with_target(true)
            .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                metadata.target().starts_with("sagent_gateway")
                    && !metadata.target().starts_with("tracing_appender")
            }));

        tracing_subscriber::registry()
            .with(agent_layer)
            .with(errors_layer)
            .with(gateway_layer)
            .with(env_filter)
            .try_init()
            .map_err(|e| anyhow::anyhow!("日志系统重复初始化：{e}"))?;
    } else {
        tracing_subscriber::registry()
            .with(agent_layer)
            .with(errors_layer)
            .with(env_filter)
            .try_init()
            .map_err(|e| anyhow::anyhow!("日志系统重复初始化：{e}"))?;
    }

    Ok(())
}

// ── 辅助函数 ──

/// 获取 setup 时传入的 sagent_home 路径。
/// 如果日志系统尚未初始化，返回空字符串。
pub fn get_sagent_home() -> &'static str {
    SAGENT_HOME.get().map(|s| s.as_str()).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_sagent_home() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let home = dir.path().to_path_buf();
        (dir, home)
    }

    #[test]
    fn test_log_mode_debug_format() {
        assert_eq!(format!("{:?}", LogMode::Development), "Development");
        assert_eq!(format!("{:?}", LogMode::Production), "Production");
        assert_eq!(format!("{:?}", LogMode::Gateway), "Gateway");
    }

    #[test]
    fn test_setup_development_does_not_panic() {
        let (_dir, home) = temp_sagent_home();
        let result = setup_logging(LogMode::Development, &home);
        let _ = result;
    }

    #[test]
    fn test_redacting_writer_redacts_sensitive_data() {
        let mut output = Vec::new();
        {
            let mut writer = RedactingWriter::new(&mut output);
            write!(writer, "用户邮箱: test@example.com\n").unwrap();
            writer.flush().unwrap();
        }
        let result = String::from_utf8(output).unwrap();
        assert!(result.contains("[邮箱已脱敏]"));
        assert!(!result.contains("test@example.com"));
    }

    #[test]
    fn test_redacting_writer_preserves_non_sensitive_data() {
        let mut output = Vec::new();
        {
            let mut writer = RedactingWriter::new(&mut output);
            write!(writer, "这是一条普通日志，不含敏感信息。\n").unwrap();
            writer.flush().unwrap();
        }
        let result = String::from_utf8(output).unwrap();
        assert!(result.contains("普通日志"));
        assert!(!result.contains("已脱敏"));
    }

    #[test]
    fn test_redacting_writer_partial_line_flushed() {
        let mut output = Vec::new();
        {
            let mut writer = RedactingWriter::new(&mut output);
            write!(writer, "phone: 13812345678").unwrap();
            writer.flush().unwrap();
        }
        let result = String::from_utf8(output).unwrap();
        assert!(result.contains("[手机号已脱敏]"));
        assert!(!result.contains("13812345678"));
    }

    #[test]
    fn test_redacting_writer_multiple_sensitive_patterns() {
        let mut output = Vec::new();
        {
            let mut writer = RedactingWriter::new(&mut output);
            write!(
                writer,
                "email: admin@test.com, phone: 13900001111, token: sk-abcdefghijklmnopqrstuvwxyz123456\n"
            )
            .unwrap();
            writer.flush().unwrap();
        }
        let result = String::from_utf8(output).unwrap();
        assert!(result.contains("[邮箱已脱敏]"));
        assert!(result.contains("[手机号已脱敏]"));
        assert!(result.contains("[APIKey已脱敏]"));
    }

    #[test]
    fn test_get_sagent_home_before_setup() {
        let home = get_sagent_home();
        let _ = home;
    }
}
