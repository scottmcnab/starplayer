//! Nonzero XM restart positions must not shorten the initial timeline.
#![cfg(feature = "xm")]

use starplayer::core::{AtEnd, ExactFixedPoint, Frame};
use starplayer::core::quirks::QuirkSelection;
use starplayer::engine::{ChannelTable, ControlClock, EndReason, EngineContext, EventSource, ScanLimits, SongPosition, Visit};
use starplayer::mixer::VoicePool;
use starplayer::model::{FormatDialect, Module, ModuleBuilder, ModuleFormat, ModuleHeader};
use starplayer::rt::Arc;
use starplayer_xm::XmFormatExtra;

const SAMPLE_RATE_HZ: u32 = 1000;
const ROW_FRAMES: u64 = 60; // Three 20 ms ticks at 125 BPM.

fn module() -> Arc<Module> {
    let mut builder = ModuleBuilder::new();
    for pattern in 0..3 {
        let mut cells = vec![0; 4 * 2 * 5];
        if pattern == 0 {
            // A short introduction: D00 on row 1 skips rows 2 and 3 of this pattern.
            cells[13] = 0x0d;
        }
        builder.add_pattern(&cells, 4, 2).expect("a valid XM pattern");
    }
    builder.set_orders(&[0, 1, 2]);
    builder.set_header(ModuleHeader {
        dialect: FormatDialect::FastTracker2,
        initial_speed: 3,
        initial_tempo: 125,
        format_extra: XmFormatExtra { restart_position: 2, flags: 0 }.encode(),
        ..ModuleHeader::new(ModuleFormat::Xm, 2)
    });
    Arc::new(builder.build().expect("valid synthetic XM"))
}

#[test]
fn public_scan_and_live_playback_include_the_intro_before_a_nonzero_restart() {
    let module = module();
    let scanned = starplayer::scan_song(&module, SAMPLE_RATE_HZ, ScanLimits::for_rate(SAMPLE_RATE_HZ)).expect("scan succeeds");
    let fallible = starplayer::try_scan_song(&module, SAMPLE_RATE_HZ, ScanLimits::for_rate(SAMPLE_RATE_HZ)).expect("fallible scan succeeds");
    assert_eq!(fallible, scanned);
    assert_eq!(scanned.timeline.end(), EndReason::Ended);
    assert_eq!(scanned.timeline.end_frame(), 10 * ROW_FRAMES, "two introduction rows, then two four-row patterns");
    assert_eq!(scanned.timeline.frame_at(0, 0), Some(0));
    assert_eq!(scanned.timeline.frame_at(0, 2), None, "D00 skips the introduction's tail");
    assert_eq!(scanned.timeline.frame_at(1, 0), Some(2 * ROW_FRAMES));
    assert_eq!(scanned.timeline.frame_at(2, 0), Some(6 * ROW_FRAMES));

    let mut source = starplayer::NativeSequencer::new(module, SAMPLE_RATE_HZ, QuirkSelection::Override(scanned.quirks)).expect("live sequencer");
    source.set_timeline(scanned.timeline);
    source.set_at_end(AtEnd::Stop);
    let mut voices = VoicePool::new(2);
    let mut channels = ChannelTable::new(2);
    let mut control = ControlClock::new(SAMPLE_RATE_HZ, Frame::ZERO);
    let mut first_position = None;
    let mut ended_at = None;
    for _ in 0..64 {
        let Some(frame) = source.next_event_frame() else { break };
        source.dispatch(frame, &mut EngineContext::new(frame, &mut voices, &mut channels, &mut control));
        if let starplayer::NativeSequencer::Xm(sequencer) = &source
            && let Some(visit) = sequencer.last_row_visit() {
            first_position.get_or_insert((visit.mark.order, visit.mark.row));
            if source.end_reached() {
                assert_eq!(visit.visit, Visit::Wrapped);
                assert_eq!(sequencer.position(), SongPosition { order: 2, pattern: 2, row: 0 }, "only the wrap uses the restart order");
                ended_at = Some(frame.0);
            }
        }
    }
    assert_eq!(first_position, Some((0, 0)), "live playback starts at order zero without a corrective seek");
    assert_eq!(ended_at, Some(10 * ROW_FRAMES));
    assert_eq!(source.next_event_frame(), None, "AtEnd::Stop ends on the scanned frame");
}

#[test]
fn both_xm_crate_constructors_begin_before_the_restart_position() {
    let module = module();
    let direct = starplayer_xm::sequencer_for(Arc::clone(&module), SAMPLE_RATE_HZ, ExactFixedPoint);
    let selected = starplayer_xm::sequencer_with_quirks(module, SAMPLE_RATE_HZ, QuirkSelection::FromDialect);
    let expected = SongPosition { order: 0, pattern: 0, row: 0 };
    assert_eq!(direct.position(), expected);
    assert_eq!(selected.position(), expected);
}
