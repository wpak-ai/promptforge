//! The voice catalog served by `GET /v1/audio/voices`.

use serde::Serialize;
use std::collections::BTreeSet;

/// One voice in the catalog.
///
/// Served as an object rather than a bare string: the OpenAI-compatible
/// ecosystem converged on `{"id", "name"}` entries, and clients that expect
/// objects silently fall back to a hardcoded voice list when handed strings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VoiceEntry {
    /// The value a request's `voice` field carries.
    pub id: String,
    /// The display name. Equal to `id`, since a catalogued voice is named by
    /// the string callers use.
    pub name: String,
}

/// The `GET /v1/audio/voices` response body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VoicesResponse {
    /// Every voice the active profile's speech models offer.
    pub voices: Vec<VoiceEntry>,
}

/// Build the catalog from every speech model's voices.
///
/// The union is sorted and deduplicated, so two models offering the same
/// voice list it once and the order does not depend on catalog order.
#[must_use]
pub fn voices_response<S: AsRef<str>>(voices: impl IntoIterator<Item = S>) -> VoicesResponse {
    let unique: BTreeSet<String> = voices
        .into_iter()
        .map(|voice| voice.as_ref().to_owned())
        .collect();
    VoicesResponse {
        voices: unique
            .into_iter()
            .map(|voice| VoiceEntry {
                name: voice.clone(),
                id: voice,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_union_is_sorted_and_deduplicated() {
        let first = ["leo".to_owned(), "tara".to_owned()];
        let second = ["tara".to_owned(), "dan".to_owned()];
        let response = voices_response(first.iter().chain(second.iter()));
        let ids: Vec<&str> = response
            .voices
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ids, ["dan", "leo", "tara"]);
    }

    #[test]
    fn no_speech_models_serve_an_empty_catalog() {
        // An empty list, never a missing field: a client probing the route
        // must be able to tell "no voices" from "no such route".
        let response = voices_response(Vec::<String>::new());
        assert!(response.voices.is_empty());
        let json = serde_json::to_value(&response).expect("serializes");
        assert_eq!(json, serde_json::json!({ "voices": [] }));
    }

    #[test]
    fn entries_serialize_as_id_and_name_objects() {
        let response = voices_response(["tara".to_owned()]);
        let json = serde_json::to_value(&response).expect("serializes");
        assert_eq!(
            json,
            serde_json::json!({ "voices": [{ "id": "tara", "name": "tara" }] })
        );
    }
}
