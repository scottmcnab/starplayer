//! Deterministic Impulse Tracker workload for the A1S voice-capacity benchmark.

extern crate alloc;

use alloc::vec::Vec;

use starplayer::core::{Error, I1F15, U0F16};
use starplayer::it::{ItCell, ItFormatExtra};
use starplayer::model::{InstrumentDef, Module, ModuleBuilder, ModuleFormat, ModuleHeader, NewNoteAction, SampleSpec};

/// The widest native IT pattern supported by the engine.
pub const MAX_CHANNELS: usize = 64;
/// IT's own virtual-voice quota. The extra global slots are reserved for live input and
/// are outside this benchmark.
pub const MAX_VOICES: usize = 256;
/// Rows in the repeating stress pattern.
pub const STRESS_ROWS: usize = 64;
/// Source frames in the looped, non-silent sample.
pub const SAMPLE_FRAMES: usize = 1_024;
/// Quiet boot interval before firmware constructs the workload or emits machine records.
/// This gives the runner time to capture the boot after its attached application reset.
pub const CAPTURE_GRACE_SECONDS: u64 = 3;
/// Periodic active-voice checks before this elapsed time are workload warm-up.
pub const VOICE_PLATEAU_SETTLE_MS: u64 = 5_000;
/// Web clients may still be connecting before this elapsed time. Firmware records their
/// counters immediately, but only requires positive progress from this point onward.
pub const WEB_LOAD_SETTLE_MS: u64 = 5_000;
/// Maximum elapsed time for which a web-load counter may remain unchanged. Equality is
/// accepted; a counter is stale only once its last observed advance is more than 5 s old.
pub const WEB_LOAD_STALL_MS: u64 = 5_000;
/// Fixed claim for the generated module image. The maximum stress image is guarded by a
/// host test so firmware can reserve this before off-stack construction begins.
pub const IMAGE_BUFFER_BYTES: usize = 32 * 1024;
/// Free internal heap required before a web benchmark starts the radio or its RTOS tasks.
///
/// The web personality adds 48 KiB specifically for radio dynamic allocations. Requiring
/// that allowance plus the benchmark's retained 8 KiB floor catches a fragmented or
/// already-consumed heap before esp-rtos reaches its infallible task-stack allocator.
pub const WEB_NETWORK_PREFLIGHT_HEAP_BYTES: usize = (48 + 8) * 1024;

/// Whether web network startup has both its dynamic allowance and the retained heap floor.
pub const fn web_network_preflight_passes(free_internal_bytes: usize) -> bool {
    free_internal_bytes >= WEB_NETWORK_PREFLIGHT_HEAP_BYTES
}

/// One workload configuration.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct StressConfig {
    /// Active pattern channels, `1..=64`.
    pub channels: usize,
    /// Voice-pool capacity exercised by repeated NNA notes, `1..=256`.
    pub voices: usize,
    /// Enable IT's fixed resonant low-pass on every voice.
    pub filtered: bool,
}

impl StressConfig {
    /// The first candidate required by I9.
    pub const MAXIMUM: StressConfig = StressConfig { channels: MAX_CHANNELS, voices: MAX_VOICES, filtered: false };

    /// Validate the native IT bounds.
    pub const fn is_valid(self) -> bool {
        self.channels > 0 && self.channels <= MAX_CHANNELS && self.voices > 0 && self.voices <= MAX_VOICES
    }
}

/// Build a native instrument-mode IT module which reaches and holds `config.voices`.
///
/// Every row retriggers every channel with NNA Continue. Once the caller's pool is full,
/// IT's normal background-voice selection steals one voice for each new note and leaves
/// the active count on the requested plateau. The final row jumps to row zero, making the
/// workload indefinite. Notes and header pans are distributed deterministically.
pub fn stress_module(config: StressConfig) -> Result<Module, Error> {
    if !config.is_valid() {
        return Err(Error::Invalid("voice-bench channels or voices are outside IT's limits"));
    }

    let amplitude = (24_000usize / config.voices).max(1) as i16;
    let pcm: Vec<i16> = (0..SAMPLE_FRAMES)
        .map(|frame| {
            let phase = (frame % 128) as i16;
            let triangle = if phase < 64 { phase * 2 - 63 } else { 191 - phase * 2 };
            (i32::from(triangle) * i32::from(amplitude) / 64) as i16
        })
        .collect();

    let mut builder = ModuleBuilder::new();
    let sample = builder.add_sample(&pcm, SampleSpec::one_shot("voice bench loop").with_forward_loop(0, SAMPLE_FRAMES as u32))?;
    let mut instrument = InstrumentDef::from_sample("voice bench", sample, U0F16::MAX);
    instrument.sample = None;
    instrument.note_sample_map = [1; starplayer::model::NOTE_MAP_LENGTH];
    for (note, mapped) in instrument.note_transpose_map.iter_mut().enumerate() {
        *mapped = note as u8;
    }
    instrument.new_note_action = NewNoteAction::Continue;
    instrument.fadeout = 0;
    if config.filtered {
        instrument.initial_filter_cutoff = Some(56);
        instrument.initial_filter_resonance = Some(96);
    }
    builder.add_instrument(instrument)?;

    let mut pattern = Vec::with_capacity(STRESS_ROWS * config.channels * 5);
    for row in 0..STRESS_ROWS {
        for channel in 0..config.channels {
            let note = 36 + ((channel * 5 + row * 7) % 48) as u8;
            let jump_to_zero = row + 1 == STRESS_ROWS && channel == 0;
            let cell = ItCell {
                note,
                instrument: 1,
                volume: 64,
                command: if jump_to_zero { 2 } else { 0 },
                info: 0,
            };
            pattern.extend_from_slice(&cell.to_bytes());
        }
    }
    builder.add_pattern(&pattern, STRESS_ROWS as u16, config.channels as u8)?;
    builder.set_orders(&[0, starplayer::model::ORDER_END]);

    let mut header = ModuleHeader::new(ModuleFormat::It, config.channels as u8);
    header.title = alloc::string::String::from("A1S voice capacity").into_boxed_str();
    header.initial_speed = 1;
    header.initial_tempo = 255;
    header.flags.linear_slides = true;
    header.format_extra = ItFormatExtra {
        flags: 0x000C,
        special: 0,
        old_instruments: false,
        has_midi_configuration: false,
    }
    .encode();
    header.default_pan = (0..config.channels)
        .map(|channel| {
            if config.channels == 1 {
                I1F15::ZERO
            } else {
                let bits = -24_576 + (49_152 * channel as i32 / (config.channels - 1) as i32);
                I1F15::from_bits(bits as i16)
            }
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    builder.set_header(header);
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use starplayer::dsp::Linear;
    use starplayer::engine::{EngineLayout, EngineSettings};
    use starplayer::rt::Arc;
    use starplayer_host_embedded::EmbeddedPlayer;

    const SAMPLE_RATE_HZ: u32 = 48_000;

    #[test]
    fn capture_grace_is_long_enough_for_serial_reconnect_without_delaying_each_case_excessively() {
        assert!((2..=3).contains(&CAPTURE_GRACE_SECONDS));
    }

    #[test]
    fn workload_settle_periods_allow_five_seconds_for_voice_and_web_startup() {
        assert_eq!(VOICE_PLATEAU_SETTLE_MS, 5_000);
        assert_eq!(WEB_LOAD_SETTLE_MS, 5_000);
        assert_eq!(WEB_LOAD_STALL_MS, 5_000);
    }

    #[test]
    fn web_network_preflight_preserves_radio_allocation_and_benchmark_reserve() {
        assert_eq!(WEB_NETWORK_PREFLIGHT_HEAP_BYTES, 56 * 1024);
        assert!(!web_network_preflight_passes(WEB_NETWORK_PREFLIGHT_HEAP_BYTES - 1));
        assert!(web_network_preflight_passes(WEB_NETWORK_PREFLIGHT_HEAP_BYTES));
    }

    #[test]
    fn maximum_stress_image_fits_the_fixed_psram_setup_claim() {
        let image = stress_module(StressConfig::MAXIMUM).expect("maximum stress module").to_image();
        assert!(image.len() <= IMAGE_BUFFER_BYTES, "maximum stress image grew to {} bytes beyond its {}-byte PSRAM claim", image.len(), IMAGE_BUFFER_BYTES);
    }

    fn render(config: StressConfig) -> (Vec<i16>, u16, u16, u32) {
        let module = Arc::new(stress_module(config).expect("valid stress module"));
        let settings = EngineSettings {
            voice_capacity: config.voices,
            channel_count: config.channels,
            scope_taps: false,
            telemetry_depth: 1,
            layout: EngineLayout::MasterOnly,
            sample_rate_hz: SAMPLE_RATE_HZ,
            ..EngineSettings::default()
        };
        let (mut render, mut control) = EmbeddedPlayer::<Linear>::open_empty(SAMPLE_RATE_HZ, settings).expect("stress player");
        control.load(module).expect("stress source");
        control.play().expect("play command");
        let mut output = alloc::vec![0i16; SAMPLE_RATE_HZ as usize * 2];
        let mut peak_active = 0u16;
        for descriptor in output.chunks_mut(256 * 2) {
            render.render(descriptor);
            peak_active = peak_active.max(control.telemetry().voices_active);
        }
        let voices_active = control.telemetry().voices_active;
        (output, voices_active, peak_active, control.voice_steals())
    }

    #[test]
    fn workload_has_exact_width_and_reaches_requested_nna_plateaus() {
        for config in [
            StressConfig { channels: 1, voices: 8, filtered: false },
            StressConfig { channels: 32, voices: 32, filtered: false },
            StressConfig { channels: 64, voices: 96, filtered: false },
            StressConfig { channels: 64, voices: 256, filtered: false },
        ] {
            let module = stress_module(config).expect("valid stress module");
            assert_eq!(module.header().channel_count as usize, config.channels);
            assert_eq!(module.patterns()[0].channels() as usize, config.channels);
            let (_, voices_active, peak_active, steals) = render(config);
            assert_eq!(peak_active as usize, config.voices, "the peak NNA plateau must equal the pool cap; final={voices_active}, steals={steals}");
            assert_eq!(voices_active as usize, config.voices, "the NNA plateau must remain at the pool cap");
            assert!(steals > 0, "the repeating workload must exercise IT stealing at the cap");
        }
    }

    #[test]
    fn output_is_deterministic_non_silent_and_does_not_clip() {
        let config = StressConfig { channels: 64, voices: 256, filtered: false };
        let (first, _, _, _) = render(config);
        let (second, _, _, _) = render(config);
        assert_eq!(first, second);
        assert!(first.iter().any(|sample| *sample != 0));
        assert!(first.iter().all(|sample| sample.unsigned_abs() < i16::MAX as u16));
    }

    #[test]
    fn filtered_case_uses_the_it_filter_and_changes_output() {
        let (plain, _, _, _) = render(StressConfig { channels: 8, voices: 32, filtered: false });
        let (filtered, _, _, _) = render(StressConfig { channels: 8, voices: 32, filtered: true });
        assert_ne!(plain, filtered);
        assert!(filtered.iter().any(|sample| *sample != 0));
    }
}
