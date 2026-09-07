//! The audio relay: a backend's byte stream, answered to the caller.

use std::future::Future;

use axum::body::Body;
use axum::http::HeaderValue;
use axum::http::header::CONTENT_TYPE;
use axum::response::Response;
use bytes::Bytes;
use futures_util::StreamExt as _;
use shared_protocol::upstream::StreamedAudio;

/// The media type used when a backend names one this crate cannot forward.
const FALLBACK_CONTENT_TYPE: &str = "application/octet-stream";

/// Whatever the caller keeps alive for a relayed stream's lifetime.
///
/// The gateway's implementation carries the dominion permit and the
/// in-flight guard, so both are released exactly when the response body
/// ends - whether it ended cleanly, failed, or was dropped by a client
/// hanging up.
pub trait StreamHold: Send + 'static {
    /// Resolves when the held work should stop, ending the stream early.
    fn cancelled(&self) -> impl Future<Output = ()> + Send;
}

/// Relay a backend's audio to the caller, holding `hold` for the stream's
/// lifetime.
///
/// This is byte passthrough, and deliberately unlike the chat relay, which
/// validates and re-serializes every chunk it forwards. Audio frames are
/// opaque: there is no per-item shape to check, and re-encoding would change
/// the very bytes the caller asked for. So the only decisions here are which
/// media type to name and when to stop.
///
/// A failure mid-stream cannot be reported in the body. The response status
/// and headers are already sent, and an OpenAI-shaped JSON error appended to
/// half a clip would be indistinguishable from audio. The stream therefore
/// ends by yielding an error, which aborts the chunked body: the caller sees
/// a truncated download, which every HTTP client already treats as a failure,
/// rather than a short clip that looks complete.
///
/// `Content-Length` is never set, so hyper frames the body as chunked and a
/// caller learns the length by reading to the end.
///
/// Client-disconnect cancellation is Drop all the way down: when the caller
/// goes away the response body is dropped, which drops this stream, which
/// drops both the upstream response (aborting the backend request) and
/// `hold`. There is no explicit release path.
#[must_use]
pub fn relay_audio<H: StreamHold>(audio: StreamedAudio, hold: H) -> Response {
    let StreamedAudio { content_type, body } = audio;
    let relayed =
        futures_util::stream::unfold((body, hold, false), |(mut body, hold, done)| async move {
            if done {
                return None;
            }
            let item: Result<Bytes, Box<dyn std::error::Error + Send + Sync>> = tokio::select! {
                chunk = body.next() => match chunk? {
                    Ok(bytes) => return Some((Ok(bytes), (body, hold, false))),
                    Err(error) => {
                        tracing::warn!(%error, "speech upstream failed mid-stream; truncating");
                        Err(Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
                    }
                },
                () = hold.cancelled() => {
                    tracing::warn!("speech stream cancelled; truncating");
                    Err(Box::new(std::io::Error::other(
                        "speech stream cancelled for profile switch",
                    )) as Box<dyn std::error::Error + Send + Sync>)
                }
            };
            Some((item, (body, hold, true)))
        });
    let mut response = Response::new(Body::from_stream(relayed));
    let content_type = HeaderValue::from_str(&content_type)
        .unwrap_or_else(|_| HeaderValue::from_static(FALLBACK_CONTENT_TYPE));
    response.headers_mut().insert(CONTENT_TYPE, content_type);
    response
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use futures_util::stream;
    use shared_protocol::ProtocolError;
    use tokio_util::sync::CancellationToken;

    use super::*;

    /// A hold that never cancels, for the ordinary streaming path.
    struct NeverCancel;
    impl StreamHold for NeverCancel {
        fn cancelled(&self) -> impl Future<Output = ()> + Send {
            std::future::pending()
        }
    }

    /// A hold the test can cancel, standing in for a profile switch.
    struct TokenHold(CancellationToken);
    impl StreamHold for TokenHold {
        fn cancelled(&self) -> impl Future<Output = ()> + Send {
            let token = self.0.clone();
            async move { token.cancelled().await }
        }
    }

    /// A hold that records its own drop, standing in for the permit whose
    /// release is the whole point of holding anything.
    struct DropProbe(Arc<AtomicBool>);
    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    impl StreamHold for DropProbe {
        fn cancelled(&self) -> impl Future<Output = ()> + Send {
            std::future::pending()
        }
    }

    fn audio(content_type: &str, items: Vec<Result<Bytes, ProtocolError>>) -> StreamedAudio {
        StreamedAudio {
            content_type: content_type.to_owned(),
            body: stream::iter(items).boxed(),
        }
    }

    async fn body_bytes(response: Response) -> Result<Bytes, axum::Error> {
        axum::body::to_bytes(response.into_body(), usize::MAX).await
    }

    #[tokio::test]
    async fn audio_and_its_media_type_pass_through_unchanged() {
        let response = relay_audio(
            audio(
                "audio/wav",
                vec![
                    Ok(Bytes::from_static(b"RIFF")),
                    Ok(Bytes::from_static(b"data")),
                ],
            ),
            NeverCancel,
        );
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("audio/wav")
        );
        // Never Content-Length: the clip's size is not known when the
        // headers go out, so the body is chunked and ends by ending.
        assert!(response.headers().get("content-length").is_none());
        let bytes = body_bytes(response).await.expect("body reads");
        assert_eq!(&bytes[..], b"RIFFdata");
    }

    #[tokio::test]
    async fn an_unforwardable_media_type_falls_back_to_octet_stream() {
        // A backend naming a header value that cannot be forwarded must not
        // take the response down with it.
        let response = relay_audio(
            audio("audio/\nwav", vec![Ok(Bytes::from_static(b"RIFF"))]),
            NeverCancel,
        );
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/octet-stream")
        );
    }

    #[tokio::test]
    async fn a_mid_stream_failure_truncates_rather_than_completing() {
        // The caller must not be able to mistake a failed synthesis for a
        // short one, so the body dies instead of ending cleanly.
        let response = relay_audio(
            audio(
                "audio/wav",
                vec![
                    Ok(Bytes::from_static(b"RIFF")),
                    Err(ProtocolError::transport(std::io::Error::other("reset"))),
                ],
            ),
            NeverCancel,
        );
        assert!(
            body_bytes(response).await.is_err(),
            "a failed stream must not read back as a complete clip"
        );
    }

    #[tokio::test]
    async fn cancellation_ends_the_stream_mid_flight() {
        let token = CancellationToken::new();
        let never_ends = StreamedAudio {
            content_type: "audio/wav".to_owned(),
            body: stream::once(async { Ok(Bytes::from_static(b"RIFF")) })
                .chain(stream::pending())
                .boxed(),
        };
        let response = relay_audio(never_ends, TokenHold(token.clone()));
        token.cancel();
        let read = tokio::time::timeout(std::time::Duration::from_secs(5), body_bytes(response))
            .await
            .expect("cancellation must end the body rather than hang");
        assert!(read.is_err(), "a cancelled stream truncates");
    }

    #[tokio::test]
    async fn the_hold_survives_until_the_body_is_read_out() {
        let released = Arc::new(AtomicBool::new(false));
        let response = relay_audio(
            audio("audio/wav", vec![Ok(Bytes::from_static(b"RIFF"))]),
            DropProbe(Arc::clone(&released)),
        );
        assert!(
            !released.load(Ordering::Acquire),
            "the hold must outlive the response's construction"
        );
        let _ = body_bytes(response).await.expect("body reads");
        assert!(
            released.load(Ordering::Acquire),
            "reading the body to its end releases the hold"
        );
    }

    #[tokio::test]
    async fn dropping_the_response_releases_the_hold() {
        // A client hanging up mid-clip must release the permit: Drop is the
        // entire mechanism, so an unread body still frees what it held.
        let released = Arc::new(AtomicBool::new(false));
        let never_ends = StreamedAudio {
            content_type: "audio/wav".to_owned(),
            body: stream::once(async { Ok(Bytes::from_static(b"RIFF")) })
                .chain(stream::pending())
                .boxed(),
        };
        let response = relay_audio(never_ends, DropProbe(Arc::clone(&released)));
        drop(response);
        assert!(
            released.load(Ordering::Acquire),
            "dropping the response must release the hold"
        );
    }
}
