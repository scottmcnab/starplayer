//! I2S master TX with a circular DMA ring, and the task that refills it.
//!
//! # The shape
//!
//! One I2S transmitter, Philips 32-bit stereo at 44 100 Hz, MCLK out on GPIO0, feeding a
//! **circular** DMA transfer over a `'static` ring of [`DMA_RING_QUANTA`] render quanta.
//! The refill is an Embassy task that awaits the DMA's available-space future, renders one or
//! more complete [`DESCRIPTOR_BYTES`] blocks into static scratch, and copies them through a
//! descriptor-aligned `push_with` closure.
//!
//! The `chunk` argument to `dma_circular_buffers_chunk_size!` decides how many descriptors are
//! *allocated*, but `DescriptorChain::new` hardcodes esp-hal's own 4 092-byte `CHUNK_SIZE`.
//! `DescriptorChain::fill` then cuts any circular ring of 8 184 bytes or fewer into exactly
//! three descriptors. The ring is therefore six quanta = 6 144 bytes, producing three equal
//! 2 048-byte descriptors. Each descriptor is exactly two render quanta, 256 stereo frames,
//! or about 5.8 ms; the whole ring is about 17.4 ms and remains under the 8 184-byte small-ring
//! limit.
//!
//! Hardware established why equal descriptor geometry and the `push` discipline matter. With the old
//! 4 096-byte ring, esp-hal made ragged 1 366 / 1 366 / 1 364-byte descriptors and steady
//! `push_with` accepted variable contiguous regions while returning descriptor ownership. At
//! +1.59 seconds the diagnostic totals were `offered=written=88748`, then grew by only
//! 89–90 KB/s with about 43 pushes/s — roughly half the then-required 176 400 B/s. Equal
//! descriptors and full closure consumption keep the write offset and descriptor ownership
//! aligned. The 32-bit-slot experiment doubles the required DMA byte rate to 352 800 B/s. See
//! `plans/reference/embedded-budget.md` §4a for the underlying esp-hal and Star FX analysis.
//!
//! # Why construction and refill share core 1
//!
//! esp-hal 1.1.2's `ChannelTx::into_async` calls `set_interrupt_handler`, which disables
//! the DMA interrupt on `Cpu::other()` and binds its handler on `Cpu::current()`. Constructing
//! the driver on core 0 and then moving only the transfer therefore leaves core 1's DMA future
//! dependent on core 0 servicing the wake interrupt. Moving construction here remains the correct
//! ownership because core 0 has 7–12 ms UART critical sections, but a timestamped run after that
//! move still advanced only 24 seconds of song by 49.86 seconds with both diagnostic counters at
//! zero. Interrupt affinity was therefore not the main cause of the observed gating.
//!
//! [`start`] must consequently run inside core 1's entry closure, immediately before
//! [`refill_task`]. This matches Star FX's working arrangement and keeps construction, real-audio
//! prefill, transfer start, silent pre-roll and steady refill on the same interrupt-owning core.
//!
//! # Real-time rules in the refill
//!
//! These are architecture §8's rules, restated where they are actually enforced:
//!
//! * **no allocation** — the descriptor scratch is a [`ConstStaticCell`], and
//!   `RenderHalf::render` allocates nothing;
//! * **no logging** — `log!` takes a lock and formats; the refill counts underruns into an
//!   atomic and the control task prints them;
//! * **no blocking I2C** — the codec is configured at boot and its volume is changed from
//!   a control task, never from here;
//! * **no panics** — every `Result` is folded into the underrun counter or ignored, since
//!   a panic on a device is a reset in the middle of a song.
//!
//! # Two things the owner must verify on hardware
//!
//! 1. **Channel order.** esp-hal sets both `tx_msb_right` and `tx_right_first` on the
//!    classic ESP32 (the second because the chip emits two clock pulses before the first
//!    sample and sending the right channel first keeps WS high across them). The two
//!    should cancel and an interleaved `[left, right]` buffer should come out the right way
//!    round. If the stereo image is reversed, set [`SWAP_CHANNELS`] and rebuild — it is one
//!    constant precisely so that the fix does not need a redesign.
//! 2. **Ring depth.** [`DMA_RING_QUANTA`] is 6 — 768 frames, 17.4 ms — because three equal
//!    descriptors must each contain a whole number of render quanta after widening the I2S slots.
//!    The control task prints `underruns=` once a second; preserve the three-descriptor,
//!    whole-quantum constraints in any later sweep.

use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

use esp_hal::dma_circular_buffers_chunk_size;
use esp_hal::i2s::master::{Channels, Config, DataFormat, I2s, I2sTx};
use esp_hal::peripherals::{DMA_I2S0, GPIO0, GPIO25, GPIO26, GPIO27, I2S0};
use esp_hal::time::Rate;
use esp_hal::Async;
use static_cell::ConstStaticCell;
use starplayer::dsp::Linear;
use starplayer_host_embedded::RenderHalf;

/// Frames in one render quantum. `starplayer::engine::RENDER_QUANTUM`, restated as a
/// `usize` the DMA arithmetic can use.
pub const QUANTUM_FRAMES: usize = 128;

/// Bytes in one 32-bit I2S sample slot.
pub const SLOT_BYTES: usize = 4;

/// Bytes in one stereo frame: two 32-bit slots.
pub const FRAME_BYTES: usize = 2 * SLOT_BYTES;

/// Bytes in one render quantum: 1 024.
pub const QUANTUM_BYTES: usize = QUANTUM_FRAMES * FRAME_BYTES;

/// DMA descriptors in the small circular ring. esp-hal fixes this at three.
pub const RING_DESCRIPTORS: usize = 3;

/// How many render quanta the DMA ring holds: three descriptors of two quanta.
pub const DMA_RING_QUANTA: usize = 6;

/// How many render quanta one DMA descriptor holds.
pub const DESCRIPTOR_QUANTA: usize = DMA_RING_QUANTA / RING_DESCRIPTORS;

/// Frames in one DMA descriptor: 256.
pub const DESCRIPTOR_FRAMES: usize = DESCRIPTOR_QUANTA * QUANTUM_FRAMES;

/// Interleaved engine `i16` samples in one DMA descriptor: 512.
pub const DESCRIPTOR_SAMPLES: usize = DESCRIPTOR_FRAMES * 2;

/// Bytes in one DMA descriptor: 2 048.
pub const DESCRIPTOR_BYTES: usize = DESCRIPTOR_FRAMES * FRAME_BYTES;

/// The DMA ring, in bytes: 6 144.
pub const DMA_RING_BYTES: usize = DMA_RING_QUANTA * QUANTUM_BYTES;

const _: () = assert!(DMA_RING_QUANTA.is_multiple_of(RING_DESCRIPTORS), "each descriptor must contain whole render quanta");
const _: () = assert!(DESCRIPTOR_BYTES == DESCRIPTOR_QUANTA * QUANTUM_BYTES);
const _: () = assert!(DESCRIPTOR_BYTES == DESCRIPTOR_SAMPLES * SLOT_BYTES);
const _: () = assert!(DMA_RING_BYTES == RING_DESCRIPTORS * DESCRIPTOR_BYTES);
const _: () = assert!(DESCRIPTOR_BYTES == 2_048, "the 32-bit-slot descriptor must be 2 048 bytes");
const _: () = assert!(DMA_RING_BYTES == 6_144, "the 32-bit-slot ring must be 6 144 bytes");
const _: () = assert!(DESCRIPTOR_BYTES <= 4_092, "one descriptor must fit esp-hal's chunk limit");
const _: () = assert!(DMA_RING_BYTES <= 8_184, "the ring must retain esp-hal's three-descriptor geometry");

/// One whole-descriptor steady-state render buffer in static DRAM.
///
/// A task local `[i16; 512]` would enlarge the task future and could materialise on the spawning
/// stack. `ConstStaticCell::take` hands the static allocation directly to the only refill task,
/// with no copy, allocation or `unsafe`.
static DESCRIPTOR_SCRATCH: ConstStaticCell<[i16; DESCRIPTOR_SAMPLES]> = ConstStaticCell::new([0; DESCRIPTOR_SAMPLES]);

/// Swap left and right on the way to the codec.
///
/// `false` unless the owner's ears say otherwise; see the module docs. It is applied in
/// the one place the interleaved samples are copied into the DMA ring, so it costs a
/// branch per quantum and nothing per sample.
pub const SWAP_CHANNELS: bool = false;

/// Descriptor handoffs completed while the codec is muted before the steady refill loop starts.
///
/// One `push_with` is enough in principle to bring esp-hal's TX accounting to life. Eight is
/// the precedent that `../star-fx` soaked for 40 minutes on the same ESP32-A1S and esp-hal
/// 1.1.2. StarPlayer's descriptors are 256 frames, or 5.8 ms at 44.1 kHz, so this is a
/// bounded roughly 46 ms pre-roll. It changes only muted startup time, not the steady-state
/// ring depth or output latency.
pub const TX_PRIME_HANDOFFS: usize = 8;

/// How many times the refill found the ring completely empty.
///
/// Written only by the refill (which may not log) and read by the control task. `Relaxed`
/// is right: it is a diagnostic counter with no other memory it orders.
static UNDERRUNS: AtomicU32 = AtomicU32::new(0);

/// How many times `available` or `push_with` returned an error.
static DMA_ERRORS: AtomicU32 = AtomicU32::new(0);

/// Cumulative whole-descriptor bytes reported by the steady loop's outer `available()` call.
static STEADY_AVAILABLE_BYTES: AtomicU32 = AtomicU32::new(0);

/// Cumulative bytes copied by steady descriptor-aligned `push_with` closures.
static STEADY_WRITTEN_BYTES: AtomicU32 = AtomicU32::new(0);

/// Cumulative steady-state `push_with` calls, including recovery and error calls.
static STEADY_PUSH_CALLS: AtomicU32 = AtomicU32::new(0);

const STARTUP_PENDING: u8 = 0;
const STARTUP_READY: u8 = 1;
const STARTUP_FAILED: u8 = 2;

/// Core 1's I2S startup and muted pre-roll state.
///
/// The release/acquire pair makes this a one-way boot handoff: core 0 does not unmute the codec
/// until core 1 has constructed the driver and finished every pre-roll DMA operation. A start
/// error is published separately from a timeout so core 0 can keep the codec muted and report
/// the existing useful failure. Firmware starts the audio task once per reset, so the state
/// never transitions backwards.
static STARTUP_STATE: AtomicU8 = AtomicU8::new(STARTUP_PENDING);

/// The in-progress circular transfer, with its ring.
pub type AudioTransfer = esp_hal::i2s::master::asynch::I2sWriteDmaTransferAsync<'static, &'static mut [u8; DMA_RING_BYTES]>;

/// The peripherals the I2S transmitter needs, as one value.
///
/// One struct rather than six arguments because they are claimed together and are only
/// ever useful together: `I2S0`, its DMA channel and the four pins are one transmitter,
/// and handing them out separately would let a second caller build a second driver over
/// the same peripheral.
pub struct Parts {
    /// The I2S controller.
    pub i2s0: I2S0<'static>,
    /// Its dedicated DMA channel.
    pub dma: DMA_I2S0<'static>,
    /// GPIO0 — `CLK_OUT1`, the codec's master clock.
    pub mclk: GPIO0<'static>,
    /// GPIO27 — the bit clock.
    pub bclk: GPIO27<'static>,
    /// GPIO25 — word select.
    pub lrck: GPIO25<'static>,
    /// GPIO26 — data out.
    pub dout: GPIO26<'static>,
}

/// What could go wrong bringing I2S up.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The I2S peripheral rejected its configuration — an unreachable sample rate, or a
    /// data format this chip cannot produce.
    Config,
    /// The DMA transfer could not be started.
    Dma,
}

/// Observable result of core 1's one-time audio startup.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StartupStatus {
    /// Core 1 has not finished I2S startup and the muted pre-roll.
    Pending,
    /// The driver is running and the refill has entered its steady loop.
    Ready,
    /// Core 1 could not configure I2S or start its DMA transfer.
    Failed,
}

impl core::fmt::Display for Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Config => formatter.write_str("the I2S peripheral rejected its configuration"),
            Error::Dma => formatter.write_str("the I2S DMA transfer would not start"),
        }
    }
}

/// Bring I2S up, prime the ring with real audio and start the circular transfer on core 1.
///
/// The ring is **prefilled** rather than started on zeros: a circular transfer begins
/// playing whatever is in the buffer the moment it starts, and 17 ms of silence at the top
/// of a song is 17 ms of a click waiting to happen when the codec unmutes. Rendering the
/// whole ring first costs one quantum's worth of work per quantum and means the very first
/// sample the codec sees is music.
///
/// MCLK is GPIO0 through `CLK_OUT1` — **research point 3, resolved**: esp-hal 1.1 does
/// expose it for the classic ESP32's I2S. `I2s::with_mclk` has a chip-specific arm there
/// (`impl ClkPin` for GPIO0/1/3 only, mapping to `CLK_OUT1`/`CLK_OUT3`/`CLK_OUT2`) which
/// programs `IO_MUX.PIN_CTRL` and connects the signal itself. No register poke and no
/// `unsafe` of ours was needed, and the fallback of running the ES8388 without MCLK was
/// not reached.
///
/// This function must be called from the same core that owns [`refill_task`]. The
/// `.into_async()` call binds the DMA interrupt to `Cpu::current()` and disables it on the other
/// core; moving an already-created [`AudioTransfer`] does not move that interrupt affinity.
pub fn start(parts: Parts, sample_rate_hz: u32, render: &mut RenderHalf<Linear>) -> Result<AudioTransfer, Error> {
    let Parts { i2s0, dma, mclk, bclk, lrck, dout } = parts;
    // The macro chunk controls allocation count: 6 144 / 2 048 is exactly three. esp-hal's
    // later small-ring split also produces those same three equal descriptor lengths.
    let (_rx_buffer, _rx_descriptors, tx_buffer, tx_descriptors) =
        dma_circular_buffers_chunk_size!(0, DMA_RING_BYTES, DESCRIPTOR_BYTES);

    let i2s = I2s::new(
        i2s0,
        dma,
        Config::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(sample_rate_hz))
            .with_data_format(DataFormat::Data32Channel32)
            .with_channels(Channels::STEREO),
    )
    .map_err(|_| Error::Config)?
    .with_mclk(mclk)
    .into_async();

    let i2s_tx: I2sTx<'static, Async> = i2s.i2s_tx
        .with_bclk(bclk)
        .with_ws(lrck)
        .with_dout(dout)
        .build(tx_descriptors);

    // Prime the whole ring before the DMA reads a byte of it.
    fill(tx_buffer.as_mut_slice(), render);

    i2s_tx.write_dma_circular_async(tx_buffer).map_err(|_| Error::Dma)
}

/// Render into `destination`, one quantum at a time, and report how many bytes were
/// written.
///
/// `destination` is the whole six-quantum DMA ring during the synchronous prefill. Like steady
/// refill, it renders engine `i16` and uses [`firmware_common::pack_i16_high_aligned_le`] to
/// sign-extend every sample into the high 16 bits of an explicit little-endian 32-bit slot.
fn fill(destination: &mut [u8], render: &mut RenderHalf<Linear>) -> usize {
    let mut scratch = [0i16; QUANTUM_FRAMES * 2];
    let mut written = 0usize;
    for output_quantum in destination.chunks_exact_mut(QUANTUM_BYTES) {
        render.render(&mut scratch);
        if SWAP_CHANNELS {
            for frame in scratch.chunks_exact_mut(2) {
                frame.swap(0, 1);
            }
        }
        written += firmware_common::pack_i16_high_aligned_le(&scratch, output_quantum);
    }
    written
}

/// Render exactly one complete DMA descriptor into static scratch.
fn render_descriptor(destination: &mut [i16; DESCRIPTOR_SAMPLES], render: &mut RenderHalf<Linear>) {
    for quantum in destination.chunks_exact_mut(QUANTUM_FRAMES * 2) {
        render.render(quantum);
        if SWAP_CHANNELS {
            for frame in quantum.chunks_exact_mut(2) {
                frame.swap(0, 1);
            }
        }
    }
}

/// Keep the ring fed, for ever.
///
/// The loop is: wait for space, render and push each complete descriptor available, repeat.
/// `available()` resolves only when the DMA has finished with at least one descriptor, so the
/// task sleeps the rest of the time and the executor is free.
///
/// An `available()` of the whole ring means the DMA consumed everything before this task
/// was scheduled — an **underrun**, audible as a click — and is counted rather than
/// logged. Errors are counted for the same reason: this is the real-time path.
///
/// # Start-up recovery
///
/// `TxCircularState::update` reports nothing free through the ring's first pass and returns
/// `DmaError::Late` as soon as it finds every descriptor CPU-owned. Of the two push calls only
/// `push_with` can recover from that state: it discards `available()`'s error and hands a
/// descriptor back to the DMA regardless, which is precisely what un-sticks the accounting.
///
/// `../star-fx` hit exactly this on an ESP32-A1S with the same esp-hal version: its transport
/// froze with one block processed while its overrun counter climbed by about 650 000 a second,
/// and audio only flowed — 901 544 consecutive blocks — once the code reached `push_with`
/// anyway.
///
/// The first eight `push_with` handoffs below establish that lead with silence before the steady
/// loop. A board run with only the fallthrough above disproved the earlier no-pre-roll decision:
/// pitch was correct, but output was heavily gated and song time advanced at roughly half wall
/// speed even with both counters at zero. The first pre-roll attempt rendered real audio in a core-0
/// async call and overflowed ProCpu's stack. Replacing its closure with silence was insufficient:
/// merely adding that async call enlarged `play`'s future enough that the existing [`start`] →
/// [`fill`] prefill then crossed the same guard. Keeping the whole async pre-roll here on core 1
/// removes it from `play` without changing either stack size.
///
/// # Whole-descriptor steady state
///
/// The next diagnostic run measured `offered=written=88748` at +1.59 seconds, followed by only
/// 89–90 KB/s and about 43 pushes/s. Stereo 16-bit at 44.1 kHz required 176 400 B/s. The ragged
/// 4 096-byte ring and variable-region `push_with` calls were returning only about half a ring
/// per physical ring cycle. The fixed geometry above makes every available byte count a whole
/// descriptor. The first fixed-geometry build then booted and completed exactly one 1 536-byte
/// steady `push`; `offered=written=1536` and `pushes=1` stayed fixed while `dma_errors` climbed
/// by about 115/s. `push` performs its own second `available()` after the outer check and render;
/// that second check returned `Late`, while the outer error arm skipped the only call capable of
/// returning descriptor ownership.
///
/// The loop therefore keeps the explicit outer `available()` for diagnostics, then always falls
/// through to descriptor-aligned `push_with`. Its internal `available()` runs immediately and
/// discards `Late`; only then does the closure render and copy, so no fallible check separates
/// rendering from the descriptor handoff. Equal descriptor geometry makes this safe in steady
/// state: the closure loops over every complete descriptor in its contiguous offer
/// and returns their full byte count, never a partial descriptor. An empty recovery offer returns
/// zero and renders nothing; the outer error already diagnosed it. See
/// `plans/reference/embedded-budget.md` §4a for the esp-hal source analysis and Star FX evidence.
#[embassy_executor::task]
pub async fn refill_task(mut transfer: AudioTransfer, render: &'static mut RenderHalf<Linear>) {
    // Match Star FX's soaked start-up sequence. The explicit `available()` keeps any initial
    // `Late` visible, while `push_with` ignores its own `available()` error and hands a
    // descriptor back anyway. An early offer may be empty; every non-empty offer becomes
    // silence. Keep render and fill out of this loop so pre-roll needs no render scratch.
    for _ in 0..TX_PRIME_HANDOFFS {
        if transfer.available().await.is_err() {
            DMA_ERRORS.fetch_add(1, Ordering::Relaxed);
        }
        if transfer
            .push_with(|destination| {
                destination.fill(0);
                destination.len()
            })
            .await
            .is_err()
        {
            DMA_ERRORS.fetch_add(1, Ordering::Relaxed);
        }
    }
    // Take the whole-descriptor scratch only after pre-roll, so it contributes neither to the
    // task's async frame nor to core 1's stack. This task is spawned exactly once per reset.
    let descriptor_scratch = DESCRIPTOR_SCRATCH.take();
    STARTUP_STATE.store(STARTUP_READY, Ordering::Release);

    loop {
        match transfer.available().await {
            Ok(available) => {
                if available >= DMA_RING_BYTES {
                    UNDERRUNS.fetch_add(1, Ordering::Relaxed);
                }
                if !available.is_multiple_of(DESCRIPTOR_BYTES) {
                    DMA_ERRORS.fetch_add(1, Ordering::Relaxed);
                }
                let whole_descriptor_bytes = available / DESCRIPTOR_BYTES * DESCRIPTOR_BYTES;
                STEADY_AVAILABLE_BYTES.fetch_add(whole_descriptor_bytes as u32, Ordering::Relaxed);
            }
            Err(_) => {
                DMA_ERRORS.fetch_add(1, Ordering::Relaxed);
            }
        }

        STEADY_PUSH_CALLS.fetch_add(1, Ordering::Relaxed);
        if transfer
            .push_with(|destination| {
                let mut written = 0usize;
                for descriptor in destination.chunks_exact_mut(DESCRIPTOR_BYTES) {
                    render_descriptor(descriptor_scratch, render);
                    let packed = firmware_common::pack_i16_high_aligned_le(&descriptor_scratch[..], descriptor);
                    if packed != DESCRIPTOR_BYTES {
                        DMA_ERRORS.fetch_add(1, Ordering::Relaxed);
                    }
                    written += packed;
                }
                if !destination.len().is_multiple_of(DESCRIPTOR_BYTES) {
                    DMA_ERRORS.fetch_add(1, Ordering::Relaxed);
                }
                STEADY_WRITTEN_BYTES.fetch_add(written as u32, Ordering::Relaxed);
                written
            })
            .await
            .is_err()
        {
            DMA_ERRORS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// How many underruns the refill has seen. For the control task's log line.
pub fn underruns() -> u32 { UNDERRUNS.load(Ordering::Relaxed) }

/// How many DMA errors the refill has seen.
pub fn dma_errors() -> u32 { DMA_ERRORS.load(Ordering::Relaxed) }

/// Whole-descriptor bytes reported by steady outer `available()` calls; wraps after roughly 6.8 hours.
pub fn steady_available_bytes() -> u32 { STEADY_AVAILABLE_BYTES.load(Ordering::Relaxed) }

/// Bytes copied by steady descriptor-aligned closures; wraps after roughly 6.8 hours.
pub fn steady_written_bytes() -> u32 { STEADY_WRITTEN_BYTES.load(Ordering::Relaxed) }

/// Number of steady-state `push_with` calls, including recovery and error calls.
pub fn steady_push_calls() -> u32 { STEADY_PUSH_CALLS.load(Ordering::Relaxed) }

/// Publish an I2S construction or transfer-start failure from core 1.
pub fn report_start_failure() { STARTUP_STATE.store(STARTUP_FAILED, Ordering::Release); }

/// Observe core 1's I2S startup and muted pre-roll state from core 0.
pub fn startup_status() -> StartupStatus {
    match STARTUP_STATE.load(Ordering::Acquire) {
        STARTUP_READY => StartupStatus::Ready,
        STARTUP_FAILED => StartupStatus::Failed,
        _ => StartupStatus::Pending,
    }
}
