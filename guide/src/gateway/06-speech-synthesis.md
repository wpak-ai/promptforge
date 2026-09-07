# Speech Synthesis

This chapter teaches you the gateway's synthesis surface: how to declare a speech model, what the speech endpoint accepts, and how audio comes back. It builds on remote models, because a speech model is an ordinary remote catalog entry with a different kind.

## Declare a speech model

A speech model is a `[[model]]` entry with `kind = "speech"`:

````
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
````

Everything a remote chat model carries applies here too: the entry resolves through the same routing table, admits through the same dominion queue, appears on GET /v1/models, and is enabled by profile membership. The chat-only fields are refused, the same way they are for embedding and classifier models.

The `voices` list names the voices this model offers. A request naming a voice outside the list is refused before the backend is called, with a message listing the valid ones. Leave the list empty to disable the check and let the backend decide; an empty entry or a duplicate in the list fails startup, because both hide a typo behind a valid-looking catalog. Voice sets belong to a checkpoint rather than to speech in general, so each model carries its own.

Local speech serving is not part of this release. A `[[local_model]]` declaring `kind = "speech"` fails startup with a message naming the remote alternative, rather than starting a child that would serve the wrong workload.

## The speech endpoint

The gateway serves OpenAI-compatible synthesis at POST /v1/audio/speech. The JSON body requires `model`, `input`, and `voice`, and accepts `response_format`, `speed`, `instructions`, and `stream_format`. Fields the gateway does not name pass through to the backend verbatim.

The `input` text is capped at 4096 characters and reaches the backend exactly as written. Speech models take direction from tags written inline in the text, such as `<laugh>` and `<sigh>`, so nothing on this path escapes or strips them.

The `response_format` chooses the container: `mp3` by default, plus `opus`, `aac`, `flac`, `wav`, and `pcm`. A caller who names none gets `mp3` written into the forwarded request rather than the backend's own default, because providers disagree about what that default is. An unrecognized format is refused rather than quietly replaced. The `speed` must fall between 0.25 and 4.0.

Voice, format, and text are all checked before the request is admitted to a queue, so a malformed request never occupies a concurrency slot.

## How audio comes back

The response body is the backend's audio, byte for byte, under the backend's own content type. Nothing re-encodes or re-frames it. There is no `Content-Length`, because a clip's length is unknown when the headers go out; the body is chunked and ends by ending.

That framing shapes how failures appear. The status and headers are sent before the first audio byte, so a synthesis that dies mid-clip cannot be reported as an error envelope: an error appended to half a clip would be indistinguishable from audio. The body ends early instead, which every HTTP client reports as a truncated download. A failure that happens before the first byte is still an ordinary JSON error.

The queue slot is held for the whole clip, not just the request that started it, so a profile switch drains a synthesis in progress. Hanging up mid-clip aborts the upstream synthesis and releases the slot immediately; nothing keeps generating audio no one is listening to.

Two upstream conditions are distinguishable by their error codes: `upstream_rate_limited` for a rate-limited provider and `upstream_unavailable` for one at capacity. A voice the model does not offer is `invalid_voice`.

## The voice catalog

GET /v1/audio/voices lists the union of every loaded speech model's voices, sorted and deduplicated, as objects with `id` and `name`. OpenAI has no such endpoint, but the OpenAI-compatible ecosystem converged on this route and clients probe for it, so the gateway serves it. The same voices also appear on each model's own GET /v1/models entry.
