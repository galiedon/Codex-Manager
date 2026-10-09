use super::super::{
    original_digests, persist, redacted_spill_body, OutboundAttemptCapture, RequestLogPayloadJob,
};
use super::*;
use codexmanager_core::storage::{Storage, PAYLOAD_STAGE_CLIENT, PAYLOAD_STAGE_UPSTREAM};

const PRIVATE: &[u8] = br#"{"model":"private-model-text","instructions":"private-instructions","Authorization":"private-auth","api_key":"private-key","input":[{"role":"user","content":"private-conversation"},{"type":"reasoning","encrypted_content":"private-cipher"},{"type":"web_search_call","status":"completed","id":"private-id","action":{"query":"private-query"}}],"tools":[{"type":"function","description":"private-tool-description"},{"type":"web_search","filters":{"allowed_domains":["private-domain"]}}],"tool_choice":"none","stream":true}"#;

fn job(stage: &str, redact: bool, preview: bool) -> RequestLogPayloadJob {
    RequestLogPayloadJob {
        trace_id: format!("privacy_{stage}_{redact}_{preview}"),
        stage: stage.to_string(),
        body: Bytes::from_static(PRIVATE),
        conversation_key: None,
        redact,
        preview,
        created_at: codexmanager_core::storage::now_ts(),
        generation: 0,
        clear_epoch: 0,
        boot_id: 0,
        original: None,
        attempt: Some(OutboundAttemptCapture {
            method: "POST".into(),
            url: "https://upstream.invalid/v1/responses".into(),
            transport: "http".into(),
            content_encoding: Some("zstd".into()),
            wire_body: Bytes::from_static(b"compressed-private-cipher-and-private-key"),
        }),
    }
}

#[test]
fn request_structure_excludes_all_arbitrary_strings_and_preserves_compatibility_facts() {
    let body = structure_only_body(PRIVATE);
    let text = std::str::from_utf8(&body).unwrap();
    assert!(!text.contains("private-"));
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["capture_policy"], "structure_only_v1");
    assert_eq!(value["input_items"], 3);
    assert_eq!(value["completed_web_search_calls"], 1);
    assert_eq!(value["tools_count"], 2);
    assert_eq!(value["web_search_declarations"], 1);
    assert_eq!(value["tool_choice"], "none");
    assert_eq!(value["has_encrypted_content"], true);
    assert_eq!(structure_only_body(&body), body);
}

#[test]
fn request_structure_never_retains_invalid_binary_or_forged_summary_content() {
    for body in [
        &b"private-plain-text"[..],
        &b"\xff\xfeprivate-binary-cipher"[..],
        &br#""private-json-string""#[..],
        &br#"[{"encrypted_content":"private-array-cipher"}]"#[..],
        &br#"{"capture_policy":"structure_only_v1","body_format":"private-forgery","input_items":null}"#[..],
    ] {
        let summary = structure_only_body(body);
        assert!(!std::str::from_utf8(&summary).unwrap().contains("private-"));
        assert_eq!(structure_only_body(&summary), summary);
    }
}

#[test]
fn request_structure_storage_is_private_in_both_modes_with_redaction_on_or_off() {
    let storage = Storage::open_in_memory().unwrap();
    storage.init().unwrap();
    for stage in [PAYLOAD_STAGE_CLIENT, PAYLOAD_STAGE_UPSTREAM] {
        for redact in [false, true] {
            for preview in [false, true] {
                let job = job(stage, redact, preview);
                let trace_id = job.trace_id.clone();
                persist::persist_request_log_payload(&storage, job);
                let stored = if preview {
                    let stored = storage
                        .find_request_log_payload_by_trace_id(&trace_id, stage)
                        .unwrap()
                        .unwrap();
                    assert!(stored.redacted);
                    stored.payload
                } else {
                    let stored = storage
                        .load_request_log_payload_full(&trace_id, stage)
                        .unwrap()
                        .unwrap();
                    assert!(stored.manifest.redacted);
                    format!("{:?}{:?}", stored.fields, stored.items)
                };
                assert!(!stored.contains("private-"), "{stored}");
                assert!(stored.contains("structure_only_v1"));
            }
        }
    }
}

#[test]
fn request_structure_spill_replay_keeps_original_hashes_without_any_body_or_wire_content() {
    for redact in [false, true] {
        for preview in [false, true] {
            let original = job(PAYLOAD_STAGE_UPSTREAM, redact, preview);
            let mut replayed = original.clone();
            replayed.body = redacted_spill_body(&original);
            replayed.original = Some(original_digests(&original));
            replayed.redact = true;
            replayed.attempt.as_mut().unwrap().wire_body = replayed.body.clone();
            let original = persist::prepare_request_log_payload(&original);
            let replayed = persist::prepare_request_log_payload(&replayed);
            let describe = |prepared: &persist::PreparedPayload| match &prepared.body {
                persist::PreparedBody::Preview(body) => format!("{body:?}"),
                persist::PreparedBody::Manifest(body) => format!("{body:?}"),
            };
            assert_eq!(describe(&original), describe(&replayed));
            assert!(!describe(&replayed).contains("private-"));
            assert_eq!(
                original.attempt.unwrap().wire_sha256,
                replayed.attempt.unwrap().wire_sha256
            );
        }
    }
}
