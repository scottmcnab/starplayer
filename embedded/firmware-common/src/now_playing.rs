//! The now-playing view model: a [`Snapshot`] reduced to what a screen, a log line or a
//! web page actually shows.
//!
//! A [`Snapshot`] is 2 616 bytes and always carries 64 channels whatever the module has
//! (I1 research point 3a). Nothing that renders it wants 64 channels: the M8-I5 display
//! has room for six fields, the control task's once-a-second UART line has room for one,
//! and M8-I6's page wants the same six as JSON. [`NowPlaying`] is those fields, `Copy`,
//! 32 bytes, and derived by one function so the three consumers cannot drift apart.
//!
//! It is deliberately **not** a formatter. `Display` prints the log line and nothing
//! else; a display driver lays the same fields out its own way.

use core::fmt::{Display, Formatter};

use starplayer_telemetry::{Snapshot, SongEnd};

/// Where the song is and how hard the engine is working, in the six numbers a reader
/// needs.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct NowPlaying {
    /// Index into the order list.
    pub order: u16,
    /// The pattern that order entry names.
    pub pattern: u16,
    /// Row within the pattern.
    pub row: u16,
    /// Ticks per row in effect (`Axx`).
    pub speed: u8,
    /// Tempo in effect (`Txx`).
    pub tempo_bpm: u16,
    /// Voices sounding in the global pool.
    pub voices_active: u16,
    /// How many of the module's channels exist.
    pub channel_count: u8,
    /// Seconds into this pass through the song.
    pub elapsed_seconds: u32,
    /// Seconds in one pass, or `None` when no timeline has been scanned.
    pub total_seconds: Option<u32>,
    /// Whether the song has been heard through once.
    pub end_reached: bool,
    /// Whether the scan found a song that loops for ever rather than stopping.
    pub loops: bool,
    /// Whether the render loop raised any warning flag — a zero-advance guard, a dropped
    /// retirement. One bit, because a screen has room for one bit; the [`Snapshot`] keeps
    /// the detail.
    pub warned: bool,
}

impl NowPlaying {
    /// Reduce a snapshot, given the rate it was rendered at.
    ///
    /// The rate is a parameter rather than a constant so that the seconds are right even
    /// if a future board runs at something other than
    /// [`SAMPLE_RATE_HZ`](crate::SAMPLE_RATE_HZ); a zero rate reports zero elapsed rather
    /// than dividing.
    pub fn from_snapshot(snapshot: &Snapshot, sample_rate_hz: u32) -> NowPlaying {
        let transport = &snapshot.transport;
        let seconds = |frames: u64| -> u32 {
            if sample_rate_hz == 0 { 0 } else { (frames / u64::from(sample_rate_hz)) as u32 }
        };
        NowPlaying {
            order: transport.order,
            pattern: transport.pattern,
            row: transport.row,
            speed: transport.speed,
            tempo_bpm: transport.tempo_bpm,
            voices_active: snapshot.voices_active,
            channel_count: snapshot.channel_count,
            elapsed_seconds: seconds(transport.song_frame),
            total_seconds: match transport.song_length_frames {
                0 => None,
                frames => Some(seconds(frames)),
            },
            end_reached: transport.end_reached,
            loops: transport.song_end == SongEnd::Loops,
            warned: snapshot.warnings.any(),
        }
    }
}

impl Display for NowPlaying {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "ord {:03} pat {:03} row {:02}/{:02} {:3} bpm  {:2}/{:2} voices  {}:{:02}",
            self.order, self.pattern, self.row, self.speed, self.tempo_bpm,
            self.voices_active, self.channel_count,
            self.elapsed_seconds / 60, self.elapsed_seconds % 60,
        )?;
        match self.total_seconds {
            Some(total) => write!(formatter, "/{}:{:02}", total / 60, total % 60)?,
            None => write!(formatter, "/--:--")?,
        }
        if self.loops {
            formatter.write_str(" loop")?;
        }
        if self.end_reached {
            formatter.write_str(" end")?;
        }
        if self.warned {
            formatter.write_str(" WARN")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;

    use starplayer_telemetry::{Snapshot, TransportState};

    use super::*;

    fn snapshot() -> Snapshot {
        Snapshot {
            voices_active: 5,
            channel_count: 8,
            transport: TransportState {
                order: 3,
                pattern: 7,
                row: 17,
                speed: 6,
                tempo_bpm: 125,
                song_frame: 44_100 * 95,
                song_length_frames: 44_100 * 212,
                song_end: starplayer_telemetry::SongEnd::Loops,
                ..TransportState::IDLE
            },
            ..Snapshot::IDLE
        }
    }

    #[test]
    fn a_snapshot_reduces_to_the_six_fields_a_screen_shows() {
        let view = NowPlaying::from_snapshot(&snapshot(), crate::SAMPLE_RATE_HZ);
        assert_eq!(view.order, 3);
        assert_eq!(view.pattern, 7);
        assert_eq!(view.row, 17);
        assert_eq!(view.speed, 6);
        assert_eq!(view.tempo_bpm, 125);
        assert_eq!(view.voices_active, 5);
        assert_eq!(view.channel_count, 8);
        assert_eq!(view.elapsed_seconds, 95);
        assert_eq!(view.total_seconds, Some(212));
        assert!(view.loops);
        assert!(!view.end_reached);
        assert!(!view.warned);
    }

    #[test]
    fn the_log_line_is_one_line_and_fits_a_terminal() {
        let line = format!("{}", NowPlaying::from_snapshot(&snapshot(), crate::SAMPLE_RATE_HZ));
        assert_eq!(line, "ord 003 pat 007 row 17/06 125 bpm   5/ 8 voices  1:35/3:32 loop");
        assert!(!line.contains('\n'));
        assert!(line.len() <= 80, "{} characters: {line}", line.len());
    }

    #[test]
    fn an_unscanned_song_has_no_total_and_says_so() {
        let mut snapshot = snapshot();
        snapshot.transport.song_length_frames = 0;
        let view = NowPlaying::from_snapshot(&snapshot, crate::SAMPLE_RATE_HZ);
        assert_eq!(view.total_seconds, None);
        assert!(format!("{view}").contains("/--:--"));
    }

    #[test]
    fn a_zero_sample_rate_reports_zero_rather_than_dividing() {
        let view = NowPlaying::from_snapshot(&snapshot(), 0);
        assert_eq!(view.elapsed_seconds, 0);
        assert_eq!(view.total_seconds, Some(0));
    }

    #[test]
    fn the_idle_snapshot_reduces_to_the_default_view() {
        let view = NowPlaying::from_snapshot(&Snapshot::IDLE, crate::SAMPLE_RATE_HZ);
        assert_eq!(view, NowPlaying::default());
    }
}
