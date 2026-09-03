//! Small display helpers shared by every host that prints a position to a person.
//!
//! Nothing here is host-specific — it is pure arithmetic over a frame count and a rate —
//! so it lives here rather than being copied between `starplayer-host-cpal`'s
//! `examples/play.rs` and `apps/starplayer-cli`'s `play` command, which both print the
//! same `m:ss` position and length.

use std::string::String;

/// Format `frames` at `sample_rate_hz` as `m:ss`.
///
/// `sample_rate_hz` of zero — a spec nothing has negotiated yet — reads as `--:--` rather
/// than dividing by zero.
pub fn format_seconds(frames: u64, sample_rate_hz: u32) -> String {
    if sample_rate_hz == 0 {
        return String::from("--:--");
    }
    let total = frames / sample_rate_hz as u64;
    std::format!("{}:{:02}", total / 60, total % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_format_as_minutes_and_seconds() {
        assert_eq!(format_seconds(0, 48_000), "0:00");
        assert_eq!(format_seconds(48_000 * 65, 48_000), "1:05");
        assert_eq!(format_seconds(123, 0), "--:--");
    }
}
