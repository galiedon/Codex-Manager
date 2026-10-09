//! Fixed-schema request structure for logs. Never copy user-provided strings.
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CapturePolicy {
    StructureOnlyV1,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum BodyFormat {
    JsonObject,
    JsonArray,
    JsonScalar,
    InvalidJson,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ToolChoice {
    Missing,
    Auto,
    None,
    Required,
    Explicit,
    Invalid,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RequestStructure {
    capture_policy: CapturePolicy,
    body_format: BodyFormat,
    input_items: Option<usize>,
    completed_web_search_calls: usize,
    tools_count: Option<usize>,
    web_search_declarations: usize,
    dynamic_tools_count: Option<usize>,
    tool_choice: ToolChoice,
    stream: Option<bool>,
    has_encrypted_content: bool,
}

fn has_encrypted_content(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            object.contains_key("encrypted_content") || object.values().any(has_encrypted_content)
        }
        Value::Array(items) => items.iter().any(has_encrypted_content),
        _ => false,
    }
}

/// Idempotent for spill replay; typed enums reject forged summaries containing
/// arbitrary text. Invalid JSON and binary/compressed bodies are never copied.
pub(in crate::gateway) fn structure_only_body(body: &[u8]) -> Bytes {
    let summary = if let Ok(summary) = serde_json::from_slice::<RequestStructure>(body) {
        summary
    } else {
        let value = serde_json::from_slice::<Value>(body).ok();
        let input = value.as_ref().and_then(|value| {
            ["input", "messages", "contents"]
                .iter()
                .find_map(|key| value.get(*key).and_then(Value::as_array))
        });
        let tools = value
            .as_ref()
            .and_then(|value| value.get("tools"))
            .and_then(Value::as_array);
        RequestStructure {
            capture_policy: CapturePolicy::StructureOnlyV1,
            body_format: match value.as_ref() {
                Some(Value::Object(_)) => BodyFormat::JsonObject,
                Some(Value::Array(_)) => BodyFormat::JsonArray,
                Some(_) => BodyFormat::JsonScalar,
                None => BodyFormat::InvalidJson,
            },
            input_items: input.map(Vec::len),
            completed_web_search_calls: input.map_or(0, |items| {
                items
                    .iter()
                    .filter(|item| {
                        item.get("type").and_then(Value::as_str) == Some("web_search_call")
                            && item.get("status").and_then(Value::as_str) == Some("completed")
                    })
                    .count()
            }),
            tools_count: tools.map(Vec::len),
            web_search_declarations: tools.map_or(0, |items| {
                items
                    .iter()
                    .filter(|item| {
                        item.get("type")
                            .and_then(Value::as_str)
                            .is_some_and(|kind| {
                                kind == "web_search" || kind.starts_with("web_search_")
                            })
                    })
                    .count()
            }),
            dynamic_tools_count: value
                .as_ref()
                .and_then(|value| value.get("dynamic_tools"))
                .and_then(Value::as_array)
                .map(Vec::len),
            tool_choice: match value.as_ref().and_then(|value| value.get("tool_choice")) {
                None => ToolChoice::Missing,
                Some(Value::String(choice)) if choice == "auto" => ToolChoice::Auto,
                Some(Value::String(choice)) if choice == "none" => ToolChoice::None,
                Some(Value::String(choice)) if choice == "required" => ToolChoice::Required,
                Some(Value::Object(_)) => ToolChoice::Explicit,
                _ => ToolChoice::Invalid,
            },
            stream: value
                .as_ref()
                .and_then(|value| value.get("stream"))
                .and_then(Value::as_bool),
            has_encrypted_content: value.as_ref().is_some_and(has_encrypted_content),
        }
    };
    Bytes::from(serde_json::to_vec(&summary).unwrap_or_else(|_| b"{}".to_vec()))
}

#[cfg(test)]
#[path = "tests/request_log_payload_structure_tests.rs"]
mod tests;
