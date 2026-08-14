//! sagent-config 配置加载集成测试。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 1 配置测试

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sagent_config::config::LogLevel;
use sagent_config::{Config, ConfigError, ConfigLoader, ConfigPaths, SynchronousMode};

static TEST_ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let suffix =
            SystemTime::now().duration_since(UNIX_EPOCH).expect("系统时间应有效").as_nanos();
        let sequence = TEST_ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sagent-config-{}-{suffix}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("应创建测试目录");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn loader(root: &TestRoot) -> ConfigLoader {
    ConfigLoader::new(ConfigPaths::from_root(root.path()))
}

#[test]
fn missing_file_returns_complete_defaults_without_creating_file() {
    let root = TestRoot::new();
    let config = loader(&root).load().expect("缺少配置文件应使用默认值");
    assert_eq!(config, Config::default());
    assert!(!root.path().join("config.yaml").exists());
}

#[test]
fn valid_yaml_round_trips_to_typed_config() {
    let root = TestRoot::new();
    fs::write(
        root.path().join("config.yaml"),
        "version: 1\nruntime:\n  shutdown_timeout_ms: 1000\n  max_live_sessions: 4\n  actor_mailbox_capacity: 8\n  event_buffer_capacity: 16\ndatabase:\n  path: data/state.db\n  busy_timeout_ms: 100\n  synchronous: normal\nrpc:\n  max_line_bytes: 1024\n  max_response_bytes: 2048\nlogging:\n  level: debug\n",
    )
    .expect("应写入测试配置");
    let config = loader(&root).load().expect("合法 YAML 应加载");
    assert_eq!(config.runtime.max_live_sessions, 4);
    assert_eq!(
        config.database.path,
        Some(root.path().join("data/state.db"))
    );
    assert_eq!(config.database.synchronous, SynchronousMode::Normal);
    assert_eq!(config.logging.level, LogLevel::Debug);
}

#[test]
fn missing_fields_use_defaults() {
    let root = TestRoot::new();
    let config = loader(&root)
        .load_yaml("runtime:\n  max_live_sessions: 2\n")
        .expect("缺少字段应使用默认值");
    assert_eq!(config.version, 1);
    assert_eq!(config.runtime.max_live_sessions, 2);
    assert_eq!(config.runtime.actor_mailbox_capacity, 256);
    assert_eq!(config.database.busy_timeout_ms, 5_000);
    assert_eq!(config.rpc.max_response_bytes, 4_194_304);
}

#[test]
fn invalid_type_reports_key_path_without_original_value() {
    let root = TestRoot::new();
    let error = loader(&root)
        .load_yaml("runtime:\n  shutdown_timeout_ms: secret-token-value\n")
        .expect_err("非法类型应失败");
    assert!(matches!(error, ConfigError::InvalidType { .. }));
    let message = error.to_string();
    assert!(message.contains("runtime.shutdown_timeout_ms"));
    assert!(!message.contains("secret-token-value"));
}

#[test]
fn unknown_secret_key_is_rejected_without_leaking_value() {
    let root = TestRoot::new();
    let error = loader(&root)
        .load_yaml("api_key: sk-secret-value\n")
        .expect_err("未知 secret 字段应失败");
    assert!(matches!(error, ConfigError::UnknownKey { .. }));
    let message = error.to_string();
    assert!(message.contains("api_key"));
    assert!(!message.contains("sk-secret-value"));
}

#[test]
fn nested_unknown_key_reports_full_key_path() {
    let root = TestRoot::new();
    let error = loader(&root)
        .load_yaml("database:\n  token: hidden\n")
        .expect_err("未知嵌套字段应失败");
    assert_eq!(error.to_string(), "未知配置字段: database.token");
}

#[test]
fn timeout_and_mailbox_boundaries_are_enforced() {
    let root = TestRoot::new();
    assert!(loader(&root).load_yaml("runtime:\n  shutdown_timeout_ms: 0\n").is_err());
    assert!(loader(&root)
        .load_yaml("runtime:\n  shutdown_timeout_ms: 600000\n  actor_mailbox_capacity: 65536\n")
        .is_ok());
    assert!(loader(&root).load_yaml("runtime:\n  actor_mailbox_capacity: 65537\n").is_err());
}

#[test]
fn enum_value_errors_report_key_path() {
    let root = TestRoot::new();
    let error = loader(&root)
        .load_yaml("logging:\n  level: noisy\n")
        .expect_err("未知日志级别应失败");
    assert_eq!(
        error.to_string(),
        "配置字段值无效: logging.level，只支持: trace, debug, info, warn, error"
    );
}

#[test]
fn database_path_null_and_absolute_path_are_stable() {
    let root = TestRoot::new();
    let null_config =
        loader(&root).load_yaml("database:\n  path: null\n").expect("null path 应合法");
    assert_eq!(null_config.database.path, None);
    let absolute = if cfg!(windows) {
        r"C:\\sagent\\state.db"
    } else {
        "/var/tmp/sagent-state.db"
    };
    let yaml = format!("database:\n  path: '{absolute}'\n");
    let config = loader(&root).load_yaml(&yaml).expect("绝对路径应合法");
    assert_eq!(config.database.path, Some(PathBuf::from(absolute)));
}

#[test]
fn same_home_path_is_independent_of_config_file_location() {
    let root = TestRoot::new();
    let paths = ConfigPaths::from_root(root.path());
    let first = ConfigLoader::new(paths.clone())
        .load_yaml("database:\n  path: state.db\n")
        .expect("配置应加载");
    let second = ConfigLoader::new(paths)
        .load_yaml("database:\n  path: state.db\n")
        .expect("相同配置应加载");
    assert_eq!(first, second);
    assert_eq!(first.database.path, Some(root.path().join("state.db")));
}

#[test]
fn loaded_snapshot_does_not_change_when_file_changes() {
    let root = TestRoot::new();
    let config_path = root.path().join("config.yaml");
    fs::write(&config_path, "runtime:\n  max_live_sessions: 2\n").expect("应写入配置");
    let snapshot = loader(&root).load().expect("应加载初始配置");
    fs::write(&config_path, "runtime:\n  max_live_sessions: 9\n").expect("应更新配置");
    assert_eq!(snapshot.runtime.max_live_sessions, 2);
}

#[test]
fn default_config_matches_resolved_fixture() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../protocols/fixtures/config-default.json"
    ))
    .expect("fixture 应为合法 JSON");
    let actual = serde_json::to_value(Config::default()).expect("默认配置应可序列化");
    assert_eq!(actual, fixture);
}
