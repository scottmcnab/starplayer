//! Byte-level fuzzing of the Standard MIDI File parser (task E5 deliverable 2).

#![no_main]

use libfuzzer_sys::fuzz_target;
use starplayer_fuzz::arm_memory_cap;
use starplayer_midi::smf::{parse_smf, probe};

fuzz_target!(|data: &[u8]| {
    arm_memory_cap(data.len());

    let probed = probe(data);
    if let Ok(smf) = parse_smf(data) {
        assert!(probed, "a file the parser accepted must also probe as an SMF");

        // The tempo-map conversion — Q32.32 remainder-carry arithmetic over whatever
        // ticks and tempo changes the file declared — must never panic or overflow, at
        // any sample rate, however pathological the file's tick values are.
        for sample_rate_hz in [1u32, 8_000, 44_100, 192_000, u32::MAX] {
            let frames = smf.to_frames(sample_rate_hz);
            let mut previous_frame = None;
            for event in &frames {
                let frame = event.frame.get();
                if let Some(previous) = previous_frame {
                    assert!(frame >= previous, "to_frames must stay non-decreasing: it walks events already sorted by tick");
                }
                previous_frame = Some(frame);
            }
            let _ = smf.length_frames(sample_rate_hz);
        }
    }
});
