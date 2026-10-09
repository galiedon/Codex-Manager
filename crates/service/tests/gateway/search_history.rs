use super::*;
use serde_json::{json, Value};

#[test]
fn search_history_http_forwarding_and_sse_error_logging_regression() {
    let _lock = test_env_guard();
    let dir = new_test_dir("codexmanager-search-history-regression");
    let db_path = dir.join("codexmanager.db");
    let _db = EnvGuard::set("CODEXMANAGER_DB_PATH", db_path.to_str().unwrap());
    let storage = Storage::open(&db_path).unwrap();
    storage.init().unwrap();
    let key = "synthetic-search-history-platform-key";
    super::prompt_cache::seed_openai_compat_gateway(&storage, key, "gk_search_history");
    let mut base = json!({
        "model":"gpt-5.3-codex", "instructions":"test instructions", "stream":true,
        "store":false, "reasoning":{"effort":"high"}, "include":["reasoning.encrypted_content"],
        "input":[{"type":"reasoning","id":"rs_test","encrypted_content":"synthetic-test-value"}],
        "tools":[], "tool_choice":"auto"
    });
    let mut cases = Vec::new();
    cases.push((base.clone(), false)); // No search history.
    for index in 0..4 {
        base["input"].as_array_mut().unwrap().push(json!({
            "type":"web_search_call", "status":"completed", "id":format!("ws_cycle_{index}"),
            "action":{"type":"search","query":"synthetic query"}
        }));
        cases.push((base.clone(), true));
    }
    let mut declared = base.clone();
    declared["tools"] = json!([{"type":"web_search", "search_context_size":"low"}]);
    cases.push((declared, false));
    let mut ordinary = base.clone();
    ordinary["tools"] =
        json!([{"type":"function","name":"read_file","parameters":{"type":"object"}}]);
    cases.push((ordinary, false));
    cases.push((base.clone(), true)); // Upstream HTTP 200 followed by SSE error.
    let completed = "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_mock\",\"output\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n";
    let error = "data: {\"type\":\"error\",\"error\":{\"type\":\"internal_error\",\"message\":\"response protection is unavailable\"}}\n\n";
    let responses = (0..cases.len())
        .map(|i| {
            (
                200,
                if i + 1 == cases.len() {
                    error
                } else {
                    completed
                }
                .to_string(),
                "text/event-stream".to_string(),
            )
        })
        .collect();
    let (addr, captured, provider) =
        start_mock_upstream_sequence_lenient_with_content_types(responses, Duration::from_secs(1));
    let _upstream = EnvGuard::set(
        "CODEXMANAGER_UPSTREAM_BASE_URL",
        &format!("http://{addr}/backend-api/codex"),
    );
    let server = TestServer::start();
    for (index, (client, repaired)) in cases.iter().enumerate() {
        let (status, body) = post_http_raw(
            &server.addr,
            "/v1/responses",
            &client.to_string(),
            &[
                ("Content-Type", "application/json"),
                ("Authorization", &format!("Bearer {key}")),
                ("originator", "codex_cli_rs"),
                ("User-Agent", "codex_cli_rs/0.120.0"),
            ],
        );
        assert_eq!(status, 200);
        assert_eq!(
            body,
            if index + 1 == cases.len() {
                error
            } else {
                completed
            }
        );
        let sent = captured.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(sent.path.starts_with("/backend-api/codex/responses"));
        let sent: Value = serde_json::from_slice(&decode_upstream_request_body(&sent)).unwrap();
        assert_eq!(sent["input"], client["input"]);
        assert_eq!(sent["reasoning"], client["reasoning"]);
        assert_eq!(sent["include"], client["include"]);
        if *repaired {
            assert_eq!(sent["tools"], json!([{"type":"web_search"}]));
            assert_eq!(sent["tool_choice"], "none");
        } else {
            // The existing typed HTTP normalizer omits an empty tools array.
            // Preserve its baseline semantics rather than requiring a new default.
            if client["tools"].as_array().is_some_and(Vec::is_empty) {
                assert!(sent.get("tools").is_none() || sent["tools"] == client["tools"]);
            } else {
                assert_eq!(sent["tools"], client["tools"]);
            }
            assert_eq!(sent["tool_choice"], client["tool_choice"]);
        }
    }
    provider.join().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let logs = storage.list_request_logs(None, 100).unwrap();
        if logs.iter().any(|log| {
            log.status_code == Some(502)
                && log
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("response protection is unavailable"))
        }) {
            assert_eq!(
                logs.iter()
                    .filter(|log| log.status_code == Some(200))
                    .count(),
                cases.len() - 1
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "SSE failure must be logged as 502"
        );
        thread::sleep(Duration::from_millis(20));
    }
}
