//! Constants shared by everything that touches sample data: the module builder that
//! *writes* the PCM blob (`starplayer-model`) and the mixer that *reads* it
//! (`starplayer-mixer`).
//!
//! It lives here rather than in either of them because the two crates have no dependency
//! edge between them — `starplayer-model` depends only on this crate — and the guard-frame
//! count is a contract they both have to agree on exactly.

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
/// | `Nearest` | `index` | 0 |
/// | `Linear` | `index + 1` | 1 |
/// | Cubic Hermite, 4-tap (M7) | `index + 2` | 2 |
/// | Windowed sinc, 8-tap (M7) | `index + 4` | 4 |
///
/// Eight is the next power of two above that maximum. It leaves room for a 16-tap sinc
/// (which would need 8) without another format-wide change, keeps each sample's data
/// 16-byte-aligned in length terms, and costs 16 bytes per sample — nothing next to the
/// sample itself.
///
/// The *leading* taps a symmetric kernel wants (`index - 3` for an 8-tap sinc) are a
/// different problem with a different answer, and M7-task-H5 gave it one:
/// [`PRE_ROLL_FRAMES`] before the sample start, plus a deferred loop wrap so that the
/// frames a kernel wants *before* a loop start come out of the trailing guard instead.
///
/// # What the guard frames contain
///
/// Filled by `starplayer_model::ModuleBuilder::add_sample`, checked by
/// `starplayer_mixer::sample::SampleData::resolve`:
///
/// * **Forward loop** — the addressable length *is* `loop_end`, and the guard frames
///   repeat the loop from `loop_start`, wrapping as many times as it takes for a loop
///   shorter than `GUARD_FRAMES`.
/// * **Everything else** — the whole sample is addressable and the guard frames are
///   silence, so a kernel interpolating over the final frame decays to zero rather than
///   clicking.
pub const GUARD_FRAMES: usize = 8;

/// Frames written *before* every sample's first frame, so a symmetric interpolator can
/// read `index - 1` (cubic Hermite) or `index - 3` (an 8-tap windowed sinc) at the very
/// start of a sample without a branch and without leaving the module's PCM blob
/// (architecture §7.1).
///
/// Every sample's stored run is therefore `pre-roll ‖ frames ‖ guard`, and a sample's
/// `pcm_offset` keeps pointing at **frame 0** — the pre-roll sits at
/// `pcm_offset - PRE_ROLL_FRAMES`, which is why every offset already written down, the
/// scope tap and the loop folds are unchanged by its arrival.
///
/// # Why eight, and why it is silence
///
/// Eight matches [`GUARD_FRAMES`] so the two ends of a sample have one number between
/// them, and it leaves the same room for a 16-tap kernel (which would want seven leading
/// frames). The frames are **silence for every loop mode**: they are read at a note's
/// attack, where a note starts from nothing, and filling them with anything else would
/// put a pre-echo in front of every trigger.
///
/// # Where a loop's leading frames come from instead
///
/// A forward loop's wrap is *deferred* by the kernel's `LEADING_FRAMES` instead of being
/// served from a second copy: the run is allowed to walk that far past `loop_end` into
/// the guard — which already holds the loop's continuation — and only then wraps, so the
/// frames before the interpolation point are always real, already-played frames. Nothing
/// in the blob has to hold two different things at one address, and the linear kernel,
/// whose `LEADING_FRAMES` is zero, wraps exactly where it always did.
pub const PRE_ROLL_FRAMES: usize = 8;
