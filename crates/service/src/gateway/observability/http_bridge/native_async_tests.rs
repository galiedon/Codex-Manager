use super::*;
use axum::body::Body;
use bytes::Bytes;
use futures_util::StreamExt;
use std::time::{Duration, Instant};

const COMPLETED_USAGE: &[u8] = b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_async\",\"status\":\"completed\",\"usage\":{\"input_tokens\":11,\"output_tokens\":7,\"total_tokens\":18},\"output\":[]}}\n\n";

fn request() -> (
    Request,
    tokio::sync::oneshot::Receiver<axum::http::Response<Body>>,
) {
    let (parts, ()) = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .body(())
        .unwrap()
        .into_parts();
    Request::new(parts, Bytes::from_static(b"{}"))
}

fn upstream(body: crate::gateway::upstream::GatewayByteStream) -> GatewayUpstreamResponse {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("content-type", "text/event-stream".parse().unwrap());
    GatewayUpstreamResponse::Stream(crate::gateway::upstream::GatewayStreamResponse::new(
        reqwest::StatusCode::OK,
        headers,
        body,
    ))
}

#[test]
fn native_completed_frame_ends_response_without_waiting_for_provider_eof() {
    crate::gateway::response_test_runtime()
        .unwrap()
        .block_on(async {
            let (request, receiver) = request();
            let (provider, body) = tokio::sync::mpsc::channel(2);
            provider
                .send(crate::gateway::upstream::GatewayByteStreamItem::Chunk(
                    Bytes::from_static(COMPLETED_USAGE),
                ))
                .await
                .unwrap();
            let delivery = tokio::spawn(respond_with_upstream_async(
                request,
                upstream(crate::gateway::upstream::GatewayByteStream::from_receiver(
                    body,
                )),
                crate::gateway::acquire_account_inflight("native-async-usage"),
                crate::gateway::ResponseAdapter::Passthrough,
                None,
                None,
                "/v1/responses",
                None,
                true,
                false,
                None,
                None,
                Instant::now(),
            ));
            let response = receiver.await.unwrap();
            let mut body = response.into_body().into_data_stream();
            assert_eq!(body.next().await.unwrap().unwrap(), COMPLETED_USAGE);
            // Keep the provider open. A complete terminal frame must close the
            // downstream body without a client cancellation or provider EOF.
            assert!(tokio::time::timeout(Duration::from_secs(2), body.next())
                .await
                .expect("terminal closes body")
                .is_none());
            let bridge = tokio::time::timeout(Duration::from_secs(2), delivery)
                .await
                .expect("terminal cancels the still-open provider")
                .unwrap()
                .unwrap();
            assert!(bridge.is_ok(true));
            assert!(bridge.delivery_error.is_none());
            assert!(bridge.stream_terminal_delivered);
            assert_eq!(bridge.usage.input_tokens, Some(11));
            assert_eq!(bridge.usage.output_tokens, Some(7));
            assert_eq!(bridge.usage.total_tokens, Some(18));
            drop(provider);
        });
}

#[test]
fn native_large_split_terminal_is_fully_delivered_before_body_ends() {
    crate::gateway::response_test_runtime().unwrap().block_on(async {
        let frame = format!("data: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"resp_large\",\"output\":[],\"metadata\":{{\"padding\":\"{}\"}}}}}}\n\n", "x".repeat(40_000));
        let (request, receiver) = request();
        let (provider, body) = tokio::sync::mpsc::channel(3);
        // Put the event type far outside the old 256-byte tail and split the
        // final blank line. Neither names nor a partial frame may close the body.
        let split = frame.len() - 1;
        provider.send(crate::gateway::upstream::GatewayByteStreamItem::Chunk(Bytes::copy_from_slice(&frame.as_bytes()[..split]))).await.unwrap();
        provider.send(crate::gateway::upstream::GatewayByteStreamItem::Chunk(Bytes::copy_from_slice(&frame.as_bytes()[split..]))).await.unwrap();
        let delivery = tokio::spawn(respond_with_upstream_async(
            request, upstream(crate::gateway::upstream::GatewayByteStream::from_receiver(body)),
            crate::gateway::acquire_account_inflight("native-large-terminal"),
            crate::gateway::ResponseAdapter::Passthrough, None, None, "/v1/responses",
            None, true, false, None, None, Instant::now(),
        ));
        let response = receiver.await.unwrap();
        let bytes = tokio::time::timeout(Duration::from_secs(2), axum::body::to_bytes(response.into_body(), 100_000)).await.unwrap().unwrap();
        assert_eq!(&bytes[..], frame.as_bytes());
        let bridge = delivery.await.unwrap().unwrap();
        assert!(bridge.is_ok(true));
        assert!(bridge.stream_terminal_delivered);
        drop(provider);
    });
}

#[test]
fn native_disconnect_before_complete_terminal_frame_remains_failed() {
    crate::gateway::response_test_runtime().unwrap().block_on(async {
        let (request, receiver) = request();
        let (provider, body) = tokio::sync::mpsc::channel(2);
        provider.send(crate::gateway::upstream::GatewayByteStreamItem::Chunk(
            Bytes::from_static(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"response.completed\"}\n\n")
        )).await.unwrap();
        let delivery = tokio::spawn(respond_with_upstream_async(
            request, upstream(crate::gateway::upstream::GatewayByteStream::from_receiver(body)),
            crate::gateway::acquire_account_inflight("native-early-cancel"),
            crate::gateway::ResponseAdapter::Passthrough, None, None, "/v1/responses",
            None, true, false, None, None, Instant::now(),
        ));
        let mut downstream = receiver.await.unwrap().into_body().into_data_stream();
        downstream.next().await.unwrap().unwrap();
        drop(downstream);
        let bridge = tokio::time::timeout(Duration::from_secs(2), delivery)
            .await.unwrap().unwrap().unwrap();
        assert!(!bridge.is_ok(true));
        assert!(!bridge.stream_terminal_seen);
        assert!(!bridge.stream_terminal_delivered);
        assert!(bridge.delivery_error.unwrap().contains("broken pipe"));
        drop(provider);
    });
}

#[test]
fn native_http_200_sse_errors_are_forwarded_and_never_successful() {
    for frame in [
        &b"data: {\"type\":\"error\",\"error\":{\"type\":\"internal_error\",\"message\":\"response protection is unavailable\"}}\n\n"[..],
        &b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\",\"message\":\"test failure\"}}}\n\n"[..],
    ] {
        crate::gateway::response_test_runtime().unwrap().block_on(async {
            let (request, receiver) = request();
            let (provider, body) = tokio::sync::mpsc::channel(2);
            provider.send(crate::gateway::upstream::GatewayByteStreamItem::Chunk(Bytes::copy_from_slice(frame)))
                .await.unwrap();
            let delivery = tokio::spawn(respond_with_upstream_async(
                request, upstream(crate::gateway::upstream::GatewayByteStream::from_receiver(body)),
                crate::gateway::acquire_account_inflight("native-sse-error"),
                crate::gateway::ResponseAdapter::Passthrough, None, None, "/v1/responses",
                None, true, false, None, None, Instant::now(),
            ));
            let response = receiver.await.unwrap();
            assert_eq!(response.status(), 200);
            let bytes = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
            assert_eq!(&bytes[..], frame);
            let bridge = tokio::time::timeout(Duration::from_secs(2), delivery)
                .await.unwrap().unwrap().unwrap();
            assert!(!bridge.is_ok(true));
            assert!(bridge.stream_terminal_seen);
            assert!(bridge.stream_terminal_delivered);
            assert!(bridge.stream_terminal_error.is_some());
            assert!(bridge.delivery_error.is_none());
            drop(provider);
        });
    }
}

#[test]
fn deferred_disconnect_drain_keeps_request_gate_until_accounting_completes() {
    crate::gateway::response_test_runtime()
        .unwrap()
        .block_on(async {
            let (mut request, receiver) = request();
            let gate =
                crate::gateway::request_gate_lock("native-async-gate", "/v1/responses", None);
            request.hold_until_complete(gate.try_acquire().unwrap().unwrap());
            let (started, accounting_started) = tokio::sync::oneshot::channel();
            let (finish, accounting_finish) = std::sync::mpsc::channel();
            defer_upstream_response(
                request,
                upstream(crate::gateway::upstream::GatewayByteStream::from_bytes(
                    Bytes::from_static(COMPLETED_USAGE),
                )),
                crate::gateway::acquire_account_inflight("native-async-gate"),
                crate::gateway::ResponseAdapter::Passthrough,
                None,
                None,
                "/v1/responses",
                None,
                true,
                None,
                None,
                Instant::now(),
                move |bridge| {
                    assert_eq!(bridge.usage.output_tokens, Some(7));
                    started.send(()).unwrap();
                    accounting_finish
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap();
                },
            )
            .unwrap();
            let response = receiver.await.unwrap();
            accounting_started.await.unwrap();
            assert!(gate.try_acquire().unwrap().is_none());
            // A closed HTTP body cannot keep Hyper's graceful shutdown alive.
            drop(response);
            let mut draining = Box::pin(drain_deferred_responses());
            assert!(
                tokio::time::timeout(Duration::from_millis(30), &mut draining)
                    .await
                    .is_err()
            );
            finish.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(2), draining)
                .await
                .expect("shutdown drains detached accounting");
            assert!(gate.try_acquire().unwrap().is_some());
        });
}
