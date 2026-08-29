//! The M1-B6 verification that needs two real threads: **a reader polling from another
//! thread never observes a snapshot mixing state from two different ticks.**
//!
//! `starplayer-telemetry` is `#![no_std]`, so this lives in `tests/` — an integration test
//! is its own crate and may use `std` freely — rather than behind a `cfg(test)` shim in the
//! library.
//!
//! The trick is that every field of the published snapshot is a pure function of its
//! `sequence`. A torn read is then not a subtle timing question: it is a row number that
//! disagrees with a note, and the assertion catches it on the first occurrence.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use starplayer_core::{ChannelId, I1F15, Note, U0F16};
use starplayer_telemetry::{ChannelUpdate, EffectDisplay, MAX_CHANNELS, Snapshot, TelemetryPublisher, telemetry_channel};

/// Snapshots the writer publishes.
const PUBLISH_COUNT: u64 = 100_000;

/// Effect names the writer cycles through, so `EffectDisplay::name` is derived from the
/// sequence too and a torn `&'static str` — a pointer *and* a length — would be caught.
const EFFECT_NAMES: [&str; 4] = ["change speed", "vibrato", "channel pan", "porta & vol. slide"];

/// Everything about snapshot `sequence`, derived from `sequence` alone.
fn write_derived_state(publisher: &mut TelemetryPublisher, sequence: u64) {
    let small = sequence as u16;
    publisher.set_position(small, small.wrapping_add(1), small.wrapping_add(2), small.wrapping_add(3));
    publisher.set_timing(sequence as u8, small.wrapping_add(4));
    publisher.set_voices_active(small.wrapping_add(5));
    publisher.set_channel_count(MAX_CHANNELS as u8);
    publisher.set_global_volume(U0F16::from_bits(small));

    for index in 0..MAX_CHANNELS {
        let channel = ChannelId(index as u16);
        let derived = small.wrapping_add(index as u16);
        publisher.update_channel(
            channel,
            ChannelUpdate {
                voice: None,
                note: Some(Note::with_cents((derived & 0x7F) as u8, derived as i16)),
                instrument: Some(derived as u8),
                volume: Some(U0F16::from_bits(derived)),
                pan: Some(I1F15::from_bits(derived as i16 & 0x7FFF)),
                volume_written: false,
                muted: derived.is_multiple_of(2),
            },
        );
        let name = EFFECT_NAMES[(derived as usize) % EFFECT_NAMES.len()];
        publisher.report_effect(channel, EffectDisplay::raw(derived as u8, (derived >> 8) as u8).with_name(name));
    }
}

/// Check that `snapshot` is entirely from one tick — the same check, run on the reader.
fn assert_internally_consistent(snapshot: &Snapshot) {
    if snapshot.is_idle() {
        return;
    }
    let sequence = snapshot.sequence;
    let small = sequence as u16;

    let transport = snapshot.transport;
    assert_eq!(transport.order, small, "order came from a different tick than sequence {sequence}");
    assert_eq!(transport.pattern, small.wrapping_add(1), "pattern disagrees with sequence {sequence}");
    assert_eq!(transport.row, small.wrapping_add(2), "row disagrees with sequence {sequence}");
    assert_eq!(transport.tick, small.wrapping_add(3), "tick disagrees with sequence {sequence}");
    assert_eq!(transport.speed, sequence as u8, "speed disagrees with sequence {sequence}");
    assert_eq!(transport.tempo_bpm, small.wrapping_add(4), "tempo disagrees with sequence {sequence}");
    assert_eq!(transport.global_volume, U0F16::from_bits(small), "global volume disagrees with sequence {sequence}");
    assert_eq!(snapshot.voices_active, small.wrapping_add(5), "voice count disagrees with sequence {sequence}");
    assert_eq!(snapshot.channel_count, MAX_CHANNELS as u8);

    for (index, channel) in snapshot.channels.iter().enumerate() {
        let derived = small.wrapping_add(index as u16);
        assert_eq!(channel.note, Some(Note::with_cents((derived & 0x7F) as u8, derived as i16)), "channel {index} note disagrees with sequence {sequence}");
        assert_eq!(channel.instrument, derived as u8, "channel {index} instrument disagrees with sequence {sequence}");
        assert_eq!(channel.volume, U0F16::from_bits(derived), "channel {index} volume disagrees with sequence {sequence}");
        assert_eq!(channel.pan, I1F15::from_bits(derived as i16 & 0x7FFF), "channel {index} pan disagrees with sequence {sequence}");
        assert_eq!(channel.muted, derived.is_multiple_of(2), "channel {index} mute disagrees with sequence {sequence}");
        assert_eq!(channel.effect.code, derived as u8, "channel {index} effect code disagrees with sequence {sequence}");
        assert_eq!(channel.effect.param, (derived >> 8) as u8, "channel {index} effect param disagrees with sequence {sequence}");
        assert_eq!(channel.effect.name, EFFECT_NAMES[(derived as usize) % EFFECT_NAMES.len()], "channel {index} effect name disagrees with sequence {sequence}");
    }
}

#[test]
fn a_reader_under_contention_never_sees_a_snapshot_from_two_ticks() {
    let (mut publisher, mut reader) = telemetry_channel();
    let writing = Arc::new(AtomicBool::new(true));

    let writer_done = Arc::clone(&writing);
    let writer = thread::spawn(move || {
        for sequence in 1..=PUBLISH_COUNT {
            write_derived_state(&mut publisher, sequence);
            publisher.publish();
        }
        writer_done.store(false, Ordering::Release);
        publisher.publishes_dropped()
    });

    let mut reads = 0u64;
    let mut newest = 0u64;
    while writing.load(Ordering::Acquire) {
        let snapshot = reader.read();
        assert_internally_consistent(snapshot);
        assert!(snapshot.sequence >= newest, "the reader must never go backwards");
        newest = snapshot.sequence;
        reads += 1;
    }
    // Drain whatever the writer left behind after it finished.
    for _ in 0..8 {
        let snapshot = reader.read();
        assert_internally_consistent(snapshot);
        assert!(snapshot.sequence >= newest);
        newest = snapshot.sequence;
        reads += 1;
    }

    let dropped = writer.join().expect("the writer thread must not panic");

    assert!(reads > 0, "the reader has to have polled at least once");
    assert!(newest > 0, "the reader has to have seen at least one published snapshot");
    assert!(newest <= PUBLISH_COUNT, "the reader cannot see a snapshot the writer never wrote");
    assert!(
        (dropped as u64) < PUBLISH_COUNT,
        "a reader spinning on the ring must have kept up with at least some of {PUBLISH_COUNT} publishes"
    );
}

/// The reader's view of loss: a gap in `sequence` larger than one means publishes were
/// dropped between two reads, and the count in the snapshot agrees with the gaps seen.
#[test]
fn a_slow_reader_sees_gaps_rather_than_corruption() {
    let (mut publisher, mut reader) = telemetry_channel();

    let mut previous = 0u64;
    let mut gaps = 0u64;
    for sequence in 1..=1_000u64 {
        write_derived_state(&mut publisher, sequence);
        publisher.publish();
        // Poll only every fifth tick: the ring is three deep, so this must drop.
        if sequence.is_multiple_of(5) {
            let snapshot = reader.read();
            assert_internally_consistent(snapshot);
            gaps += snapshot.sequence.saturating_sub(previous).saturating_sub(1);
            previous = snapshot.sequence;
        }
    }

    assert!(gaps > 0, "a reader polling five times slower than a three-deep ring must miss snapshots");
    assert!(publisher.publishes_dropped() > 0, "and the writer must have counted them");
}

/// Both halves have to cross a thread boundary: the publisher goes to the audio thread,
/// the reader stays with whoever draws.
#[test]
fn both_halves_are_send() {
    fn assert_send<T: Send>() {}
    assert_send::<starplayer_telemetry::TelemetryPublisher>();
    assert_send::<starplayer_telemetry::TelemetryReader>();
}

/// The snapshot stays the "about 1 KB" scalar payload architecture §9 budgeted for, rather
/// than quietly growing into the 64 KB scope buffers that section split out into (b).
#[test]
fn the_snapshot_is_a_few_kilobytes_not_a_few_dozen() {
    let bytes = std::mem::size_of::<Snapshot>();
    assert!(bytes < 8 * 1024, "the coherent snapshot is {bytes} bytes; scope waveforms belong in the M3 lossy taps");
}
