# gateway-tts

This crate owns the gateway-side speech service: voice validation against a
model's catalogued voices, the voice-catalog union, and the audio relay that
streams a backend's bytes to the caller.

- Speech service only: no HTTP routing, no bearer-auth policy, no model
  resolution, no profile switching. The gateway mounts the routes, checks the
  credential, resolves the model, and admits through the dominion queue.
- The crate never names gateway concepts (`GatewayError`, `AppState`,
  `check_auth`); failures return its own `TtsError`.
- The relay holds whatever the caller hands it for the stream's lifetime and
  drops it when the body ends. Drop is the entire cancellation mechanism:
  there is no explicit release path, so nothing may outlive the response.
- Audio bytes are never inspected, re-encoded, or buffered whole. Synthesis
  text is never rewritten anywhere on this path: speech models take inline
  direction from bracketed tags, which any escaping would destroy.
