use sagent_provider::{
    ProviderError, ProviderEvent, StopReason, TokenUsage,
    openai_event::OpenAiStreamParser,
    sse::{SseDecoder, SseEvent, SseFrame},
};

#[test]
fn parses_normal_fixture_and_done_marker() {
    let fixture = include_bytes!("fixtures/provider/normal_text.sse");
    let mut decoder = SseDecoder::new();
    let mut events = decoder.push(fixture).expect("fixture 应能解析");
    events.extend(decoder.finish().expect("fixture EOF 应能冲刷"));

    assert_eq!(events.len(), 4);
    assert!(matches!(events[0], SseEvent::Frame(_)));
    assert!(matches!(events[1], SseEvent::Frame(_)));
    assert!(matches!(events[2], SseEvent::Frame(_)));
    assert_eq!(events[3], SseEvent::Done);
}

#[test]
fn parses_split_json_fixture_across_arbitrary_chunks() {
    let fixture = include_bytes!("fixtures/provider/split_json.sse");
    let mut decoder = SseDecoder::new();
    let mut events = Vec::new();
    for chunk in fixture.chunks(3) {
        events.extend(decoder.push(chunk).expect("任意字节分片都应能解析"));
    }
    events.extend(decoder.finish().expect("fixture 应能完成冲刷"));

    assert_eq!(events.len(), 3);
    assert_eq!(events[2], SseEvent::Done);
}

#[test]
fn parser_preserves_utf8_split_inside_character() {
    let bytes = "data: 你\n\ndata: 好\n\n".as_bytes();
    let split = bytes.iter().position(|byte| *byte == 0xe4).unwrap() + 1;
    let mut decoder = SseDecoder::new();
    assert!(decoder.push(&bytes[..split]).unwrap().is_empty());
    let mut events = decoder.push(&bytes[split..]).unwrap();
    events.extend(decoder.finish().unwrap());

    assert_eq!(
        events,
        vec![
            SseEvent::Frame(SseFrame {
                event: None,
                id: None,
                data: "你".into()
            }),
            SseEvent::Frame(SseFrame {
                event: None,
                id: None,
                data: "好".into()
            })
        ]
    );
}

#[test]
fn openai_stream_parser_maps_delta_finish_usage_and_done() {
    let fixture = include_bytes!("fixtures/provider/usage.sse");
    let mut parser = OpenAiStreamParser::new();
    let mut events = Vec::new();
    for chunk in fixture.chunks(5) {
        events.extend(parser.push(chunk).expect("usage fixture 应能解析"));
    }
    events.extend(parser.finish().expect("finish 和 done 都存在"));

    assert_eq!(
        events,
        vec![
            ProviderEvent::TextDelta {
                text: "完成".into()
            },
            ProviderEvent::Finished {
                reason: StopReason::Stop
            },
            ProviderEvent::Usage {
                usage: TokenUsage {
                    prompt_tokens: 10,
                    completion_tokens: 2,
                    total_tokens: 12
                }
            }
        ]
    );
}

#[test]
fn eof_without_finish_is_incomplete_stream() {
    let fixture = include_bytes!("fixtures/provider/eof_without_finish.sse");
    let mut parser = OpenAiStreamParser::new();
    for chunk in fixture.chunks(2) {
        parser.push(chunk).expect("部分文本本身是合法 JSON");
    }
    assert_eq!(
        parser.finish().expect_err("没有 finish 必须失败"),
        ProviderError::IncompleteStream
    );
}
