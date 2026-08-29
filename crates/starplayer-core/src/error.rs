//! The one error type the loaders, the module builder and the platform byte sources all
//! speak (architecture §11 lists `Error` as part of this crate).
//!
//! It is deliberately tiny and `Copy`: every variant carries either nothing, a `usize`
//! pair, or a `&'static str` reason string. No allocation, no formatting machinery, no
//! `std::io::Error` — an embedded loader reading out of flash and a WASM loader reading
//! out of a `Uint8Array` have to produce the same errors as a native one.

use core::fmt;

/// What can go wrong while reading a module and turning it into a
/// `starplayer_model::Module`.
///
/// Reason strings are `&'static str` and are written for a developer reading a log, not
/// for an end user; a UI should render the variant, not the string.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Error {
    /// The byte source ended before the loader had read everything it needed.
    ///
    /// `offset` is where the read started and `needed` is how many bytes it wanted, so a
    /// fuzz report says exactly which field ran off the end.
    Truncated {
        /// Offset the failed read started at.
        offset: usize,
        /// Bytes the failed read wanted.
        needed: usize,
    },
    /// The file does not carry the signature of the format that was asked to load it.
    BadMagic,
    /// A well-formed file using a feature this build does not implement — a compression
    /// scheme, a format revision, an instrument type.
    Unsupported(&'static str),
    /// A structurally broken file: a field contradicts another field, or a value is
    /// outside what the format allows.
    Invalid(&'static str),
    /// An index or offset points outside the data it indexes — a sample offset past the
    /// end of the PCM blob, an order entry naming a pattern that does not exist.
    OutOfRange,
    /// A value that would not fit the model's `u32` offsets, or a count past a hard limit
    /// the engine imposes.
    TooLarge(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated { offset, needed } => write!(formatter, "truncated: wanted {needed} byte(s) at offset {offset}"),
            Error::BadMagic => write!(formatter, "not a module of this format (bad magic)"),
            Error::Unsupported(what) => write!(formatter, "unsupported: {what}"),
            Error::Invalid(what) => write!(formatter, "invalid module: {what}"),
            Error::OutOfRange => write!(formatter, "index or offset out of range"),
            Error::TooLarge(what) => write!(formatter, "too large: {what}"),
        }
    }
}

impl core::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_compare_and_hash_as_plain_data() {
        assert_eq!(Error::Truncated { offset: 4, needed: 16 }, Error::Truncated { offset: 4, needed: 16 });
        assert_ne!(Error::Invalid("a"), Error::Invalid("b"));
        assert_ne!(Error::BadMagic, Error::OutOfRange);
    }
}
