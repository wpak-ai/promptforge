<!-- audience: implementor or operator verifying the speech endpoint against a live provider -->
# Verifying the speech endpoint against a live provider

Phase 1 of `report-gateway-tts-endpoint.md` shipped without a live provider check: the offline suite covers the gateway's own behavior, but provider dialects drift and only a real call records what a provider actually does today. This note is the recipe for that check, plus what to write down.

## Set up a throwaway profile

Add to a scratch `gateway.toml` (never a working config, since this profile exists to be deleted):

```toml
[[endpoint]]
id = "together"
protocol = "openai"
base_url = "https://api.together.xyz/v1"
api_key = "${TOGETHER_API_KEY}"

[[model]]
name = "orpheus"
kind = "speech"
description = "Orpheus 3B conversational speech synthesis"
context = 8192
upstream = "canopylabs/orpheus-3b-0.1-ft"
endpoints = ["together"]
voices = ["tara", "leah", "jess", "leo", "dan", "mia", "zac", "zoe"]

[[profile]]
name = "speech-check"
models = ["orpheus"]
```

Export `TOGETHER_API_KEY` and start the gateway on that config with `--profile speech-check`. For OpenAI instead, point `base_url` at `https://api.openai.com/v1`, set `upstream` to `tts-1` or `gpt-4o-mini-tts`, and use that provider's voice names.

## Make the calls

```bash
curl -sS -D headers.txt http://127.0.0.1:8081/v1/audio/speech -H "Authorization: Bearer $GATEWAY_KEY" -H 'Content-Type: application/json' -d '{"model":"orpheus","input":"Hello <chuckle> there.","voice":"tara"}' -o out-default.bin
```

Then repeat with `"response_format":"wav"` into `out.wav`, and check the catalog:

```bash
curl -sS http://127.0.0.1:8081/v1/audio/voices -H "Authorization: Bearer $GATEWAY_KEY"
```

## Record what you observe

The point of the exercise is the record, not the audio. Note each of these against the date:

- The `Content-Type` the provider returned for the default request, and whether the bytes are actually MPEG audio. The gateway pins `response_format` to `mp3` when the caller omits it, so a provider whose own default is `wav` should still return MPEG here. A mismatch means the pin is not reaching the provider.
- Whether the response was chunked or carried a `Content-Length`, and whether audio arrived progressively or in one burst. Together streams only with `stream=true` and `response_format=raw`, so a plain call is expected to be non-streaming; OpenAI streams chunked binary.
- Whether the inline `<chuckle>` tag was spoken as direction or read aloud as text. Read aloud means something on the path is treating the tag as literal text.
- The envelopes behind a rate limit and a capacity failure, if you can provoke them: the gateway should render `upstream_rate_limited` (429) and `upstream_unavailable` (503).
- Any field the provider rejected or ignored, especially `instructions` and `stream_format`.

Delete the throwaway profile and the key when finished.
