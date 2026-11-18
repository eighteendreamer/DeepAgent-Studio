//! Synthetic text lifecycles shared by HTTP framing, byte and timeout tests.
#![allow(dead_code)]

use super::WireMode;
use serde_json::{json, Value};

pub const MODES: [WireMode; 3] = [
    WireMode::Responses,
    WireMode::ChatCompletions,
    WireMode::Messages,
];

pub fn endpoint(mode: WireMode) -> &'static str {
    match mode {
        WireMode::Responses => "/responses",
        WireMode::ChatCompletions => "/chat/completions",
        WireMode::Messages => "/v1/messages",
    }
}

pub fn start(mode: WireMode) -> Vec<String> {
    match mode {
        WireMode::Responses | WireMode::ChatCompletions => vec![],
        WireMode::Messages => vec![
            message_start(),
            json!({"type":"content_block_start","index":0,
                "content_block":{"type":"text","text":""}})
            .to_string(),
        ],
    }
}

pub fn message_start() -> String {
    json!({"type":"message_start","message":{
        "id":"h30-response","type":"message","role":"assistant",
        "model":"deepseek-v4-flash","content":[],"stop_reason":null,
        "stop_sequence":null,"usage":{"input_tokens":9,"output_tokens":0}
    }})
    .to_string()
}

pub fn delta(mode: WireMode, text: &str, sequence: usize) -> String {
    match mode {
        WireMode::Responses => json!({"type":"response.output_text.delta",
            "sequence_number":sequence,"item_id":"h30-output","output_index":0,
            "content_index":0,"delta":text}),
        WireMode::ChatCompletions => json!({"choices":[{
            "index":0,"delta":{"content":text},"finish_reason":null}]}),
        WireMode::Messages => json!({"type":"content_block_delta","index":0,
            "delta":{"type":"text_delta","text":text}}),
    }
    .to_string()
}

pub fn finish(mode: WireMode, text: &str, sequence: usize) -> Vec<String> {
    let events: Vec<Value> = match mode {
        WireMode::Responses => vec![json!({"type":"response.completed",
        "sequence_number":sequence,"response":{
            "id":"h30-response","object":"response","created_at":1,
            "model":"deepseek-v4-flash","status":"completed",
            "output":[{"type":"message","id":"h30-output","role":"assistant",
                "content":[{"type":"output_text","text":text,"annotations":[]}]}],
            "usage":{"input_tokens":9,"output_tokens":2,"total_tokens":11}
        }})],
        WireMode::ChatCompletions => vec![
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
            json!({"choices":[],"usage":{
                "prompt_tokens":9,"completion_tokens":2,"total_tokens":11}}),
        ],
        WireMode::Messages => vec![
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{
                "stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":2}}),
            json!({"type":"message_stop"}),
        ],
    };
    let mut events: Vec<_> = events.into_iter().map(|event| event.to_string()).collect();
    if mode == WireMode::ChatCompletions {
        events.push("[DONE]".into());
    }
    events
}

pub fn encode(events: impl IntoIterator<Item = String>) -> Vec<u8> {
    events
        .into_iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>()
        .into_bytes()
}
