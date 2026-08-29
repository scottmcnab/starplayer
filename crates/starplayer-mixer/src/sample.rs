//! Sample data as the mixer sees it: [`GUARD_FRAMES`], [`LoopSpan`], [`SampleRegion`]
//! (offsets) and [`SampleData`] (the resolved borrow).
//!
//! # Offsets in the voice, a borrow in the kernel
//!
//! Architecture §6 pins the module layout as one `pcm: Box<[i16]>` blob plus `u32`
//! offsets — no nested references, so `Arc<Module>` is trivially `Send + Sync`, the whole
//! thing is hashable for goldens, and an embedded target can borrow sample data straight
//! out of memory-mapped flash. A [`Voice`](crate::voice::Voice) therefore stores a
//! [`SampleRegion`], and the render kernel resolves it against the blob once per segment
//! into a [`SampleData`].
//!
//! # Where this type goes in M1
//!
//! [`SampleData`] is a **view**, not a home. `starplayer-model` is still a stub; when
//! **M1-task-B1** builds the real `Module`, the blob and the `SampleIndex` table move
//! there and `SampleRegion` becomes the mixer-facing projection of `SampleIndex`. Nothing
//! in this module needs to change shape for that — only where the bytes come from.

use alloc::vec::Vec;

/// Frames appended to every sample's PCM so an interpolator can read past the end of the
/// data, or past the loop point, without a branch in the inner loop (architecture §6).
///
/// # Why eight (task A3, research point 2)
///
/// The count has to satisfy the widest kernel that can ever be selected at run time, not
/// the widest one implemented today, because it is baked into the sample data by the
/// loader and changing it later is a format-wide change:
///
/// | Kernel | Frames read at or after `index` | Guard frames needed |
/// |---|---|---|
/// | [`Nearest`](starplayer_dsp::Nearest) | `index` | 0 |
/// | [`Linear`](starplayer_dsp::Linear) | `index + 1` | 1 |
/// | Cubic Hermite, 4-tap (M7) | `index + 2` | 2 |
/// | Windowed sinc, 8-tap (M7) | `index + 4` | 4 |
///
/// Eight is the next power of two above that maximum. It leaves room for a 16-tap sinc
/// (which would need 8) without another format-wide change, keeps each sample's data
/// 16-byte-aligned in length terms, and costs 16 bytes per sample — nothing next to the
/// sample itself.
///
/// The *leading* taps a symmetric kernel wants (`index - 3` for an 8-tap sinc) are a
/// different problem with a different answer — a pre-roll before the sample start and
/// before the loop start — and belong to M7 along with the kernels that need them.
pub const GUARD_FRAMES: usize = 8;

/// Enforced at compile time rather than in a test, so that adding a kernel which needs
/// more guard frames than the samples carry cannot build at all.
const _: () = assert!(GUARD_FRAMES >= <starplayer_dsp::Linear as starplayer_dsp::Interpolate>::GUARD_FRAMES_REQUIRED);
const _: () = assert!(GUARD_FRAMES >= 4, "an 8-tap windowed sinc reads index + 4 (M7)");

/// A forward loop, in source frames: `start..end`, half-open.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LoopSpan {
    start: u32,
    end: u32,
}

impl LoopSpan {
    /// A loop over `start..end`, or `None` if that span is empty or inverted.
    pub const fn new(start: u32, end: u32) -> Option<LoopSpan> {
        if end > start { Some(LoopSpan { start, end }) } else { None }
    }

    /// First frame of the loop.
    pub const fn start(self) -> u32 { self.start }

    /// One past the last frame of the loop.
    pub const fn end(self) -> u32 { self.end }

    /// Frames in the loop. Always at least one.
    pub const fn length(self) -> u32 { self.end - self.start }
}

/// Where one sample lives in the module's PCM blob, and how it loops.
///
/// `length_frames` counts the **addressable** frames; the [`GUARD_FRAMES`] that follow
/// them in the blob are readable by the interpolator but are never a playback position.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SampleRegion {
    pcm_offset: u32,
    length_frames: u32,
    loop_span: Option<LoopSpan>,
}

impl Default for SampleRegion {
    /// An empty one-shot: a voice built from it finishes on its very first frame, which
    /// is what a slot in a fresh [`VoicePool`](crate::voice::VoicePool) holds.
    fn default() -> SampleRegion { SampleRegion::one_shot(0, 0) }
}

impl SampleRegion {
    /// A sample that plays once and then ends the voice.
    pub const fn one_shot(pcm_offset: u32, length_frames: u32) -> SampleRegion {
        SampleRegion { pcm_offset, length_frames, loop_span: None }
    }

    /// A forward-looping sample.
    ///
    /// **The addressable length is the loop end**, deliberately. The guard frames of a
    /// looping sample hold a copy of the frames from `loop_start` onward, so that a
    /// kernel reading past `loop_end` sees the start of the loop rather than whatever
    /// followed it in the file. That only works if nothing else follows it — so a
    /// forward-looping sample stores frames `0..loop_end` and discards the tail, which is
    /// sound because a forward loop never plays a frame at or after `loop_end`.
    ///
    /// Sustain loops and bidirectional loops (M6) break that assumption and will carry
    /// their own small lookahead buffers rather than widening this one.
    ///
    /// A degenerate span collapses to [`SampleRegion::one_shot`].
    pub const fn looping(pcm_offset: u32, loop_span: LoopSpan) -> SampleRegion {
        SampleRegion { pcm_offset, length_frames: loop_span.end, loop_span: Some(loop_span) }
    }

    /// Offset of the sample's first frame within the module's PCM blob.
    pub const fn pcm_offset(self) -> u32 { self.pcm_offset }

    /// Addressable frames, excluding the guard frames.
    pub const fn length_frames(self) -> u32 { self.length_frames }

    /// The forward loop, if the sample has one.
    pub const fn loop_span(self) -> Option<LoopSpan> { self.loop_span }

    /// Frames this region occupies in the blob, guard frames included.
    pub const fn stored_frames(self) -> usize { self.length_frames as usize + GUARD_FRAMES }
}

/// A [`SampleRegion`] resolved against a PCM blob.
///
/// [`SampleData::frames`] includes the guard frames, so an interpolator may read up to
/// [`GUARD_FRAMES`] past `length_frames` without a bounds failure.
#[derive(Copy, Clone, Debug)]
pub struct SampleData<'pcm> {
    frames: &'pcm [i16],
    length_frames: u32,
    loop_span: Option<LoopSpan>,
}

impl<'pcm> SampleData<'pcm> {
    /// Resolve `region` against `blob`, or `None` if the region does not fit.
    ///
    /// A `None` here means a corrupt or mismatched module; the mixer treats it as a voice
    /// that has finished rather than as an error, because `render()` may not panic.
    pub fn resolve(blob: &'pcm [i16], region: SampleRegion) -> Option<SampleData<'pcm>> {
        let start = region.pcm_offset() as usize;
        let end = start.checked_add(region.stored_frames())?;
        let frames = blob.get(start..end)?;
        Some(SampleData { frames, length_frames: region.length_frames(), loop_span: region.loop_span() })
    }

    /// The sample's frames, guard frames included. Indices are sample-relative.
    pub const fn frames(&self) -> &'pcm [i16] { self.frames }

    /// Addressable frames, excluding the guard frames.
    pub const fn length_frames(&self) -> u32 { self.length_frames }

    /// The forward loop, if the sample has one.
    pub const fn loop_span(&self) -> Option<LoopSpan> { self.loop_span }
}

/// Append `pcm` to a module PCM blob with its [`GUARD_FRAMES`] filled in, and return the
/// [`SampleRegion`] that addresses it.
///
/// This is the function a loader calls once per sample in **M1-task-B1** while it builds
/// the module's single `pcm` blob; it is here rather than in `starplayer-model` because
/// the guard-frame contract belongs to the mixer that relies on it.
///
/// * **Looping** — the stored frames are `0..loop_end` (see [`SampleRegion::looping`])
///   and the guard frames repeat the loop from `loop_start`, wrapping as many times as
///   it takes for a loop shorter than [`GUARD_FRAMES`].
/// * **One-shot** — the whole sample is stored and the guard frames are silence, so a
///   kernel interpolating over the final frame decays to zero instead of clicking.
///
/// A `loop_span` that does not fit inside `pcm` is ignored and the sample is stored as a
/// one-shot; a loader that cares should validate before calling.
pub fn append_guarded_sample(blob: &mut Vec<i16>, pcm: &[i16], loop_span: Option<LoopSpan>) -> SampleRegion {
    let pcm_offset = blob.len().min(u32::MAX as usize) as u32;
    let usable_loop = loop_span.filter(|span| span.end() as usize <= pcm.len());

    match usable_loop {
        Some(span) => {
            if let Some(body) = pcm.get(..span.end() as usize) {
                blob.extend_from_slice(body);
            }
            for guard_index in 0..GUARD_FRAMES {
                let wrapped = span.start() as usize + guard_index % span.length() as usize;
                blob.push(pcm.get(wrapped).copied().unwrap_or(0));
            }
            SampleRegion::looping(pcm_offset, span)
        }
        None => {
            blob.extend_from_slice(pcm);
            for _ in 0..GUARD_FRAMES {
                blob.push(0);
            }
            SampleRegion::one_shot(pcm_offset, pcm.len().min(u32::MAX as usize) as u32)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn a_one_shot_sample_is_guarded_with_silence() {
        let mut blob = Vec::new();
        let region = append_guarded_sample(&mut blob, &[10, 20, 30], None);

        assert_eq!(region.pcm_offset(), 0);
        assert_eq!(region.length_frames(), 3);
        assert_eq!(region.loop_span(), None);
        assert_eq!(blob, vec![10, 20, 30, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_looping_sample_is_truncated_at_loop_end_and_wrapped_into_the_guard() {
        let mut blob = Vec::new();
        let pcm = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        let span = LoopSpan::new(4, 8).expect("a valid loop span");
        let region = append_guarded_sample(&mut blob, &pcm, Some(span));

        assert_eq!(region.length_frames(), 8, "the tail after loop_end is never audible and is discarded");
        assert_eq!(region.loop_span(), Some(span));
        assert_eq!(blob, vec![0, 1, 2, 3, 4, 5, 6, 7, /* guard: */ 4, 5, 6, 7, 4, 5, 6, 7]);
    }

    #[test]
    fn a_loop_shorter_than_the_guard_wraps_repeatedly() {
        let mut blob = Vec::new();
        let span = LoopSpan::new(1, 3).expect("a valid loop span");
        append_guarded_sample(&mut blob, &[7, 8, 9], Some(span));
        assert_eq!(blob, vec![7, 8, 9, /* guard: */ 8, 9, 8, 9, 8, 9, 8, 9]);
    }

    #[test]
    fn an_out_of_range_loop_falls_back_to_one_shot() {
        let mut blob = Vec::new();
        let span = LoopSpan::new(1, 99).expect("a valid loop span");
        let region = append_guarded_sample(&mut blob, &[7, 8, 9], Some(span));
        assert_eq!(region.loop_span(), None);
        assert_eq!(region.length_frames(), 3);
    }

    #[test]
    fn samples_append_back_to_back_with_their_own_offsets() {
        let mut blob = Vec::new();
        let first = append_guarded_sample(&mut blob, &[1, 2], None);
        let second = append_guarded_sample(&mut blob, &[3], None);
        assert_eq!(first.pcm_offset(), 0);
        assert_eq!(second.pcm_offset(), 2 + GUARD_FRAMES as u32);
        assert_eq!(blob.len(), second.pcm_offset() as usize + second.stored_frames());
    }

    #[test]
    fn resolve_rejects_a_region_that_does_not_fit() {
        let blob = [0i16; 4];
        assert!(SampleData::resolve(&blob, SampleRegion::one_shot(0, 4)).is_none(), "no room for the guard frames");
        assert!(SampleData::resolve(&blob, SampleRegion::one_shot(9, 0)).is_none());
    }

    #[test]
    fn resolve_yields_a_sample_relative_view() {
        let mut blob = Vec::new();
        append_guarded_sample(&mut blob, &[1, 2], None);
        let region = append_guarded_sample(&mut blob, &[30, 40, 50], None);

        let sample = SampleData::resolve(&blob, region).expect("the region fits");
        assert_eq!(sample.length_frames(), 3);
        assert_eq!(sample.frames().first().copied(), Some(30));
        assert_eq!(sample.frames().len(), 3 + GUARD_FRAMES);
    }
}
