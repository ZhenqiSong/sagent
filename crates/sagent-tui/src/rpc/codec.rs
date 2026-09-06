//! 有界 NDJSON 编解码。
//!
//! `sagent-rpc` 的 stdout 是严格的协议流，不是日志流。这里将每行限制在固定上限内，
//! 防止故障子进程省略换行而令 TUI 无界分配内存。

use std::io;

use sagent_protocol::{JsonRpcEvent, JsonRpcRequest, JsonRpcResponse};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

/// 单条 NDJSON 协议帧允许的最大字节数，与服务端 stdio transport 保持同一数量级。
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// 服务端 stdout 上可接收的两种帧。
#[derive(Debug)]
pub enum ServerFrame {
    /// 与一个带 id 请求关联的响应。
    Response(JsonRpcResponse<Value>),
    /// 服务端主动通知，例如 `gateway.ready`。
    Event(JsonRpcEvent<Value>),
}

/// 将请求编码为一条完整 NDJSON 记录；writer 是 stdin 的唯一所有者。
pub async fn write_request<W: AsyncWrite + Unpin>(
    writer: &mut W,
    request: &JsonRpcRequest,
) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(request)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await
}

/// 读取一条有界记录；返回 `None` 表示 stdout EOF。
pub async fn read_frame<R: AsyncBufRead + Unpin>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut frame = Vec::new();

    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Ok(Some(frame))
            };
        }

        let consumed = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |index| index + 1);
        if frame.len().saturating_add(consumed) > MAX_FRAME_BYTES {
            // 已知该帧不可信时不必继续保留字节；调用者会关闭整个协议连接。
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "RPC NDJSON 帧超过最大长度",
            ));
        }
        frame.extend_from_slice(&buffer[..consumed]);
        reader.consume(consumed);

        if frame.ends_with(b"\n") {
            return Ok(Some(frame));
        }
    }
}

/// 解析服务端帧；`method: event` 与普通 response 的语义必须保持互斥。
pub fn decode_server_frame(bytes: &[u8]) -> Result<ServerFrame, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("RPC stdout 不是合法 JSON：{error}"))?;
    if value.get("method").and_then(Value::as_str) == Some("event") {
        return serde_json::from_value(value)
            .map(ServerFrame::Event)
            .map_err(|error| format!("RPC event 信封无效：{error}"));
    }
    serde_json::from_value(value)
        .map(ServerFrame::Response)
        .map_err(|error| format!("RPC response 信封无效：{error}"))
}

#[cfg(test)]
mod tests {
    use sagent_protocol::{JsonRpcRequest, RequestId};
    use serde_json::json;
    use tokio::io::{AsyncWriteExt, BufReader, duplex};

    use super::{ServerFrame, decode_server_frame, read_frame, write_request};

    #[tokio::test]
    async fn request_writer_emits_exactly_one_newline_delimited_json_record() {
        let (mut client, server) = duplex(1024);
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(RequestId::Number(7.into())),
            method: "gateway.ping".to_owned(),
            params: Some(json!({})),
        };

        write_request(&mut client, &request)
            .await
            .expect("请求应能写入内存 transport");
        let mut reader = BufReader::new(server);
        let frame = read_frame(&mut reader)
            .await
            .expect("应能读取 NDJSON")
            .expect("writer 已写入一帧");

        assert!(frame.ends_with(b"\n"));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&frame).expect("帧应保持 JSON")["method"],
            json!("gateway.ping")
        );
    }

    #[test]
    fn event_never_decodes_as_a_response() {
        let frame = decode_server_frame(
            br#"{"jsonrpc":"2.0","method":"event","params":{"type":"gateway.ready","payload":{}}}"#,
        )
        .expect("合法 event 应被识别");

        assert!(matches!(frame, ServerFrame::Event(_)));
    }

    #[tokio::test]
    async fn oversized_frame_is_rejected_before_unbounded_buffering() {
        let (mut writer, reader) = duplex(super::MAX_FRAME_BYTES * 2);
        let payload = vec![b'x'; super::MAX_FRAME_BYTES + 1];
        writer
            .write_all(&payload)
            .await
            .expect("内存 transport 应接收测试字节");
        let mut reader = BufReader::new(reader);

        let error = read_frame(&mut reader)
            .await
            .expect_err("超长且无换行的 stdout 帧必须失败");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}
