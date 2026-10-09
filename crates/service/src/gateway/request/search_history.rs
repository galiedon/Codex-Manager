use bytes::Bytes;
use serde_json::{json, Value};

/// Supply a history-only declaration without granting a new tool capability.
/// This is a structural compatibility rule, not a compaction prompt classifier.
pub(super) fn repair_zero_permission_search_history(
    path: &str,
    body: Bytes,
    client_value: Option<&Value>,
) -> Bytes {
    if path.split('?').next() != Some("/v1/responses") {
        return body;
    }
    let Some(obj) = client_value.and_then(Value::as_object) else {
        return body;
    };
    let Some(input) = obj.get("input").and_then(Value::as_array) else {
        return body;
    };
    if !input.iter().any(|item| {
        item.get("type").and_then(Value::as_str) == Some("web_search_call")
            && item.get("status").and_then(Value::as_str) == Some("completed")
    }) {
        return body;
    }
    let tools = match obj.get("tools") {
        Some(Value::Array(tools)) => Some(tools),
        None => None,
        // Do not turn malformed requests into different, valid requests.
        Some(_) => return body,
    };
    if tools.is_some_and(|tools| {
        tools.iter().any(|tool| {
            tool.get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind == "web_search" || kind.starts_with("web_search_"))
        })
    }) {
        return body;
    }
    let zero_permission = match obj.get("tool_choice") {
        Some(Value::String(choice)) if choice == "none" => true,
        choice if choice.is_none() || choice.and_then(Value::as_str) == Some("auto") => {
            // dynamic_tools are merged later; they are callable even with tools=[].
            tools.is_some_and(Vec::is_empty)
                && obj
                    .get("dynamic_tools")
                    .is_none_or(|value| value.as_array().is_some_and(Vec::is_empty))
        }
        _ => false,
    };
    if !zero_permission {
        return body;
    }
    let mut repaired = obj.clone();
    let tools = repaired.entry("tools").or_insert_with(|| json!([]));
    tools
        .as_array_mut()
        .expect("checked tools array")
        .push(json!({"type": "web_search"}));
    repaired.insert("tool_choice".to_string(), json!("none"));
    serde_json::to_vec(&repaired)
        .map(Bytes::from)
        .unwrap_or(body)
}

#[cfg(test)]
#[path = "search_history_tests.rs"]
mod tests;
