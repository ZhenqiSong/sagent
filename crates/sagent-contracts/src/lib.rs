//! Sagent 自有的版本化行为契约 fixture runner。
//!
//! fixture 只描述可观察输入与正常化输出；它们不是源码快照，也不依赖 Python 或其他
//! runtime 作为 oracle。这样独立项目重构内部实现时，仍能守住已发布的外部行为。

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use sagent_agent::{PromptToolCall, Transcript};
use sagent_config::{
    StorageDescriptor, StorageKind, load_profile_config, normalize_profile_name,
    read_public_config_from_config, resolve_paths, resolve_provider_config_from_config,
};
use sagent_protocol::{ClientHelloParams, negotiate_hello};
use sagent_store::{NewGeneration, NewSession, SqliteDatabase};
use sagent_tools::{CommandRisk, ToolDefinition, ToolRegistry, classify_command};
use sagent_types::SessionId;
use serde::Deserialize;
use serde_json::{Value, json};

/// fixture 格式版本与行为版本绑定；未知版本必须失败，避免新 runner 静默误读旧语义。
const CONTRACT_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
struct Fixture {
    contract_version: u32,
    kind: String,
    input: Value,
    expected: Value,
}

/// 执行仓库根目录的全部契约 fixture。
///
/// 根目录从 crate 位置推导，保证本地、CI 和 `cargo run` 使用同一套受版本控制的输入，
/// 而不会意外读取开发者的 home 或当前工作目录。
pub fn run_all() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .context("contract crate must live under <repo>/crates")?;
    run_directory(&root.join("contracts"))
}

/// 执行 `directory` 下的全部 JSON fixture，并在失败链中保留具体文件路径。
pub fn run_directory(directory: &Path) -> Result<()> {
    let mut files = Vec::new();
    collect_json(directory, &mut files)?;
    if files.is_empty() {
        bail!("no contract fixtures found under {}", directory.display());
    }
    for path in files {
        run_fixture(&path).with_context(|| format!("contract fixture {}", path.display()))?;
    }
    Ok(())
}

fn collect_json(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory).with_context(|| format!("read {}", directory.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect_json(&path, files)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            files.push(path);
        }
    }
    // 固定执行顺序，使 CI 中的首个失败和本地一致，避免文件系统枚举顺序制造噪音。
    files.sort();
    Ok(())
}

fn run_fixture(path: &Path) -> Result<()> {
    let fixture: Fixture = serde_json::from_slice(&fs::read(path)?)?;
    if fixture.contract_version != CONTRACT_VERSION {
        bail!(
            "unsupported contract_version {}, expected {CONTRACT_VERSION}",
            fixture.contract_version
        );
    }
    let actual = match fixture.kind.as_str() {
        "rpc_hello" => rpc_hello(fixture.input)?,
        "transcript" => transcript(fixture.input)?,
        "tool_registry" => tool_registry(fixture.input)?,
        "command_policy" => command_policy(fixture.input)?,
        "profile_name" => profile_name(fixture.input)?,
        "storage_descriptor" => storage_descriptor(fixture.input)?,
        "store_schema" => store_schema(fixture.input)?,
        "provider_config_snapshot" => provider_config_snapshot(fixture.input)?,
        "generation_record" => generation_record(fixture.input)?,
        other => bail!("unknown contract kind {other:?}"),
    };
    // 事件顺序、role、capability 与 hash 都是契约的一部分；不能以“只含关键字段”的
    // 宽松比较掩盖不兼容变更。
    if actual != fixture.expected {
        bail!(
            "normalized result mismatch\nexpected: {}\nactual: {}",
            fixture.expected,
            actual
        );
    }
    Ok(())
}

fn rpc_hello(input: Value) -> Result<Value> {
    let params: ClientHelloParams = serde_json::from_value(input)?;
    Ok(serde_json::to_value(negotiate_hello(&params)?)?)
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum TranscriptOperation {
    User {
        content: String,
    },
    Assistant {
        content: String,
        #[serde(default)]
        tool_calls: Vec<PromptToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

fn transcript(input: Value) -> Result<Value> {
    let operations: Vec<TranscriptOperation> = serde_json::from_value(input)?;
    let mut transcript = Transcript::new();
    for operation in operations {
        match operation {
            TranscriptOperation::User { content } => transcript.append_user(content)?,
            TranscriptOperation::Assistant {
                content,
                tool_calls,
            } => transcript.append_assistant(content, tool_calls)?,
            TranscriptOperation::Tool {
                tool_call_id,
                content,
            } => transcript.append_tool_result(tool_call_id, content)?,
        }
    }
    Ok(serde_json::to_value(transcript.messages())?)
}

fn tool_registry(input: Value) -> Result<Value> {
    let definitions: Vec<ToolDefinition> = serde_json::from_value(input)?;
    let mut registry = ToolRegistry::new();
    for definition in definitions {
        registry.register(definition)?;
    }
    Ok(json!({
        "names": registry.names(),
        "model_schema": registry.model_schema()?,
        "schema_hash": registry.schema_hash()?,
    }))
}

fn command_policy(input: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Input {
        command: String,
    }
    let input: Input = serde_json::from_value(input)?;
    Ok(match classify_command(&input.command) {
        CommandRisk::Safe => json!({"risk": "safe"}),
        CommandRisk::RequireApproval {
            policy_key,
            summary,
        } => {
            json!({"risk": "approval_required", "policy_key": policy_key, "summary": summary})
        }
        CommandRisk::Deny { reason } => json!({"risk": "deny", "reason": reason}),
    })
}

fn profile_name(input: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Input {
        value: String,
    }
    let input: Input = serde_json::from_value(input)?;
    Ok(json!({"normalized": normalize_profile_name(&input.value)?.as_str()}))
}

/// 验证存储 descriptor 的配置边界，并只输出不含秘密的规范化摘要。
fn storage_descriptor(input: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Input {
        yaml: String,
    }
    let input: Input = serde_json::from_value(input)?;
    let descriptor: StorageDescriptor = serde_yaml::from_str(&input.yaml)?;
    descriptor.validate()?;
    let kind = match descriptor.kind {
        StorageKind::Sqlite => "sqlite",
        StorageKind::Remote => "remote",
    };
    Ok(json!({
        "kind": kind,
        "path": descriptor.path.map(|path| path.to_string_lossy().into_owned()),
        "has_connection_env": descriptor.connection_env.is_some(),
        "schema": descriptor.schema,
        "namespace": descriptor.namespace,
        "read_only": descriptor.read_only,
    }))
}

fn store_schema(input: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Input {
        database_file: Option<String>,
        fresh: Option<bool>,
    }
    let input: Input = serde_json::from_value(input)?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .context("repo root")?;
    // fresh schema fixture 必须在临时位置创建，绝不能打开或升级仓库中的 fixture DB。
    let temporary =
        std::env::temp_dir().join(format!("sagent-contract-store-{}.db", std::process::id()));
    let path = match (input.database_file, input.fresh.unwrap_or(false)) {
        (Some(file), false) => root.join("contracts").join(file),
        (None, true) => {
            let _ = fs::remove_file(&temporary);
            temporary.clone()
        }
        _ => bail!("store_schema requires database_file or fresh: true"),
    };
    let store = if path == temporary {
        SqliteDatabase::open_readwrite(&path)?
    } else {
        SqliteDatabase::open_readonly(&path)?
    };
    let info = store.inspect_schema()?;
    drop(store);
    if path == temporary {
        // SQLite 数据库句柄持有连接时 Windows 无法删除文件，故在 drop 后清理临时状态。
        let _ = fs::remove_file(path);
    }
    Ok(json!({"schema_version": info.schema_version, "has_fts5": info.has_fts5}))
}

/// 验证 Profile 配置只在启动时读取一次，并且 Provider 解析的调试输出不会泄漏密钥。
fn provider_config_snapshot(input: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Input {
        yaml: String,
        env: String,
        secret: String,
    }

    let input: Input = serde_json::from_value(input)?;
    let root = std::env::temp_dir().join(format!(
        "sagent-contract-provider-snapshot-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).context("创建 Provider snapshot 临时目录失败")?;
    fs::write(root.join("config.yaml"), input.yaml).context("写入 Provider 配置 fixture 失败")?;
    fs::write(root.join(".env"), input.env).context("写入 Provider 凭据 fixture 失败")?;

    // 先解析完整快照和 Provider 配置，再删除源文件；后续摘要仍能工作，证明运行时依赖
    // 的是同一份不可变输入，而不是在每个 resolver 内重新读取 config.yaml。
    let result = (|| -> Result<Value> {
        let profile = normalize_profile_name("default")?;
        let paths = resolve_paths(Some(&root), Some(&profile))?;
        let config = load_profile_config(&paths)?;
        let resolved = resolve_provider_config_from_config(&paths, &config, None, None)?;
        let before_delete = read_public_config_from_config(&paths, &config)?;
        fs::remove_file(&paths.config_yaml).context("删除配置 fixture 失败")?;
        fs::remove_file(&paths.env_file).context("删除凭据 fixture 失败")?;
        let after_delete = read_public_config_from_config(&paths, &config)?;
        let debug = format!("{resolved:?}");
        Ok(json!({
            "profile": after_delete.profile,
            "provider": resolved.provider,
            "model": resolved.model,
            "provider_names": after_delete.provider_names,
            "unknown_fields": after_delete.unknown_fields,
            "snapshot_survives_config_delete": before_delete == after_delete,
            "resolved_debug_contains_secret": debug.contains(&input.secret),
        }))
    })();
    // 临时 fixture 必须无论断言成功或失败都清理，避免凭据样例残留在宿主临时目录。
    let _ = fs::remove_dir_all(&root);
    result
}

/// 验证 generation 的模型、prompt/tool hash 和 profile revision 按原值持久化并可恢复。
fn generation_record(input: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Input {
        session_id: String,
        generation: i64,
        system_hash: String,
        tool_schema_hash: String,
        model_id: String,
        profile_revision: String,
    }

    let input: Input = serde_json::from_value(input)?;
    let database_file = std::env::temp_dir().join(format!(
        "sagent-contract-generation-{}.db",
        std::process::id()
    ));
    let _ = fs::remove_file(&database_file);
    let result = (|| -> Result<Value> {
        let mut database = SqliteDatabase::open_readwrite(&database_file)?;
        let session_id = SessionId::new(input.session_id);
        database.create_session(&NewSession {
            id: session_id.clone(),
            source: Some("contract".to_owned()),
            model: Some(input.model_id.clone()),
            title: None,
            started_at: "2026-09-17T00:00:00Z".to_owned(),
        })?;
        database.create_generation(&NewGeneration {
            session_id: session_id.clone(),
            generation: input.generation,
            system_hash: input.system_hash,
            tool_schema_hash: input.tool_schema_hash,
            model_id: input.model_id,
            profile_revision: input.profile_revision,
            created_at: "2026-09-17T00:00:00Z".to_owned(),
        })?;
        let stored = database
            .get_generation(&session_id, input.generation)?
            .context("generation fixture 未能读回已写入记录")?;
        Ok(json!({
            "generation": stored.generation,
            "system_hash": stored.system_hash,
            "tool_schema_hash": stored.tool_schema_hash,
            "model_id": stored.model_id,
            "profile_revision": stored.profile_revision,
        }))
    })();
    // Windows 仍持有数据库连接时无法删除文件，所以必须先结束上面的闭包再清理。
    let _ = fs::remove_file(&database_file);
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn repository_contracts_hold() {
        super::run_all().unwrap();
    }
}
