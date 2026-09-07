# shared-protocol

The OpenAI wire protocol and upstream abstraction for the PromptForge
inference gateway: request/response wire types with trust-boundary
validation, the `Upstream` trait and its `OpenAiUpstream` passthrough,
bounded HTTP client helpers, and the protocol-level error types.

This crate is the shared protocol contract between the gateway, its local
inference subsystem, and external clients. It contains no local inference,
no routing, and no HTTP server handlers.

Speech synthesis opens a byte stream rather than a typed body: `StreamedAudio`
carries the media type and the backend's bytes, and everything above decides
how to frame them.
