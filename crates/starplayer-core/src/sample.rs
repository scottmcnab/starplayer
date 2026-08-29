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
/// different problem with a different answer — a pre-roll before the sample start and
/// before the loop start — and belong to M7 along with the kernels that need them.
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
