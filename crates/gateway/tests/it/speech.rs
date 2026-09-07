//! Speech route: byte passthrough, the voice check, the kind guard, the
//! voice catalog, and permit lifetime across a streamed clip.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, Method, header::AUTHORIZATION, header::CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use futures_util::StreamExt as _;
use gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::support::{
    PHASE_TIMEOUT, RecordedRequest, Recorder, ReleaseTx, TestServer, json_within, next_arrival,
    send_within, spawn_backend,
};

/// A 44-byte RIFF header plus a little PCM: a clip whose bytes are
/// recognizable on the way out.
const WAV_BYTES: &[u8] = b"RIFF$\x00\x00\x00WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00\
\x80\x3e\x00\x00\x00\x7d\x00\x00\x02\x00\x10\x00data\x08\x00\x00\x00\x01\x02\x03\x04\x05\x06\x07\x08";

/// A fake speech backend that records each request, then answers with the
/// canned clip.
async fn recording_speech_backend() -> (SocketAddr, Recorder) {
    async fn speech(
        State(recorder): State<Recorder>,
        method: Method,
        uri: axum::http::Uri,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Response {
        let authorization = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        recorder.lock().unwrap().push(RecordedRequest {
            method: method.to_string(),
            path: uri.path().to_string(),
            authorization,
            body,
        });
        ([(CONTENT_TYPE, "audio/wav")], WAV_BYTES).into_response()
    }

    let recorder: Recorder = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route("/audio/speech", post(speech))
        .with_state(Arc::clone(&recorder));
    (spawn_backend(router).await, recorder)
}

/// A fake speech backend that answers with a status and body of the test's
/// choosing, for the upstream-failure mappings.
async fn failing_speech_backend(status: u16) -> SocketAddr {
    let router = Router::new().route(
        "/audio/speech",
        post(move || async move {
            (
                axum::http::StatusCode::from_u16(status).unwrap(),
                "upstream said no",
            )
        }),
    );
    spawn_backend(router).await
}

/// Start a gateway serving speech models. With `max_concurrency`, the
/// endpoint is bound to a dominion pool capped at that many in-flight
/// requests; without it the endpoint is an unlimited pass-through.
async fn speech_gateway(backend: SocketAddr, max_concurrency: Option<usize>) -> TestServer {
    let (dominion_block, binding) = match max_concurrency {
        Some(limit) => (
            format!(
                r#"
[[dominion]]
id = "pool"
kind = "remote"
max_concurrency = {limit}
max_queue = 10
"#
            ),
            "\ndominion = \"pool\"",
        ),
        None => (String::new(), ""),
    };
    let toml = format!(
        r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"
{dominion_block}
[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""{binding}

[[model]]
name = "orpheus"
kind = "speech"
description = "a speech model for integration"
context = 8192
upstream = "backend-orpheus"
endpoints = ["fake"]
voices = ["tara", "leo"]

[[model]]
name = "open-voice"
kind = "speech"
description = "a speech model naming no voices"
context = 8192
upstream = "backend-open"
endpoints = ["fake"]

[[model]]
name = "chat-model"
description = "a chat model for the kind guard"
context = 8192
thinking = "never"
upstream = "backend-chat"
endpoints = ["fake"]
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    TestServer::start(gateway).await
}

fn speech_body(model: &str, voice: &str) -> Value {
    serde_json::json!({
        "model": model,
        "input": "Hello <laugh> there",
        "voice": voice,
    })
}

/// IT-005/006 for the speech route: the backend records the request, so we
/// assert exactly what the gateway forwarded - the rewritten upstream model,
/// the verbatim text with its inline direction tags, and the pinned response
/// format - and that the client's bearer is not leaked. The clip comes back
/// byte for byte under the backend's own media type.
#[tokio::test]
async fn remote_passthrough_relays_audio_and_rewrites_the_model() {
    let (backend, recorder) = recording_speech_backend().await;
    let gateway = speech_gateway(backend, None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&speech_body("orpheus", "tara")),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("audio/wav"),
        "the backend's media type reaches the caller"
    );
    assert!(
        response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .is_none(),
        "a synthesized clip has no known length, so the body is chunked"
    );
    let audio = tokio::time::timeout(PHASE_TIMEOUT, response.bytes())
        .await
        .expect("audio body exceeded the phase timeout")
        .expect("audio body read failed");
    assert_eq!(&audio[..], WAV_BYTES, "audio passes through byte for byte");

    // Cloned out of the lock so no guard is alive across the shutdown await.
    let calls = recorder.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "exactly one upstream call");
    let request = &calls[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/audio/speech");
    assert_eq!(
        request.body.get("model").and_then(Value::as_str),
        Some("backend-orpheus"),
        "the backend sees its own model name"
    );
    assert_eq!(
        request.body.get("input").and_then(Value::as_str),
        Some("Hello <laugh> there"),
        "inline direction tags reach the backend unescaped"
    );
    assert_eq!(
        request.body.get("voice").and_then(Value::as_str),
        Some("tara")
    );
    assert_eq!(
        request.body.get("response_format").and_then(Value::as_str),
        Some("mp3"),
        "the caller named no format, so the gateway pins its own default"
    );
    assert_ne!(
        request.authorization.as_deref(),
        Some("Bearer test-token"),
        "caller bearer must not leak to the upstream"
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn an_uncatalogued_voice_is_refused_without_calling_the_backend() {
    let (backend, recorder) = recording_speech_backend().await;
    let gateway = speech_gateway(backend, None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&speech_body("orpheus", "nova")),
    )
    .await;
    assert_eq!(response.status().as_u16(), 400);
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("invalid_voice")
    );
    let message = body
        .pointer("/error/message")
        .and_then(Value::as_str)
        .expect("a message");
    assert!(
        message.contains("tara, leo"),
        "the refusal names the valid voices: {message}"
    );
    assert!(
        recorder.lock().unwrap().is_empty(),
        "a refused voice never reaches the backend"
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn a_model_naming_no_voices_accepts_any_voice() {
    // An empty `voices` list means the backend chooses, so the gateway must
    // forward rather than refuse.
    let (backend, recorder) = recording_speech_backend().await;
    let gateway = speech_gateway(backend, None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&speech_body("open-voice", "whatever")),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        recorder.lock().unwrap()[0]
            .body
            .get("voice")
            .and_then(Value::as_str),
        Some("whatever")
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn the_kind_guard_rejects_both_directions() {
    let (backend, recorder) = recording_speech_backend().await;
    let gateway = speech_gateway(backend, None).await;
    let client = reqwest::Client::new();

    // A chat model on the speech route.
    let response = send_within(
        client
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&speech_body("chat-model", "tara")),
    )
    .await;
    assert_eq!(response.status().as_u16(), 400);
    assert_eq!(
        json_within(response)
            .await
            .pointer("/error/code")
            .and_then(Value::as_str),
        Some("kind_mismatch")
    );

    // A speech model on the chat, embeddings, and rerank routes.
    for (route, body) in [
        (
            "/v1/chat/completions",
            serde_json::json!({ "model": "orpheus", "messages": [{ "role": "user", "content": "hi" }] }),
        ),
        (
            "/v1/embeddings",
            serde_json::json!({ "model": "orpheus", "input": "hi" }),
        ),
        (
            "/v1/rerank",
            serde_json::json!({ "model": "orpheus", "query": "hi", "documents": ["a"] }),
        ),
    ] {
        let response = send_within(
            client
                .post(format!("http://{}{route}", gateway.addr))
                .bearer_auth("test-token")
                .json(&body),
        )
        .await;
        assert_eq!(response.status().as_u16(), 400, "{route}");
        assert_eq!(
            json_within(response)
                .await
                .pointer("/error/code")
                .and_then(Value::as_str),
            Some("kind_mismatch"),
            "{route}"
        );
    }

    assert!(
        recorder.lock().unwrap().is_empty(),
        "the kind guard runs before any upstream call"
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn the_catalog_lists_speech_models_and_their_voices() {
    let (backend, _recorder) = recording_speech_backend().await;
    let gateway = speech_gateway(backend, None).await;
    let client = reqwest::Client::new();

    let catalog = json_within(
        send_within(
            client
                .get(format!("http://{}/v1/models", gateway.addr))
                .bearer_auth("test-token"),
        )
        .await,
    )
    .await;
    let orpheus = catalog["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "orpheus")
        .expect("the speech model is catalogued");
    assert_eq!(orpheus["kind"], "speech");
    assert_eq!(orpheus["voices"], serde_json::json!(["tara", "leo"]));

    // The voices route unions every speech model, sorted and deduplicated.
    let voices = json_within(
        send_within(
            client
                .get(format!("http://{}/v1/audio/voices", gateway.addr))
                .bearer_auth("test-token"),
        )
        .await,
    )
    .await;
    assert_eq!(
        voices,
        serde_json::json!({
            "voices": [
                { "id": "leo", "name": "leo" },
                { "id": "tara", "name": "tara" }
            ]
        })
    );
    gateway.shutdown().await;
}

/// A speech backend that hands the test a release handle on arrival, then
/// streams the clip in two parts: the second waits for the release, so the
/// response body stays open while the test checks the queue.
async fn gated_speech_backend() -> (SocketAddr, UnboundedReceiver<ReleaseTx>) {
    async fn speech(State(arrivals): State<UnboundedSender<ReleaseTx>>) -> Response {
        let (release, released) = oneshot::channel();
        let _ = arrivals.send(release);
        let head = futures_util::stream::once(async {
            Ok::<_, std::convert::Infallible>(axum::body::Bytes::from_static(&WAV_BYTES[..44]))
        });
        let tail = futures_util::stream::once(async move {
            let _ = released.await;
            Ok(axum::body::Bytes::from_static(&WAV_BYTES[44..]))
        });
        (
            [(CONTENT_TYPE, "audio/wav")],
            axum::body::Body::from_stream(head.chain(tail)),
        )
            .into_response()
    }

    let (arrivals, receiver) = mpsc::unbounded_channel::<ReleaseTx>();
    let router = Router::new()
        .route("/audio/speech", post(speech))
        .with_state(arrivals);
    (spawn_backend(router).await, receiver)
}

/// Under concurrency=1, a speech request holds the dominion permit for the
/// clip's whole lifetime: a second request is not admitted until the first
/// body has ended.
#[tokio::test]
async fn the_permit_is_held_until_the_clip_ends() {
    let (backend, mut arrivals) = gated_speech_backend().await;
    let gateway = speech_gateway(backend, Some(1)).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let spawn_speech = |client: reqwest::Client, url: String| {
        tokio::spawn(async move {
            client
                .post(url)
                .bearer_auth("test-token")
                .json(&speech_body("orpheus", "tara"))
                .send()
                .await
        })
    };

    let first = spawn_speech(client.clone(), url.clone());
    let release_first = next_arrival(&mut arrivals).await;

    let second = spawn_speech(client.clone(), url.clone());
    assert!(
        matches!(arrivals.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "the second request must not reach the backend while the clip streams"
    );

    let first_response = tokio::time::timeout(PHASE_TIMEOUT, first)
        .await
        .expect("first response exceeded the phase timeout")
        .expect("first task joins")
        .expect("first request sends");
    assert_eq!(first_response.status().as_u16(), 200);
    release_first.send(()).unwrap();
    let audio = tokio::time::timeout(PHASE_TIMEOUT, first_response.bytes())
        .await
        .expect("audio body exceeded the phase timeout")
        .expect("audio body read failed");
    assert_eq!(&audio[..], WAV_BYTES, "the whole clip arrives");

    // With the first body drained, the permit is free and the second is
    // admitted.
    let release_second = next_arrival(&mut arrivals).await;
    release_second.send(()).unwrap();
    let second_response = tokio::time::timeout(PHASE_TIMEOUT, second)
        .await
        .expect("second response exceeded the phase timeout")
        .expect("second task joins")
        .expect("second request sends");
    assert_eq!(second_response.status().as_u16(), 200);
    gateway.shutdown().await;
}

/// A client disconnect mid-clip cancels the upstream synthesis: dropping the
/// response body drops the relay, which drops the gateway's upstream
/// connection, which the backend observes as its own body being dropped.
/// Drop is the entire mechanism - there is no explicit cancel path.
#[tokio::test]
async fn client_disconnect_aborts_the_upstream_synthesis() {
    /// Signals once the backend's response body is dropped mid-clip.
    struct NotifyOnDrop(UnboundedSender<()>);
    impl Drop for NotifyOnDrop {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    let (dropped, mut observed) = mpsc::unbounded_channel::<()>();
    let backend = spawn_backend(Router::new().route(
        "/audio/speech",
        post(move || {
            let dropped = dropped.clone();
            async move {
                let head = futures_util::stream::once(async {
                    Ok::<_, std::convert::Infallible>(axum::body::Bytes::from_static(
                        &WAV_BYTES[..44],
                    ))
                });
                let rest = futures_util::stream::once(async move {
                    let _notify = NotifyOnDrop(dropped);
                    futures_util::future::pending::<()>().await;
                    unreachable!("the clip never finishes on its own")
                });
                (
                    [(CONTENT_TYPE, "audio/wav")],
                    axum::body::Body::from_stream(head.chain(rest)),
                )
                    .into_response()
            }
        }),
    ))
    .await;
    let gateway = speech_gateway(backend, None).await;

    let mut response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&speech_body("orpheus", "tara")),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    // Read the first chunk so the clip is genuinely mid-flight, then hang up.
    let first = tokio::time::timeout(PHASE_TIMEOUT, response.chunk())
        .await
        .expect("first chunk read exceeded the phase timeout")
        .expect("first chunk read failed");
    assert!(first.is_some(), "first chunk arrived");
    drop(response);

    tokio::time::timeout(PHASE_TIMEOUT, observed.recv())
        .await
        .expect("backend did not observe the disconnect within the phase timeout")
        .expect("disconnect notification channel closed");
    gateway.shutdown().await;
}

/// A synthesis that dies after the clip has started streaming must not read
/// back as a complete one. Once the status and headers are sent, the failure
/// can only be reported by the body dying, never by a JSON envelope spliced
/// into audio the caller is already playing.
///
/// The failure is held until the test has read the first chunk, so the clip
/// is genuinely mid-flight: a backend that dies instantly is instead caught
/// before relaying starts and answered as a normal JSON error, which the
/// upstream-status tests cover.
#[tokio::test]
async fn a_mid_clip_upstream_failure_truncates_the_body() {
    let (arrivals, mut receiver) = mpsc::unbounded_channel::<ReleaseTx>();
    let backend = spawn_backend(
        Router::new()
            .route(
                "/audio/speech",
                post(
                    |State(arrivals): State<UnboundedSender<ReleaseTx>>| async move {
                        let (release, released) = oneshot::channel();
                        let _ = arrivals.send(release);
                        let head = futures_util::stream::once(async {
                            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(
                                &WAV_BYTES[..44],
                            ))
                        });
                        let rest = futures_util::stream::once(async move {
                            let _ = released.await;
                            Err::<axum::body::Bytes, _>(std::io::Error::other("synthesis died"))
                        });
                        (
                            [(CONTENT_TYPE, "audio/wav")],
                            axum::body::Body::from_stream(head.chain(rest)),
                        )
                            .into_response()
                    },
                ),
            )
            .with_state(arrivals),
    )
    .await;
    let gateway = speech_gateway(backend, None).await;

    let mut response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&speech_body("orpheus", "tara")),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let first = tokio::time::timeout(PHASE_TIMEOUT, response.chunk())
        .await
        .expect("first chunk read exceeded the phase timeout")
        .expect("first chunk read failed");
    assert!(first.is_some(), "the clip started arriving");

    // Now kill the synthesis mid-clip.
    next_arrival(&mut receiver).await.send(()).unwrap();
    let rest = tokio::time::timeout(PHASE_TIMEOUT, response.bytes())
        .await
        .expect("body read exceeded the phase timeout");
    assert!(
        rest.is_err(),
        "a failed synthesis must not read back as a complete clip"
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn upstream_rate_limits_and_capacity_carry_distinct_codes() {
    for (status, expected_status, expected_code) in [
        (429, 429, "upstream_rate_limited"),
        (503, 503, "upstream_unavailable"),
    ] {
        let backend = failing_speech_backend(status).await;
        let gateway = speech_gateway(backend, None).await;
        let response = send_within(
            reqwest::Client::new()
                .post(format!("http://{}/v1/audio/speech", gateway.addr))
                .bearer_auth("test-token")
                .json(&speech_body("orpheus", "tara")),
        )
        .await;
        assert_eq!(response.status().as_u16(), expected_status, "{status}");
        assert_eq!(
            json_within(response)
                .await
                .pointer("/error/code")
                .and_then(Value::as_str),
            Some(expected_code),
            "{status}"
        );
        gateway.shutdown().await;
    }
}
