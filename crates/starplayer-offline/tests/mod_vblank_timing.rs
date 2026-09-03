//! M2-C10: the CIA-versus-VBlank resolver, end to end through `starplayer::scan_song`.
//!
//! Every module here is synthesised by this file. The MOD that motivated the task —
//! `K-P-K.MOD`, whose `F20` fermata under the ProTracker CIA rule sets 32 BPM for the last
//! 61 orders and turns a 10.7-minute song into a 29-minute one — is third-party and is not
//! in the repository; the *fermata* fixture below is the same shape in miniature.

use starplayer::core::quirks::{ModTiming, QuirkSelection, QuirkSet};
use starplayer::engine::ScanLimits;
use starplayer::mod_file::{ModCell, TimingVerdict, timing_verdict_for};
use starplayer::model::Module;
use starplayer::rt::Arc;
use starplayer_offline::{scanned_song, song_timeline};

const SAMPLE_RATE_HZ: u32 = 44_100;
const HEADER_BYTES: usize = 1084;
const CHANNELS: usize = 4;
const ROWS: usize = 64;
const CELL_BYTES: usize = 4;
const PATTERN_BYTES: usize = ROWS * CHANNELS * CELL_BYTES;
const SAMPLE_FRAMES: usize = 256;
/// One tick at the default 125 BPM and 44.1 kHz: `44100 * 2.5 / 125`.
const TICK_FRAMES: u64 = 882;

/// A four-channel MOD with one looping sample, an explicit tag, an explicit order list and
/// cells placed at `(pattern, row, channel)`.
fn synthetic_mod(tag: &[u8; 4], orders: &[u8], patterns: usize, cells: &[(usize, usize, usize, ModCell)]) -> Vec<u8> {
    let mut bytes = vec![0; HEADER_BYTES + patterns * PATTERN_BYTES + SAMPLE_FRAMES];
    // Sample 1: a looping 256-byte block, so a voice never runs out during the test.
    bytes[42..44].copy_from_slice(&((SAMPLE_FRAMES / 2) as u16).to_be_bytes());
    bytes[45] = 64;
    bytes[48..50].copy_from_slice(&((SAMPLE_FRAMES / 2) as u16).to_be_bytes());
    bytes[950] = orders.len() as u8;
    bytes[952..952 + orders.len()].copy_from_slice(orders);
    bytes[1080..1084].copy_from_slice(tag);
    for (pattern, row, channel, cell) in cells {
        let offset = HEADER_BYTES + pattern * PATTERN_BYTES + (row * CHANNELS + channel) * CELL_BYTES;
        bytes[offset..offset + CELL_BYTES].copy_from_slice(&cell.to_bytes());
    }
    bytes
}

fn speed(param: u8) -> ModCell { ModCell { effect: 0xF, param, ..ModCell::EMPTY } }
fn note(period: u16) -> ModCell { ModCell { period, instrument: 1, ..ModCell::EMPTY } }
fn note_with(period: u16, param: u8) -> ModCell { ModCell { period, instrument: 1, effect: 0xF, param } }

fn load(bytes: &[u8]) -> Arc<Module> { Arc::new(starplayer::mod_file::load(bytes).expect("a loadable MOD")) }

/// The length of one pass under an explicitly chosen timing, for comparison against what
/// the resolver picked on its own.
fn end_frame_under(module: &Arc<Module>, timing: ModTiming) -> u64 {
    let quirks = QuirkSet { mod_timing: timing, ..module.header().dialect.quirks() };
    let mut sequencer = starplayer::mod_file::sequencer_with_quirks(Arc::clone(module), SAMPLE_RATE_HZ, QuirkSelection::Override(quirks));
    starplayer::engine::scan_timeline(&mut sequencer, ScanLimits::for_rate(SAMPLE_RATE_HZ)).end_frame()
}

/// The task's motivating shape: a fermata written as `F20` on the last row of a section,
/// under a chord, with the next pattern immediately restoring `F04`. Thirty orders of it.
///
/// Read as CIA that is 32 BPM for twenty-nine orders — about ten minutes. Read as VBlank it
/// is one 32-tick row and then the song's own speed, about two and a half.
fn fermata_mod() -> Vec<u8> {
    let mut orders = vec![0u8];
    orders.extend(core::iter::repeat_n(1u8, 29));
    synthetic_mod(b"M.K.", &orders, 2, &[
        (0, 0, 0, speed(0x04)),
        (0, 63, 0, note_with(428, 0x20)),
        (0, 63, 1, note(508)),
        (0, 63, 2, note(570)),
        (0, 63, 3, note(640)),
        (1, 0, 0, speed(0x04)),
    ])
}

#[test]
fn a_fermata_module_is_recognised_as_vblank_by_the_length_comparison() {
    let bytes = fermata_mod();
    let module = load(&bytes);
    assert_eq!(timing_verdict_for(&module), Some(TimingVerdict::CompareLengths), "one high Fxx, no mixed row, fewer than eight orders' worth of end silence");

    let cia_frames = end_frame_under(&module, ModTiming::Cia);
    let vblank_frames = end_frame_under(&module, ModTiming::VBlank);
    assert!(cia_frames > 480 * SAMPLE_RATE_HZ as u64, "the CIA reading is past libxmp's eight-minute threshold ({} s)", cia_frames / SAMPLE_RATE_HZ as u64);
    assert!(vblank_frames < cia_frames / 3, "and the VBlank reading is far shorter ({} s)", vblank_frames / SAMPLE_RATE_HZ as u64);

    let scanned = scanned_song(&module, SAMPLE_RATE_HZ).expect("the fixture scans");
    assert_eq!(scanned.quirks.mod_timing, ModTiming::VBlank, "the resolver kept the shorter pass");
    assert_eq!(scanned.timeline.end_frame(), vblank_frames);
    assert_eq!(song_timeline(&module, SAMPLE_RATE_HZ).expect("scans").end_frame(), vblank_frames, "the offline wrapper reports the same length");

    // The row arrivals, not the audio: the fermata row lasts 32 ticks and the row after it
    // is back to the song's own four.
    let held = scanned.timeline.mark_at(0, 63).expect("the fermata row is played").frame;
    let after = scanned.timeline.mark_at(1, 0).expect("the next order's first row is played").frame;
    let next = scanned.timeline.mark_at(1, 1).expect("and the row after that").frame;
    assert_eq!(after - held, 32 * TICK_FRAMES, "F20 is a 32-tick row on the vertical blank");
    assert_eq!(next - after, 4 * TICK_FRAMES, "and the following rows are back to F04");
}

#[test]
fn one_mixed_row_makes_the_same_module_cia() {
    // The same fixture with an `F06` and an `F7D` on one row: two different meanings for
    // the same command byte on one row is only possible on a CIA tracker.
    let mut orders = vec![0u8];
    orders.extend(core::iter::repeat_n(1u8, 29));
    let bytes = synthetic_mod(b"M.K.", &orders, 2, &[
        (0, 0, 0, speed(0x04)),
        (0, 63, 0, note_with(428, 0x20)),
        (0, 63, 1, note(508)),
        (0, 63, 2, note(570)),
        (0, 63, 3, note(640)),
        (1, 0, 0, speed(0x04)),
        (1, 10, 0, speed(0x06)),
        (1, 10, 2, speed(0x7D)),
    ]);
    let module = load(&bytes);
    assert_eq!(timing_verdict_for(&module), Some(TimingVerdict::Cia), "a mixed row cancels the comparison outright");

    let scanned = scanned_song(&module, SAMPLE_RATE_HZ).expect("the fixture scans");
    assert_eq!(scanned.quirks.mod_timing, ModTiming::Cia);
    assert_eq!(scanned.timeline.end_frame(), end_frame_under(&module, ModTiming::Cia), "the CIA scan is the one that was kept");
}

/// A deliberately slow short song must not be sped up: the comparison is only reached when
/// the CIA reading is already past eight minutes.
#[test]
fn a_genuinely_slow_short_module_stays_cia() {
    let bytes = synthetic_mod(b"M.K.", &[0, 1], 2, &[(0, 0, 0, speed(0x20)), (0, 0, 1, note(428))]);
    let module = load(&bytes);
    assert_eq!(timing_verdict_for(&module), Some(TimingVerdict::CompareLengths));

    let cia_frames = end_frame_under(&module, ModTiming::Cia);
    assert!(cia_frames < 480 * SAMPLE_RATE_HZ as u64, "the whole song is under the threshold at 32 BPM ({} s)", cia_frames / SAMPLE_RATE_HZ as u64);

    let scanned = scanned_song(&module, SAMPLE_RATE_HZ).expect("the fixture scans");
    assert_eq!(scanned.quirks.mod_timing, ModTiming::Cia, "under the threshold there is no second scan and the file is taken at its word");
    assert_eq!(scanned.timeline.end_frame(), cia_frames);
}

/// libxmp's end-silence rule: at least eight orders, and the only high `Fxx` in a pattern
/// that only the last two orders play. No comparison is run at all.
#[test]
fn an_end_silence_module_is_vblank_without_a_comparison() {
    let bytes = synthetic_mod(b"M.K.", &[0, 0, 0, 0, 0, 0, 0, 1], 2, &[(0, 0, 1, note(428)), (1, 0, 0, speed(0x30))]);
    let module = load(&bytes);
    assert_eq!(timing_verdict_for(&module), Some(TimingVerdict::VBlank));

    let scanned = scanned_song(&module, SAMPLE_RATE_HZ).expect("the fixture scans");
    assert_eq!(scanned.quirks.mod_timing, ModTiming::VBlank);
    assert_eq!(scanned.timeline.end_frame(), end_frame_under(&module, ModTiming::VBlank));
    // Short enough that the length comparison would never have been reached anyway, which
    // is exactly why the rule exists.
    assert!(scanned.timeline.end_frame() < 480 * SAMPLE_RATE_HZ as u64);
}

/// NoiseTracker has no CIA timer, so its two tags settle the question from the header.
#[test]
fn a_noisetracker_tag_is_vblank_from_the_header_alone() {
    let bytes = synthetic_mod(b"M&K!", &[0, 1], 2, &[(0, 0, 0, speed(0x30)), (0, 0, 1, note(428))]);
    let module = load(&bytes);
    assert_eq!(module.header().dialect, starplayer::core::quirks::FormatDialect::Noisetracker);
    assert_eq!(timing_verdict_for(&module), Some(TimingVerdict::VBlank));

    let scanned = scanned_song(&module, SAMPLE_RATE_HZ).expect("the fixture scans");
    assert_eq!(scanned.quirks.mod_timing, ModTiming::VBlank);
    assert_eq!(scanned.timeline.end_frame(), end_frame_under(&module, ModTiming::VBlank));

    // The same bytes under the ProTracker tag are a short module the comparison leaves
    // alone, so the tag really is what made the difference.
    let mut protracker = bytes.clone();
    protracker[1080..1084].copy_from_slice(b"M.K.");
    let protracker = load(&protracker);
    assert_eq!(timing_verdict_for(&protracker), Some(TimingVerdict::CompareLengths));
    assert_eq!(scanned_song(&protracker, SAMPLE_RATE_HZ).expect("scans").quirks.mod_timing, ModTiming::Cia);
}

/// S3M and MTM are CIA only and go through the same entry point unchanged.
#[test]
fn the_other_formats_resolve_their_dialect_and_scan_once() {
    let s3m = Arc::new(starplayer::s3m::load(include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M")).expect("REFLEX loads"));
    let scanned = scanned_song(&s3m, SAMPLE_RATE_HZ).expect("REFLEX scans");
    assert_eq!(scanned.quirks, s3m.header().dialect.quirks(), "an S3M's quirks are its dialect's, untouched");
    assert_eq!(scanned.quirks.mod_timing, ModTiming::Cia);
    assert_eq!(scanned.timeline, song_timeline(&s3m, SAMPLE_RATE_HZ).expect("scans"));
}

/// C10 research point 3: a CIA scan that runs out its budget never reached the end of one
/// pass, so it is past the threshold by definition and the rescan happens anyway. The
/// rescan gets its own fresh budget; a module that is over budget *both* ways is not
/// compared on two truncated lengths and keeps CIA.
#[test]
fn a_cia_scan_that_runs_out_of_budget_is_rescanned_and_a_hopeless_one_keeps_cia() {
    let module = load(&fermata_mod());
    let vblank_frames = end_frame_under(&module, ModTiming::VBlank);

    // A leash the CIA reading cannot finish inside but the VBlank reading can.
    let between = ScanLimits { max_frames: 200 * SAMPLE_RATE_HZ as u64, max_ticks: 1_000_000 };
    let scanned = starplayer::scan_song(&module, SAMPLE_RATE_HZ, between).expect("the fixture scans");
    // The fixture's order list runs out rather than jumping back, so a pass that finishes
    // inside the leash *ends* rather than looping (task D2). What matters here is only
    // that it is not a `Budget` end.
    assert_eq!(scanned.timeline.end(), starplayer::engine::EndReason::Ended, "the VBlank pass finished inside the leash");
    assert_eq!(scanned.quirks.mod_timing, ModTiming::VBlank);
    assert_eq!(scanned.timeline.end_frame(), vblank_frames, "and it is the same pass a full budget produces");

    // A leash neither reading can finish inside.
    let hopeless = ScanLimits { max_frames: 100 * SAMPLE_RATE_HZ as u64, max_ticks: 1_000_000 };
    let scanned = starplayer::scan_song(&module, SAMPLE_RATE_HZ, hopeless).expect("the fixture scans");
    assert_eq!(scanned.timeline.end(), starplayer::engine::EndReason::Budget);
    assert_eq!(scanned.quirks.mod_timing, ModTiming::Cia, "two truncated lengths are not a comparison");
}
