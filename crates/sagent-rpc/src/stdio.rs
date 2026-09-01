//! 标准输入输出上的 NDJSON JSON-RPC transport。

use std::io::{self, BufRead, Write};

use sagent_protocol::{
    DispatchService, EventParams, JsonRpcError, JsonRpcEvent, JsonRpcRequest, JsonRpcResponse,
    ProtocolError, ProtocolFeatures, RequestId, dispatch,
};
use serde::Serialize;
use serde_json::Value;

/// 单行请求最大字节数，防止 transport 无界读取。
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// 写出一条完整 NDJSON 帧；调用者应确保 writer 是 stdout。
pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// 写出服务启动完成事件。
pub fn write_ready<W: Write>(writer: &mut W) -> io::Result<()> {
    write_frame(
        writer,
        &JsonRpcEvent {
            jsonrpc: "2.0".to_owned(),
            method: "event".to_owned(),
            params: EventParams {
                event_type: "gateway.ready".to_owned(),
                payload: ProtocolFeatures::phase_three(),
            },
        },
    )
}

/// 读取 stdin 的 NDJSON 请求并按顺序写回响应。
pub fn run<R: BufRead, W: Write, S: DispatchService>(
    reader: &mut R,
    writer: &mut W,
    service: &S,
) -> io::Result<()> {
    write_ready(writer)?;
    let mut line = String::new();

    loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Ok(());
        }
        if bytes > MAX_FRAME_BYTES {
            write_error(
                writer,
                RequestId::Null,
                ProtocolError::InvalidParams(format!(
                    "request frame exceeds {MAX_FRAME_BYTES} bytes"
                )),
            )?;
            continue;
        }

        let request = match serde_json::from_str::<JsonRpcRequest>(line.trim_end()) {
            Ok(request) => request,
            Err(_) => {
                write_frame(
                    writer,
                    &JsonRpcResponse::<Value>::failure(
                        RequestId::Null,
                        JsonRpcError {
                            code: -32700,
                            message: "parse error".to_owned(),
                            data: None,
                        },
                    ),
                )?;
                continue;
            }
        };

        if let Some(response) = dispatch(request, service) {
            write_frame(writer, &response)?;
        }
    }
}

fn write_error<W: Write>(writer: &mut W, id: RequestId, error: ProtocolError) -> io::Result<()> {
    write_frame(
        writer,
        &JsonRpcResponse::<Value>::failure(id, error.to_jsonrpc()),
    )
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use sagent_protocol::{
        GatewayPingResult, SessionListParams, SessionListResult, SessionReadService,
        SessionResumeParams, SessionResumeResult,
    };

    use super::{MAX_FRAME_BYTES, run};

    struct FakeService;
    impl sagent_protocol::GatewayService for FakeService {
        fn ping(&self) -> GatewayPingResult {
            GatewayPingResult {
                ok: true,
                protocol_version: 1,
            }
        }
    }
    impl SessionReadService for FakeService {
        fn list_sessions(
            &self,
            _: &SessionListParams,
        ) -> Result<SessionListResult, sagent_protocol::ProtocolError> {
            Ok(SessionListResult {
                sessions: vec![],
                limit: 50,
                offset: 0,
            })
        }
        fn resume_session(
            &self,
            _: &SessionResumeParams,
        ) -> Result<SessionResumeResult, sagent_protocol::ProtocolError> {
            Err(sagent_protocol::ProtocolError::SessionNotFound(
                "not-used".to_owned(),
            ))
        }
    }

    #[test]
    fn emits_ready_then_ping_and_ignores_notification_response() {
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"gateway.ping\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"gateway.ping\"}\n";
        let mut output = Vec::new();
        run(&mut Cursor::new(input), &mut output, &FakeService).expect("stdio 应成功");
        let text = String::from_utf8(output).expect("输出应为 UTF-8");
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("gateway.ready"));
        assert!(lines[1].contains("\"id\":1"));
    }

    #[test]
    fn malformed_json_returns_parse_error() {
        let mut output = Vec::new();
        run(&mut Cursor::new(b"not-json\n"), &mut output, &FakeService).expect("stdio 应成功");
        let text = String::from_utf8(output).expect("输出应为 UTF-8");
        assert!(text.contains("parse error"));
    }

    #[test]
    fn frame_limit_is_one_megabyte() {
        assert_eq!(MAX_FRAME_BYTES, 1024 * 1024);
    }
}
