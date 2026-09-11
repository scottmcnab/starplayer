//! The now-playing view model: a [`Snapshot`] reduced to what a screen, a log line or a
//! web page actually shows.
//!
//! A [`Snapshot`] is 2 616 bytes and always carries 64 channels whatever the module has
//! (I1 research point 3a). Nothing that renders it wants 64 channels: the M8-I5 display
//! has room for [`MAX_DISPLAY_CHANNELS`], the control task's once-a-second UART line has
//! room for one, and M8-I6's page wants the same fields as JSON. [`NowPlaying`] is those
//! fields, `Copy` and derived by one function so the consumers cannot drift apart.
//!
//! It is deliberately **not** a formatter for the screen. `Display` prints the one-line
//! transport summary the CLI and the UART log both want
//! (`apps/starplayer-cli/src/play.rs::report`'s shape); a display driver (M8-I5's
//! `screen.rs`) lays the header and the per-channel rows out its own way from the plain
//! fields below.
//!
//! # `Copy`, with no heap and no `alloc::String`
//!
//! [`NowPlaying`] crosses from the control task into the display task by value every
//! frame the screen redraws (M8-I5), so it has to be cheap to copy and must not allocate.
//! `heapless::String` is not `Copy` (its buffer is `[MaybeUninit<u8>; N]` plus a length,
//! and the crate does not derive `Copy` for it), so the module title and each channel's
//! note use [`FixedStr`] instead — a fixed-capacity byte buffer that *is* `Copy`, built
//! without `unsafe` (this crate is `#![forbid(unsafe_code)]`).

use core::fmt::{Display, Formatter};

use starplayer::core::{Note, U0F16};
use starplayer_telemetry::{Snapshot, SongEnd};

/// A fixed-capacity, `Copy` UTF-8 string of at most `N` bytes.
///
/// [`NowPlaying::title`] is the reason this exists: a module title has to ride in a
/// `Copy` struct, and `heapless::String<N>` is not `Copy`. Truncates at a character
/// boundary rather than splitting one, and never panics.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FixedStr<const N: usize> {
    bytes: [u8; N],
    len: u8,
}

// `Default` is implemented by hand rather than derived: the standard library has no
// blanket `impl<T: Default, const N: usize> Default for [T; N]` for a *generic* `N` (only
// for literal array lengths), so `#[derive(Default)]` cannot see through `[u8; N]` here.
impl<const N: usize> Default for FixedStr<N> {
    fn default() -> FixedStr<N> { FixedStr::EMPTY }
}

impl<const N: usize> FixedStr<N> {
    /// An empty string.
    pub const EMPTY: FixedStr<N> = FixedStr { bytes: [0; N], len: 0 };

    /// Copy as much of `text` as fits in `N` bytes, truncating at the last whole
    /// character rather than splitting one.
    pub fn new(text: &str) -> FixedStr<N> {
        let mut end = text.len().min(N);
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        let mut bytes = [0u8; N];
        bytes[..end].copy_from_slice(&text.as_bytes()[..end]);
        FixedStr { bytes, len: end as u8 }
    }

    /// The string, as `&str`.
    ///
    /// Always valid UTF-8: [`FixedStr::new`] is the only constructor and it truncates on
    /// a character boundary, so this never has to fall back to `"?"` — the `unwrap_or`
    /// exists only so the accessor cannot panic if that invariant is ever broken.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }

    /// The capacity, in bytes.
    pub const fn capacity() -> usize { N }
}

impl<const N: usize> Display for FixedStr<N> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> core::fmt::Result { formatter.write_str(self.as_str()) }
}

/// Channel rows [`NowPlaying`] carries: enough for every format currently in scope —
/// IT's 64-channel pattern is the widest, but the M8-I5 screen has room for this many
/// 14 px rows at 240×280, and a module with more channels than fit simply has its
/// overflow left off the display, not off the mix.
pub const MAX_DISPLAY_CHANNELS: usize = 16;

/// The three ASCII characters a channel row's note column shows: `"C-5"`-style tracker
/// notation, or `"..."` when the channel has never sounded.
///
/// Octave numbering follows [`Note::MIDDLE_C`] (MIDI 60) landing on `C-5`, i.e.
/// `octave = semitone / 12`. This is a display-only convention local to this crate — it
/// does not have to (and does not) match any one format's own on-disk octave numbering,
/// which is exactly why the format crates keep their own `tracker_notation`-style
/// helpers (e.g. `starplayer-it::pattern::ChannelCell::tracker_notation`) for pattern
/// views; this one draws from the engine's own MIDI-based [`Note`] instead.
fn note_label(note: Option<Note>) -> [u8; 3] {
    const NOTE_NAMES: [[u8; 2]; 12] =
        [*b"C-", *b"C#", *b"D-", *b"D#", *b"E-", *b"F-", *b"F#", *b"G-", *b"G#", *b"A-", *b"A#", *b"B-"];
    match note {
        None => *b"...",
        Some(note) => {
            let semitone = note.semitone;
            let name = NOTE_NAMES[(semitone % 12) as usize];
            let octave = (semitone / 12).min(9);
            [name[0], name[1], b'0' + octave]
        }
    }
}

/// One channel, as the screen draws it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelRow {
    /// The instrument the channel last played, one-based, or 0 for none yet.
    pub instrument: u8,
    /// The note the channel is sounding, or the last one it sounded — `"C-5"`-style
    /// tracker notation, sticky like [`starplayer_telemetry::ChannelState::note`], or
    /// `"..."` when nothing has played yet.
    pub note: [u8; 3],
    /// Peak-hold VU level, quantised to the screen's 16-cell bar: `0..=16`.
    pub vu: u8,
    /// The row's effect, spelled out in English — [`EffectDisplay::name`]'s whole reason
    /// to exist (`plans/reference/original-star-ui.md` §12).
    ///
    /// [`EffectDisplay::name`]: starplayer_telemetry::EffectDisplay::name
    pub effect_name: &'static str,
    /// Whether a voice is still sounding on this channel.
    pub active: bool,
}

/// Where the song is and how hard the engine is working, plus what the screen's channel
/// rows show — the six numbers a log line needs, and the [`MAX_DISPLAY_CHANNELS`] rows a
/// screen needs, derived from the same [`Snapshot`] so the two never disagree.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct NowPlaying {
    /// The module's title, as the file spells it (`ModuleHeader::title`), truncated to
    /// [`FixedStr`]'s capacity.
    pub title: FixedStr<28>,
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
    /// The host's master volume — not the module's own global volume, which the
    /// original's status line does not show either.
    pub volume: U0F16,
    /// Voices sounding in the global pool.
    pub voices_active: u16,
    /// How many of the module's channels exist. May exceed [`MAX_DISPLAY_CHANNELS`]; a
    /// renderer draws `channel_count.min(MAX_DISPLAY_CHANNELS)` rows out of [`channels`](NowPlaying::channels).
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
    /// Up to [`MAX_DISPLAY_CHANNELS`] channel rows, in channel order. Only the first
    /// `channel_count.min(MAX_DISPLAY_CHANNELS)` are meaningful; the rest are
    /// [`ChannelRow::default`].
    pub channels: [ChannelRow; MAX_DISPLAY_CHANNELS],
}

impl NowPlaying {
    /// Reduce a snapshot, given the rate it was rendered at, the module's title and the
    /// host's current master volume.
    ///
    /// The rate is a parameter rather than a constant so that the seconds are right even
    /// if a future board runs at something other than
    /// [`SAMPLE_RATE_HZ`](crate::SAMPLE_RATE_HZ); a zero rate reports zero elapsed rather
    /// than dividing. `title` and `volume` do not live in [`Snapshot`] — the module's
    /// header and the host's control-side state, both owned by [`ControlHalf`], are the
    /// title's and the volume's sources respectively — so they arrive as parameters
    /// rather than being read out of the snapshot.
    ///
    /// [`ControlHalf`]: starplayer_host_embedded::ControlHalf
    pub fn from_snapshot(snapshot: &Snapshot, sample_rate_hz: u32, title: &str, volume: U0F16) -> NowPlaying {
        let transport = &snapshot.transport;
        let seconds = |frames: u64| -> u32 {
            if sample_rate_hz == 0 { 0 } else { (frames / u64::from(sample_rate_hz)) as u32 }
        };

        let mut channels = [ChannelRow::default(); MAX_DISPLAY_CHANNELS];
        for (row, channel) in channels.iter_mut().zip(snapshot.active_channels()) {
            *row = ChannelRow {
                instrument: channel.instrument,
                note: note_label(channel.note),
                // `+ 1` before scaling so full scale (`u16::MAX`) lands exactly on the
                // top cell (`16`) instead of truncating to `15`; `min(16)` is the safety
                // net, not the rounding.
                vu: (((u32::from(channel.vu_level.to_bits()) + 1) * 16) / (u32::from(u16::MAX) + 1)).min(16) as u8,
                effect_name: channel.effect.name,
                active: channel.active,
            };
        }

        NowPlaying {
            title: FixedStr::new(title),
            order: transport.order,
            pattern: transport.pattern,
            row: transport.row,
            speed: transport.speed,
            tempo_bpm: transport.tempo_bpm,
            volume,
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
            channels,
        }
    }

    /// The channel rows a renderer should actually draw: `channel_count` clamped to
    /// [`MAX_DISPLAY_CHANNELS`] and to the array's own length, so a corrupt
    /// `channel_count` cannot index past it.
    pub fn displayed_channels(&self) -> &[ChannelRow] {
        let count = (self.channel_count as usize).min(MAX_DISPLAY_CHANNELS).min(self.channels.len());
        &self.channels[..count]
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

    use starplayer_telemetry::{ChannelState, EffectDisplay, Snapshot, TransportState};

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
    fn a_snapshot_reduces_to_the_header_fields_a_screen_shows() {
        let view = NowPlaying::from_snapshot(&snapshot(), crate::SAMPLE_RATE_HZ, "Petri", U0F16::MAX);
        assert_eq!(view.title.as_str(), "Petri");
        assert_eq!(view.order, 3);
        assert_eq!(view.pattern, 7);
        assert_eq!(view.row, 17);
        assert_eq!(view.speed, 6);
        assert_eq!(view.tempo_bpm, 125);
        assert_eq!(view.volume, U0F16::MAX);
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
        let line = format!("{}", NowPlaying::from_snapshot(&snapshot(), crate::SAMPLE_RATE_HZ, "Petri", U0F16::MAX));
        assert_eq!(line, "ord 003 pat 007 row 17/06 125 bpm   5/ 8 voices  1:35/3:32 loop");
        assert!(!line.contains('\n'));
        assert!(line.len() <= 80, "{} characters: {line}", line.len());
        // The header line is deliberately silent about the title and the channel rows —
        // a display driver lays those out its own way.
        assert!(!line.contains("Petri"));
    }

    #[test]
    fn an_unscanned_song_has_no_total_and_says_so() {
        let mut snapshot = snapshot();
        snapshot.transport.song_length_frames = 0;
        let view = NowPlaying::from_snapshot(&snapshot, crate::SAMPLE_RATE_HZ, "", U0F16::MAX);
        assert_eq!(view.total_seconds, None);
        assert!(format!("{view}").contains("/--:--"));
    }

    #[test]
    fn a_zero_sample_rate_reports_zero_rather_than_dividing() {
        let view = NowPlaying::from_snapshot(&snapshot(), 0, "", U0F16::MAX);
        assert_eq!(view.elapsed_seconds, 0);
        assert_eq!(view.total_seconds, Some(0));
    }

    #[test]
    fn the_idle_snapshot_reduces_to_the_default_view_apart_from_title_and_volume() {
        let view = NowPlaying::from_snapshot(&Snapshot::IDLE, crate::SAMPLE_RATE_HZ, "", U0F16::ZERO);
        assert_eq!(view, NowPlaying::default());
        assert_eq!(view.displayed_channels().len(), 0);
    }

    #[test]
    fn a_title_longer_than_the_capacity_is_truncated_on_a_character_boundary() {
        // 29 ASCII 'x's, one past FixedStr<28>'s capacity.
        let long = "x".repeat(29);
        let view = NowPlaying::from_snapshot(&Snapshot::IDLE, crate::SAMPLE_RATE_HZ, &long, U0F16::MAX);
        assert_eq!(view.title.as_str().len(), 28);

        // A multi-byte character sitting right on the boundary is dropped whole rather
        // than split into invalid UTF-8.
        let mut multibyte = "x".repeat(27);
        multibyte.push('€'); // 3-byte character starting at byte 27, capacity 28
        let view = NowPlaying::from_snapshot(&Snapshot::IDLE, crate::SAMPLE_RATE_HZ, &multibyte, U0F16::MAX);
        assert_eq!(view.title.as_str(), "x".repeat(27));
    }

    #[test]
    fn channel_rows_carry_the_instrument_note_vu_effect_and_active_flag() {
        let mut snapshot = Snapshot { channel_count: 2, ..Snapshot::IDLE };
        snapshot.channels[0] = ChannelState {
            note: Some(Note::MIDDLE_C),
            instrument: 3,
            vu_level: U0F16::MAX,
            effect: EffectDisplay::raw(6, 0x20).with_name("change speed"),
            active: true,
            ..ChannelState::SILENT
        };
        snapshot.channels[1] = ChannelState::SILENT;

        let view = NowPlaying::from_snapshot(&snapshot, crate::SAMPLE_RATE_HZ, "", U0F16::MAX);
        let rows = view.displayed_channels();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].instrument, 3);
        assert_eq!(rows[0].note, *b"C-5");
        assert_eq!(rows[0].vu, 16, "full-scale VU quantises to the top of a 16-cell bar");
        assert_eq!(rows[0].effect_name, "change speed");
        assert!(rows[0].active);

        assert_eq!(rows[1].note, *b"...", "a channel that has never played shows the silent glyph");
        assert!(!rows[1].active);
    }

    #[test]
    fn displayed_channels_is_bounded_by_the_display_capacity_however_wide_the_module() {
        let snapshot = Snapshot { channel_count: 200, ..Snapshot::IDLE };
        let view = NowPlaying::from_snapshot(&snapshot, crate::SAMPLE_RATE_HZ, "", U0F16::MAX);
        assert_eq!(view.displayed_channels().len(), MAX_DISPLAY_CHANNELS, "a corrupt count cannot index past the array");
    }

    #[test]
    fn note_octaves_follow_middle_c_as_c_dash_5() {
        assert_eq!(note_label(Some(Note::MIDDLE_C)), *b"C-5");
        assert_eq!(note_label(Some(Note::new(61))), *b"C#5");
        assert_eq!(note_label(None), *b"...");
    }
}
