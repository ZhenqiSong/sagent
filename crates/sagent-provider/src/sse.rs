//! SSE 字节分帧器。
//!
//! 该模块不依赖 HTTP client，只处理 SSE 的行和事件边界。原始字节会先缓存到完整行，
//! 因此 UTF-8 多字节字符跨 TCP chunk 时不会被错误地切开。

use crate::ProviderError;
use std::str;

/// 单个 SSE frame 的最大字节数，防止异常响应导致无界内存增长。
pub const MAX_SSE_FRAME_SIZE: usize = 1024 * 1024;

/// 已完成的 SSE frame。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SseFrame {
    pub event: Option<String>,
    pub id: Option<String>,
    pub data: String,
}

/// SSE 解码后的事件；`Done` 是 OpenAI-compatible 流的特殊结束标记。
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SseEvent {
    Frame(SseFrame),
    Done,
}

/// 增量 SSE decoder。
#[derive(Debug, Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
    data_lines: Vec<String>,
    event: Option<String>,
    id: Option<String>,
    frame_size: usize,
}

impl SseDecoder {
    /// 创建空 decoder。
    pub fn new() -> Self {
        Self::default()
    }

    /// 推入任意网络字节块，返回其中已经由空行结束的事件。
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, ProviderError> {
        self.buffer.extend_from_slice(bytes);

        let mut events = Vec::new();
        while let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=newline).collect();
            if let Some(event) = self.consume_line(&line[..line.len() - 1])? {
                events.push(event);
            }
        }
        // 只有尚未遇到换行的待处理行需要单独限制；一个网络 chunk 可以合法地包含
        // 多个小 frame，不能按 chunk 总大小误判超限。
        if self.buffer.len() > MAX_SSE_FRAME_SIZE {
            return Err(protocol("SSE 行超过大小限制"));
        }
        Ok(events)
    }

    /// 在底层连接 EOF 时冲刷最后一行和未以空行结束的 frame。
    ///
    /// OpenAI-compatible adapter 仍需检查是否同时看到了 finish 和 `[DONE]`；只有
    /// 两者都存在才算正常完成，否则应返回 `ProviderError::IncompleteStream`。
    pub fn finish(&mut self) -> Result<Vec<SseEvent>, ProviderError> {
        let mut events = Vec::new();
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            if let Some(event) = self.consume_line(&line)? {
                events.push(event);
            }
        }
        if (!self.data_lines.is_empty() || self.event.is_some() || self.id.is_some())
            && let Some(event) = self.dispatch_frame()?
        {
            events.push(event);
        }
        Ok(events)
    }

    fn consume_line(&mut self, line: &[u8]) -> Result<Option<SseEvent>, ProviderError> {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        self.frame_size = self
            .frame_size
            .checked_add(line.len().saturating_add(1))
            .ok_or_else(|| protocol("SSE frame 大小溢出"))?;
        if self.frame_size > MAX_SSE_FRAME_SIZE {
            return Err(protocol("SSE frame 超过大小限制"));
        }

        if line.is_empty() {
            return self.dispatch_frame();
        }
        if line[0] == b':' {
            return Ok(None);
        }

        let (field, raw_value) = match line.iter().position(|byte| *byte == b':') {
            Some(index) => (&line[..index], &line[index + 1..]),
            None => (line, &[][..]),
        };
        let value = raw_value.strip_prefix(b" ").unwrap_or(raw_value);
        let field = str::from_utf8(field).map_err(|_| protocol("SSE 字段不是 UTF-8"))?;
        let value = str::from_utf8(value).map_err(|_| protocol("SSE 数据不是 UTF-8"))?;

        match field {
            "data" => self.data_lines.push(value.to_owned()),
            "event" => self.event = Some(value.to_owned()),
            "id" => self.id = Some(value.to_owned()),
            // SSE 的 retry 字段和未知字段对模型事件没有意义，安全忽略。
            "retry" => {}
            _ => {}
        }
        Ok(None)
    }

    fn dispatch_frame(&mut self) -> Result<Option<SseEvent>, ProviderError> {
        self.frame_size = 0;
        let data_lines = std::mem::take(&mut self.data_lines);
        let event = self.event.take();
        let id = self.id.take();
        if data_lines.is_empty() {
            return Ok(None);
        }

        let data = data_lines.join("\n");
        if data == "[DONE]" {
            return Ok(Some(SseEvent::Done));
        }
        Ok(Some(SseEvent::Frame(SseFrame { event, id, data })))
    }
}

fn protocol(message: impl Into<String>) -> ProviderError {
    ProviderError::Protocol(message.into())
}

#[cfg(test)]
mod tests {
    use super::{MAX_SSE_FRAME_SIZE, SseDecoder, SseEvent, SseFrame};
    use crate::ProviderError;

    #[test]
    fn parses_lf_and_crlf_frames() {
        let mut decoder = SseDecoder::new();
        let events = decoder
            .push(b"event: message\r\nid: 1\r\ndata: hello\r\n\r\n")
            .expect("CRLF 应能解析");
        assert_eq!(
            events,
            vec![SseEvent::Frame(SseFrame {
                event: Some("message".into()),
                id: Some("1".into()),
                data: "hello".into(),
            })]
        );
    }

    #[test]
    fn joins_multiple_data_lines_and_ignores_comments() {
        let mut decoder = SseDecoder::new();
        let events = decoder
            .push(b": keep-alive\nretry: 1000\ndata: first\ndata: second\n\n")
            .expect("data 行应能合并");
        assert_eq!(
            events[0],
            SseEvent::Frame(SseFrame {
                event: None,
                id: None,
                data: "first\nsecond".into(),
            })
        );
    }

    #[test]
    fn handles_split_utf8_and_final_line_without_newline() {
        let payload = "data: 你好\n\n";
        let bytes = payload.as_bytes();
        let split = bytes.iter().position(|byte| *byte == 0xe4).unwrap() + 1;
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(&bytes[..split]).unwrap().is_empty());
        let events = decoder.push(&bytes[split..]).expect("UTF-8 分片应能合并");
        assert_eq!(
            events[0].clone(),
            SseEvent::Frame(SseFrame {
                event: None,
                id: None,
                data: "你好".into(),
            })
        );

        let mut eof_decoder = SseDecoder::new();
        eof_decoder.push(b"data: eof").unwrap();
        assert_eq!(
            eof_decoder.finish().unwrap(),
            vec![SseEvent::Frame(SseFrame {
                event: None,
                id: None,
                data: "eof".into(),
            })]
        );
    }

    #[test]
    fn parses_done_marker() {
        let mut decoder = SseDecoder::new();
        assert_eq!(
            decoder.push(b"data: [DONE]\n\n").unwrap(),
            vec![SseEvent::Done]
        );
    }

    #[test]
    fn rejects_invalid_utf8_and_oversized_frame() {
        let mut invalid = SseDecoder::new();
        assert!(matches!(
            invalid.push(b"data: \xff\n\n"),
            Err(ProviderError::Protocol(_))
        ));

        let mut oversized = SseDecoder::new();
        let bytes = vec![b'a'; MAX_SSE_FRAME_SIZE + 1];
        assert!(matches!(
            oversized.push(&bytes),
            Err(ProviderError::Protocol(_))
        ));
    }
}
