//! [`ScopeTaps`] — the engine's side of telemetry (b): per-channel oscilloscope taps
//! (architecture §9(b), M3-D6).
//!
//! Present only under `feature = "telemetry"`, the same feature as the scalar snapshot, so
//! a host that wants scopes already has half (a).
//!
//! # The tap does not read the mix
//!
//! There are **no per-channel buses**. [`VoicePool::accumulate_masked`] sums every voice
//! into one accumulator in slot order, and that summation order is exactly what makes the
//! float path's output independent of the host's block size and what the golden hashes
//! fingerprint. Accumulating per channel to get a scope signal would change it, and the
//! goldens would move — for a picture.
//!
//! So the tap samples **voice state** instead, at the start of each render segment, and
//! never touches the accumulator at all:
//!
//! * the engine's `render_quantum` already splits each 128-frame quantum into segments at
//!   event boundaries and accumulates each segment at a quantum-relative `offset`;
//! * immediately *before* a segment's accumulation, this walks every sounding voice, and
//!   for each tap bucket whose first frame `TAP_BUCKET_FRAMES · b` lies inside the
//!   segment, reads one PCM frame at the voice's position advanced by that many steps,
//!   scales it by the voice's volume, and sums it into the ring of channel
//!   `voice.tag.channel`;
//! * a bucket's first frame is inside exactly one segment, so each bucket is filled
//!   exactly once per quantum and its value is a pure function of the quantum and the
//!   engine's state — never of the host's block size. That is what
//!   `tests/block_size_determinism.rs` pins.
//!
//! # What it deliberately ignores
//!
//! Interpolation, the gain ramps, pan, the master bus and (from M6) the per-voice filter.
//! §9(b) tolerates exactly this: it is a picture, not the audio. The alternative — running
//! the real kernel a second time into a per-channel window — is a second mixer's worth of
//! work per quantum to make a 128-pixel trace slightly more honest.
//!
//! A **muted** channel is still tapped. Muting is a mixer-side discard (the voice renders
//! into scratch and its state advances exactly as if it were audible), and the UI showing
//! a muted channel's own signal is the point of a per-channel scope.
//!
//! # Real-time safety
//!
//! The rings are allocated in [`ScopeTaps::new`], from `Engine::with_settings`. Everything
//! here is `Relaxed` loads and stores over storage that already exists: no allocation, no
//! lock, no panic, and every index goes through `get`.

use alloc::boxed::Box;
use alloc::vec::Vec;

use starplayer_mixer::{SampleData, VoicePool, folded_frame};
use starplayer_rt::{TAP_BUCKETS_PER_QUANTUM, TAP_BUCKET_FRAMES, TapReader, TapRing, TapWriter};

/// One tap ring per channel, and the bucket cursor the render loop advances.
///
/// Owned by the [`Engine`](crate::Engine); the reader halves leave through
/// [`Engine::scope_readers`](crate::Engine::scope_readers) exactly once, the way
/// [`Engine::telemetry_reader`](crate::Engine::telemetry_reader) does.
pub struct ScopeTaps {
    writers: Box<[TapWriter]>,
    /// The bucket counter the current quantum starts at. Advances by
    /// [`TAP_BUCKETS_PER_QUANTUM`] per quantum, and wraps with `u32`.
    quantum_bucket: u32,
}

impl ScopeTaps {
    /// Allocate `channel_count` rings and split them. **The only allocation.**
    pub fn new(channel_count: usize) -> (ScopeTaps, Box<[TapReader]>) {
        let mut writers = Vec::with_capacity(channel_count);
        let mut readers = Vec::with_capacity(channel_count);
        for _ in 0..channel_count {
            let (writer, reader) = TapRing::new();
            writers.push(writer);
            readers.push(reader);
        }
        let taps = ScopeTaps { writers: writers.into_boxed_slice(), quantum_bucket: 0 };
        (taps, readers.into_boxed_slice())
    }

    /// Channels with a ring.
    pub fn channel_count(&self) -> usize { self.writers.len() }

    /// Clear this quantum's buckets, so a segment can sum into them.
    ///
    /// Zeroing here rather than tracking which buckets a segment touched is what lets the
    /// per-voice walk be a plain saturating add: a channel nothing plays on this quantum
    /// reads as silence because nothing added to its zeros, not because anything had to
    /// notice it was idle.
    pub fn begin_quantum(&mut self) {
        for writer in self.writers.iter() {
            for bucket in 0..TAP_BUCKETS_PER_QUANTUM as u32 {
                writer.write(self.quantum_bucket.wrapping_add(bucket), 0);
            }
        }
    }

    /// Sample every sounding voice for the buckets that begin inside
    /// `offset .. offset + span` of the current quantum.
    ///
    /// Call it **before** the segment is accumulated, with the same `pcm` blob the mixer
    /// is about to read, so the positions sampled are the ones the segment renders from.
    pub fn sample_segment(&mut self, voices: &VoicePool, pcm: &[i16], offset: usize, span: usize) {
        let first_bucket = offset.div_ceil(TAP_BUCKET_FRAMES);
        let last_bucket = offset.saturating_add(span).div_ceil(TAP_BUCKET_FRAMES).min(TAP_BUCKETS_PER_QUANTUM);
        if first_bucket >= last_bucket {
            return;
        }

        for (_, voice) in voices.iter() {
            let Some(writer) = self.writers.get(voice.tag.channel as usize) else { continue };
            // A voice on a channel the rings were not sized for. The rings are allocated
            // once, so there is nowhere to put it; the mixer still plays it.
            let Some(sample) = SampleData::resolve(pcm, voice.region()) else { continue };

            let position = voice.position() as i128;
            let step = voice.params.step.to_bits() as i128;
            let reverse = voice.is_reversed();
            let volume = voice.params.volume.to_bits() as i32;

            for bucket in first_bucket..last_bucket {
                let frames_ahead = (bucket * TAP_BUCKET_FRAMES).saturating_sub(offset) as i128;
                let travelled = frames_ahead * step;
                let ahead = if reverse { position - travelled } else { position + travelled };
                // A one-shot that has run past its end writes nothing, which leaves the
                // zero `begin_quantum` put there: the trace falls silent where the voice
                // does.
                let Some(frame) = folded_frame(sample, ahead, reverse) else { continue };
                let scaled = ((frame as i32 * volume) >> 16).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                writer.accumulate(self.quantum_bucket.wrapping_add(bucket as u32), scaled);
            }
        }
    }

    /// Publish the quantum and move the cursor on.
    pub fn end_quantum(&mut self) {
        self.quantum_bucket = self.quantum_bucket.wrapping_add(TAP_BUCKETS_PER_QUANTUM as u32);
        for writer in self.writers.iter() {
            writer.commit(self.quantum_bucket);
        }
    }
}

impl core::fmt::Debug for ScopeTaps {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("ScopeTaps")
            .field("channels", &self.writers.len())
            .field("quantum_bucket", &self.quantum_bucket)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    use starplayer_core::{Step, U0F16, VoiceParams};
    use starplayer_mixer::{LoopSpan, SampleRegion, VoiceTag, append_guarded_sample};

    /// A looping ramp: frame `index` holds `index * 64`, so a bucket's value names the
    /// frame it was read at.
    fn ramp_blob(frames: usize) -> (Vec<i16>, SampleRegion) {
        let pcm: Vec<i16> = (0..frames).map(|index| (index as i16) * 64).collect();
        let mut blob = Vec::new();
        let region = append_guarded_sample(&mut blob, &pcm, LoopSpan::new(0, frames as u32));
        (blob, region)
    }

    fn params(volume: U0F16) -> VoiceParams {
        VoiceParams { step: Step::ONE, volume, ..VoiceParams::SILENT }
    }

    fn tag(channel: u8) -> VoiceTag { VoiceTag { channel, instrument: 1, sample: 1, note: 60 } }

    #[test]
    fn one_voice_fills_its_own_channel_with_the_frames_the_buckets_begin_at() {
        let (blob, region) = ramp_blob(256);
        let mut voices = VoicePool::new(4);
        voices.allocate(tag(2), region, params(U0F16::from_bits(32_768)), 0).expect("a fresh pool has room");

        let (mut taps, readers) = ScopeTaps::new(4);
        taps.begin_quantum();
        taps.sample_segment(&voices, &blob, 0, 128);
        taps.end_quantum();

        let mut window = vec![0i16; TAP_BUCKETS_PER_QUANTUM];
        let seen = readers.get(2).expect("channel 2 has a ring").latest(&mut window);
        assert_eq!(seen, TAP_BUCKETS_PER_QUANTUM as u32);
        // Half volume of a ramp reading frame `4 * bucket`, whose value is `256 * bucket`.
        let expected: Vec<i16> = (0..TAP_BUCKETS_PER_QUANTUM).map(|bucket| (bucket as i16) * 128).collect();
        assert_eq!(window, expected);

        let mut silent = vec![-1i16; TAP_BUCKETS_PER_QUANTUM];
        readers.first().expect("channel 0 has a ring").latest(&mut silent);
        assert_eq!(silent, vec![0i16; TAP_BUCKETS_PER_QUANTUM], "a channel nothing plays on stays silent");
    }

    #[test]
    fn a_quantum_split_into_segments_produces_the_same_buckets_as_one_segment() {
        let (blob, region) = ramp_blob(256);
        let mut whole = VoicePool::new(2);
        whole.allocate(tag(0), region, params(U0F16::MAX), 0).expect("room");
        let mut split = VoicePool::new(2);
        split.allocate(tag(0), region, params(U0F16::MAX), 0).expect("room");

        let (mut whole_taps, whole_readers) = ScopeTaps::new(1);
        whole_taps.begin_quantum();
        whole_taps.sample_segment(&whole, &blob, 0, 128);
        whole_taps.end_quantum();

        // The same quantum cut at 37 and 90 — neither a bucket boundary — with each
        // segment's voice position advanced the way the mixer would have advanced it.
        let (mut split_taps, split_readers) = ScopeTaps::new(1);
        split_taps.begin_quantum();
        let mut rendered = 0usize;
        for span in [37usize, 53, 38] {
            split_taps.sample_segment(&split, &blob, rendered, span);
            for (_, voice) in split.iter_mut() {
                let advanced = voice.position() + ((span as u64) << 32);
                voice.set_position(advanced % ((256u64) << 32));
            }
            rendered += span;
        }
        split_taps.end_quantum();

        let mut one = vec![0i16; TAP_BUCKETS_PER_QUANTUM];
        let mut many = vec![0i16; TAP_BUCKETS_PER_QUANTUM];
        whole_readers.first().expect("a ring").latest(&mut one);
        split_readers.first().expect("a ring").latest(&mut many);
        assert_eq!(many, one, "a bucket belongs to exactly one segment, and reads the same frame either way");
    }

    #[test]
    fn two_voices_on_one_channel_sum_and_saturate() {
        let (blob, region) = ramp_blob(256);
        let mut voices = VoicePool::new(4);
        for _ in 0..2 {
            voices.allocate(tag(1), region, params(U0F16::MAX), 0).expect("room");
        }

        let (mut taps, readers) = ScopeTaps::new(2);
        taps.begin_quantum();
        taps.sample_segment(&voices, &blob, 0, 128);
        taps.end_quantum();

        let mut window = vec![0i16; TAP_BUCKETS_PER_QUANTUM];
        readers.get(1).expect("channel 1 has a ring").latest(&mut window);
        // Bucket b reads frame 4b, worth 256b at full volume; two voices make it 512b,
        // which saturates from bucket 64 — past a quantum's 32 — so check the sum, and
        // then check saturation directly by driving the same bucket four times.
        assert_eq!(window.get(4).copied(), Some(2 * 4 * 4 * 64 - 2), "two voices sum");
        assert!(window.iter().all(|value| *value >= 0), "no wrap into the negatives");

        let (mut loud_taps, loud_readers) = ScopeTaps::new(1);
        let mut loud = VoicePool::new(8);
        for _ in 0..8 {
            loud.allocate(tag(0), region, params(U0F16::MAX), 200).expect("room");
        }
        loud_taps.begin_quantum();
        loud_taps.sample_segment(&loud, &blob, 0, 128);
        loud_taps.end_quantum();
        let mut window = vec![0i16; TAP_BUCKETS_PER_QUANTUM];
        loud_readers.first().expect("a ring").latest(&mut window);
        assert_eq!(window.first().copied(), Some(i16::MAX), "eight loud voices saturate rather than wrapping");
    }

    #[test]
    fn a_voice_on_a_channel_past_the_ring_count_is_skipped_rather_than_folded_onto_another() {
        let (blob, region) = ramp_blob(256);
        let mut voices = VoicePool::new(2);
        voices.allocate(tag(9), region, params(U0F16::MAX), 0).expect("room");

        let (mut taps, readers) = ScopeTaps::new(4);
        taps.begin_quantum();
        taps.sample_segment(&voices, &blob, 0, 128);
        taps.end_quantum();

        for reader in readers.iter() {
            let mut window = vec![-1i16; TAP_BUCKETS_PER_QUANTUM];
            reader.latest(&mut window);
            assert_eq!(window, vec![0i16; TAP_BUCKETS_PER_QUANTUM], "nothing landed on a channel that was not asked for");
        }
    }
}
