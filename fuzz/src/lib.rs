//! Shared support for the eight loader fuzz targets: the memory cap, the structured
//! mutation program, and the post-load walk that proves a produced [`Module`] is
//! self-consistent.
//!
//! # The memory cap (M2-C7 deliverable 2)
//!
//! "Never OOM" is the second half of the loader invariant, and an OOM is worthless as a
//! finding if the operating system kills the process first: the fuzzer records no
//! artifact, CI reports "killed", and nobody can reproduce it. So this crate installs a
//! global allocator that **counts live bytes** and refuses an allocation that would take
//! one input past its budget. Refusing means returning null, which makes the standard
//! library call `handle_alloc_error` and abort — and an abort is a crash libFuzzer
//! records, minimises and writes an artifact for, exactly like a panic.
//!
//! Returning null rather than panicking is deliberate. Unwinding out of `GlobalAlloc::alloc`
//! runs destructors while a collection is mid-resize, and the panic payload is itself
//! boxed — that is, allocated — so a panicking allocator re-enters itself on the way to
//! reporting. Abort has neither problem and loses nothing: the input that caused it is on
//! disk either way.
//!
//! The budget is deliberately far above anything a legitimate module needs, because it is
//! not a fidelity check. It is there to turn "this 400-byte file asked for two gigabytes"
//! into a reproducible artifact.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use arbitrary::Arbitrary;
use starplayer_model::Module;

// ── the memory cap ──────────────────────────────────────────────────────────────────

/// Budget floor: what an input of no size at all is still allowed to allocate.
///
/// It has to clear the loaders' own production caps by a wide margin — the S3M loader's
/// decoded-pattern budget alone has a 4 MiB floor (research point 3) — so that a
/// *legitimately* bounded allocation is never mistaken for a runaway one.
pub const MEMORY_CAP_FLOOR_BYTES: usize = 64 * 1024 * 1024;

/// Budget per input byte, on top of the floor.
///
/// A module's decoded form is bigger than its file: 8-bit PCM widens to `i16`, packed S3M
/// patterns expand to a fixed 64 × channels × 5, and MTM tracks are copied once per
/// channel that references them. Sixty-four times the input covers all of that with two
/// orders of magnitude to spare.
pub const MEMORY_CAP_PER_INPUT_BYTE: usize = 64;

/// Live bytes handed out by [`CappedAllocator`] and not yet returned.
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Live bytes at the moment the current input started, so the cap measures what *this*
/// input allocated rather than what the process is holding.
static BASELINE_BYTES: AtomicUsize = AtomicUsize::new(0);

/// The current input's budget, in bytes above [`BASELINE_BYTES`].
static BUDGET_BYTES: AtomicUsize = AtomicUsize::new(usize::MAX);

/// The system allocator, with a per-input ceiling.
pub struct CappedAllocator;

// SAFETY: every method forwards to `System`, which satisfies the `GlobalAlloc` contract;
// the wrapper only adds an accounting counter and a refusal, and a refusal is spelled as
// the null pointer the trait already defines for "allocation failed". `realloc` and
// `alloc_zeroed` are deliberately left to the trait's default implementations, which are
// written in terms of `alloc` and `dealloc` and are therefore accounted for free.
unsafe impl GlobalAlloc for CappedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();
        let live = LIVE_BYTES.fetch_add(size, Ordering::Relaxed).saturating_add(size);
        let ceiling = BASELINE_BYTES.load(Ordering::Relaxed).saturating_add(BUDGET_BYTES.load(Ordering::Relaxed));
        if live > ceiling {
            LIVE_BYTES.fetch_sub(size, Ordering::Relaxed);
            return core::ptr::null_mut();
        }
        // SAFETY: `layout` is the caller's, forwarded unchanged.
        let pointer = unsafe { System.alloc(layout) };
        if pointer.is_null() {
            LIVE_BYTES.fetch_sub(size, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: `pointer` and `layout` are the caller's, forwarded unchanged, and this
        // allocator only ever hands out blocks obtained from `System`.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CappedAllocator = CappedAllocator;

/// Start a new input's budget: [`MEMORY_CAP_FLOOR_BYTES`] plus
/// [`MEMORY_CAP_PER_INPUT_BYTE`] per byte of input.
///
/// Call it first in every `fuzz_target!` body.
pub fn arm_memory_cap(input_len: usize) {
    BASELINE_BYTES.store(LIVE_BYTES.load(Ordering::Relaxed), Ordering::Relaxed);
    BUDGET_BYTES.store(MEMORY_CAP_FLOOR_BYTES.saturating_add(input_len.saturating_mul(MEMORY_CAP_PER_INPUT_BYTE)), Ordering::Relaxed);
}

// ── what a loaded module has to survive ─────────────────────────────────────────────

/// Walk everything a host would read off a freshly loaded module.
///
/// The loader invariant is not merely "no panic while parsing": it is that whatever comes
/// back is a `Module` the rest of the engine can index. `ModuleBuilder::build` checks the
/// index sets, and this walks them the way a sequencer and a pattern view would, so an
/// accessor that trusts a field the builder does not check still shows up as a crash.
pub fn walk_module(module: &Module) {
    let header = module.header();
    for channel in 0..header.channel_count {
        let _ = header.channel_pan(channel);
    }
    for order in 0..module.orders().len().saturating_add(1) {
        let _ = module.order_entry(order);
    }
    for (index, _) in module.patterns().iter().enumerate() {
        let id = starplayer_model::PatternId(index as u16);
        let _ = module.pattern(id);
        let _ = module.pattern_bytes(id);
    }
    for (index, _) in module.samples().iter().enumerate() {
        let id = starplayer_core::SampleId(index as u16);
        let _ = module.sample(id);
        let _ = module.sample_pcm(id);
    }
    for (index, _) in module.instruments().iter().enumerate() {
        let _ = module.instrument(starplayer_core::InstrumentId(index as u16));
    }
}

/// Walk every MOD cell of every pattern through the format's own view.
pub fn walk_mod_cells(module: &Module) {
    for index in 0..module.patterns().len() {
        let Some(view) = starplayer_mod::PatternView::new(module, starplayer_model::PatternId(index as u16)) else { continue };
        for row in 0..starplayer_mod::ROWS {
            for channel in 0..module.header().channel_count {
                if let Some(cell) = view.cell(row, channel) {
                    let _ = cell.display();
                }
            }
        }
    }
}

/// Walk every MTM cell of every pattern through the format's own view.
pub fn walk_mtm_cells(module: &Module) {
    for index in 0..module.patterns().len() {
        let Some(view) = starplayer_mtm::PatternView::new(module, starplayer_model::PatternId(index as u16)) else { continue };
        for row in 0..starplayer_mtm::ROWS {
            for channel in 0..module.header().channel_count {
                if let Some(cell) = view.cell(row, channel) {
                    let _ = cell.display();
                }
            }
        }
    }
}

/// Walk every S3M cell of every pattern through the format's own view.
pub fn walk_s3m_cells(module: &Module) {
    for index in 0..module.patterns().len() {
        let Some(view) = starplayer_s3m::PatternView::new(module, starplayer_model::PatternId(index as u16)) else { continue };
        for row in 0..starplayer_s3m::ROWS {
            for channel in 0..module.header().channel_count {
                if let Some(cell) = view.cell(row, channel) {
                    let _ = cell.display();
                }
            }
        }
    }
}

/// Walk every XM cell of every pattern through the format's own view.
///
/// XM patterns do **not** all have the same row count — the format allows 1..=256 and the
/// loader honours what each pattern's header says — so the row bound comes from the
/// pattern index rather than from a constant, unlike the three older formats above.
pub fn walk_xm_cells(module: &Module) {
    for index in 0..module.patterns().len() {
        let id = starplayer_model::PatternId(index as u16);
        let Some(pattern) = module.pattern(id) else { continue };
        let Some(view) = starplayer_xm::PatternView::new(module, id) else { continue };
        for row in 0..pattern.rows() {
            for channel in 0..module.header().channel_count {
                if let Some(cell) = view.cell(row, channel) {
                    let _ = cell.display();
                    let _ = cell.volume_effect_name();
                }
            }
        }
    }
}

/// Walk every IT cell of every pattern through the format's own view.
///
/// IT patterns do not all have the same number of rows, unlike the other three formats',
/// so the row bound comes from each pattern's own index rather than from a constant.
pub fn walk_it_cells(module: &Module) {
    for index in 0..module.patterns().len() {
        let id = starplayer_model::PatternId(index as u16);
        let Some(view) = starplayer_it::PatternView::new(module, id) else { continue };
        for row in 0..view.rows() {
            for channel in 0..view.channels() {
                if let Some(cell) = view.cell(row, channel) {
                    let _ = cell.display();
                    let _ = cell.tracker_notation();
                    let _ = cell.volume_command().name();
                }
            }
        }
    }
}

// ── the structured mutation program (M2-C7 deliverable 3) ───────────────────────────

/// One byte written over a base module.
#[derive(Arbitrary, Clone, Copy, Debug)]
pub struct Patch {
    /// Reduced modulo the base's length, so every generated patch lands somewhere.
    pub offset: u32,
    pub value: u8,
}

/// A mutation program applied to a **valid** module.
///
/// Random bytes almost never get past a magic number, and when they do they rarely get
/// past a length field, so a byte-level fuzzer spends most of its budget in the first
/// hundred bytes of a loader. Starting from a module that already loads and disturbing
/// its fields reaches the pattern unpacker, the sample decoder, the loop clamps and the
/// order-list mapping on the first iteration instead of the millionth.
#[derive(Arbitrary, Clone, Debug)]
pub struct Mutation {
    /// Which of the target's base modules to start from, reduced modulo their count.
    pub base: u8,
    pub patches: Vec<Patch>,
    /// Cut the file short. `None` keeps its length.
    pub truncate_to: Option<u32>,
    /// Bytes of zero padding to append, bounded to keep the input small.
    pub append_zeros: u16,
}

/// Largest tail a [`Mutation`] may append, so a mutation cannot itself become the memory
/// cap's reason for firing.
pub const MAX_APPENDED_ZEROS: usize = 4096;

impl Mutation {
    /// Apply this program to one of `bases` and return the mutated bytes.
    pub fn apply(&self, bases: &[&[u8]]) -> Vec<u8> {
        let base = bases.get(self.base as usize % bases.len().max(1)).copied().unwrap_or_default();
        let mut bytes = base.to_vec();
        if !bytes.is_empty() {
            for patch in &self.patches {
                let offset = patch.offset as usize % bytes.len();
                if let Some(byte) = bytes.get_mut(offset) {
                    *byte = patch.value;
                }
            }
        }
        if let Some(length) = self.truncate_to {
            bytes.truncate(length as usize % (base.len().saturating_add(1)).max(1));
        }
        bytes.resize(bytes.len().saturating_add(self.append_zeros as usize % MAX_APPENDED_ZEROS), 0);
        bytes
    }
}
