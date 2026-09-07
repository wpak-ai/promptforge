# gateway-tts

The gateway-side speech service for PromptForge: voice validation against a
model's catalogued voices, the `GET /v1/audio/voices` catalog union, and the
audio relay behind `POST /v1/audio/speech`.

The gateway owns the routes, the bearer check, model resolution, and dominion
queue admission; this crate owns what happens to a speech request once the
model is known. Audio is relayed byte for byte: a synthesized clip is opaque,
so nothing here parses, re-encodes, or buffers it whole.
