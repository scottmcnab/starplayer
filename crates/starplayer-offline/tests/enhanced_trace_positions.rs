#![cfg(feature = "trace")]
//! M10-K5a deliverable 2's proof: enhancing a module shifts every per-tick sample
//! position by the enhancement's own factor **exactly**, and moves nothing else.
//!
//! # Why this is the right test
//!
//! The playback scale touches three things — the step a voice is triggered with, the step
//! a repitch writes, and the frame a sample-offset command starts at — and every one of
//! them shows up in the trace's `pos=` column and nowhere else. If the step were scaled
//! but the offset were not, a `9xx`/`Oxx` that lands past the end of the plain sample
//! would land *inside* the quadrupled one and the position would diverge on the first
//! tick. If a loop's points did not scale exactly, the divergence would appear at the
//! first wrap. And if anything else had moved — the note, the instrument, the volume, the
//! period, the pan — this would catch that too, because those columns are compared for
//! equality rather than for a shift.
//!
//! The trace engine interpolates with `Linear`, whose `LEADING_FRAMES` is zero, so a
//! forward loop wraps exactly at `loop_end` and the wrap scales exactly. A symmetric
//! kernel defers the wrap by a fixed number of *frames*, which does not scale; that is a
//! rendering difference, not a sequencing one, and it is why the invariant is stated
//! against the trace rather than against a render.

use starplayer::rt::Arc;
use starplayer_enhance::{SincUpsampler, UpsampleFactor};
use starplayer::model::Module;
use starplayer_offline::{TraceOptions, fixtures, trace_loaded_module};

const REFLEX: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M");
/// PETRI uses 146 `Oxx` sample-offset commands against real samples, so it is the S3M case
/// where the offset scaling is actually exercised in range rather than past the end.
const PETRI: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/PETRI.S3M");

/// Enough ticks to reach every fixture's loops and offsets without tracing a whole song.
const TICKS: usize = 900;

fn options() -> TraceOptions { TraceOptions { ticks: Some(TICKS), host_block_frames: 128 } }

/// Trace `module` plain and 4x-enhanced, and assert the enhanced positions are the plain
/// ones shifted left by two with every other column untouched.
fn positions_scale_by_four(name: &str, bytes: &[u8]) {
    let module = starplayer::load(bytes).unwrap_or_else(|error| panic!("{name} loads: {error:?}"));
    let enhanced = module.enhanced(&SincUpsampler::new(UpsampleFactor::Four)).expect("a 4x rebuild");
    assert!(enhanced.samples().iter().any(|sample| sample.rate_scale_log2() == 2), "{name}: the rebuild has to have scaled something");

    let plain_trace = trace_loaded_module(Arc::new(module), options()).unwrap_or_else(|error| panic!("{name} traces: {error:?}"));
    let enhanced_trace = trace_loaded_module(Arc::new(enhanced), options()).unwrap_or_else(|error| panic!("{name} traces enhanced: {error:?}"));

    assert_eq!(enhanced_trace.ticks.len(), plain_trace.ticks.len(), "{name}: the two traces have to cover the same ticks");
    assert!(plain_trace.ticks.len() > 32, "{name}: a trace of {} ticks proves nothing", plain_trace.ticks.len());

    let mut positions_seen = 0usize;
    for (plain, enhanced) in plain_trace.ticks.iter().zip(enhanced_trace.ticks.iter()) {
        let tick = plain.tick;
        assert_eq!(enhanced.frame, plain.frame, "{name} tick {tick}: the tick landed on a different frame");
        assert_eq!(enhanced.position, plain.position, "{name} tick {tick}: the song position moved");
        assert_eq!((enhanced.speed, enhanced.bpm, enhanced.global_volume), (plain.speed, plain.bpm, plain.global_volume), "{name} tick {tick}: the timing moved");
        assert_eq!(enhanced.channels.len(), plain.channels.len(), "{name} tick {tick}: the channel count moved");
        assert_eq!(enhanced.voices.len(), plain.voices.len(), "{name} tick {tick}: the background voice count moved");

        for (plain, enhanced) in plain.channels.iter().zip(enhanced.channels.iter()) {
            let channel = plain.channel;
            assert_eq!(enhanced.active, plain.active, "{name} tick {tick} ch {channel}: activity moved");
            assert_eq!(enhanced.note, plain.note, "{name} tick {tick} ch {channel}: the note moved");
            assert_eq!(enhanced.instrument, plain.instrument, "{name} tick {tick} ch {channel}: the instrument moved");
            assert_eq!(enhanced.sample, plain.sample, "{name} tick {tick} ch {channel}: the sample moved");
            assert_eq!(enhanced.volume, plain.volume, "{name} tick {tick} ch {channel}: the volume moved");
            assert_eq!(enhanced.period, plain.period, "{name} tick {tick} ch {channel}: the period moved");
            assert_eq!(enhanced.pan, plain.pan, "{name} tick {tick} ch {channel}: the pan moved");
            assert_eq!((enhanced.cutoff, enhanced.resonance), (plain.cutoff, plain.resonance), "{name} tick {tick} ch {channel}: the filter moved");
            assert_eq!(enhanced.flags, plain.flags, "{name} tick {tick} ch {channel}: the dirty flags moved");
            assert_eq!(enhanced.position, plain.position << 2, "{name} tick {tick} ch {channel}: position {:#x} is not {:#x} << 2", enhanced.position, plain.position);
            if plain.position != 0 {
                positions_seen += 1;
            }
        }

        for (plain, enhanced) in plain.voices.iter().zip(enhanced.voices.iter()) {
            let voice = plain.voice;
            assert_eq!(enhanced.root, plain.root, "{name} tick {tick} vc {voice}: the root channel moved");
            assert_eq!(enhanced.note, plain.note, "{name} tick {tick} vc {voice}: the note moved");
            assert_eq!(enhanced.sample, plain.sample, "{name} tick {tick} vc {voice}: the sample moved");
            assert_eq!(enhanced.volume, plain.volume, "{name} tick {tick} vc {voice}: the volume moved");
            assert_eq!(enhanced.period, plain.period, "{name} tick {tick} vc {voice}: the period moved");
            assert_eq!(enhanced.position, plain.position << 2, "{name} tick {tick} vc {voice}: position {:#x} is not {:#x} << 2", enhanced.position, plain.position);
        }
    }
    assert!(positions_seen > 100, "{name}: only {positions_seen} non-zero positions were compared");
}

/// The same, at 2x, so the invariant is a shift by the recorded exponent rather than a
/// coincidence of the number four.
fn positions_scale_by_two(name: &str, bytes: &[u8]) {
    let module = starplayer::load(bytes).unwrap_or_else(|error| panic!("{name} loads: {error:?}"));
    let enhanced = module.enhanced(&SincUpsampler::new(UpsampleFactor::Two)).expect("a 2x rebuild");
    let plain_trace = trace_loaded_module(Arc::new(module), options()).expect("a plain trace");
    let enhanced_trace = trace_loaded_module(Arc::new(enhanced), options()).expect("an enhanced trace");

    assert_eq!(enhanced_trace.ticks.len(), plain_trace.ticks.len(), "{name}: the two traces have to cover the same ticks");
    for (plain, enhanced) in plain_trace.ticks.iter().zip(enhanced_trace.ticks.iter()) {
        let tick = plain.tick;
        for (plain, enhanced) in plain.channels.iter().zip(enhanced.channels.iter()) {
            assert_eq!(enhanced.position, plain.position << 1, "{name} tick {tick} ch {}: position is not the plain one doubled", plain.channel);
        }
    }
}

#[test]
fn a_mod_traces_the_same_song_with_every_position_shifted() {
    positions_scale_by_four("mod", &fixtures::synthetic_mod());
}

#[test]
fn an_mtm_traces_the_same_song_with_every_position_shifted() {
    positions_scale_by_four("mtm", &fixtures::synthetic_mtm());
}

#[test]
fn an_s3m_with_in_range_sample_offsets_traces_the_same_song_with_every_position_shifted() {
    positions_scale_by_four("s3m/petri", PETRI);
}

#[test]
fn an_s3m_with_loops_and_portamento_traces_the_same_song_with_every_position_shifted() {
    positions_scale_by_four("s3m/reflex", REFLEX);
}

#[test]
fn an_xm_traces_the_same_song_with_every_position_shifted() {
    positions_scale_by_four("xm", &fixtures::synthetic_xm());
}

#[test]
fn an_it_with_an_in_range_sample_offset_traces_the_same_song_with_every_position_shifted() {
    positions_scale_by_four("it", &fixtures::synthetic_it_with_offset());
}

#[test]
fn a_two_times_rebuild_shifts_by_one_rather_than_by_two() {
    positions_scale_by_two("s3m/petri", PETRI);
    positions_scale_by_two("mod", &fixtures::synthetic_mod());
}

/// The control: with no enhancement at all, the two traces are the same text. This is what
/// says the harness above would notice a difference if there were one.
#[test]
fn an_unenhanced_rebuild_traces_identically() {
    for (name, bytes) in [("mod", fixtures::synthetic_mod()), ("it", fixtures::synthetic_it_with_offset())] {
        let module = starplayer::load(&bytes).expect("the fixture loads");
        let rebuilt = module.enhanced(&SincUpsampler::new(UpsampleFactor::Four).with_rate_ceiling(1)).expect("a rebuild that declines every sample");
        let before = trace_loaded_module(Arc::new(module), options()).expect("a plain trace");
        let after = trace_loaded_module(Arc::new(rebuilt), options()).expect("a rebuilt trace");
        assert_eq!(after.to_text(), before.to_text(), "{name}: a rebuild that changes no sample must trace identically");
    }
}

/// The one site where a position is deliberately **quantised** rather than carried: IT's
/// sustain-loop release truncates the voice's Q32.32 cursor to a whole frame
/// (`SusAfterLoop.it`, `processor.rs`'s `release_voice`). A whole frame of a quadrupled
/// sample is a quarter of a whole frame of the original, so the enhanced position lands on
/// the nearest quadrupled frame at or after `4 × plain` — never further than three frames
/// away, and never behind.
///
/// This is unit-consistent, not proportional, and it is the honest bound. Everything else
/// in the trace still matches exactly, which is what the assertions below say.
#[test]
fn an_it_sustain_loop_release_quantises_its_position_to_a_whole_frame() {
    let module = starplayer::load(&fixtures::synthetic_it()).expect("the synthetic IT loads");
    let enhanced = module.enhanced(&SincUpsampler::new(UpsampleFactor::Four)).expect("a 4x rebuild");
    let plain_trace = trace_loaded_module(Arc::new(module), options()).expect("a plain trace");
    let enhanced_trace = trace_loaded_module(Arc::new(enhanced), options()).expect("an enhanced trace");

    assert_eq!(enhanced_trace.ticks.len(), plain_trace.ticks.len());
    let mut exact = 0usize;
    let mut quantised = 0usize;
    for (plain, enhanced) in plain_trace.ticks.iter().zip(enhanced_trace.ticks.iter()) {
        let tick = plain.tick;
        let rows = plain.channels.iter().map(|row| (row.note, row.sample, row.volume, row.period, row.pan, row.flags));
        let enhanced_rows = enhanced.channels.iter().map(|row| (row.note, row.sample, row.volume, row.period, row.pan, row.flags));
        assert!(rows.eq(enhanced_rows), "{name} tick {tick}: a column other than the position moved", name = "it");

        let positions = plain.channels.iter().map(|row| row.position).chain(plain.voices.iter().map(|row| row.position));
        let enhanced_positions = enhanced.channels.iter().map(|row| row.position).chain(enhanced.voices.iter().map(|row| row.position));
        for (plain, enhanced) in positions.zip(enhanced_positions) {
            let scaled = plain << 2;
            match enhanced == scaled {
                true => exact += 1,
                false => {
                    quantised += 1;
                    let drift = enhanced.checked_sub(scaled).unwrap_or_else(|| panic!("tick {tick}: the enhanced position {enhanced:#x} is behind {scaled:#x}"));
                    assert!(drift < 4u64 << 32, "tick {tick}: the enhanced position drifted {} frames past the scaled one", drift >> 32);
                }
            }
        }
    }
    assert!(exact > 100, "only {exact} positions matched exactly");
    assert!(quantised > 0, "this fixture is supposed to reach the sustain-release quantisation");
}

/// A sanity check on the fixtures themselves: the modules this file traces really do use a
/// sample-offset command, so the offset half of the scaling is under test.
#[test]
fn the_offset_fixtures_really_carry_an_offset_command() {
    let it = starplayer::load(&fixtures::synthetic_it_with_offset()).expect("the offset IT loads");
    assert!(pattern_uses_command(&it, starplayer::model::it_command_code(b'O')), "the IT fixture must carry an Oxx");
    let petri = starplayer::load(PETRI).expect("PETRI loads");
    assert!(pattern_uses_command(&petri, starplayer::model::s3m_command_code(b'O')), "PETRI must carry an Oxx");
}

/// Both S3M and IT store an unpacked `note, instrument, volume, command, parameter` cell
/// per channel in the module blob, so one scan serves both.
fn pattern_uses_command(module: &Module, command: u8) -> bool {
    module.patterns().iter().enumerate().any(|(index, pattern)| {
        let Some(bytes) = module.pattern_bytes(starplayer::model::PatternId(index as u16)) else { return false };
        let stride = bytes.len() / (pattern.rows() as usize * pattern.channels() as usize).max(1);
        stride >= 4 && bytes.chunks(stride).any(|cell| cell.get(3) == Some(&command))
    })
}
