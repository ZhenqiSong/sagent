//! JSON-RPC 方法分发器和 Phase 1 Session RPC 装配。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use sagent_api::error::{ErrorCode, ErrorObject};
use sagent_api::event::{EventEnvelope, EventParams};
use sagent_api::logging;
use sagent_api::session::{CreateParams, GetParams, ListParams, ResumeParams, SubscribeParams};
use sagent_runtime::{RuntimeError, RuntimeHandle, SessionEvent, SessionView};
use sagent_session::{CreateSession, ListSessions, MessageRange, SessionCursor};
use sagent_types::ids::{EventId, RequestId, SessionId};
use sagent_types::message::Message;
use sagent_types::version::{Capabilities, ProtocolVersion};
use tracing::{debug, error, info, warn};

use crate::stdio::{MAX_ID_BYTES, MAX_METHOD_BYTES};

/// 分发结果：成功返回 Some(JSON response)，通知返回 None。
pub type DispatchResult = Result<Option<serde_json::Value>, (Option<RequestId>, ErrorObject)>;
type ParsedRequest = (Option<RequestId>, String, serde_json::Value, bool);

/// stdio 连接上的 live Session event subscriptions。
pub type Subscriptions = HashMap<SessionId, sagent_runtime::EventReceiver>;

static EVENT_ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 分发 Phase 0 核心方法。
pub fn dispatch(line: &str, caps: &Capabilities) -> DispatchResult {
    let value: serde_json::Value = serde_json::from_str(line).map_err(|error| {
        error!(error = %error, "JSON 解析失败");
        (
            None,
            ErrorObject::parse_error(format!("Parse error: {error}")),
        )
    })?;
    let object = value.as_object().ok_or_else(|| {
        (
            None,
            ErrorObject::invalid_request("request must be an object"),
        )
    })?;
    const ALLOWED_FIELDS: &[&str] = &["jsonrpc", "id", "method", "params"];
    if object.keys().any(|key| !ALLOWED_FIELDS.contains(&key.as_str())) {
        return Err((
            extract_id(&value),
            ErrorObject::invalid_request("unknown envelope field"),
        ));
    }
    if value.get("jsonrpc").and_then(serde_json::Value::as_str) != Some("2.0") {
        return Err((
            extract_id(&value),
            ErrorObject::invalid_request("jsonrpc must be \"2.0\""),
        ));
    }
    validate_id(&value)?;
    let request_id = extract_id(&value);
    let span = logging::request_span(&format_request_id(&request_id));
    let _guard = span.enter();
    let notification = !object.contains_key("id");
    let method = value
        .get("method")
        .and_then(serde_json::Value::as_str)
        .filter(|method| !method.is_empty())
        .ok_or_else(|| {
            (
                request_id.clone(),
                ErrorObject::invalid_request("missing method"),
            )
        })?;
    if method.len() > MAX_METHOD_BYTES {
        return Err((
            request_id,
            ErrorObject::payload_too_large("method exceeds 256 bytes"),
        ));
    }
    let params = value.get("params").cloned().unwrap_or_else(|| serde_json::json!({}));
    if !params.is_object() {
        return Err((
            request_id,
            ErrorObject::invalid_params("params must be a JSON object"),
        ));
    }
    if !caps.validate_method(method) {
        warn!(method = %method, "未知方法");
        return Err((request_id, ErrorObject::method_not_found(method)));
    }
    debug!(method = %method, params = %logging::redact_sensitive(&params), "收到 JSON-RPC 请求");

    let result = match method {
        "rpc.echo" => Ok(params),
        "protocol.describe" => handle_protocol_describe(caps),
        "health.get" => handle_health(),
        _ => Err(ErrorObject::method_not_found(method)),
    };
    match result {
        Ok(_result) if notification => Ok(None),
        Ok(result) => {
            info!(method = %method, "请求处理成功");
            Ok(Some(build_success_response(
                request_id.expect("non-notification request must have id"),
                result,
            )))
        },
        Err(error) => {
            error!(method = %method, error_code = error.code, error_message = %error.message, "请求处理失败");
            Err((request_id, error))
        },
    }
}

/// 分发 Phase 1 Session RPC。同步 transport 通过 Tokio runtime 调用异步 Runtime。
pub fn dispatch_runtime(
    line: &str,
    caps: &Capabilities,
    runtime: &RuntimeHandle,
    async_runtime: &tokio::runtime::Runtime,
    subscriptions: &mut Subscriptions,
) -> DispatchResult {
    let (request_id, method, params, notification) = parse_request(line)?;
    let span = logging::request_span(&format_request_id(&request_id));
    let _guard = span.enter();
    if !caps.validate_method(&method) {
        warn!(method = %method, "未知方法");
        return Err((request_id, ErrorObject::method_not_found(&method)));
    }
    if !method.starts_with("session.") {
        if notification {
            info!(method = %method, "notification 处理完成，不返回 response");
        }
        return dispatch(line, caps);
    }
    let result = match method.as_str() {
        "session.create" => session_create(&params, runtime, async_runtime),
        "session.list" => session_list(&params, runtime, async_runtime),
        "session.get" => session_get(&params, runtime, async_runtime),
        "session.resume" => session_resume(&params, runtime, async_runtime),
        "session.subscribe" => session_subscribe(&params, runtime, async_runtime, subscriptions),
        _ => Err(ErrorObject::method_not_found(&method)),
    }
    .map_err(|error| (request_id.clone(), error))?;
    if notification {
        Ok(None)
    } else {
        Ok(Some(build_success_response(
            request_id.expect("request id 已校验"),
            result,
        )))
    }
}

fn parse_request(line: &str) -> Result<ParsedRequest, (Option<RequestId>, ErrorObject)> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(|error| {
        error!(error = %error, "JSON 解析失败");
        (
            None,
            ErrorObject::parse_error(format!("Parse error: {error}")),
        )
    })?;
    let object = value.as_object().ok_or_else(|| {
        (
            None,
            ErrorObject::invalid_request("request must be an object"),
        )
    })?;
    const ALLOWED_FIELDS: &[&str] = &["jsonrpc", "id", "method", "params"];
    if object.keys().any(|key| !ALLOWED_FIELDS.contains(&key.as_str())) {
        return Err((
            extract_id(&value),
            ErrorObject::invalid_request("unknown envelope field"),
        ));
    }
    if value.get("jsonrpc").and_then(serde_json::Value::as_str) != Some("2.0") {
        return Err((
            extract_id(&value),
            ErrorObject::invalid_request("jsonrpc must be \"2.0\""),
        ));
    }
    validate_id(&value)?;
    let request_id = extract_id(&value);
    let notification = !object.contains_key("id");
    let method = value
        .get("method")
        .and_then(serde_json::Value::as_str)
        .filter(|method| !method.is_empty())
        .ok_or_else(|| {
            (
                request_id.clone(),
                ErrorObject::invalid_request("missing method"),
            )
        })?
        .to_string();
    if method.len() > MAX_METHOD_BYTES {
        return Err((
            request_id,
            ErrorObject::payload_too_large("method exceeds 256 bytes"),
        ));
    }
    let params = value.get("params").cloned().unwrap_or_else(|| serde_json::json!({}));
    if !params.is_object() {
        return Err((
            request_id,
            ErrorObject::invalid_params("params must be a JSON object"),
        ));
    }
    debug!(method = %method, params = %logging::redact_sensitive(&params), "收到 JSON-RPC 请求");
    Ok((request_id, method, params, notification))
}

fn validate_id(value: &serde_json::Value) -> Result<(), (Option<RequestId>, ErrorObject)> {
    let Some(id) = value.get("id") else {
        return Ok(());
    };
    if !is_valid_request_id(id) {
        return Err((
            None,
            ErrorObject::invalid_request("id must be a string or integer"),
        ));
    }
    if request_id_bytes(id) > MAX_ID_BYTES {
        return Err((
            extract_id(value),
            ErrorObject::payload_too_large("request id exceeds 256 bytes"),
        ));
    }
    Ok(())
}

fn parse_params<T: serde::de::DeserializeOwned>(
    params: &serde_json::Value,
) -> Result<T, ErrorObject> {
    serde_json::from_value(params.clone())
        .map_err(|error| ErrorObject::invalid_params(format!("invalid params: {error}")))
}

fn session_create(
    params: &serde_json::Value,
    runtime: &RuntimeHandle,
    rt: &tokio::runtime::Runtime,
) -> Result<serde_json::Value, ErrorObject> {
    let params: CreateParams = parse_params(params)?;
    let mut input = CreateSession::new(params.source.unwrap_or_else(|| "stdio".to_string()));
    input.title = params.title;
    input.cwd = params.cwd;
    input.metadata = params.metadata;
    let handle = rt.block_on(runtime.create_session(input)).map_err(runtime_error_to_rpc)?;
    let snapshot = rt.block_on(handle.snapshot()).map_err(actor_error_to_rpc)?;
    serde_json::to_value(snapshot.session)
        .map_err(|_| ErrorObject::from_code(ErrorCode::InternalError))
}

fn session_list(
    params: &serde_json::Value,
    runtime: &RuntimeHandle,
    rt: &tokio::runtime::Runtime,
) -> Result<serde_json::Value, ErrorObject> {
    let params: ListParams = parse_params(params)?;
    let query = ListSessions {
        limit: params.limit,
        before: params.before.map(|cursor| SessionCursor {
            updated_at: cursor.updated_at,
            id: cursor.id,
        }),
        source: params.source,
        status: params.status,
    };
    let sessions = rt.block_on(runtime.list_sessions(query)).map_err(runtime_error_to_rpc)?;
    serde_json::to_value(sessions).map_err(|_| ErrorObject::from_code(ErrorCode::InternalError))
}

fn session_get(
    params: &serde_json::Value,
    runtime: &RuntimeHandle,
    rt: &tokio::runtime::Runtime,
) -> Result<serde_json::Value, ErrorObject> {
    let params: GetParams = parse_params(params)?;
    let view = rt
        .block_on(runtime.get_session(&params.session_id))
        .map_err(runtime_error_to_rpc)?
        .ok_or_else(|| ErrorObject::from_code(ErrorCode::SessionNotFound))?;
    let (session, messages) = match view {
        SessionView::Live(handle) => {
            let snapshot = rt.block_on(handle.snapshot()).map_err(actor_error_to_rpc)?;
            let messages = rt
                .block_on(handle.list_messages(MessageRange {
                    limit: params.limit,
                    after_sequence: params.after_sequence,
                }))
                .map_err(actor_error_to_rpc)?;
            (snapshot.session, messages)
        },
        SessionView::Snapshot(snapshot) => (
            snapshot.session,
            page_messages(snapshot.messages, params.after_sequence, params.limit),
        ),
    };
    Ok(session_response(session, messages))
}

fn session_resume(
    params: &serde_json::Value,
    runtime: &RuntimeHandle,
    rt: &tokio::runtime::Runtime,
) -> Result<serde_json::Value, ErrorObject> {
    let params: ResumeParams = parse_params(params)?;
    let handle = rt
        .block_on(runtime.resume_session(&params.session_id))
        .map_err(runtime_error_to_rpc)?;
    let snapshot = rt.block_on(handle.snapshot()).map_err(actor_error_to_rpc)?;
    Ok(session_response(snapshot.session, snapshot.messages))
}

fn session_subscribe(
    params: &serde_json::Value,
    runtime: &RuntimeHandle,
    rt: &tokio::runtime::Runtime,
    subscriptions: &mut Subscriptions,
) -> Result<serde_json::Value, ErrorObject> {
    let params: SubscribeParams = parse_params(params)?;
    if params.after_seq != 0 {
        return Err(ErrorObject::from_code(ErrorCode::SequenceUnavailable));
    }
    let handle = rt
        .block_on(runtime.resume_session(&params.session_id))
        .map_err(runtime_error_to_rpc)?;
    let receiver = rt.block_on(handle.subscribe()).map_err(actor_error_to_rpc)?;
    subscriptions.insert(params.session_id.clone(), receiver);
    Ok(
        serde_json::json!({"session_id": params.session_id, "subscribed": true, "after_seq": params.after_seq}),
    )
}

fn page_messages(messages: Vec<Message>, after: Option<u64>, limit: Option<u32>) -> Vec<Message> {
    messages
        .into_iter()
        .filter(|message| after.map_or(true, |value| message.sequence > value))
        .take(limit.unwrap_or(50) as usize)
        .collect()
}

fn session_response(
    session: sagent_types::session::Session,
    messages: Vec<Message>,
) -> serde_json::Value {
    serde_json::json!({"session": session, "messages": messages, "has_more": false})
}

/// Drain 当前已排队的事件并转换为 JSON-RPC notification。
pub fn drain_events(subscriptions: &mut Subscriptions) -> Vec<serde_json::Value> {
    subscriptions
        .values_mut()
        .flat_map(|receiver| {
            let mut events = Vec::new();
            while let Ok(event) = receiver.try_recv() {
                events.push(event_to_envelope(event));
            }
            events
        })
        .collect()
}

fn event_to_envelope(event: SessionEvent) -> serde_json::Value {
    let (method, session_id, seq, timestamp, data) = match event {
        SessionEvent::Created { session, seq } => (
            "session.created",
            session.id.clone(),
            seq,
            session.updated_at.clone(),
            serde_json::json!({"session": session}),
        ),
        SessionEvent::MessageAppended {
            message,
            revision,
            seq,
        } => (
            "message.appended",
            message.session_id.clone(),
            seq,
            message.created_at.clone(),
            serde_json::json!({"message": message, "revision": revision}),
        ),
        SessionEvent::Closed { session, seq } => (
            "session.closed",
            session.id.clone(),
            seq,
            session.updated_at.clone(),
            serde_json::json!({"session": session}),
        ),
        SessionEvent::Recovered { session, seq } => (
            "session.recovered",
            session.id.clone(),
            seq,
            session.updated_at.clone(),
            serde_json::json!({"session": session}),
        ),
        SessionEvent::Failed {
            session_id,
            error,
            seq,
        } => (
            "session.failed",
            session_id,
            seq,
            "1970-01-01T00:00:00Z".to_string(),
            serde_json::json!({"error": error}),
        ),
    };
    let event_id = EVENT_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
    serde_json::to_value(EventEnvelope {
        jsonrpc: "2.0".to_string(),
        method: method.to_string(),
        params: EventParams {
            event_id: EventId(format!("evt_{event_id}")),
            session_id: Some(session_id),
            turn_id: None,
            seq,
            timestamp,
            data,
        },
    })
    .expect("event envelope is serializable")
}

fn runtime_error_to_rpc(error: RuntimeError) -> ErrorObject {
    match error {
        RuntimeError::SessionNotFound(_)
        | RuntimeError::Repository(sagent_session::RepositoryError::NotFound(_)) => {
            ErrorObject::from_code(ErrorCode::SessionNotFound)
        },
        RuntimeError::ShuttingDown => ErrorObject::from_code(ErrorCode::RuntimeShuttingDown),
        RuntimeError::Repository(sagent_session::RepositoryError::SessionClosed(_)) => {
            ErrorObject::from_code(ErrorCode::SessionAlreadyClosed)
        },
        RuntimeError::Repository(sagent_session::RepositoryError::LimitExceeded { .. }) => {
            ErrorObject::from_code(ErrorCode::TranscriptTooLarge)
        },
        RuntimeError::Database(sagent_session::DatabaseError::Unsupported { .. }) => {
            ErrorObject::from_code(ErrorCode::DatabaseSchemaUnsupported)
        },
        _ => ErrorObject::from_code(ErrorCode::DatabaseUnavailable),
    }
}

fn actor_error_to_rpc(error: sagent_runtime::ActorError) -> ErrorObject {
    match error {
        sagent_runtime::ActorError::MailboxFull(_) => {
            ErrorObject::from_code(ErrorCode::MailboxFull)
        },
        sagent_runtime::ActorError::Shutdown(_) => {
            ErrorObject::from_code(ErrorCode::RuntimeShuttingDown)
        },
        sagent_runtime::ActorError::Repository(sagent_session::RepositoryError::SessionClosed(
            _,
        )) => ErrorObject::from_code(ErrorCode::SessionAlreadyClosed),
        sagent_runtime::ActorError::Repository(sagent_session::RepositoryError::NotFound(_)) => {
            ErrorObject::from_code(ErrorCode::SessionNotFound)
        },
        sagent_runtime::ActorError::Repository(
            sagent_session::RepositoryError::LimitExceeded { .. },
        ) => ErrorObject::from_code(ErrorCode::TranscriptTooLarge),
        _ => ErrorObject::from_code(ErrorCode::DatabaseUnavailable),
    }
}

fn extract_id(value: &serde_json::Value) -> Option<RequestId> {
    match value.get("id") {
        Some(serde_json::Value::String(value)) => Some(RequestId::String(value.clone())),
        Some(serde_json::Value::Number(value)) => value.as_i64().map(RequestId::Number),
        _ => None,
    }
}

fn is_valid_request_id(value: &serde_json::Value) -> bool {
    value.as_str().is_some() || value.as_i64().is_some()
}
fn request_id_bytes(value: &serde_json::Value) -> usize {
    value.as_str().map_or_else(|| value.to_string().len(), str::len)
}
fn format_request_id(value: &Option<RequestId>) -> String {
    value.as_ref().map_or_else(|| "null".to_string(), ToString::to_string)
}

fn build_success_response(id: RequestId, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// 构建 JSON-RPC error response。
pub fn build_error_response(id: Option<RequestId>, error: &ErrorObject) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": error.code, "message": error.message, "data": error.data}})
}

fn handle_protocol_describe(caps: &Capabilities) -> Result<serde_json::Value, ErrorObject> {
    let version = caps.protocol_version();
    Ok(
        serde_json::json!({"protocol": version.protocol, "version": version.version, "runtime_version": version.runtime_version, "features": caps.feature_names()}),
    )
}

fn handle_health() -> Result<serde_json::Value, ErrorObject> {
    let version = ProtocolVersion::default();
    Ok(
        serde_json::json!({"status": "ok", "protocol": version.protocol, "version": version.version}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_round_trip() {
        let response = dispatch(
            r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":{"ok":true}}"#,
            &Capabilities::default(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(response["result"]["ok"], true);
    }

    #[test]
    fn invalid_params_are_rejected() {
        let (_, error) = dispatch(
            r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":[]}"#,
            &Capabilities::default(),
        )
        .unwrap_err();
        assert_eq!(error.code, -32602);
    }
}
