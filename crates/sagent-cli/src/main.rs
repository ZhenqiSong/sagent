//! Sagent 命令行入口。
//!
//! 作者：SongZQ

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sagent_config::{
    list_profile_names, normalize_profile_name, paths::platform_default_home, paths::profile_root,
    read_active_profile, resolve_active_paths, set_active_profile,
};
use sagent_store::{MessageQuery, MessageSearchQuery, NewSession, Store};
use sagent_types::SessionId;
use uuid::Uuid;

/// Sagent 命令行参数。
#[derive(Debug, Parser)]
#[command(name = "sagent", version, about = "Sagent 命令行工具")]
struct Cli {
    /// 覆盖 Sagent 根目录；必须是绝对路径，主要用于隔离部署和测试。
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    /// 仅对本次命令覆盖当前 profile，不修改 active-profile。
    #[arg(long, global = true)]
    profile: Option<String>,
    #[command(subcommand)]
    command: Command,
}

/// 一级命令。
#[derive(Debug, Subcommand)]
enum Command {
    /// 管理独立的 Sagent profile。
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// 查看当前 profile 的持久化会话。
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
}

/// profile 子命令。
#[derive(Debug, Subcommand)]
enum ProfileCommand {
    /// 列出默认 profile 与根目录中所有可用的命名 profile。
    List,
    /// 创建一个拥有独立配置和会话数据库的命名 profile。
    Create {
        /// profile 名称；会规范化为小写。
        name: String,
    },
    /// 选择后续命令默认使用的 profile。
    Use {
        /// 已存在的 profile 名称；default 表示根目录 profile。
        name: String,
    },
}

/// session 子命令。
#[derive(Debug, Subcommand)]
enum SessionCommand {
    /// 在当前 profile 中创建空会话。
    Create {
        /// 可选的会话标题。
        #[arg(long)]
        title: Option<String>,
        /// 可选的模型标识。
        #[arg(long)]
        model: Option<String>,
    },
    /// 显示一条会话的元数据和用户可见消息历史。
    Show {
        /// 要查看的完整会话 ID。
        session_id: String,
        /// 最多显示的消息数量，超过时保留最新消息。
        #[arg(long, default_value_t = 50)]
        limit: u32,
        /// 从最新消息向前跳过的数量。
        #[arg(long, default_value_t = 0)]
        offset: u32,
    },
    /// 在当前 profile 的消息历史中全文搜索。
    Search {
        /// FTS5 查询表达式。
        query: String,
        /// 最多返回的命中数量。
        #[arg(long, default_value_t = 20)]
        limit: u32,
        /// 限定到一个会话；默认搜索当前 profile 的全部会话。
        #[arg(long)]
        session_id: Option<String>,
    },
    /// 按最后活动时间倒序列出会话。
    List {
        /// 最多返回的会话数量。
        #[arg(long, default_value_t = 20)]
        limit: u32,
        /// 跳过前面的会话数量。
        #[arg(long, default_value_t = 0)]
        offset: u32,
    },
}

/// 解析 profile list 所使用的根目录。
///
/// 当 home 恰好指向某个命名 profile 时，仍回到其父根目录列出全部 profile。
fn profile_list_root(home: Option<&Path>) -> Result<PathBuf> {
    let home = home
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("SAGENT_HOME").map(PathBuf::from))
        .unwrap_or_else(platform_default_home);
    if !home.is_absolute() {
        anyhow::bail!("--home 必须是绝对路径");
    }
    Ok(profile_root(&home))
}

/// 返回 profile list 的稳定文本行，便于 CLI 输出与单元测试复用。
fn profile_list_lines(home: Option<&Path>) -> Result<Vec<String>> {
    let root = profile_list_root(home)?;
    let active = read_active_profile(&root)?;
    Ok(list_profile_names(&root)?
        .into_iter()
        .map(|profile| {
            if profile == active {
                format!("* {}", profile.as_str())
            } else {
                format!("  {}", profile.as_str())
            }
        })
        .collect())
}

/// 一个新 profile 的最小配置文件。
///
/// 此处只建立合法的空 YAML 文档；具体模型、Provider 等用户配置会在后续
/// setup 命令中写入，避免 create 命令猜测用户的运行偏好。
const INITIAL_CONFIG_YAML: &str = "# Sagent profile configuration.\n{}\n";

/// 创建命名 profile 的目录、初始配置和 SQLite 状态库。
///
/// 创建只允许落在根目录的 profiles 直接子目录中。初始化失败时会删除本次刚
/// 创建的 profile 目录，避免留下可被 list 误认为完整 profile 的半成品。
fn create_profile(home: Option<&Path>, name: &str) -> Result<PathBuf> {
    let root = profile_list_root(home)?;
    let profile = normalize_profile_name(name)?;
    if profile.as_str() == "default" {
        anyhow::bail!("default profile 使用根目录，无需创建");
    }

    create_profile_with_initializer(&root, profile.as_str(), |profile_dir| {
        fs::write(profile_dir.join("config.yaml"), INITIAL_CONFIG_YAML)
            .context("写入初始 config.yaml 失败")?;
        Store::open_readwrite(&profile_dir.join("state.db"))
            .context("初始化 profile state.db 失败")?;
        Ok(())
    })
}

/// 执行创建目录和失败回滚；初始化器抽出后可精确测试半成品清理行为。
fn create_profile_with_initializer(
    root: &Path,
    name: &str,
    initialize: impl FnOnce(&Path) -> Result<()>,
) -> Result<PathBuf> {
    let profile = normalize_profile_name(name)?;
    if profile.as_str() == "default" {
        anyhow::bail!("default profile 使用根目录，无需创建");
    }
    if !root.is_absolute() {
        anyhow::bail!("--home 必须是绝对路径");
    }

    let profile_dir = profile_root(root).join("profiles").join(profile.as_str());
    let parent = profile_dir
        .parent()
        .expect("profile 目录始终位于 profiles 子目录中");
    fs::create_dir_all(parent)
        .with_context(|| format!("创建 profile 父目录失败：{}", parent.display()))?;
    fs::create_dir(&profile_dir)
        .with_context(|| format!("创建 profile '{}' 失败；名称可能已经存在", profile.as_str()))?;

    if let Err(error) = initialize(&profile_dir) {
        fs::remove_dir_all(&profile_dir).with_context(|| {
            format!(
                "清理初始化失败的 profile 目录失败：{}",
                profile_dir.display()
            )
        })?;
        return Err(error);
    }
    Ok(profile_dir)
}

/// 选择一个 profile 并返回规范化后的名称。
fn use_profile(home: Option<&Path>, name: &str) -> Result<String> {
    let root = profile_list_root(home)?;
    let profile = normalize_profile_name(name)?;
    set_active_profile(&root, &profile)?;
    Ok(profile.as_str().to_owned())
}

/// 解析当前命令实际访问的 profile 路径。
fn current_paths(
    home: Option<&Path>,
    profile_override: Option<&str>,
) -> Result<sagent_config::SagentPaths> {
    let profile = profile_override.map(normalize_profile_name).transpose()?;
    resolve_active_paths(home, profile.as_ref())
}

/// 返回 session list 的稳定文本行。
fn session_list_lines(
    home: Option<&Path>,
    profile_override: Option<&str>,
    limit: u32,
    offset: u32,
) -> Result<Vec<String>> {
    let paths = current_paths(home, profile_override)?;
    let store = Store::open_readonly(&paths.state_db)
        .with_context(|| format!("打开当前 profile 数据库失败：{}", paths.state_db.display()))?;
    Ok(store
        .list_sessions(limit, offset)?
        .into_iter()
        .map(|session| {
            format!(
                "{}\t{}\t{}\t{}",
                session.id.as_str(),
                session.title.as_deref().unwrap_or("-"),
                session.message_count,
                session.last_active.as_deref().unwrap_or("-")
            )
        })
        .collect())
}

/// 返回 session show 的稳定文本行。
///
/// 展示读取必须使用 get_messages_for_display：它保留压缩前的原始历史，
/// 同时排除仅供模型恢复上下文的 hidden 压缩摘要。
fn session_show_lines(
    home: Option<&Path>,
    profile_override: Option<&str>,
    session_id: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<String>> {
    let paths = current_paths(home, profile_override)?;
    let store = Store::open_readonly(&paths.state_db)
        .with_context(|| format!("打开当前 profile 数据库失败：{}", paths.state_db.display()))?;
    let session_id = SessionId::new(session_id);
    let session = store
        .get_session(&session_id)?
        .with_context(|| format!("会话不存在：{}", session_id.as_str()))?;
    let messages = store.get_messages_for_display(
        &session_id,
        &MessageQuery {
            limit: Some(limit),
            offset,
            latest: true,
            ..MessageQuery::default()
        },
    )?;

    let mut lines = vec![
        format!("ID: {}", session.id.as_str()),
        format!("标题: {}", session.title.as_deref().unwrap_or("-")),
        format!("来源: {}", session.source.as_deref().unwrap_or("-")),
        format!("模型: {}", session.model.as_deref().unwrap_or("-")),
        format!("开始时间: {}", session.started_at.as_deref().unwrap_or("-")),
        format!("结束时间: {}", session.ended_at.as_deref().unwrap_or("-")),
        format!("消息数: {}", session.message_count),
        "消息:".to_owned(),
    ];
    lines.extend(
        messages
            .into_iter()
            .map(|message| format!("[{}] {}", message.role, message.content)),
    );
    Ok(lines)
}

/// 返回 session search 的稳定文本行。
///
/// 默认搜索活动消息和压缩归档消息；回退、重试前等普通非活动消息由 Store
/// 过滤，避免搜索结果泄露已经被用户撤回的分支。
fn session_search_lines(
    home: Option<&Path>,
    profile_override: Option<&str>,
    query: &str,
    limit: u32,
    session_id: Option<&str>,
) -> Result<Vec<String>> {
    let paths = current_paths(home, profile_override)?;
    let store = Store::open_readonly(&paths.state_db)
        .with_context(|| format!("打开当前 profile 数据库失败：{}", paths.state_db.display()))?;
    let mut search = MessageSearchQuery::new(query);
    search.limit = limit;
    search.session_id = session_id.map(SessionId::new);
    Ok(store
        .search_messages(&search)?
        .into_iter()
        .map(|hit| {
            format!(
                "{}\t{}\t{:.6}\t{}",
                hit.session_id.as_str(),
                hit.message_id.expect("消息搜索命中必须包含消息 ID").get(),
                hit.rank.unwrap_or_default(),
                hit.snippet
            )
        })
        .collect())
}

/// 把 Unix 秒数转换为 UTC 公历日期。
///
/// 采用公历 400 年周期算法，避免仅为 CLI 创建时间戳引入额外时间库。
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

/// 生成当前 UTC 的 RFC 3339 毫秒时间戳。
fn rfc3339_now(now: SystemTime) -> Result<String> {
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

/// 生成与 Python Hermes 兼容的时间前缀，并使用完整 UUID v4 防止碰撞。
fn session_id_from_clock(now: SystemTime) -> Result<SessionId> {
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

/// 创建会话并返回写入数据库的 ID。
fn create_session(
    home: Option<&Path>,
    profile_override: Option<&str>,
    title: Option<String>,
    model: Option<String>,
) -> Result<SessionId> {
    let now = SystemTime::now();
    let session_id = session_id_from_clock(now)?;
    let started_at = rfc3339_now(now)?;
    create_session_with_id(home, profile_override, session_id, title, model, started_at)
}

/// 使用调用方指定的 ID 与时间创建会话，供生产编排和确定性测试复用。
fn create_session_with_id(
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

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Profile {
            command: ProfileCommand::List,
        } => {
            for line in profile_list_lines(cli.home.as_deref())? {
                println!("{line}");
            }
        }
        Command::Profile {
            command: ProfileCommand::Create { name },
        } => {
            let path = create_profile(cli.home.as_deref(), &name)?;
            println!("已创建 profile: {}", path.display());
        }
        Command::Profile {
            command: ProfileCommand::Use { name },
        } => {
            let profile = use_profile(cli.home.as_deref(), &name)?;
            println!("当前 profile: {profile}");
        }
        Command::Session {
            command: SessionCommand::Create { title, model },
        } => {
            let session_id =
                create_session(cli.home.as_deref(), cli.profile.as_deref(), title, model)?;
            println!("已创建会话: {}", session_id.as_str());
        }
        Command::Session {
            command:
                SessionCommand::Show {
                    session_id,
                    limit,
                    offset,
                },
        } => {
            for line in session_show_lines(
                cli.home.as_deref(),
                cli.profile.as_deref(),
                &session_id,
                limit,
                offset,
            )? {
                println!("{line}");
            }
        }
        Command::Session {
            command:
                SessionCommand::Search {
                    query,
                    limit,
                    session_id,
                },
        } => {
            for line in session_search_lines(
                cli.home.as_deref(),
                cli.profile.as_deref(),
                &query,
                limit,
                session_id.as_deref(),
            )? {
                println!("{line}");
            }
        }
        Command::Session {
            command: SessionCommand::List { limit, offset },
        } => {
            for line in
                session_list_lines(cli.home.as_deref(), cli.profile.as_deref(), limit, offset)?
            {
                println!("{line}");
            }
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    run(Cli::parse())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use clap::Parser;
    use sagent_store::{NewMessage, NewSession, Store};

    use super::{
        Cli, create_profile, create_profile_with_initializer, create_session_with_id,
        profile_list_lines, profile_list_root, rfc3339_now, session_id_from_clock,
        session_list_lines, session_search_lines, session_show_lines, use_profile,
    };

    #[test]
    fn parses_profile_list_with_absolute_home() {
        let cli = Cli::try_parse_from(["sagent", "--home", "C:\\\\sagent-test", "profile", "list"])
            .expect("profile list 参数应可解析");

        assert_eq!(
            cli.home.expect("应保留 home 参数"),
            std::path::PathBuf::from("C:\\\\sagent-test")
        );
    }

    #[test]
    fn profile_list_returns_default_then_sorted_named_profiles() {
        let root = std::env::temp_dir().join(format!("sagent-cli-profiles-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("profiles").join("writer")).expect("应能创建 writer profile");
        fs::create_dir_all(root.join("profiles").join("coder")).expect("应能创建 coder profile");

        assert_eq!(
            profile_list_lines(Some(&root)).expect("应能列出 profile"),
            vec!["* default", "  coder", "  writer"]
        );
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn profile_list_rejects_relative_home() {
        let error = profile_list_root(Some(std::path::Path::new("relative")))
            .expect_err("必须拒绝相对路径");

        assert!(error.to_string().contains("绝对路径"));
    }

    #[test]
    fn create_profile_writes_config_and_migrated_database() {
        let root = std::env::temp_dir().join(format!("sagent-cli-create-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");

        let path = create_profile(Some(&root), "Coder").expect("应能创建 profile");

        assert_eq!(path, root.join("profiles").join("coder"));
        assert_eq!(
            fs::read_to_string(path.join("config.yaml")).expect("应能读取初始配置"),
            super::INITIAL_CONFIG_YAML
        );
        assert!(
            Store::open_readonly(&path.join("state.db")).is_ok(),
            "创建命令应完成数据库迁移"
        );
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn create_profile_rejects_existing_directory_without_overwriting_it() {
        let root =
            std::env::temp_dir().join(format!("sagent-cli-duplicate-{}", std::process::id()));
        let profile_dir = root.join("profiles").join("coder");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&profile_dir).expect("应能创建既有 profile");
        fs::write(profile_dir.join("keep.txt"), "do not overwrite").expect("应能写入哨兵文件");

        assert!(create_profile(Some(&root), "coder").is_err());
        assert_eq!(
            fs::read_to_string(profile_dir.join("keep.txt")).expect("既有文件不应被删除"),
            "do not overwrite"
        );
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn create_profile_removes_partial_directory_when_initialization_fails() {
        let root =
            std::env::temp_dir().join(format!("sagent-cli-create-fail-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");

        let result = create_profile_with_initializer(&root, "broken", |_| {
            anyhow::bail!("模拟数据库初始化失败")
        });

        assert!(result.is_err());
        assert!(
            !root.join("profiles").join("broken").exists(),
            "初始化失败后不能留下半成品 profile"
        );
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn use_profile_changes_list_marker_and_rejects_unknown_name() {
        let root = std::env::temp_dir().join(format!("sagent-cli-use-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("profiles").join("coder")).expect("应能创建 profile");

        assert_eq!(
            use_profile(Some(&root), "Coder").expect("应能选择 profile"),
            "coder"
        );
        assert_eq!(
            profile_list_lines(Some(&root)).expect("应能标记当前 profile"),
            vec!["  default", "* coder"]
        );
        assert!(use_profile(Some(&root), "missing").is_err());
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn session_list_uses_active_profile_and_honors_explicit_override() {
        let root =
            std::env::temp_dir().join(format!("sagent-cli-session-list-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");
        create_profile(Some(&root), "coder").expect("应能创建 coder profile");
        use_profile(Some(&root), "coder").expect("应能选择 coder");

        let coder_db = root.join("profiles").join("coder").join("state.db");
        let mut coder_store = Store::open_readwrite(&coder_db).expect("应能打开 coder 数据库");
        coder_store
            .create_session(&NewSession {
                id: sagent_types::SessionId::new("coder-session"),
                source: Some("cli".to_owned()),
                model: None,
                title: Some("Coder 会话".to_owned()),
                started_at: "2026-08-30T12:00:00Z".to_owned(),
            })
            .expect("应能创建 coder 会话");

        let default_db = root.join("state.db");
        let mut default_store =
            Store::open_readwrite(&default_db).expect("应能初始化 default 数据库");
        default_store
            .create_session(&NewSession {
                id: sagent_types::SessionId::new("default-session"),
                source: Some("cli".to_owned()),
                model: None,
                title: Some("默认会话".to_owned()),
                started_at: "2026-08-30T12:01:00Z".to_owned(),
            })
            .expect("应能创建默认会话");

        assert_eq!(
            session_list_lines(Some(&root), None, 20, 0).expect("应能列出当前 profile 会话"),
            vec!["coder-session\tCoder 会话\t0\t2026-08-30T12:00:00Z"]
        );
        assert_eq!(
            session_list_lines(Some(&root), Some("default"), 20, 0)
                .expect("显式 profile 应覆盖当前选择"),
            vec!["default-session\t默认会话\t0\t2026-08-30T12:01:00Z"]
        );
        drop(coder_store);
        drop(default_store);
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn creates_session_in_current_profile_and_preserves_optional_metadata() {
        let root =
            std::env::temp_dir().join(format!("sagent-cli-session-create-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");
        create_profile(Some(&root), "coder").expect("应能创建 coder profile");
        use_profile(Some(&root), "coder").expect("应能选择 coder");

        let id = create_session_with_id(
            Some(&root),
            None,
            sagent_types::SessionId::new("fixed-session"),
            Some("迁移讨论".to_owned()),
            Some("test-model".to_owned()),
            "2026-08-30T13:00:00.000Z".to_owned(),
        )
        .expect("应能向当前 profile 创建会话");
        assert_eq!(id.as_str(), "fixed-session");

        let store = Store::open_readonly(&root.join("profiles").join("coder").join("state.db"))
            .expect("应能打开当前 profile 数据库");
        let session = store
            .get_session(&id)
            .expect("读取不应失败")
            .expect("新会话应存在");
        assert_eq!(session.title.as_deref(), Some("迁移讨论"));
        assert_eq!(session.model.as_deref(), Some("test-model"));
        drop(store);

        assert!(
            create_session_with_id(
                Some(&root),
                None,
                id,
                None,
                None,
                "2026-08-30T13:00:01.000Z".to_owned(),
            )
            .is_err(),
            "重复 ID 不能覆盖原会话"
        );
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn session_show_displays_archived_history_but_hides_compaction_summary() {
        let root =
            std::env::temp_dir().join(format!("sagent-cli-session-show-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");
        create_profile(Some(&root), "coder").expect("应能创建 coder profile");
        use_profile(Some(&root), "coder").expect("应能选择 coder");
        let session_id = create_session_with_id(
            Some(&root),
            None,
            sagent_types::SessionId::new("show-session"),
            Some("展示测试".to_owned()),
            Some("test-model".to_owned()),
            "2026-08-30T14:00:00.000Z".to_owned(),
        )
        .expect("应能创建会话");

        let database = root.join("profiles").join("coder").join("state.db");
        let mut store = Store::open_readwrite(&database).expect("应能打开数据库");
        store
            .append_message(&NewMessage::new(
                session_id.clone(),
                "user",
                "早期问题",
                "2026-08-30T14:01:00.000Z",
            ))
            .expect("应能追加用户消息");
        store
            .append_message(&NewMessage::new(
                session_id.clone(),
                "assistant",
                "早期回答",
                "2026-08-30T14:02:00.000Z",
            ))
            .expect("应能追加助手消息");
        store
            .archive_and_compact(
                &session_id,
                &[NewMessage::compressed_summary(
                    session_id.clone(),
                    "assistant",
                    "内部压缩摘要",
                    "2026-08-30T14:03:00.000Z",
                )],
                "2026-08-30T14:03:00.000Z",
            )
            .expect("应能压缩历史");
        drop(store);

        let lines = session_show_lines(Some(&root), None, session_id.as_str(), 50, 0)
            .expect("应能显示会话");
        assert_eq!(
            lines,
            vec![
                "ID: show-session",
                "标题: 展示测试",
                "来源: cli",
                "模型: test-model",
                "开始时间: 2026-08-30T14:00:00.000Z",
                "结束时间: -",
                "消息数: 1",
                "消息:",
                "[user] 早期问题",
                "[assistant] 早期回答",
            ]
        );
        assert!(
            session_show_lines(Some(&root), None, "missing-session", 50, 0).is_err(),
            "不存在的会话必须报错"
        );
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn session_search_scopes_results_and_hides_rewound_messages() {
        let root =
            std::env::temp_dir().join(format!("sagent-cli-session-search-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");
        create_profile(Some(&root), "coder").expect("应能创建 coder profile");
        use_profile(Some(&root), "coder").expect("应能选择 coder");

        let kept_session = create_session_with_id(
            Some(&root),
            None,
            sagent_types::SessionId::new("search-keep"),
            None,
            None,
            "2026-08-30T15:00:00.000Z".to_owned(),
        )
        .expect("应能创建保留会话");
        let database = root.join("profiles").join("coder").join("state.db");
        let mut store = Store::open_readwrite(&database).expect("应能打开数据库");
        store
            .append_message(&NewMessage::new(
                kept_session.clone(),
                "user",
                "sagentterm 保留消息",
                "2026-08-30T15:01:00.000Z",
            ))
            .expect("应能写入可搜索消息");

        let rewound_session = sagent_types::SessionId::new("search-rewound");
        store
            .create_session(&NewSession {
                id: rewound_session.clone(),
                source: Some("cli".to_owned()),
                model: None,
                title: None,
                started_at: "2026-08-30T15:02:00.000Z".to_owned(),
            })
            .expect("应能创建回退会话");
        let rewound_message = store
            .append_message(&NewMessage::new(
                rewound_session.clone(),
                "user",
                "sagentterm 已回退消息",
                "2026-08-30T15:03:00.000Z",
            ))
            .expect("应能写入待回退消息");
        store
            .rewind_to_message(
                &rewound_session,
                rewound_message,
                "2026-08-30T15:04:00.000Z",
            )
            .expect("应能回退消息");
        drop(store);

        let hits = session_search_lines(Some(&root), None, "sagentterm", 20, None)
            .expect("应能搜索当前 profile");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].starts_with("search-keep\t"));
        assert!(hits[0].contains("[sagentterm]"));

        let scoped = session_search_lines(
            Some(&root),
            None,
            "sagentterm",
            20,
            Some(kept_session.as_str()),
        )
        .expect("应能按会话限定搜索");
        assert_eq!(scoped.len(), 1);
        assert!(session_search_lines(Some(&root), None, "   ", 20, None).is_err());
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn session_clock_helpers_produce_stable_timestamp_and_distinct_uuid_ids() {
        let epoch = std::time::UNIX_EPOCH;

        assert_eq!(
            rfc3339_now(epoch).expect("epoch 应可格式化"),
            "1970-01-01T00:00:00.000Z"
        );
        let first = session_id_from_clock(epoch).expect("应能生成 ID");
        let second = session_id_from_clock(epoch).expect("应能生成 ID");
        assert_ne!(first, second);
        assert!(first.as_str().starts_with("19700101_000000_"));
        assert_eq!(first.as_str().len(), 16 + 32);
    }
}
