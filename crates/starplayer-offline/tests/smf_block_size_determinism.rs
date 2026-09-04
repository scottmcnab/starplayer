//! Task E5 deliverable 5: a hand-assembled Standard MIDI File, format 1, two tracks, a
//! tempo change and running status, renders byte-identically at the six block-size
//! invariant sizes through the synthetic IT fixture's instruments.
//!
//! This is the `.mid` counterpart to `crates/starplayer-engine/tests/block_size_determinism.rs`:
//! the same claim design goal 3 makes for a tracker source now proven for
//! `MidiSource<SmfSequencer>`. Samples are compared as bit patterns, never with `==` on
//! floats, exactly as that test's own doc comment explains — this one stays in the
//! integer `MonoI16` output the goldens already trust rather than reopening that question.

use starplayer::dsp::Linear;
use starplayer::mixer::{FixedPath, MonoI16};

/// The host block sizes the invariant is stated over — identical to every other
/// block-size-independence test in this repository (design goal 3).
const BLOCK_SIZES: [usize; 6] = [1, 3, 64, 128, 4096, 8191];

const SAMPLE_RATE_HZ: u32 = 44_100;

/// `MThd` + one track chunk per body in `track_bodies`, each with a mandatory
/// `end_of_track` meta appended. Mirrors `starplayer-midi/src/sequencer.rs`'s own test
/// helper; duplicated here because that helper is private to its crate.
fn smf_bytes(format: u16, division: u16, track_bodies: &[&[u8]]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"MThd");
    bytes.extend_from_slice(&6u32.to_be_bytes());
    bytes.extend_from_slice(&format.to_be_bytes());
    bytes.extend_from_slice(&(track_bodies.len() as u16).to_be_bytes());
    bytes.extend_from_slice(&division.to_be_bytes());
    for body in track_bodies {
        bytes.extend_from_slice(b"MTrk");
        let mut track = Vec::new();
        track.extend_from_slice(body);
        track.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]); // end of track
        bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&track);
    }
    bytes
}

/// Format 1, two tracks, a tempo change on track 0 and running status on both tracks.
fn hand_assembled_smf() -> Vec<u8> {
    let track0: &[u8] = &[
        0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, // set_tempo 500000us (120 BPM) at tick 0
        0x60, 0x90, 60, 100, // note on, tick 96
        0x30, 60, 0, // note off via running status, tick 144
        0x30, 64, 96, // note on via running status (channel 0, note 64), tick 192
        0x60, 64, 0, // note off via running status, tick 288
    ];
    let track1: &[u8] = &[
        0x18, 0x91, 67, 90, // note on channel 1, tick 24
        0x81, 0x10, 67, 0, // note off via running status, tick 24 + 144 = 168 (VLQ(144) = 0x81 0x10)
    ];
    smf_bytes(1, 96, &[track0, track1])
}

/// The pinned synthetic IT fixture's instruments, exactly as
/// `render_allocation.rs`'s `rendering_a_midi_source_allocates_nothing` uses them.
fn instruments_module_bytes() -> Vec<u8> { starplayer_offline::fixtures::synthetic_it() }

#[test]
fn a_hand_assembled_smf_renders_byte_identically_at_every_block_size() {
    let smf = hand_assembled_smf();
    let instruments = instruments_module_bytes();

    let reference = starplayer_offline::render_smf_song::<FixedPath, Linear, MonoI16>(&smf, &instruments, SAMPLE_RATE_HZ, BLOCK_SIZES[0])
        .expect("the hand-assembled file renders through the synthetic IT fixture's instruments");
    assert!(!reference.is_empty(), "the file must produce some audio to make this a real test");

    for &host_block_frames in &BLOCK_SIZES[1..] {
        let rendered = starplayer_offline::render_smf_song::<FixedPath, Linear, MonoI16>(&smf, &instruments, SAMPLE_RATE_HZ, host_block_frames)
            .unwrap_or_else(|error| panic!("block size {host_block_frames} failed to render: {error}"));
        assert_eq!(rendered, reference, "block size {host_block_frames} must be byte-identical to block size {}", BLOCK_SIZES[0]);
    }
}
