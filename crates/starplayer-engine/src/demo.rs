//! A four-byte toy tracker format, so the sequencer can be exercised before any real
//! loader exists.
//!
//! This is the sequencer's [`ScriptedSource`](crate::ScriptedSource): a fixture that lives
//! in the library rather than in a test file because the block-size determinism test, the
//! sequencer's own tests and — later — every format crate's timing tests all want the same
//! one. It is deliberately *not* a format: there are no effect memories, no periods and no
//! per-tick effects. It exists to make the timing spine testable, and B4's ST3 processor is
//! what a real [`TrackerProcessor`] looks like.
//!
//! # The encoding
//!
//! A row is `channel_count` cells of [`DEMO_CELL_BYTES`] bytes each:
//!
//! | Byte | Meaning |
//! |---|---|
//! | 0 | note: [`DEMO_NOTE_NONE`], [`DEMO_NOTE_CUT`], or a semitone index 1..=200 |
//! | 1 | volume 0..=64, or [`DEMO_VOLUME_FULL`] |
//! | 2 | command: one of the `DEMO_*` command bytes below, or 0 for none |
//! | 3 | command parameter |
//!
//! Commands are the ones that move the timing spine, spelled as the ASCII letters their
//! S3M equivalents use so a test reads like a pattern: `A` speed, `B` order jump,
//! `C` break to row, `T` tempo, `S` pattern delay, and `X` stop.

use alloc::vec;
use alloc::vec::Vec;

use starplayer_core::fixed::unit_from_ratio;
use starplayer_core::{ChannelId, Step, U0F16, VoiceParams};
use starplayer_mixer::{SampleRegion, VoiceTag};

use crate::sequencer::{Jump, OrderEntry, PatternData, RowRef, TickContext, TickOutcome, TrackerProcessor};

/// Bytes per channel per row.
pub const DEMO_CELL_BYTES: usize = 4;

/// No note in this cell.
pub const DEMO_NOTE_NONE: u8 = 0;
/// Stop whatever the channel is sounding.
pub const DEMO_NOTE_CUT: u8 = 255;
/// The semitone index that plays a sample at exactly its own rate.
pub const DEMO_REFERENCE_NOTE: u8 = 48;
/// Volume byte meaning "full scale" rather than a 0..=64 tracker volume.
pub const DEMO_VOLUME_FULL: u8 = 255;

/// `Axx` — set ticks per row.
pub const DEMO_SET_SPEED: u8 = b'A';
/// `Bxx` — jump to an order-list index.
pub const DEMO_ORDER_JUMP: u8 = b'B';
/// `Cxx` — break to a row of the next pattern.
pub const DEMO_BREAK_ROW: u8 = b'C';
/// `Txx` — set tempo in BPM.
pub const DEMO_SET_TEMPO: u8 = b'T';
/// `SEx` — repeat this row `x` more times.
pub const DEMO_PATTERN_DELAY: u8 = b'S';
/// End the song.
pub const DEMO_STOP: u8 = b'X';

/// One cell of the toy format.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct DemoCell {
    /// [`DEMO_NOTE_NONE`], [`DEMO_NOTE_CUT`], or a semitone index.
    pub note: u8,
    /// 0..=64, or [`DEMO_VOLUME_FULL`].
    pub volume: u8,
    /// One of the `DEMO_*` command bytes, or 0.
    pub command: u8,
    /// The command's parameter.
    pub param: u8,
}

impl DemoCell {
    /// An empty cell.
    pub const EMPTY: DemoCell = DemoCell { note: DEMO_NOTE_NONE, volume: DEMO_VOLUME_FULL, command: 0, param: 0 };

    /// A cell that plays `note` at full volume.
    pub const fn note(note: u8) -> DemoCell { DemoCell { note, ..DemoCell::EMPTY } }

    /// A cell that carries only a command.
    pub const fn command(command: u8, param: u8) -> DemoCell { DemoCell { command, param, ..DemoCell::EMPTY } }

    const fn bytes(self) -> [u8; DEMO_CELL_BYTES] { [self.note, self.volume, self.command, self.param] }
}

/// Patterns of a fixed row count and a fixed channel count, packed as [`DemoCell`] bytes.
///
/// A real format's [`PatternData`] is variable-length packed data; this one is a flat
/// array, because the point of the fixture is the *timing*, not the unpacking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoPatternData {
    orders: Vec<OrderEntry>,
    pattern_count: u16,
    rows_per_pattern: u16,
    channel_count: u8,
    cells: Vec<u8>,
}

impl DemoPatternData {
    /// `pattern_count` empty patterns, played in order, then end of song.
    pub fn new(pattern_count: u16, rows_per_pattern: u16, channel_count: u8) -> DemoPatternData {
        let row_bytes = rows_per_pattern as usize * channel_count as usize * DEMO_CELL_BYTES;
        let mut cells = vec![0u8; pattern_count as usize * row_bytes];
        for cell in cells.chunks_exact_mut(DEMO_CELL_BYTES) {
            if let Some(volume) = cell.get_mut(1) {
                *volume = DEMO_VOLUME_FULL;
            }
        }
        let orders: Vec<OrderEntry> = (0..pattern_count).map(OrderEntry::Pattern).collect();
        DemoPatternData { orders, pattern_count, rows_per_pattern, channel_count, cells }
    }

    /// Replace the order list.
    pub fn with_orders(mut self, orders: Vec<OrderEntry>) -> DemoPatternData {
        self.orders = orders;
        self
    }

    /// Write one cell. Out-of-range coordinates are ignored rather than an error, so a test
    /// fixture cannot panic in a place that has nothing to do with what it is testing.
    pub fn set(&mut self, pattern: u16, row: u16, channel: u8, cell: DemoCell) -> &mut DemoPatternData {
        if pattern >= self.pattern_count || row >= self.rows_per_pattern || channel >= self.channel_count {
            return self;
        }
        let index = self.cell_offset(pattern, row, channel);
        if let Some(target) = self.cells.get_mut(index..index + DEMO_CELL_BYTES) {
            target.copy_from_slice(&cell.bytes());
        }
        self
    }

    fn cell_offset(&self, pattern: u16, row: u16, channel: u8) -> usize {
        let row_stride = self.channel_count as usize * DEMO_CELL_BYTES;
        let pattern_stride = self.rows_per_pattern as usize * row_stride;
        pattern as usize * pattern_stride + row as usize * row_stride + channel as usize * DEMO_CELL_BYTES
    }
}

impl PatternData for DemoPatternData {
    fn order_count(&self) -> u16 { self.orders.len() as u16 }

    fn order(&self, order: u16) -> Option<OrderEntry> { self.orders.get(order as usize).copied() }

    fn channel_count(&self) -> u8 { self.channel_count }

    fn rows_in_pattern(&self, pattern: u16) -> Option<u16> {
        if pattern < self.pattern_count { Some(self.rows_per_pattern) } else { None }
    }

    fn row_bytes(&self, pattern: u16, row: u16) -> Option<&[u8]> {
        if pattern >= self.pattern_count || row >= self.rows_per_pattern {
            return None;
        }
        let start = self.cell_offset(pattern, row, 0);
        let length = self.channel_count as usize * DEMO_CELL_BYTES;
        self.cells.get(start..start + length)
    }
}

/// A [`TrackerProcessor`] for the toy format.
///
/// Triggers one fixed sample per note, honours the six timing commands, and does nothing
/// per tick. Its counters make it easy for a test to assert that ticks and rows happened at
/// all rather than only that the output was identical.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoProcessor {
    region: SampleRegion,
    base_step: Step,
    rows_played: u32,
    ticks_played: u32,
    notes_triggered: u32,
}

impl DemoProcessor {
    /// A processor that plays `region`, whose [`DEMO_REFERENCE_NOTE`] sounds at `base_step`.
    pub const fn new(region: SampleRegion, base_step: Step) -> DemoProcessor {
        DemoProcessor { region, base_step, rows_played: 0, ticks_played: 0, notes_triggered: 0 }
    }

    /// Rows fetched so far.
    pub const fn rows_played(&self) -> u32 { self.rows_played }

    /// Ticks that were not the first tick of a row.
    pub const fn ticks_played(&self) -> u32 { self.ticks_played }

    /// Notes started so far.
    pub const fn notes_triggered(&self) -> u32 { self.notes_triggered }

    /// A linear, table-free pitch: `base_step * note / DEMO_REFERENCE_NOTE`.
    ///
    /// Not a semitone ratio — that would need a table, and this fixture only has to produce
    /// a *different, deterministic* step per note. Real pitch comes from
    /// `PeriodFromNote` / `PeriodToPitch` in the format crate.
    fn step_for(&self, note: u8) -> Step {
        Step::from_bits(self.base_step.to_bits().saturating_mul(note as u64) / DEMO_REFERENCE_NOTE as u64)
    }
}

/// A toy volume byte as a unit scalar.
fn demo_volume(volume: u8) -> U0F16 {
    if volume == DEMO_VOLUME_FULL { U0F16::MAX } else { unit_from_ratio(volume as u32, 64) }
}

impl TrackerProcessor for DemoProcessor {
    /// The demo processor keeps counters rather than replay state, and a seek must not
    /// discard what a caller is counting, so there is nothing to reset.
    fn reset(&mut self) {}

    fn row(&mut self, context: &mut TickContext<'_>, row: RowRef<'_>) -> TickOutcome {
        self.rows_played = self.rows_played.saturating_add(1);
        let mut outcome = context.outcome();

        for (index, cell) in row.bytes.chunks_exact(DEMO_CELL_BYTES).enumerate() {
            let note = cell.first().copied().unwrap_or(DEMO_NOTE_NONE);
            let volume = cell.get(1).copied().unwrap_or(DEMO_VOLUME_FULL);
            let command = cell.get(2).copied().unwrap_or(0);
            let param = cell.get(3).copied().unwrap_or(0);
            let channel = ChannelId(index as u16);

            match note {
                DEMO_NOTE_NONE => {}
                DEMO_NOTE_CUT => {
                    context.stop_channel(channel);
                }
                note => {
                    let params = VoiceParams { step: self.step_for(note), volume: demo_volume(volume), ..VoiceParams::SILENT };
                    let tag = VoiceTag { channel: index as u8, instrument: 1, sample: 1, note };
                    if context.trigger_channel(channel, tag, self.region, params, 0).is_some() {
                        self.notes_triggered = self.notes_triggered.saturating_add(1);
                    }
                }
            }

            match command {
                DEMO_SET_SPEED => outcome.speed = param,
                DEMO_ORDER_JUMP => outcome.jump = Some(Jump::to_order(param as u16)),
                DEMO_BREAK_ROW => outcome.jump = Some(Jump::break_to_row(param as u16)),
                DEMO_SET_TEMPO => outcome.tempo_bpm = param as u16,
                DEMO_PATTERN_DELAY => outcome.pattern_delay = param,
                DEMO_STOP => outcome.stop = true,
                _ => {}
            }
        }

        outcome
    }

    fn tick(&mut self, context: &mut TickContext<'_>) -> TickOutcome {
        self.ticks_played = self.ticks_played.saturating_add(1);
        context.outcome()
    }
}
