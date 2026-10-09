use super::*;

fn fixture() -> Value {
    json!({
        "model": "test-model", "input": [
            {"type":"web_search_call", "status":"completed", "id":"ws_test", "action":{"type":"search", "query":"test"}},
            {"type":"reasoning", "id":"rs_test", "encrypted_content":"synthetic-test-value"},
            {"role":"user", "content":"test summary"}
        ],
        "tools": [], "tool_choice":"auto", "stream":true, "store":false,
        "reasoning":{"effort":"high"}, "include":["reasoning.encrypted_content"]
    })
}

fn repair(path: &str, value: &Value) -> Value {
    let body = Bytes::from(serde_json::to_vec(value).unwrap());
    serde_json::from_slice(&repair_zero_permission_search_history(
        path,
        body,
        Some(value),
    ))
    .unwrap()
}

#[test]
fn zero_permission_history_declares_search_without_altering_history_or_reasoning() {
    for choice in [Some("auto"), Some("none"), None] {
        let mut original = fixture();
        if let Some(choice) = choice {
            original["tool_choice"] = json!(choice);
        } else {
            original.as_object_mut().unwrap().remove("tool_choice");
        }
        let actual = repair("/v1/responses?test=1", &original);
        let mut expected = original.clone();
        expected["tools"] = json!([{"type":"web_search"}]);
        expected["tool_choice"] = json!("none");
        assert_eq!(actual, expected);
        assert_eq!(repair("/v1/responses", &actual), actual);
    }
}

#[test]
fn no_completed_history_and_nested_text_mentions_are_byte_identical() {
    for input in [
        json!([]),
        json!([{"role":"user","content":"web_search_call completed"}]),
        json!([{"type":"web_search_call","status":"in_progress"}]),
    ] {
        let mut original = fixture();
        original["input"] = input;
        let body = Bytes::from(serde_json::to_vec_pretty(&original).unwrap());
        assert_eq!(
            repair_zero_permission_search_history("/v1/responses", body.clone(), Some(&original)),
            body
        );
    }
}

#[test]
fn existing_search_declarations_keep_configuration_and_choice() {
    for kind in [
        "web_search",
        "web_search_2025_08_26",
        "web_search_preview",
        "web_search_preview_2025_03_11",
    ] {
        let mut original = fixture();
        original["tools"] = json!([{"type":kind,"search_context_size":"low","filters":{"allowed_domains":["example.test"]}}]);
        assert_eq!(repair("/v1/responses", &original), original);
    }
}

#[test]
fn ordinary_tool_permissions_are_not_expanded_or_disabled() {
    for choice in [
        json!("auto"),
        json!("required"),
        json!({"type":"function","name":"read_file"}),
        json!({"type":"allowed_tools","mode":"auto","tools":[{"type":"function","name":"read_file"}]}),
    ] {
        let mut original = fixture();
        original["tools"] =
            json!([{"type":"function","name":"read_file","parameters":{"type":"object"}}]);
        original["tool_choice"] = choice;
        assert_eq!(repair("/v1/responses", &original), original);
    }
    let mut dynamic = fixture();
    dynamic["dynamic_tools"] = json!([{"name":"dynamic_read","input_schema":{"type":"object"}}]);
    assert_eq!(repair("/v1/responses", &dynamic), dynamic);
}

#[test]
fn disabled_existing_functions_are_preserved_when_search_is_added() {
    let mut original = fixture();
    original["tools"] = json!([{"type":"function","name":"read_file","strict":true,"parameters":{"type":"object"}}]);
    original["tool_choice"] = json!("none");
    let actual = repair("/v1/responses", &original);
    assert_eq!(actual["tools"][0], original["tools"][0]);
    assert_eq!(actual["tools"][1], json!({"type":"web_search"}));
    assert_eq!(actual["tool_choice"], "none");
}

#[test]
fn independent_compact_chat_and_unrecognized_paths_are_untouched() {
    let original = fixture();
    for path in [
        "/v1/responses/compact",
        "/v1/responses/compact?q=1",
        "/v1/chat/completions",
        "/v1/responses/other",
    ] {
        assert_eq!(repair(path, &original), original);
    }
}

#[test]
fn malformed_or_unspecified_permissions_are_not_inferred_from_defaults() {
    for tools in [Some(json!(null)), Some(json!({})), None] {
        let mut original = fixture();
        if let Some(tools) = tools {
            original["tools"] = tools;
        } else {
            original.as_object_mut().unwrap().remove("tools");
        }
        assert_eq!(repair("/v1/responses", &original), original);
    }
    let mut disabled = fixture();
    disabled.as_object_mut().unwrap().remove("tools");
    disabled["tool_choice"] = json!("none");
    assert_eq!(
        repair("/v1/responses", &disabled)["tools"],
        json!([{"type":"web_search"}])
    );
    for choice in [json!("required"), json!(null), json!({"type":"web_search"})] {
        let mut original = fixture();
        original["tool_choice"] = choice;
        assert_eq!(repair("/v1/responses", &original), original);
    }
}

#[test]
fn repeated_new_search_history_is_repaired_on_every_request_through_transport_rules() {
    let mut original = fixture();
    for index in 0..4 {
        original["input"].as_array_mut().unwrap().push(json!({"type":"web_search_call", "status":"completed", "id":format!("ws_next_{index}")}));
        let repaired = repair("/v1/responses", &original);
        let normalized =
            super::super::official_responses_http::normalize_official_responses_http_body(
                "/v1/responses",
                serde_json::to_vec(&repaired).unwrap(),
            );
        let forwarded = super::super::request_rewrite::apply_codex_candidate_transport_rules(
            "/v1/responses",
            normalized,
        );
        let actual: Value = serde_json::from_slice(&forwarded).unwrap();
        assert_eq!(actual["input"], original["input"]);
        assert_eq!(actual["reasoning"], original["reasoning"]);
        assert_eq!(actual["tools"], json!([{"type":"web_search"}]));
        assert_eq!(actual["tool_choice"], "none");
    }
}
