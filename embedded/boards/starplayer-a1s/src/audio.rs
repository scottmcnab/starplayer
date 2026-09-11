//! I2S master TX with a circular DMA ring, and the task that refills it.
//!
//! # The shape
//!
//! One I2S transmitter, Philips 16-bit stereo at 44 100 Hz, MCLK out on GPIO0, feeding a
//! **circular** DMA transfer over a `'static` ring of [`DMA_RING_QUANTA`] render quanta.
//! The refill is an Embassy task that awaits the DMA's available-space future and calls
//! [`RenderHalf::render`] for exactly the space the DMA has finished with — never more
//! than the ring holds, and never into a region the DMA still owns, because
//! `push_with` hands out only the free part.
//!
//! Each DMA descriptor is exactly one render quantum
//! ([`QUANTUM_BYTES`] = 128 frames × 4 bytes), which makes the available-space figure a
//! whole number of quanta and the refill's block size the engine's own. That is not
//! required for correctness — design goal 3 says the output must be identical at *any*
//! block size, and `RenderHalf::render`'s cadence is what makes that true through a host —
//! but it means the DMA boundary and the control cadence coincide, so a seek or a stop
//! lands on the frame the engine says it does with no extra ragged block in the way.
//!
//! # Real-time rules in the refill
//!
//! These are architecture §8's rules, restated where they are actually enforced:
//!
//! * **no allocation** — the scratch is one stack array of [`QUANTUM_FRAMES`] frames, and
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
//! 2. **Ring depth.** [`DMA_RING_QUANTA`] is 8 — 1 024 frames, 23.2 ms. Research point 4
//!    asks for the smallest depth that never underruns with the radio off, doubled for
//!    M8-I6's headroom. The control task prints `underruns=` once a second, so the
//!    measurement is: set the constant to 2, 4, 8, watch.

use core::sync::atomic::{AtomicU32, Ordering};

use esp_hal::dma_circular_buffers_chunk_size;
use esp_hal::i2s::master::{Channels, Config, DataFormat, I2s, I2sTx};
use esp_hal::peripherals::{DMA_I2S0, GPIO0, GPIO25, GPIO26, GPIO27, I2S0};
use esp_hal::time::Rate;
use esp_hal::Async;
use starplayer::dsp::Linear;
use starplayer_host_embedded::RenderHalf;

/// Frames in one render quantum. `starplayer::engine::RENDER_QUANTUM`, restated as a
/// `usize` the DMA arithmetic can use.
pub const QUANTUM_FRAMES: usize = 128;

/// Bytes in one stereo `i16` frame.
pub const FRAME_BYTES: usize = 4;

/// Bytes in one render quantum: 512.
pub const QUANTUM_BYTES: usize = QUANTUM_FRAMES * FRAME_BYTES;

/// How many render quanta the DMA ring holds.
///
/// 8 quanta is 1 024 frames — 23.2 ms at 44 100 Hz — which is a generous starting point:
/// the refill has to be scheduled twice within that window to keep the ring fed, and the
/// one measurement research point 4 asks for is how much smaller it can go. It is also the
/// **only** latency knob on this path; the render quantum itself is not one (architecture
/// Q2).
pub const DMA_RING_QUANTA: usize = 8;

/// The DMA ring, in bytes: 4 096.
pub const DMA_RING_BYTES: usize = DMA_RING_QUANTA * QUANTUM_BYTES;

/// Swap left and right on the way to the codec.
///
/// `false` unless the owner's ears say otherwise; see the module docs. It is applied in
/// the one place the interleaved samples are copied into the DMA ring, so it costs a
/// branch per quantum and nothing per sample.
pub const SWAP_CHANNELS: bool = false;

/// How many times the refill found the ring completely empty.
///
/// Written only by the refill (which may not log) and read by the control task. `Relaxed`
/// is right: it is a diagnostic counter with no other memory it orders.
static UNDERRUNS: AtomicU32 = AtomicU32::new(0);

/// How many times `push_with` or `available` returned an error.
static DMA_ERRORS: AtomicU32 = AtomicU32::new(0);

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

impl core::fmt::Display for Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Config => formatter.write_str("the I2S peripheral rejected its configuration"),
            Error::Dma => formatter.write_str("the I2S DMA transfer would not start"),
        }
    }
}

/// Bring I2S up, prime the ring with real audio and start the circular transfer.
///
/// The ring is **prefilled** rather than started on zeros: a circular transfer begins
/// playing whatever is in the buffer the moment it starts, and 23 ms of silence at the top
/// of a song is 23 ms of a click waiting to happen when the codec unmutes. Rendering the
/// whole ring first costs one quantum's worth of work per quantum and means the very first
/// sample the codec sees is music.
///
/// MCLK is GPIO0 through `CLK_OUT1` — **research point 3, resolved**: esp-hal 1.1 does
/// expose it for the classic ESP32's I2S. `I2s::with_mclk` has a chip-specific arm there
/// (`impl ClkPin` for GPIO0/1/3 only, mapping to `CLK_OUT1`/`CLK_OUT3`/`CLK_OUT2`) which
/// programs `IO_MUX.PIN_CTRL` and connects the signal itself. No register poke and no
/// `unsafe` of ours was needed, and the fallback of running the ES8388 without MCLK was
/// not reached.
pub fn start(parts: Parts, sample_rate_hz: u32, render: &mut RenderHalf<Linear>) -> Result<AudioTransfer, Error> {
    let Parts { i2s0, dma, mclk, bclk, lrck, dout } = parts;
    let (_rx_buffer, _rx_descriptors, tx_buffer, tx_descriptors) =
        dma_circular_buffers_chunk_size!(0, DMA_RING_BYTES, QUANTUM_BYTES);

    let i2s = I2s::new(
        i2s0,
        dma,
        Config::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(sample_rate_hz))
            .with_data_format(DataFormat::Data16Channel16)
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
/// `destination` is a whole number of quanta in the prefill and whatever `push_with`
/// offers afterwards, so the tail is handled by rendering fewer samples rather than by
/// refusing: `RenderHalf::render` counts interleaved samples and carries its in-force gain
/// across calls, so a block that is not a whole number of frames leaves the stream aligned
/// (I1's `a_render_that_is_not_a_whole_number_of_frames_keeps_the_stream_aligned`).
fn fill(destination: &mut [u8], render: &mut RenderHalf<Linear>) -> usize {
    let mut scratch = [0i16; QUANTUM_FRAMES * 2];
    let mut written = 0usize;
    while written < destination.len() {
        let remaining = destination.len() - written;
        let samples = (remaining / 2).min(scratch.len());
        if samples == 0 {
            // An odd trailing byte cannot hold a sample; leave it for the next call rather
            // than emitting half of one.
            break;
        }
        let block = &mut scratch[..samples];
        render.render(block);
        if SWAP_CHANNELS {
            for frame in block.chunks_exact_mut(2) {
                frame.swap(0, 1);
            }
        }
        let bytes: &[u8] = bytemuck::cast_slice(&block[..]);
        destination[written..written + bytes.len()].copy_from_slice(bytes);
        written += bytes.len();
    }
    written
}

/// Keep the ring fed, for ever.
///
/// The loop is: wait for space, render into it, repeat. `available()` resolves only when
/// the DMA has finished with at least one descriptor, so the task sleeps the rest of the
/// time and the executor is free.
///
/// An `available()` of the whole ring means the DMA consumed everything before this task
/// was scheduled — an **underrun**, audible as a click — and is counted rather than
/// logged. Errors are counted for the same reason: this is the real-time path.
#[embassy_executor::task]
pub async fn refill_task(mut transfer: AudioTransfer, render: &'static mut RenderHalf<Linear>) {
    loop {
        match transfer.available().await {
            Ok(available) => {
                if available >= DMA_RING_BYTES {
                    UNDERRUNS.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(_) => {
                DMA_ERRORS.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        }
        if transfer.push_with(|destination| fill(destination, render)).await.is_err() {
            DMA_ERRORS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// How many underruns the refill has seen. For the control task's log line.
pub fn underruns() -> u32 { UNDERRUNS.load(Ordering::Relaxed) }

/// How many DMA errors the refill has seen.
pub fn dma_errors() -> u32 { DMA_ERRORS.load(Ordering::Relaxed) }
