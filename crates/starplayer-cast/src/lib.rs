//! `starplayer-cast` — a Google Cast sender, so a module can play on a Google Home or
//! Nest speaker.
//!
//! # What a Cast receiver is, and why audio has to leave this machine
//!
//! A Cast device runs *receiver applications*, not senders' code. The one this crate
//! drives is the **Default Media Receiver** (`CC1AD845`) — Google-hosted, present on every
//! Cast device, and needing no developer registration at all. It does exactly one useful
//! thing: it takes a media URL and plays it, reporting position and player state back over
//! the CASTv2 control channel.
//!
//! That shape decides the architecture. The receiver plays a *URL*, so the audio has to be
//! reachable over HTTP from the speaker's own network position — the speaker fetches it,
//! we do not push it. A CLI has a machine behind it, so this crate renders the module here,
//! serves the bytes from a small HTTP server bound to the LAN interface that reaches the
//! device, and tells the receiver to play that URL. The URL can never be `127.0.0.1`: the
//! speaker would dial its own loopback and hear nothing.
//!
//! Running StarPlayer's own engine *on* the speaker is the other Cast shape — a custom Web
//! Receiver — and it is A4-N3/N4, not this crate.
//!
//! # Why this crate is synchronous
//!
//! There is no async runtime anywhere in this workspace and this crate does not add one.
//! Everything here is blocking I/O on a thread:
//!
//! - the CASTv2 session is a blocking TLS socket, driven from the control thread, with a
//!   heartbeat thread that only ever *sends*;
//! - the media server is `tiny_http`, a thread per connection;
//! - the `--live` encoder runs on the audio driver thread, which is pacing itself against
//!   the wall clock anyway.
//!
//! One session, one server and one encoder is not a concurrency problem that wants an
//! executor; it is three threads and two channels. An executor would be a dependency and a
//! colour on every function for no behaviour we need.
//!
//! # Real-time safety
//!
//! [`live::CastStreamBackend`] is an [`AudioBackend`](starplayer_host::AudioBackend) like
//! any other: the render callback fills a pre-sized scratch buffer and returns. Encoding,
//! channel sends and clock pacing all happen on the driver thread *after* the callback has
//! returned, so design goal 5 holds unchanged along the audio path.

#![forbid(unsafe_code)]

pub mod discover;
pub mod encode;
pub mod live;
pub mod serve;
pub mod session;

use std::fmt;

pub use discover::{CastDevice, discover, find};
pub use encode::{CastEncoder, FlacEncoder, WavEncoder};
pub use live::CastStreamBackend;
pub use serve::MediaServer;
pub use session::{CastSession, MediaRequest, MediaStatus, StreamKind};

/// The Google-hosted Default Media Receiver's application id.
///
/// Every Cast device has it, no registration is needed to launch it, and it plays a media
/// URL. See this module's doc for what that implies for the rest of the crate.
pub const DEFAULT_MEDIA_RECEIVER_APP_ID: &str = "CC1AD845";

/// The mDNS service type every Cast device and speaker group answers on.
pub const CAST_SERVICE_TYPE: &str = "_googlecast._tcp.local.";

/// The TCP port a Cast device's CASTv2 endpoint listens on.
pub const CAST_PORT: u16 = 8009;

/// Everything that can go wrong between this machine and a Cast device.
///
/// One flat enum with a `String` in every arm, because every one of them ends up as a
/// single line printed by the CLI. The layers underneath — `rust_cast`, `rustls`,
/// `mdns-sd`, `tiny_http`, `flacenc` — have error types of their own, and none of them
/// crosses this boundary: swapping `rust_cast` for another CASTv2 client is meant to be a
/// change to one module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CastError {
    /// Browsing for `_googlecast._tcp` failed, or an ambiguous device name was given.
    Discovery(String),
    /// The TCP or TLS connection to the device could not be established.
    Connect(String),
    /// A CASTv2 message could not be sent, received, or understood.
    Protocol(String),
    /// The receiver refused to launch, or reported no application after launching.
    Launch(String),
    /// The receiver refused the media, or reported no media session after loading.
    Load(String),
    /// Something that was waited for did not arrive in time.
    Timeout(String),
    /// A socket, a file or an encoder failed.
    Io(String),
}

impl fmt::Display for CastError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CastError::Discovery(message) => write!(formatter, "cast discovery failed: {message}"),
            CastError::Connect(message) => write!(formatter, "could not connect to the cast device: {message}"),
            CastError::Protocol(message) => write!(formatter, "cast protocol error: {message}"),
            CastError::Launch(message) => write!(formatter, "could not launch the cast receiver: {message}"),
            CastError::Load(message) => write!(formatter, "the cast receiver would not load the media: {message}"),
            CastError::Timeout(message) => write!(formatter, "timed out: {message}"),
            CastError::Io(message) => write!(formatter, "i/o error: {message}"),
        }
    }
}

impl std::error::Error for CastError {}

/// Turn a module title, or any other text, into something safe to put in a URL path.
///
/// Keeps ASCII letters, digits, `_` and `-`; collapses every run of anything else into a
/// single `-`; trims leading and trailing `-`. An empty result is reported as `None` so
/// the caller can fall back (file stem, then `module`) rather than serving `/.flac`.
///
/// Some receivers sniff the extension off the path, which is why this exists at all: the
/// URL has to end `.flac` or `.wav` and everything before it has to survive being typed
/// into a `GET` line.
pub fn slugify(text: &str) -> Option<String> {
    let mut slug = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
            slug.push(character);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let trimmed = slug.trim_matches('-');
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_becomes_a_url_safe_stem() {
        assert_eq!(slugify("Beyond Music").as_deref(), Some("Beyond-Music"));
        assert_eq!(slugify("  spaced  out  ").as_deref(), Some("spaced-out"));
        assert_eq!(slugify("keep_me-1").as_deref(), Some("keep_me-1"));
        assert_eq!(slugify("a/b?c#d").as_deref(), Some("a-b-c-d"));
    }

    #[test]
    fn a_title_with_nothing_ascii_in_it_has_no_slug() {
        assert_eq!(slugify(""), None);
        assert_eq!(slugify("   "), None);
        assert_eq!(slugify("♪♫"), None);
    }

    #[test]
    fn every_error_prints_as_one_line() {
        let errors = [
            CastError::Discovery(String::from("no responder")),
            CastError::Connect(String::from("refused")),
            CastError::Protocol(String::from("bad frame")),
            CastError::Launch(String::from("busy")),
            CastError::Load(String::from("unsupported")),
            CastError::Timeout(String::from("no status")),
            CastError::Io(String::from("broken pipe")),
        ];
        for error in errors {
            let text = error.to_string();
            assert!(!text.contains('\n'), "{text:?} is more than one line");
            assert!(!text.is_empty());
        }
    }
}
