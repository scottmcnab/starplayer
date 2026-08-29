//! AudioWorklet glue for the browser: moves rendered quanta across the worklet boundary
//! and carries commands and telemetry the other way.
//!
//! Must always build for `wasm32-unknown-unknown`; `xtask ci --job wasm-build` checks it.
//!
//! Allowed dependency edges: `starplayer`, plus `wasm-bindgen`.
//!
//! # What this crate is, in M0
//!
//! The engine does not exist yet. Task M0-A4 proves the *plumbing* — worklet
//! instantiation, the 128-frame quantum, a lock-free command path, a telemetry path, and
//! a wasm heap that never grows on the audio thread — with a sine wave, so that when
//! M1's tracker code arrives an audio bug can be told apart from a plumbing bug. The
//! oscillator, the command ring and the peak meter therefore live here for now; M1
//! replaces them with `starplayer-engine`, `starplayer-rt` and `starplayer-telemetry`
//! behind the same four entry points.
//!
//! # The contract with the worklet
//!
//! ```text
//! main thread                      audio (worklet) thread
//! -----------                      ----------------------
//! compile WebAssembly.Module  ───▶  initSync, then init(sample_rate, channels)
//! read output_ptr/output_len  ◀───  a wasm-owned planar f32 buffer
//! SharedArrayBuffer ring      ───▶  drained each quantum → push_set_frequency
//!                             ◀───  process(frames) → peak, published back out
//! ```
//!
//! Everything `process()` touches is allocated by `init()`. `process()` performs no
//! allocation, so wasm linear memory never grows on the audio thread and the
//! `Float32Array` the worklet holds over that memory is never detached. That last point
//! is not a micro-optimisation: a detached view is silence, and a growing heap is an
//! audible glitch.
//!
//! # `unsafe_code`
//!
//! The lint is `deny`, not `forbid`, purely because `#[wasm_bindgen]` expands to
//! `unsafe extern "C"` shims. No line written by hand in this crate is unsafe, and the
//! allowance is scoped to the module holding the exports.

#![deny(unsafe_code)]

mod command;
mod peak;
mod sine;

pub use command::{Command, CommandRing, COMMAND_RING_CAPACITY};
pub use peak::PeakMeter;
pub use sine::{SineOscillator, SineTable, SINE_TABLE_LENGTH};

use core::cell::RefCell;

/// AudioWorklet always calls `process()` with exactly 128 frames, which is why
/// `RENDER_QUANTUM` is 128 (architecture §1.4). The page asserts the two agree at run
/// time rather than taking the specification's word for it.
pub const RENDER_QUANTUM: usize = 128;

/// Frames the output buffer is sized for. Eight quanta of headroom, so a host that ever
/// asks for more than 128 is clamped rather than left rendering garbage — and so the
/// same buffer serves an offline render later.
pub const MAX_FRAMES_PER_CALL: usize = 1024;

/// Channels the buffer is sized for, independent of how many `init` is asked for.
pub const MAX_CHANNELS: usize = 2;

/// Bytes handed to the allocator and immediately released during `init`, before any
/// buffer is allocated and before the worklet takes a view over linear memory.
///
/// This forces whatever `memory.grow` the run is going to need to happen at init, on the
/// setup path, rather than in the middle of a quantum. `dlmalloc` cannot return pages to
/// the engine, so the freed space stays in its arena and satisfies the real allocations
/// that follow.
const HEAP_RESERVE_BYTES: usize = 512 * 1024;

/// Starting pitch: A above middle C, in the middle of the page's 110–880 Hz slider.
const DEFAULT_FREQUENCY_HZ: f32 = 440.0;

/// Output level. Deliberately well below full scale — this is a sine into somebody's
/// headphones, and the peak meter still has plenty to show.
const OUTPUT_GAIN: f32 = 0.2;

/// Everything `process()` needs, allocated once by `init()`.
struct SineHost {
    table: SineTable,
    oscillator: SineOscillator,
    commands: CommandRing,
    meter: PeakMeter,
    /// Planar f32, channel `c` occupying `[c * MAX_FRAMES_PER_CALL, ...)`. Planar rather
    /// than interleaved because that is the layout `outputs[0][channel]` wants, so the
    /// worklet's copy-out is a single `Float32Array.set` per channel with no shuffling.
    output: Vec<f32>,
    channels: usize,
    quanta_rendered: u64,
}

impl SineHost {
    fn new(sample_rate: f32, channels: u32) -> Self {
        let channels = (channels as usize).clamp(1, MAX_CHANNELS);
        Self {
            table: SineTable::new(),
            oscillator: SineOscillator::new(sample_rate, DEFAULT_FREQUENCY_HZ, OUTPUT_GAIN),
            commands: CommandRing::new(),
            meter: PeakMeter::new(),
            output: vec![0.0; MAX_FRAMES_PER_CALL * MAX_CHANNELS],
            channels,
            quanta_rendered: 0,
        }
    }

    /// Applies every queued command. This is architecture §1.2's `drain_commands()` at
    /// the top of the render loop, at spike scale.
    fn drain_commands(&mut self) {
        while let Some(command) = self.commands.pop() {
            match command {
                Command::SetFrequency(hertz) => self.oscillator.set_frequency(hertz),
            }
        }
    }

    /// Renders `frames` frames into the output buffer and returns the peak level.
    ///
    /// No allocation, no locks, no panics — the three hard rules of architecture §8.
    fn process(&mut self, frames: usize) -> f32 {
        self.drain_commands();

        let frames = frames.min(MAX_FRAMES_PER_CALL);
        for index in 0..frames {
            self.output[index] = self.oscillator.next_sample(&self.table);
        }
        for channel in 1..self.channels {
            self.output.copy_within(0..frames, channel * MAX_FRAMES_PER_CALL);
        }

        self.quanta_rendered = self.quanta_rendered.wrapping_add(1);
        self.meter.observe(&self.output[0..frames])
    }
}

thread_local! {
    /// One instance per wasm instance. A `thread_local` rather than a `static mut`
    /// because it is the only way to hold mutable global state without `unsafe`; wasm in
    /// a worklet is single-threaded, so there is no contention to lose to.
    static HOST: RefCell<Option<SineHost>> = const { RefCell::new(None) };
}

/// Runs `action` against the initialised host, or returns `fallback` if `init` has not
/// run or the cell is somehow already borrowed. Never panics: on the audio thread a
/// panic is an aborted `AudioContext`, so an uninitialised host renders silence instead.
fn with_host<T>(fallback: T, action: impl FnOnce(&mut SineHost) -> T) -> T {
    HOST.with(|cell| match cell.try_borrow_mut() {
        Ok(mut slot) => match slot.as_mut() {
            Some(host) => action(host),
            None => fallback,
        },
        Err(_) => fallback,
    })
}

#[allow(unsafe_code, reason = "`#[wasm_bindgen]` expands to `unsafe extern \"C\"` shims")]
mod exports {
    use super::*;
    use wasm_bindgen::prelude::wasm_bindgen;

    /// Allocates every buffer the audio path will ever touch and starts the oscillator.
    ///
    /// Must be called exactly once, from the worklet's constructor, *before* the worklet
    /// takes any `Float32Array` view over linear memory. Calling it again rebuilds the
    /// host, which invalidates the previously reported pointer.
    #[wasm_bindgen]
    pub fn init(sample_rate: f32, channels: u32) {
        // Grow the heap now, while it is safe to. See `HEAP_RESERVE_BYTES`. `black_box`
        // is what stops the optimiser from deleting an allocation nothing reads — the
        // side effect on the allocator *is* the point.
        drop(core::hint::black_box(vec![0u8; HEAP_RESERVE_BYTES]));
        let host = SineHost::new(sample_rate, channels);
        HOST.with(|cell| {
            if let Ok(mut slot) = cell.try_borrow_mut() {
                *slot = Some(host);
            }
        });
    }

    /// Byte offset of the output buffer within wasm linear memory. Valid until `init` is
    /// called again; stable for the life of the worklet otherwise, because nothing after
    /// `init` allocates.
    #[wasm_bindgen]
    pub fn output_ptr() -> u32 {
        with_host(0, |host| host.output.as_ptr() as usize as u32)
    }

    /// Length of the output buffer in `f32` elements, across all channels.
    #[wasm_bindgen]
    pub fn output_len() -> u32 {
        (MAX_FRAMES_PER_CALL * MAX_CHANNELS) as u32
    }

    /// Distance in `f32` elements between the start of one channel's block and the next.
    #[wasm_bindgen]
    pub fn output_channel_stride() -> u32 {
        MAX_FRAMES_PER_CALL as u32
    }

    /// The engine's internal render quantum, for the page to check against the size
    /// AudioWorklet actually hands it.
    #[wasm_bindgen]
    pub fn render_quantum() -> u32 {
        RENDER_QUANTUM as u32
    }

    /// Queues a pitch change. Called by the worklet once per record it drained out of
    /// the cross-thread ring, never by the page directly. Returns `false` if the ring is
    /// full, which the page surfaces as a fault.
    #[wasm_bindgen]
    pub fn push_set_frequency(hertz: f32) -> bool {
        with_host(false, |host| host.commands.push(Command::SetFrequency(hertz)))
    }

    /// Renders one block and returns the peak level of it. The audio path.
    #[wasm_bindgen]
    pub fn process(frames: u32) -> f32 {
        with_host(0.0, |host| host.process(frames as usize))
    }

    /// Last published peak, without rendering. For a telemetry reader that is not the
    /// audio callback itself.
    #[wasm_bindgen]
    pub fn peak_level() -> f32 {
        with_host(0.0, |host| host.meter.level())
    }

    /// The frequency the oscillator is actually running at, which trails the slider
    /// while the slew catches up.
    #[wasm_bindgen]
    pub fn current_frequency() -> f32 {
        with_host(0.0, |host| host.oscillator.current_frequency())
    }

    /// Commands discarded because the ring was full. Anything but zero is a defect.
    #[wasm_bindgen]
    pub fn dropped_commands() -> u32 {
        with_host(0, |host| host.commands.dropped())
    }

    /// Quanta rendered since `init`. The page divides this by elapsed time to prove the
    /// callback is actually keeping up.
    #[wasm_bindgen]
    pub fn quanta_rendered() -> f64 {
        with_host(0.0, |host| host.quanta_rendered as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_host_renders_the_requested_frames_into_every_channel() {
        let mut host = SineHost::new(48_000.0, 2);
        host.process(RENDER_QUANTUM);
        // The fade-in means the first quantum is quiet, so run a while and then compare.
        for _ in 0..200 { host.process(RENDER_QUANTUM); }
        let left = &host.output[0..RENDER_QUANTUM];
        let right = &host.output[MAX_FRAMES_PER_CALL..MAX_FRAMES_PER_CALL + RENDER_QUANTUM];
        assert_eq!(left, right, "both channels carry the same mono sine");
        assert!(left.iter().any(|sample| sample.abs() > 0.1), "the oscillator is actually running");
    }

    #[test]
    fn a_queued_command_takes_effect_on_the_next_process_call() {
        let mut host = SineHost::new(48_000.0, 1);
        assert!(host.commands.push(Command::SetFrequency(110.0)));
        host.process(RENDER_QUANTUM);
        assert!(host.commands.is_empty(), "the drain happened inside process");
        for _ in 0..1000 { host.process(RENDER_QUANTUM); }
        assert!((host.oscillator.current_frequency() - 110.0).abs() < 0.5);
    }

    #[test]
    fn an_over_long_request_is_clamped_rather_than_panicking() {
        let mut host = SineHost::new(48_000.0, 2);
        let peak = host.process(MAX_FRAMES_PER_CALL * 4);
        assert!(peak.is_finite());
    }

    #[test]
    fn output_stays_inside_the_gain_ceiling() {
        let mut host = SineHost::new(48_000.0, 2);
        let mut peak = 0.0f32;
        for _ in 0..2000 { peak = peak.max(host.process(RENDER_QUANTUM)); }
        assert!(peak <= OUTPUT_GAIN + 1e-3, "peak {peak} is within the configured gain");
        assert!(peak > OUTPUT_GAIN * 0.9, "peak {peak} reached the configured gain");
    }

    #[test]
    fn an_uninitialised_host_renders_silence_instead_of_panicking() {
        assert_eq!(with_host(0.0f32, |host| host.process(RENDER_QUANTUM)), 0.0);
    }
}
