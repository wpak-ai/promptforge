//! The speech service's failure type and the voice check that produces it.

/// A speech request the gateway understood but cannot serve.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TtsError {
    /// The requested voice is not one the model offers.
    #[error("voice {voice:?} is not offered by model {model}; valid voices: {}", valid.join(", "))]
    InvalidVoice {
        /// The caller-facing model name.
        model: String,
        /// The voice the caller asked for.
        voice: String,
        /// The voices the model does offer, in catalog order.
        valid: Vec<String>,
    },
}

/// Check a requested voice against the voices `model` offers.
///
/// An empty `voices` list means the catalog names none, so the backend
/// chooses and any voice passes: an operator who lists no voices has opted
/// out of this check rather than forbidden every voice.
///
/// # Errors
/// Returns [`TtsError::InvalidVoice`] naming the valid voices when `voices`
/// is non-empty and does not contain `voice`.
pub fn check_voice(model: &str, voice: &str, voices: &[String]) -> Result<(), TtsError> {
    if voices.is_empty() || voices.iter().any(|known| known == voice) {
        return Ok(());
    }
    Err(TtsError::InvalidVoice {
        model: model.to_owned(),
        voice: voice.to_owned(),
        valid: voices.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voices() -> Vec<String> {
        vec!["tara".to_owned(), "leo".to_owned()]
    }

    #[test]
    fn a_catalogued_voice_is_accepted() {
        assert!(check_voice("orpheus", "tara", &voices()).is_ok());
        assert!(check_voice("orpheus", "leo", &voices()).is_ok());
    }

    #[test]
    fn an_uncatalogued_voice_is_refused_by_name() {
        let err = check_voice("orpheus", "nova", &voices()).expect_err("must refuse");
        let message = err.to_string();
        assert!(message.contains("nova"), "names the bad voice: {message}");
        assert!(message.contains("orpheus"), "names the model: {message}");
        assert!(
            message.contains("tara, leo"),
            "names what the caller may ask for instead: {message}"
        );
    }

    #[test]
    fn an_empty_catalog_checks_nothing() {
        // No voices listed is "the backend decides", not "no voice is valid":
        // a model whose voice set the operator did not enumerate must stay
        // usable.
        assert!(check_voice("orpheus", "anything", &[]).is_ok());
    }

    #[test]
    fn the_voice_match_is_exact() {
        assert!(check_voice("orpheus", "Tara", &voices()).is_err());
        assert!(check_voice("orpheus", "tara ", &voices()).is_err());
    }
}
