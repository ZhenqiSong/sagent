//! P0.2 的离线性能基准 runner。
//!
//! 此二进制只测量本地纯 Rust/SQLite 路径，不调用真实 Provider，也不读取用户的
//! `SAGENT_HOME`。默认将结果输出到 stdout；只有显式 `--write-baseline` 才会更新
//! 仓库内的比较基线，避免日常运行意外改写受版本控制的数据。

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags};
use sagent_agent::{PromptMessage, PromptRole, PromptSnapshot, SystemPromptParts};
use sagent_protocol::{ClientHelloCapabilities, ClientHelloParams, negotiate_hello};
use sagent_store::{MessageSearchQuery, NewMessage, NewSession, Store};
use sagent_types::{ClientId, ClientSurface, SessionId, TurnId};
use serde::Serialize;

/// benchmark 输出格式版本；基线比较器据此拒绝未知统计口径。
const BENCHMARK_VERSION: u32 = 1;

/// 命令行显式控制的 fixture 规模和输出策略。
#[derive(Debug)]
struct Arguments {
    /// 临时 Store 中生成的消息数；不使用用户历史数据，保证可重复。
    messages: usize,
    /// 每个指标的采样次数；统计值只在同一环境中比较。
    iterations: usize,
    /// 只有用户明确选择时才覆盖受版本控制的基线文件。
    write_baseline: bool,
}

/// 一次运行可序列化、可审查的完整性能报告。
#[derive(Serialize)]
struct BenchmarkReport {
    /// 输出格式与指标语义的版本。
    benchmark_version: u32,
    /// 影响数值可比性的宿主信息，不记录机器名或用户路径。
    environment: Environment,
    /// 确认本次测量采用的离线输入规模。
    fixture: FixtureInfo,
    /// 每个独立操作的统计结果。
    metrics: Vec<Metric>,
}

/// 不泄露主机身份、但足以判断基线是否可横向比较的运行环境。
#[derive(Serialize)]
struct Environment {
    /// 编译目标操作系统。
    os: &'static str,
    /// 编译目标架构。
    arch: &'static str,
    /// 可用逻辑 CPU 数，帮助解释负载差异。
    logical_cpus: usize,
    /// 实际执行基准的 Rust 编译器版本。
    rustc: String,
}

/// fixture 本身的规模与外部副作用边界。
#[derive(Serialize)]
struct FixtureInfo {
    /// 临时 SQLite fixture 的消息总数。
    messages: usize,
    /// 每个指标的样本数量。
    iterations: usize,
    /// 明确本轮没有网络 Provider，以免将网络延迟误读为 runtime 回归。
    provider: &'static str,
    /// 明确数据来自临时库，而不是开发者 profile。
    data_source: &'static str,
    /// `EXPLAIN QUERY PLAN` 已确认 FTS 搜索走 virtual-table 索引，而非普通表扫描。
    fts_uses_virtual_table_index: bool,
}

/// 单一操作的单位和统计样本。
#[derive(Serialize)]
struct Metric {
    /// 稳定的指标标识，供后续比较器匹配。
    name: &'static str,
    /// 所有延迟统一使用微秒，避免不同指标混用单位。
    unit: &'static str,
    /// 已排序样本导出的统计摘要。
    samples: SampleStats,
}

/// 不将完整原始样本写入基线，降低无意义的提交噪音。
#[derive(Serialize)]
struct SampleStats {
    /// 最快一次观测值。
    min: u128,
    /// 中位数，作为常规比较主值。
    median: u128,
    /// 第 95 百分位，暴露偶发抖动。
    p95: u128,
    /// 最慢一次观测值，供诊断而非 CI 硬阈值使用。
    max: u128,
}

/// 运行基准并按用户意图决定是否发布新的基线。
fn main() -> Result<()> {
    let arguments = parse_arguments(std::env::args().skip(1))?;
    let report = run(&arguments)?;
    let json = serde_json::to_vec_pretty(&report)?;
    if arguments.write_baseline {
        let path = repository_root()?.join("benchmarks").join("baseline.json");
        fs::write(&path, &json).with_context(|| format!("write {}", path.display()))?;
    }
    println!("{}", String::from_utf8(json)?);
    Ok(())
}

/// 解析有限的 runner 参数；未知参数失败，避免拼写错误静默改变测量范围。
fn parse_arguments(arguments: impl Iterator<Item = String>) -> Result<Arguments> {
    let mut parsed = Arguments {
        messages: 10_000,
        iterations: 7,
        write_baseline: false,
    };
    let mut arguments = arguments.peekable();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--messages" => parsed.messages = parse_positive("--messages", arguments.next())?,
            "--iterations" => parsed.iterations = parse_positive("--iterations", arguments.next())?,
            "--write-baseline" => parsed.write_baseline = true,
            "--help" | "-h" => {
                println!(
                    "Usage: cargo run -p sagent-benchmarks -- [--messages N] [--iterations N] [--write-baseline]"
                );
                std::process::exit(0);
            }
            other => bail!("unknown argument {other:?}"),
        }
    }
    Ok(parsed)
}

/// 将规模参数限制为正数，防止空 fixture 产生无意义且看似很快的基线。
fn parse_positive(flag: &str, value: Option<String>) -> Result<usize> {
    let value = value.with_context(|| format!("{flag} requires a value"))?;
    let parsed: usize = value
        .parse()
        .with_context(|| format!("{flag} must be a positive integer"))?;
    if parsed == 0 {
        bail!("{flag} must be greater than zero");
    }
    Ok(parsed)
}

/// 在单个临时数据库内完成所有测量，避免磁盘路径和 fixture 内容在指标间漂移。
fn run(arguments: &Arguments) -> Result<BenchmarkReport> {
    let database = temporary_database_path();
    let store = create_fixture_store(&database, arguments.messages)?;
    let fts_uses_virtual_table_index = verify_fts_query_plan(&database)?;
    let session_id = SessionId::new("benchmark-session");
    let metrics = vec![
        metric("rpc_hello", arguments.iterations, benchmark_hello)?,
        metric("prompt_snapshot", arguments.iterations, || {
            benchmark_prompt_snapshot()
        })?,
        metric("store_open", arguments.iterations, || {
            benchmark_store_open(&database)
        })?,
        metric("session_list", arguments.iterations, || {
            benchmark_session_list(&store)
        })?,
        metric("fts_search", arguments.iterations, || {
            benchmark_fts_search(&store, &session_id)
        })?,
    ];
    drop(store);
    // 任何清理失败都应暴露给调用者，避免 benchmark 在用户临时目录积累数据库文件。
    fs::remove_file(&database).with_context(|| format!("remove {}", database.display()))?;
    Ok(BenchmarkReport {
        benchmark_version: BENCHMARK_VERSION,
        environment: Environment {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            logical_cpus: std::thread::available_parallelism().map_or(1, usize::from),
            rustc: rustc_version(),
        },
        fixture: FixtureInfo {
            messages: arguments.messages,
            iterations: arguments.iterations,
            provider: "none (offline)",
            data_source: "temporary SQLite fixture",
            fts_uses_virtual_table_index,
        },
        metrics,
    })
}

/// 读取 SQLite 执行计划，确认 FTS 指标确实覆盖 FTS5 virtual-table 路径。
///
/// 这是结构性诊断而不是耗时阈值：不同 OS/SQLite 版本可改变耗时，但若计划不再出现
/// virtual-table index，100k 搜索数值就不再代表预期实现，runner 应直接失败。
fn verify_fts_query_plan(path: &Path) -> Result<bool> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("以只读方式检查 FTS query plan 失败：{}", path.display()))?;
    let mut statement = connection
        .prepare(
            "EXPLAIN QUERY PLAN
             SELECT m.id
             FROM messages_fts
             INNER JOIN messages AS m ON m.id = messages_fts.rowid
             WHERE messages_fts MATCH ?1 AND m.session_id = ?2
             ORDER BY bm25(messages_fts), m.id DESC
             LIMIT 20",
        )
        .context("准备 FTS query plan 失败")?;
    let details = statement
        .query_map(["benchmark", "benchmark-session"], |row| {
            row.get::<_, String>(3)
        })
        .context("执行 FTS query plan 失败")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("读取 FTS query plan 失败")?;
    let uses_index = details
        .iter()
        .any(|detail| detail.contains("messages_fts") && detail.contains("VIRTUAL TABLE INDEX"));
    if !uses_index {
        anyhow::bail!("FTS query plan 未使用 messages_fts virtual-table index：{details:?}");
    }
    Ok(uses_index)
}

/// 将一个操作测量并包装为统一单位，保证 JSON 消费者无需推断统计口径。
fn metric(
    name: &'static str,
    iterations: usize,
    operation: impl FnMut() -> Result<()>,
) -> Result<Metric> {
    Ok(Metric {
        name,
        unit: "microseconds",
        samples: measure(iterations, operation)?,
    })
}

/// 采样后排序计算分位数；不设置绝对耗时断言，防止共享 CI 主机导致脆弱测试。
fn measure(iterations: usize, mut operation: impl FnMut() -> Result<()>) -> Result<SampleStats> {
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        operation()?;
        samples.push(started.elapsed().as_micros());
    }
    samples.sort_unstable();
    let percentile_index = (samples.len() * 95).div_ceil(100).saturating_sub(1);
    Ok(SampleStats {
        min: samples[0],
        median: samples[samples.len() / 2],
        p95: samples[percentile_index],
        max: samples[samples.len() - 1],
    })
}

/// 测量纯协议握手协商，不包含 transport 或网络成本。
fn benchmark_hello() -> Result<()> {
    let params = ClientHelloParams {
        protocol_version: 1,
        client_id: ClientId::new(),
        surface: ClientSurface::Tui,
        capabilities: ClientHelloCapabilities {
            interactive_approval: true,
            supports_stream_edits: true,
        },
    };
    let _ = negotiate_hello(&params)?;
    Ok(())
}

/// 测量稳定 prompt 快照和 hash 构造，保护缓存边界的热路径。
fn benchmark_prompt_snapshot() -> Result<()> {
    let system = SystemPromptParts {
        stable: "benchmark system".into(),
        ..Default::default()
    };
    let snapshot = PromptSnapshot::new(
        SessionId::new("benchmark-session"),
        TurnId::new(),
        &system,
        vec![
            PromptMessage::new(PromptRole::System, system.render()),
            PromptMessage::new(PromptRole::User, "benchmark request"),
        ],
    )?;
    let _ = snapshot.hash()?;
    Ok(())
}

/// 测量只读打开与连接探测，避免把 fixture 写入初始化算入读路径。
fn benchmark_store_open(path: &Path) -> Result<()> {
    let store = Store::open_readonly(path)?;
    store.verify_connection()?;
    Ok(())
}

/// 测量常见的有限会话列表读取。
fn benchmark_session_list(store: &Store) -> Result<()> {
    let _ = store.list_sessions(20, 0)?;
    Ok(())
}

/// 测量固定 scope 下的 FTS 查询，确保命中集合不随用户数据变化。
fn benchmark_fts_search(store: &Store, session_id: &SessionId) -> Result<()> {
    let mut query = MessageSearchQuery::new("benchmark");
    query.session_id = Some(session_id.clone());
    query.limit = 20;
    let _ = store.search_messages(&query)?;
    Ok(())
}

/// 构造时间戳、内容和角色均固定的临时 Store，隔离真实 profile 与网络凭据。
fn create_fixture_store(path: &Path, messages: usize) -> Result<Store> {
    let mut store = Store::open_readwrite(path)?;
    let session_id = SessionId::new("benchmark-session");
    store.create_session(&NewSession {
        id: session_id.clone(),
        source: Some("benchmark".into()),
        model: Some("offline".into()),
        title: Some("benchmark fixture".into()),
        started_at: "2026-01-01T00:00:00Z".into(),
    })?;
    // 10k/100k fixture 必须以单事务初始化：逐条提交测试到的是 fsync 次数，
    // 而不是 P1.3 要保护的分页与 FTS 查询路径。
    let messages = (0..messages)
        .map(|index| {
            NewMessage::new(
                session_id.clone(),
                if index % 2 == 0 { "user" } else { "assistant" },
                format!("benchmark message {index}"),
                "2026-01-01T00:00:00Z",
            )
        })
        .collect::<Vec<_>>();
    store.append_messages(&messages)?;
    Ok(store)
}

/// 为每次运行生成唯一临时路径，避免并行 benchmark 共享同一个 SQLite 文件。
fn temporary_database_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos();
    std::env::temp_dir().join(format!(
        "sagent-benchmark-{}-{nonce}.db",
        std::process::id()
    ))
}

/// 从 crate 固定位置定位仓库，禁止当前目录影响 `--write-baseline` 的写入目标。
fn repository_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("benchmark crate must live under <repo>/crates")
}

/// 捕获实际编译器版本；无法执行 `rustc` 时保留 unknown 而不阻断离线测量。
fn rustc_version() -> String {
    Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|output| output.trim().to_owned())
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::{Arguments, parse_arguments, run};

    #[test]
    fn parses_explicit_bounded_fixture_size() {
        let arguments = parse_arguments(
            ["--messages", "32", "--iterations", "3"]
                .map(str::to_owned)
                .into_iter(),
        )
        .unwrap();
        assert_eq!(arguments.messages, 32);
        assert_eq!(arguments.iterations, 3);
    }

    #[test]
    fn produces_all_offline_metrics() {
        let report = run(&Arguments {
            messages: 32,
            iterations: 3,
            write_baseline: false,
        })
        .unwrap();
        assert_eq!(report.metrics.len(), 5);
        assert!(
            report
                .metrics
                .iter()
                .all(|metric| metric.samples.max >= metric.samples.min)
        );
        assert!(
            report.fixture.fts_uses_virtual_table_index,
            "离线 fixture 的 FTS 查询必须使用 virtual-table 索引"
        );
    }
}
