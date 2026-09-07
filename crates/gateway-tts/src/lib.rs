//! The gateway-side speech service: voice validation, the voice catalog, and
//! the audio relay behind `POST /v1/audio/speech`.
//!
//! The gateway owns the routes, the bearer check, model resolution, and
//! dominion queue admission; this crate owns what happens to a speech request
//! once the model is known. A synthesized clip is opaque bytes, so
//! [`relay_audio`] forwards them unread rather than re-validating per item the
//! way the chat relay does - see its documentation for why that departure is
//! the correct discipline here.

mod error;
mod relay;
mod voices;

pub use crate::error::{TtsError, check_voice};
pub use crate::relay::{StreamHold, relay_audio};
pub use crate::voices::{VoiceEntry, VoicesResponse, voices_response};
