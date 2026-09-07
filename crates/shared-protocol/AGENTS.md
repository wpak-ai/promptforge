# shared-protocol

This crate owns the OpenAI wire protocol and the upstream abstraction: the wire types and their validation, the `Upstream` trait and `OpenAiUpstream`, the bounded HTTP client helpers, and the protocol-level error types.

- OpenAI wire protocol and upstream abstraction only: no local inference, no routing, no axum handlers. Audio is opaque here: this crate opens a byte stream and names its media type; framing, relaying, and transcoding belong above it.
- The crate never names gateway-local concepts (`LocalError`, profile switching, dominion queues); the `Upstream::shutdown` seam is typed on this crate's own `ShutdownError` so no edge points back into gateway code.
