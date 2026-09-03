//! [`Voice`], [`VoiceTag`] and the fixed-capacity [`VoicePool`] (architecture §5).

use alloc::vec;
use alloc::boxed::Box;

use starplayer_core::{DirtyBits, FilterParams, VoiceId, VoiceParams};
use starplayer_dsp::{FilterCoefficients, GainRamp, Interpolate};

use crate::gain::{RAMP_FRAMES, voice_gain_units};
use crate::kernel::{VoiceStatus, accumulate_voice};
use crate::path::{MixPath, Stereo};
use crate::sample::SampleRegion;

/// What a voice is playing, for the benefit of code that has to *find* voices rather than
/// drive them (architecture §5.1).
///
/// Five bytes — six once padded — present from the start so IT's Duplicate Check has
/// somewhere to look in M6. Every other format early-outs the whole matching loop, so the
/// cost to MOD, S3M and MTM is these bytes and one branch.
///
/// `sample` is sixteen bits because IT's Duplicate Check compares sample *numbers* and an
/// IT module may carry up to 99 samples per instrument across a bank far wider than a
/// byte: a clamped number would make two different samples compare equal and cut the
/// wrong voice.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct VoiceTag {
    /// Which channel triggered the voice. Still set for an IT background voice, which is
    /// no longer *owned* by that channel.
    pub channel: u8,
    /// One-based tracker instrument number; zero means none.
    pub instrument: u8,
    /// One-based tracker sample number; zero means none.
    pub sample: u16,
    /// Which note.
    pub note: u8,
}

/// One mixing path's half of a voice's resonant filter: the delay line, and the
/// coefficients it is currently being spent with.
///
/// `state` is `[y[n−1], y[n−2]]`, in whatever the path's
/// [`Mono`](crate::path::MixPath::Mono) type is — on the fixed path it is kept
/// pre-amplified (`starplayer_dsp::FILTER_PREAMP_BITS`), which is why it is `i32` rather
/// than `i16`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PathFilter<Sample> {
    /// The two-pole delay line, most recent first.
    pub state: [Sample; 2],
    /// What [`VoiceFilter::refresh`] last computed for this path.
    pub coefficients: FilterCoefficients<Sample>,
}

impl Default for PathFilter<f32> {
    fn default() -> PathFilter<f32> { PathFilter { state: [0.0; 2], coefficients: FilterCoefficients::<f32>::PASS_THROUGH } }
}

impl Default for PathFilter<i32> {
    fn default() -> PathFilter<i32> { PathFilter { state: [0; 2], coefficients: FilterCoefficients::<i32>::PASS_THROUGH } }
}

/// One voice's resonant low-pass state (IT; architecture §7.2).
///
/// # Why both paths are carried at once
///
/// A [`Voice`] is not generic over the mixing path — one pool serves whichever path the
/// engine was built with — so the struct holds a delay line and a coefficient set for
/// each and [`MixPath::path_filter`] picks. The unused
/// half costs twenty bytes and is never touched: in particular `refresh` computes only
/// the coefficients of the path that asked, so a bare-metal fixed-path build never
/// evaluates a float expression here.
///
/// # Why there is no dirty bit
///
/// A filter write raises [`DirtyBits::PITCH`] today and the trace deliberately reports no
/// flag for one (`dirty_bit_for`). Rather than add a bit — which would change the trace's
/// flag column and every golden that carries it — this compares the four bytes of
/// [`FilterParams`] against what the coefficients were computed from. That is cheaper
/// than a bit test, cannot be missed by an owner that forgets to raise it, and covers a
/// sample-rate change as well.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct VoiceFilter {
    /// The float path's delay line and coefficients.
    pub float: PathFilter<f32>,
    /// The fixed path's delay line and coefficients.
    pub fixed: PathFilter<i32>,
    /// What the coefficients were computed from.
    source: FilterParams,
    /// The sample rate they were computed at. Zero means "nothing computed yet", which no
    /// real rate is.
    source_sample_rate_hz: u32,
    /// The IT header's extended filter range (OpenMPT's `SONG_EXFILTERRANGE`).
    extended_range: bool,
    /// Whether the filter is doing anything at all.
    active: bool,
}

impl VoiceFilter {
    /// Whether the kernel has to run the filter for this voice.
    pub const fn is_active(&self) -> bool { self.active }

    /// Whether this voice's cutoff law uses IT's extended filter range.
    pub const fn has_extended_range(&self) -> bool { self.extended_range }

    /// Select the extended filter range, invalidating any cached coefficients.
    ///
    /// A module-level property that only the format's own processor knows, so the owner
    /// of the voice sets it; it defaults to off, which is what every module without the
    /// IT header bit wants.
    pub const fn set_extended_range(&mut self, extended_range: bool) {
        self.extended_range = extended_range;
        self.source_sample_rate_hz = 0;
    }

    /// Zero the delay line — a new note, not a change of parameters.
    pub const fn reset_state(&mut self) {
        self.float.state = [0.0; 2];
        self.fixed.state = [0; 2];
    }

    /// Recompute `Path`'s coefficients if, and only if, something they depend on moved.
    ///
    /// In practice this is once per tracker tick for a voice whose filter is being swept
    /// and never again for one that is not — never per frame, which is the whole point of
    /// caching them on the voice.
    pub fn refresh<Path: MixPath>(&mut self, filter: FilterParams, sample_rate_hz: u32) {
        if self.source == filter && self.source_sample_rate_hz == sample_rate_hz {
            return;
        }
        self.source = filter;
        self.source_sample_rate_hz = sample_rate_hz;
        // IT's own rule, from `SetupChannelFilter`: a fully open cutoff with no resonance
        // is not a filter at all. `FilterParams::from_it` maps exactly that pair onto
        // `FilterParams::BYPASS`, so one comparison answers it.
        self.active = !filter.is_bypass();
        if self.active {
            let (cutoff, resonance) = filter.to_it();
            Path::path_filter(self).coefficients = Path::coefficients(cutoff, resonance, sample_rate_hz, self.extended_range);
        }
    }
}

/// One sounding sample.
///
/// # The ramp state lives here
///
/// A voice carries its own left/right [`GainRamp`] pair rather than the mixer keeping a
/// side table, because the ramp is part of what the voice *is*: it survives being split
/// across render segments, it is what makes a stop a fade rather than a cut, and it has to
/// travel with the voice when the pool hands it out. Both ramps advance one step per
/// output frame and know nothing about block boundaries, which is what keeps a ramped
/// render byte-identical at every host buffer size.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Voice {
    /// Pitch, volume, pan, filter and the dirty bits. Written directly by whoever owns
    /// the voice, never queued (architecture §2.1).
    pub params: VoiceParams,
    /// What this voice is playing, for Duplicate Check and for telemetry.
    pub tag: VoiceTag,
    region: SampleRegion,
    /// Q32.32 playback position within the sample, sample-relative.
    position: u64,
    /// Left/right gain, in the units of [`voice_gain_units`], mid-ramp.
    gains: Stereo<GainRamp>,
    /// IT's per-voice resonant low-pass: the delay line, the coefficients, and what they
    /// were computed from. Bypassed — and therefore free — for MOD, S3M and MTM.
    filter: VoiceFilter,
    /// Whether a ping-pong loop is currently travelling backwards.
    reverse: bool,
    /// Whether the voice is fading out towards release.
    stopping: bool,
    /// A region queued to replace `region` the next time the voice reaches a boundary.
    ///
    /// ProTracker's instrument-only and tone-portamento sample swaps write the new
    /// sample's pointer and length into Paula but leave DMA running, so the change lands
    /// when the channel next reloads those registers — at the loop point, or at the end
    /// of a one-shot. One fixed-size slot per voice keeps that RT-safe: no allocation, no
    /// lock, and the accumulation loop looks at it only where it already handles the
    /// boundary. See accuracy policy D12.
    pending_region: Option<SampleRegion>,
}

impl Voice {
    /// A voice playing `region` from `offset_frames`, with `params`.
    ///
    /// The gain **ramps up from silence** over [`RAMP_FRAMES`] rather than starting at
    /// full volume. Most samples start near zero and would not click either way, but the
    /// ones that do not — a looped waveform captured mid-cycle, which is most of a
    /// chiptune's instruments — click on every single note otherwise. This is the modern
    /// equivalent of what the GUS driver got from hardware: `_GIRQStartVoice` programmed
    /// the volume and *then* started the voice, one ramp behind the note
    /// (`plans/reference/original-s3mlib-analysis.md` §7).
    pub fn new(tag: VoiceTag, region: SampleRegion, params: VoiceParams, offset_frames: u32) -> Voice {
        let mut voice = Voice {
            params,
            tag,
            region,
            position: (offset_frames as u64) << 32,
            gains: Stereo::new(GainRamp::steady(0), GainRamp::steady(0)),
            // A new note starts the filter from silence, the way `HandleNoteChangeFilter`
            // calls `SetupChannelFilter(chn, true)` only when `chn.triggerNote` is set.
            filter: VoiceFilter::default(),
            reverse: false,
            stopping: false,
            pending_region: None,
        };
        voice.params.dirty.remove(DirtyBits::STOP);
        voice.glide_to_params();
        voice
    }

    /// Skip the attack ramp and start at full gain.
    ///
    /// For a caller that knows its sample starts at zero and wants the attack unaltered —
    /// and for tests that would rather assert on a steady gain than on a ramp.
    pub fn settle_gains(&mut self) {
        let units = voice_gain_units(self.params.volume, self.params.pan);
        self.gains.left.jump_to(units.left);
        self.gains.right.jump_to(units.right);
    }

    /// The gain both channels are currently mixing at, in [`voice_gain_units`] units.
    pub const fn current_gain_units(&self) -> Stereo<i32> {
        Stereo::new(self.gains.left.current(), self.gains.right.current())
    }

    /// Whether either channel's gain is still moving.
    pub const fn is_ramping(&self) -> bool { self.gains.left.is_ramping() || self.gains.right.is_ramping() }

    /// Whether a ping-pong loop is currently travelling backwards.
    pub const fn is_reversed(&self) -> bool { self.reverse }

    /// Set the ping-pong direction. The kernel's other write-back.
    pub const fn set_reversed(&mut self, reverse: bool) { self.reverse = reverse; }

    /// Frames of gain ramp left to run, over both channels.
    pub(crate) fn gain_ramp_frames_remaining(&self) -> u32 {
        self.gains.left.frames_remaining().max(self.gains.right.frames_remaining())
    }

    /// Everything one bounded run mutates that is not the position: the gain ramps to
    /// advance and the filter to spend. Handed out as one pair because the kernel needs
    /// both at once and they are disjoint fields.
    pub(crate) fn run_state_mut(&mut self) -> (&mut Stereo<GainRamp>, &mut VoiceFilter) { (&mut self.gains, &mut self.filter) }

    /// This voice's resonant filter.
    pub const fn filter(&self) -> &VoiceFilter { &self.filter }

    /// This voice's resonant filter, for its owner to configure — the extended filter
    /// range is the one thing about it that comes from outside the mixer.
    pub const fn filter_mut(&mut self) -> &mut VoiceFilter { &mut self.filter }

    /// Whether a stopped voice has finished fading and may be released.
    pub(crate) fn finished_ramping_out(&self) -> bool { self.stopping && !self.is_ramping() }

    /// Consume the dirty bits and point the gain ramps at wherever they now have to go.
    ///
    /// `VOLUME` and `PAN` both retarget one composite gain, so a pan slide and a volume
    /// slide landing on the same frame produce one movement rather than two. `SAMPLE` does
    /// **not** retarget anything: a retrigger changes the waveform, not the gain, and
    /// smoothing the waveform discontinuity a retrigger creates means deferring the restart
    /// behind a fade — which is what the GUS driver did in its ramp-end IRQ, and which
    /// belongs to whoever owns the channel, not to the mixer.
    ///
    /// Returns [`VoiceStatus::Finished`] for a stopped voice that has nothing left to fade.
    pub(crate) fn retarget_gains(&mut self) -> VoiceStatus {
        if self.params.dirty.contains(DirtyBits::STOP) {
            self.stopping = true;
        }
        if self.stopping {
            self.gains.left.glide_to(0, RAMP_FRAMES);
            self.gains.right.glide_to(0, RAMP_FRAMES);
        } else if self.params.dirty.intersects(DirtyBits::VOLUME | DirtyBits::PAN) {
            self.glide_to_params();
        }
        // Consumed, the way the original's `SB_ProcessTracks` ends with
        // `mov [edi+_ChannelFlag],0` (`STARPLAY/S3MLIB.ASM` ~5828).
        self.params.clear_dirty();

        if self.finished_ramping_out() { VoiceStatus::Finished } else { VoiceStatus::Sounding }
    }

    fn glide_to_params(&mut self) {
        let units = voice_gain_units(self.params.volume, self.params.pan);
        self.gains.left.glide_to(units.left, RAMP_FRAMES);
        self.gains.right.glide_to(units.right, RAMP_FRAMES);
    }

    /// Which sample this voice plays, and where it lives in the PCM blob.
    pub const fn region(&self) -> SampleRegion { self.region }

    /// Q32.32 playback position, relative to the sample's first frame.
    pub const fn position(&self) -> u64 { self.position }

    /// Move the playback position. The kernel's only write-back.
    pub const fn set_position(&mut self, position: u64) { self.position = position; }

    /// Point this voice at a different sample without disturbing its position — what
    /// `Gxx` tone portamento and IT's sample-swap semantics need in M1.
    ///
    /// The resonant filter's delay line is **not** reset. This is a swap in the middle of
    /// a sounding note, not a new note: OpenMPT resets the filter only under
    /// `chn.triggerNote` (`HandleNoteChangeFilter`), and `FilterPortaSmpChange.it` is the
    /// case that pins it. See accuracy policy D73.
    pub fn set_region(&mut self, region: SampleRegion) {
        self.region = region;
        self.pending_region = None;
        self.params.dirty.insert(DirtyBits::SAMPLE);
    }

    /// Queue `region` to replace this voice's sample the next time it reaches a boundary
    /// — a forward loop's wrap, or the end of a one-shot (accuracy policy D12).
    ///
    /// A zero-length region is ProTracker's null sample: the voice stops at that
    /// boundary rather than continuing. A second queue before the boundary replaces the
    /// first, which is what writing the Paula registers twice does.
    pub fn queue_region(&mut self, region: SampleRegion) { self.pending_region = Some(region); }

    /// The region waiting for the next boundary, if any.
    pub const fn pending_region(&self) -> Option<SampleRegion> { self.pending_region }

    /// Take the queued region, leaving the slot empty. The kernel's boundary handler.
    pub(crate) const fn take_pending_region(&mut self) -> Option<SampleRegion> { self.pending_region.take() }

    /// Adopt a queued region at a boundary, without the [`DirtyBits::SAMPLE`] a caller's
    /// explicit [`Voice::set_region`] would raise: nothing outside the voice asked for
    /// this, and the gain ramps must not be disturbed by it.
    pub(crate) const fn adopt_region(&mut self, region: SampleRegion) { self.region = region; }

    /// Restart from `offset_frames` (`Oxx`, and every plain retrigger).
    ///
    /// A retrigger is a position change, and ProTracker's queued swap takes effect on one
    /// immediately rather than waiting for a boundary that the restart has just moved
    /// (libxmp `libxmp_mixer_voicepos`, OpenMPT `InstrSwapRetrigger.mod`).
    pub fn retrigger(&mut self, offset_frames: u32) {
        if let Some(region) = self.pending_region.take() { self.region = region; }
        self.position = (offset_frames as u64) << 32;
        self.reverse = false;
        // A restart is a new note, and OpenMPT resets the filter's delay line on one
        // (`HandleNoteChangeFilter` → `SetupChannelFilter(chn, true)`). Carrying the two
        // delay values across a jump to a different part of the waveform would ring the
        // filter with a discontinuity that was never in the signal.
        self.filter.reset_state();
        self.params.dirty.insert(DirtyBits::SAMPLE);
    }

    /// Whether the owner has asked this voice to stop — the original's `_CHN_StopVoice`.
    pub const fn wants_stop(&self) -> bool {
        self.stopping || self.params.dirty.contains(DirtyBits::STOP)
    }

    /// Ask this voice to stop.
    ///
    /// The voice **fades** over [`RAMP_FRAMES`] and is released when the fade lands, rather
    /// than being cut where it stands. A hard cut of a voice mid-waveform is a step
    /// discontinuity — the loudest click a mixer can make — and every S3M `SCx` note cut,
    /// every `Kxx`, and every note that steals a voice would produce one.
    pub fn stop(&mut self) { self.params.dirty.insert(DirtyBits::STOP); }
}

/// Sentinel for "no slot" in the free list. A pool can hold at most
/// [`VoicePool::MAX_CAPACITY`] voices, so this index is never a real one.
const NO_SLOT: u16 = u16::MAX;

#[derive(Copy, Clone, Debug)]
struct VoiceSlot {
    voice: Voice,
    generation: u16,
    active: bool,
    /// Next slot on the free list, or [`NO_SLOT`].
    next_free: u16,
}

impl Default for VoiceSlot {
    fn default() -> VoiceSlot {
        VoiceSlot { voice: Voice::default(), generation: 0, active: false, next_free: NO_SLOT }
    }
}

/// One global, fixed-capacity pool of voices with generational handles
/// (architecture §5.2).
///
/// # Fixed capacity
///
/// The slots are allocated once, in [`VoicePool::new`], and the pool never grows. A
/// `Vec` that could reallocate is an allocation on the audio thread, which is the
/// landmine architecture §8 exists to defuse. [`VoicePool::allocate`] returning `None`
/// when the pool is full is therefore a *normal* outcome, not an error: it is the point
/// at which IT's voice-stealing heuristic (M6) gets to choose a victim.
///
/// # Generational handles
///
/// A voice can be stolen out from under its owner, so every instrument must tolerate a
/// stale handle. The generation counter is bumped on release, which makes a handle stale
/// the moment its voice ends — before the slot is even reused — and
/// [`VoicePool::get_mut`] returns `None` for it. Silently getting somebody else's voice
/// is a multi-day debugging session; `Option` makes it impossible.
///
/// # Not per-instrument sub-pools
///
/// One global pool, because IT steals across all channels against a single virtual-channel
/// limit and sub-pools fragment.
#[derive(Debug)]
pub struct VoicePool {
    slots: Box<[VoiceSlot]>,
    free_head: u16,
    active_count: u16,
}

impl VoicePool {
    /// The largest pool that can be addressed by a 16-bit slot index, leaving `u16::MAX`
    /// free as the free-list sentinel.
    pub const MAX_CAPACITY: usize = u16::MAX as usize;

    /// Allocate a pool of `capacity` voices. This is the **only** allocation the pool
    /// ever performs; `capacity` is clamped to [`VoicePool::MAX_CAPACITY`].
    pub fn new(capacity: usize) -> VoicePool {
        let capacity = capacity.min(VoicePool::MAX_CAPACITY);
        let mut slots = vec![VoiceSlot::default(); capacity];

        // Link every slot onto the free list, lowest index first, so a fresh pool hands
        // out slot 0, then 1, then 2 — deterministic, and readable in a trace.
        let mut free_head = NO_SLOT;
        for index in (0..capacity).rev() {
            if let Some(slot) = slots.get_mut(index) {
                slot.next_free = free_head;
                free_head = index as u16;
            }
        }

        VoicePool { slots: slots.into_boxed_slice(), free_head, active_count: 0 }
    }

    /// How many voices the pool can hold.
    pub fn capacity(&self) -> usize { self.slots.len() }

    /// How many voices are currently sounding.
    pub fn voices_active(&self) -> usize { self.active_count as usize }

    /// Take a free slot, or `None` if the pool is full.
    ///
    /// Stealing a victim when the pool is full is a **policy** and lands with IT in M6;
    /// until then a full pool simply drops the note, which is what the original does.
    pub fn allocate(&mut self, tag: VoiceTag, region: SampleRegion, params: VoiceParams, offset_frames: u32) -> Option<VoiceId> {
        let index = self.free_head;
        let slot = self.slots.get_mut(index as usize)?;

        self.free_head = slot.next_free;
        slot.next_free = NO_SLOT;
        slot.active = true;
        slot.voice = Voice::new(tag, region, params, offset_frames);
        self.active_count = self.active_count.saturating_add(1);

        Some(VoiceId::new(index, slot.generation))
    }

    /// Return a voice to the pool, invalidating `id`. Returns whether it was live.
    /// Release every voice at once, invalidating every outstanding [`VoiceId`].
    ///
    /// What a module swap needs: each voice's [`SampleRegion`] indexes the PCM of the
    /// module it was triggered from, so once that module is gone the voice would read the
    /// new module's samples at the old offsets. No ramp — the data it would ramp over is
    /// already the wrong data.
    pub fn release_all(&mut self) {
        for index in 0..self.slots.len() {
            let free_head = self.free_head;
            let Some(slot) = self.slots.get_mut(index) else { continue };
            if !slot.active {
                continue;
            }
            slot.active = false;
            slot.generation = slot.generation.wrapping_add(1);
            slot.next_free = free_head;
            slot.voice = Voice::default();
            self.free_head = index as u16;
            self.active_count = self.active_count.saturating_sub(1);
        }
    }

    pub fn release(&mut self, id: VoiceId) -> bool {
        let free_head = self.free_head;
        let Some(slot) = self.live_slot_mut(id) else { return false };

        slot.active = false;
        slot.generation = slot.generation.wrapping_add(1);
        slot.next_free = free_head;
        slot.voice = Voice::default();

        self.free_head = id.index();
        self.active_count = self.active_count.saturating_sub(1);
        true
    }

    /// The voice `id` refers to, or `None` if it has ended or been stolen.
    pub fn get(&self, id: VoiceId) -> Option<&Voice> {
        let slot = self.slots.get(id.index() as usize)?;
        if slot.active && slot.generation == id.generation() { Some(&slot.voice) } else { None }
    }

    /// The voice `id` refers to, or `None` if it has ended or been stolen.
    pub fn get_mut(&mut self, id: VoiceId) -> Option<&mut Voice> {
        self.live_slot_mut(id).map(|slot| &mut slot.voice)
    }

    /// Every sounding voice, in slot order.
    pub fn iter(&self) -> impl Iterator<Item = (VoiceId, &Voice)> {
        self.slots.iter().enumerate().filter(|(_, slot)| slot.active).map(|(index, slot)| {
            (VoiceId::new(index as u16, slot.generation), &slot.voice)
        })
    }

    /// Every sounding voice, in slot order, mutably — the same ids [`VoicePool::iter`]
    /// hands out.
    ///
    /// # What this is for
    ///
    /// A format that keeps **per-voice articulation state of its own** — XM's and IT's
    /// envelope positions, fadeout level, key-off flag and auto-vibrato phase — holds it
    /// in a parallel array indexed by [`VoiceId::index`] and validated against the id's
    /// generation, and walks this iterator **once per tick** from inside its own
    /// `TrackerProcessor::tick` to advance every entry. That reaches background voices
    /// too: a voice detached by an IT New Note Action is owned by no channel, so a walk
    /// over the channel table would miss it.
    ///
    /// Slot order and active slots only, so the walk is deterministic and a format's own
    /// state advances in a fixed order. No allocation.
    ///
    /// A slot the format never allocated — a future MIDI sample player sharing this pool,
    /// or a scripted test source — shows up here too. Its id will not match the one the
    /// format stored for that slot, so the format skips it rather than adopting it.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (VoiceId, &mut Voice)> {
        self.slots.iter_mut().enumerate().filter(|(_, slot)| slot.active).map(|(index, slot)| {
            (VoiceId::new(index as u16, slot.generation), &mut slot.voice)
        })
    }

    /// Render every sounding voice into `destination`, releasing the ones that end.
    ///
    /// Voices are accumulated in **slot order**, which is what makes the float path's
    /// summation order — and therefore its exact `f32` result — independent of the host's
    /// block size.
    ///
    /// `sample_rate_hz` is the mixer's output rate. Nothing but IT's resonant filter reads
    /// it — the resample step is already a ratio and knows nothing about absolute time —
    /// but the filter's cutoff is a real frequency, so its coefficients cannot be derived
    /// without it.
    pub fn accumulate<Path: MixPath, Interp: Interpolate>(&mut self, pcm: &[i16], destination: &mut [Path::Accumulator], sample_rate_hz: u32) {
        self.accumulate_masked::<Path, Interp>(pcm, destination, &mut [], sample_rate_hz, |_| false);
    }

    /// [`VoicePool::accumulate`], with the voices whose `tag.channel` satisfies `is_muted`
    /// rendered into `discard` instead of `destination`.
    ///
    /// This is how a host mutes a channel. The muted voice keeps running exactly as it
    /// would have — position, loop, ramps, and every parameter write its channel makes —
    /// so unmuting resumes mid-note, and the output at every other channel is
    /// bit-identical to an unmuted render. `discard` must be at least as long as
    /// `destination`; if it is shorter, muted voices are skipped for this call and their
    /// state does not advance, which is the lesser evil next to a panic on the audio
    /// thread.
    ///
    /// A muted voice's **filter runs too**, for exactly the reason its position and ramps
    /// do: the filter is a two-pole recursion whose output depends on the two frames
    /// before it, so skipping it would leave the delay line holding whatever was there
    /// when the channel was muted and unmuting would ring it. Muting is a discard at the
    /// bus, not a shortcut through the voice.
    pub fn accumulate_masked<Path: MixPath, Interp: Interpolate>(
        &mut self,
        pcm: &[i16],
        destination: &mut [Path::Accumulator],
        discard: &mut [Path::Accumulator],
        sample_rate_hz: u32,
        is_muted: impl Fn(u8) -> bool,
    ) {
        let mut free_head = self.free_head;
        let mut active_count = self.active_count;
        let mut discard = discard.get_mut(..destination.len());

        for (index, slot) in self.slots.iter_mut().enumerate() {
            if !slot.active {
                continue;
            }
            let status = if is_muted(slot.voice.tag.channel) {
                match discard.as_deref_mut() {
                    Some(discard) => accumulate_voice::<Path, Interp>(&mut slot.voice, pcm, discard, sample_rate_hz),
                    None => VoiceStatus::Sounding,
                }
            } else {
                accumulate_voice::<Path, Interp>(&mut slot.voice, pcm, destination, sample_rate_hz)
            };
            if status == VoiceStatus::Finished {
                slot.active = false;
                slot.generation = slot.generation.wrapping_add(1);
                slot.next_free = free_head;
                slot.voice = Voice::default();
                free_head = index as u16;
                active_count = active_count.saturating_sub(1);
            }
        }

        self.free_head = free_head;
        self.active_count = active_count;
    }

    fn live_slot_mut(&mut self, id: VoiceId) -> Option<&mut VoiceSlot> {
        let slot = self.slots.get_mut(id.index() as usize)?;
        if slot.active && slot.generation == id.generation() { Some(slot) } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use starplayer_core::{Step, U0F16};
    use starplayer_dsp::Linear;

    use crate::gain::RAMP_FRAMES;
    use crate::path::{FixedFrame, FixedPath};
    use crate::sample::{LoopSpan, append_guarded_sample};

    fn sounding_params() -> VoiceParams {
        VoiceParams { step: Step::ONE, volume: U0F16::MAX, ..VoiceParams::SILENT }
    }

    fn one_shot_blob(frames: usize) -> (Vec<i16>, SampleRegion) {
        let pcm: Vec<i16> = (0..frames).map(|index| 1_000 + index as i16).collect();
        let mut blob = Vec::new();
        let region = append_guarded_sample(&mut blob, &pcm, None);
        (blob, region)
    }

    #[test]
    fn a_fresh_pool_is_empty_and_hands_out_slots_in_order() {
        let mut pool = VoicePool::new(3);
        assert_eq!(pool.capacity(), 3);
        assert_eq!(pool.voices_active(), 0);

        let first = pool.allocate(VoiceTag::default(), SampleRegion::default(), VoiceParams::SILENT, 0).expect("slot 0");
        let second = pool.allocate(VoiceTag::default(), SampleRegion::default(), VoiceParams::SILENT, 0).expect("slot 1");
        assert_eq!((first.index(), second.index()), (0, 1));
        assert_eq!(pool.voices_active(), 2);
    }

    #[test]
    fn iter_mut_visits_exactly_the_active_slots_in_order_with_the_ids_iter_hands_out() {
        let mut pool = VoicePool::new(4);
        let first = pool.allocate(VoiceTag::default(), SampleRegion::default(), VoiceParams::SILENT, 0).expect("slot 0");
        let second = pool.allocate(VoiceTag::default(), SampleRegion::default(), VoiceParams::SILENT, 0).expect("slot 1");
        let third = pool.allocate(VoiceTag::default(), SampleRegion::default(), VoiceParams::SILENT, 0).expect("slot 2");
        assert!(pool.release(second), "leave a hole in the middle");

        let visited: Vec<VoiceId> = pool.iter().map(|(id, _)| id).collect();
        let visited_mutably: Vec<VoiceId> = pool.iter_mut().map(|(id, _)| id).collect();
        assert_eq!(visited_mutably, alloc::vec![first, third], "active slots only, in slot order");
        assert_eq!(visited_mutably, visited, "the same ids the shared walk hands out");
    }

    #[test]
    fn iter_mut_writes_land_on_the_voices_it_names() {
        let mut pool = VoicePool::new(2);
        let first = pool.allocate(VoiceTag::default(), SampleRegion::default(), VoiceParams::SILENT, 0).expect("slot 0");
        let second = pool.allocate(VoiceTag::default(), SampleRegion::default(), VoiceParams::SILENT, 0).expect("slot 1");

        // What a format's per-tick articulation walk does: write straight to `params`,
        // keyed by the slot index into its own parallel array.
        for (id, voice) in pool.iter_mut() {
            voice.params.volume = U0F16::from_bits(1_000 * (id.index() + 1));
        }
        assert_eq!(pool.get(first).map(|voice| voice.params.volume), Some(U0F16::from_bits(1_000)));
        assert_eq!(pool.get(second).map(|voice| voice.params.volume), Some(U0F16::from_bits(2_000)));
    }

    #[test]
    fn a_full_pool_declines_rather_than_growing() {
        let mut pool = VoicePool::new(1);
        assert!(pool.allocate(VoiceTag::default(), SampleRegion::default(), VoiceParams::SILENT, 0).is_some());
        assert!(pool.allocate(VoiceTag::default(), SampleRegion::default(), VoiceParams::SILENT, 0).is_none());
        assert_eq!(pool.voices_active(), 1);
    }

    #[test]
    fn a_released_handle_is_stale_immediately_and_after_reuse() {
        let mut pool = VoicePool::new(2);
        let first = pool.allocate(VoiceTag::default(), SampleRegion::default(), VoiceParams::SILENT, 0).expect("slot 0");
        assert!(pool.get_mut(first).is_some());

        assert!(pool.release(first));
        assert!(pool.get_mut(first).is_none(), "a released handle is stale before the slot is even reused");
        assert!(!pool.release(first), "releasing twice is a no-op, not a corruption");

        let reused = pool.allocate(VoiceTag::default(), SampleRegion::default(), VoiceParams::SILENT, 0).expect("the freed slot");
        assert_eq!(reused.index(), first.index(), "the free list is LIFO, so the slot comes straight back");
        assert_ne!(reused.generation(), first.generation());
        assert!(pool.get_mut(first).is_none(), "the stale handle does not resolve to the new occupant");
        assert!(pool.get_mut(reused).is_some());
    }

    #[test]
    fn a_voice_that_reaches_the_end_of_a_one_shot_releases_itself() {
        let (blob, region) = one_shot_blob(4);
        let mut pool = VoicePool::new(2);
        let voice = pool.allocate(VoiceTag::default(), region, sounding_params(), 0).expect("slot 0");

        let mut destination = [FixedFrame::default(); 3];
        pool.accumulate::<FixedPath, Linear>(&blob, &mut destination, 44_100);
        assert_eq!(pool.voices_active(), 1, "three of four frames rendered");

        pool.accumulate::<FixedPath, Linear>(&blob, &mut destination, 44_100);
        assert_eq!(pool.voices_active(), 0);
        assert!(pool.get(voice).is_none(), "the handle went stale when the voice ended");
    }

    #[test]
    fn a_stopped_voice_fades_out_and_is_then_released() {
        let (blob, region) = one_shot_blob(512);
        let mut pool = VoicePool::new(2);
        let voice = pool.allocate(VoiceTag::default(), region, sounding_params(), 0).expect("slot 0");
        {
            let voice = pool.get_mut(voice).expect("the voice is live");
            voice.settle_gains();
            voice.stop();
        }

        let mut destination = [FixedFrame::default(); RAMP_FRAMES as usize / 2];
        pool.accumulate::<FixedPath, Linear>(&blob, &mut destination, 44_100);
        assert_eq!(pool.voices_active(), 1, "half a ramp in, the voice is still fading");
        let first = destination.first().map(|frame| frame.left).unwrap_or(0);
        let last = destination.last().map(|frame| frame.left).unwrap_or(0);
        assert!(first != 0 && last.abs() < first.abs(), "the fade got somewhere: {first} then {last}");

        pool.accumulate::<FixedPath, Linear>(&blob, &mut destination, 44_100);
        assert_eq!(pool.voices_active(), 0, "and the voice is released the moment the fade lands");
        assert!(pool.get(voice).is_none(), "the handle went stale");
    }

    #[test]
    fn stopping_a_silent_voice_releases_it_at_once() {
        let (blob, region) = one_shot_blob(64);
        let mut pool = VoicePool::new(2);
        let voice = pool.allocate(VoiceTag::default(), region, VoiceParams::SILENT, 0).expect("slot 0");
        pool.get_mut(voice).expect("the voice is live").stop();

        let mut destination = [FixedFrame::default(); 4];
        pool.accumulate::<FixedPath, Linear>(&blob, &mut destination, 44_100);
        assert_eq!(pool.voices_active(), 0, "there is nothing to fade");
        assert_eq!(destination, [FixedFrame::default(); 4]);
    }

    /// The attack ramp is the other half of the anti-click story: a voice starts from
    /// silence rather than jumping to full gain on its first frame.
    #[test]
    fn a_fresh_voice_ramps_up_from_silence() {
        let (blob, region) = one_shot_blob(512);
        let mut pool = VoicePool::new(1);
        pool.allocate(VoiceTag::default(), region, sounding_params(), 0).expect("slot 0");

        let mut destination = [FixedFrame::default(); RAMP_FRAMES as usize * 2];
        pool.accumulate::<FixedPath, Linear>(&blob, &mut destination, 44_100);
        let first = destination.first().map(|frame| frame.left).unwrap_or(0);
        let settled = destination.last().map(|frame| frame.left).unwrap_or(0);
        assert!(first.abs() * 8 < settled.abs(), "the first frame is a fraction of the settled gain: {first} then {settled}");
    }

    #[test]
    fn a_looping_voice_never_ends() {
        let pcm: Vec<i16> = (0..8).map(|index| 100 + index as i16).collect();
        let mut blob = Vec::new();
        let region = append_guarded_sample(&mut blob, &pcm, LoopSpan::new(2, 8));

        let mut pool = VoicePool::new(1);
        pool.allocate(VoiceTag::default(), region, sounding_params(), 0).expect("slot 0");

        let mut destination = [FixedFrame::default(); 100];
        pool.accumulate::<FixedPath, Linear>(&blob, &mut destination, 44_100);
        assert_eq!(pool.voices_active(), 1);
    }

    #[test]
    fn splitting_a_run_produces_the_same_frames_as_rendering_it_whole() {
        let pcm: Vec<i16> = (0..12).map(|index| 500 + 37 * index as i16).collect();
        let mut blob = Vec::new();
        let region = append_guarded_sample(&mut blob, &pcm, LoopSpan::new(3, 12));
        // A step that is not a whole number of frames, so the fractional position is
        // exercised across every split point.
        let params = VoiceParams { step: Step::from_ratio(7, 3), volume: U0F16::MAX, ..VoiceParams::SILENT };

        let render = |chunk_length: usize| {
            let mut output = [FixedFrame::default(); 40];
            let mut pool = VoicePool::new(1);
            pool.allocate(VoiceTag::default(), region, params, 0).expect("slot 0");
            let mut written = 0;
            while written < output.len() {
                let end = (written + chunk_length).min(output.len());
                if let Some(window) = output.get_mut(written..end) {
                    pool.accumulate::<FixedPath, Linear>(&blob, window, 44_100);
                }
                written = end;
            }
            output
        };

        let whole = render(40);
        for chunk_length in [1usize, 3, 7, 11, 25] {
            assert_eq!(render(chunk_length), whole, "chunk length {chunk_length} changed the output");
        }
    }
}
