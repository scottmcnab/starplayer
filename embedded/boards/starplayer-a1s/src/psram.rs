//! The PSRAM arena: where an uploaded module's bytes live, and why they do not live on
//! the heap.
//!
//! # The erratum this module exists for
//!
//! esp-alloc's own documentation records it:
//!
//! > On the ESP32, ESP32-S2 and ESP32-S3 the atomic instructions do not work correctly
//! > when the memory they access is located in PSRAM.
//!
//! This engine puts atomics on the heap — [`starplayer_rt::Arc`]'s reference count, the
//! seqlocks `starplayer-host-embedded` carries its frame clocks in, the telemetry ring's
//! sequence. esp-alloc's `GlobalAlloc::alloc` is `alloc_caps(EnumSet::empty(), layout)`,
//! and an empty capability set is a subset of *every* region's capabilities, so it takes
//! the first registered region that has room: register PSRAM at all and an ordinary
//! allocation lands there the moment internal DRAM fills. That is a silently corrupted
//! reference count, not an allocation failure, and M8-I3 refused to accept it in the
//! audio build.
//!
//! So the `web` build does not register PSRAM with the global allocator either. It takes
//! the region esp-hal mapped and hands out **whole buffers** from it by hand. The
//! guarantee is then structural rather than statistical: no `Arc`, no seqlock and no
//! atomic can reach PSRAM, because the global allocator has never heard of it.
//!
//! # What does live there
//!
//! Five regions, claimed once at boot and never freed ([`Arena::claim`]):
//!
//! * **64 KiB reserved for network futures**. The selected station or portal personality
//!   claims only its actual future bodies from this tail reservation;
//! * **256 KiB decoder workspace**, reused by every raw upload and never visible to the
//!   global allocator;
//! * **the upload staging buffer** — the raw bytes of whatever a browser posted, streamed
//!   straight off the socket so a 200 KB file never touches the 48 KiB DRAM heap;
//! * **two module-image buffers**, used ping-pong. A [`Module`](starplayer_model::Module)
//!   built with `Module::from_image` borrows its pattern blob and its PCM **in place**, so
//!   the image behind the module that is playing must stay untouched for as long as it
//!   plays. The next upload is therefore built in the *other* buffer, and the one the
//!   retiring module was borrowing only becomes writable again once that module has come
//!   back through the engine's garbage channel and been dropped
//!   ([`ControlHalf::collect_garbage`](starplayer_host_embedded::ControlHalf::collect_garbage)).
//!
//! Only the `Arc<Module>` itself, the module's four small index vectors and the
//! sequencer are allocated — in DRAM, by the global allocator, as they always were.
//!
//! The selected network personality claims its picoserve worker future bodies from the
//! reserved prefix through
//! [`Arena::claim_raw`]. The worker's Embassy task
//! header, scheduler state and pointer proxy remain in internal DRAM; see
//! [`crate::psram_task`] for the placement and safety argument. Station and portal are
//! mutually exclusive, so only the futures that actually run consume arena space.
//!
//! # The other thing PSRAM cannot do
//!
//! On the classic ESP32 flash and PSRAM are reached through the same cache. An erase or
//! write that turns the cache off therefore makes PSRAM unreadable as well as flash, so
//! "play out of PSRAM while writing flash" is not a trick that works here. `store.rs`
//! says what is done instead.

use core::{
    alloc::Layout,
    mem::{align_of, size_of},
    ptr::NonNull,
    slice,
};

/// Space kept available for the station's two picoserve worker futures or the mutually
/// exclusive provisioning worker.
pub const NETWORK_BYTES: usize = 64 * 1024;

/// Reusable bulk scratch for incremental native-format decoding.
pub const DECODER_WORKSPACE_BYTES: usize = 256 * 1024;

/// What [`Arena::claim`] aligns every buffer to.
///
/// [`Module::from_image`](starplayer_model::Module::from_image) *refuses* an image whose
/// address is not 4-byte aligned rather than silently copying its PCM out of it (M8-I2),
/// and the whole point of these buffers is that it does not copy.
const ALIGNMENT: usize = 4;

/// A bump allocator over the mapped PSRAM region.
///
/// Claim-only: there is no `free`, because buffers and network-worker futures are claimed
/// at boot and live for the life of the program. That is the entire lifetime story, and
/// it is what makes the `&'static` views [`Buffer::as_mut`] hands out sound.
pub struct Arena {
    next: usize,
    end: usize,
}

impl Arena {
    /// An arena with no storage, used when the board reported no PSRAM. Keeping this a
    /// real value lets provisioning still start and report its ordinary out-of-PSRAM
    /// error without inventing a nullable arena pointer.
    pub const fn empty() -> Arena { Arena { next: 0, end: 0 } }

    /// Take the region `esp_hal::psram::Psram::raw_parts` reported.
    ///
    /// The caller must not register the same region with `esp_alloc::HEAP` — see the
    /// module docs for why the `web` build does not.
    pub fn new(start: *mut u8, size: usize) -> Arena {
        let base = start as usize;
        Arena { next: base, end: base.saturating_add(size) }
    }

    /// Bytes still unclaimed. Reported at boot so the owner's log says how much PSRAM
    /// the upload buffers left behind.
    pub fn remaining(&self) -> usize { self.end.saturating_sub(self.next) }

    /// Claim raw storage with `layout`'s size and alignment for the life of the program.
    ///
    /// This is deliberately the only allocator-like operation the arena exposes. It
    /// advances the same monotonic cursor as [`Arena::claim`], cannot free or resize a
    /// claim, and returns an untyped pointer rather than registering PSRAM with the
    /// global allocator. [`crate::psram_task`] uses it to initialize one future directly
    /// in its final address.
    pub(crate) fn claim_raw(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        // `Layout` guarantees a nonzero power-of-two alignment. Rounding with checked
        // addition rejects address overflow before masking the low bits off.
        let alignment_mask = layout.align() - 1;
        let start = self.next.checked_add(alignment_mask)? & !alignment_mask;
        let end = start.checked_add(layout.size())?;
        if end > self.end {
            return None;
        }
        let pointer = NonNull::new(start as *mut u8)?;
        self.next = end;
        Some(pointer)
    }

    /// Claim `bytes` of PSRAM, 4-byte aligned, for the life of the program.
    ///
    /// `None` when the region has no room left — which is the normal answer on a module
    /// with no PSRAM fitted, and is reported as "the web build runs without upload"
    /// rather than as a boot failure.
    pub fn claim(&mut self, bytes: usize) -> Option<Buffer> {
        let layout = Layout::from_size_align(bytes, ALIGNMENT).ok()?;
        let start = self.claim_raw(layout)?;
        Some(Buffer { start: start.as_ptr(), capacity: bytes })
    }

    /// Claim and return the first `bytes` as a disjoint arena, leaving the remainder in
    /// `self`. Used once at boot to make the network reservation structural: upload
    /// buffers can never consume it, even if their layout changes later.
    pub fn split_prefix(&mut self, bytes: usize) -> Option<Arena> {
        let layout = Layout::from_size_align(bytes, ALIGNMENT).ok()?;
        let start = self.claim_raw(layout)?;
        Some(Arena { next: start.as_ptr() as usize, end: start.as_ptr() as usize + bytes })
    }
}

/// One claimed PSRAM buffer.
///
/// Deliberately a raw pointer and a length rather than a `&'static mut [u8]`: the buffer
/// is *reused*, and each use hands out a `&'static [u8]` that a [`Module`] borrows from.
/// A `&'static mut` cannot express that — reborrowing it as `&'static [u8]` would freeze
/// it for the rest of the program — so the aliasing obligation is carried by hand, stated
/// on [`Buffer::as_mut`], and discharged by the ping-pong in `web.rs`.
///
/// [`Module`]: starplayer_model::Module
pub struct Buffer {
    start: *mut u8,
    capacity: usize,
}

// SAFETY: a `Buffer` is a pointer to a region that nothing else in the firmware touches —
// the arena hands each one out exactly once and never again — so moving it between tasks
// or cores transfers sole ownership, which is exactly what `Send` claims. It is
// deliberately **not** `Sync`: two references would be two writers.
unsafe impl Send for Buffer {}

impl Buffer {
    /// How many bytes this buffer can hold.
    pub const fn capacity(&self) -> usize { self.capacity }

    /// A mutable view of the whole buffer, for streaming a request body into.
    ///
    /// # Safety
    ///
    /// No [`Module`](starplayer_model::Module) may still be borrowing this buffer's
    /// contents. The upload staging buffer is never borrowed by a module, so its caller
    /// discharges this trivially; an image buffer's caller is `web.rs`'s ping-pong, which
    /// only writes the half the retired module has already been dropped from.
    pub unsafe fn as_mut(&mut self) -> &'static mut [u8] {
        // SAFETY: `start` addresses `capacity` bytes of mapped PSRAM that the arena handed
        // out exactly once and that nothing frees, so the `'static` lifetime is honest.
        // Uniqueness is the caller's obligation, stated above. The bytes are uninitialised
        // PSRAM on the first call — `u8` has no invalid bit patterns, so a read before a
        // write is a garbage byte rather than undefined behaviour.
        unsafe { slice::from_raw_parts_mut(self.start, self.capacity) }
    }

    /// The `'static` view of the first `length` bytes, for a body that was streamed
    /// straight into this buffer rather than copied in.
    ///
    /// # Safety
    ///
    /// The same obligation as [`Buffer::as_mut`], plus: `length` bytes must actually have
    /// been written by a preceding [`Buffer::as_mut`] borrow.
    pub unsafe fn view(&self, length: usize) -> Option<&'static [u8]> {
        if length > self.capacity {
            return None;
        }
        // SAFETY: as `Buffer::as_mut`, with the initialisation obligation stated above instead of
        // discharged by a copy here.
        Some(unsafe { slice::from_raw_parts(self.start, length) })
    }

    /// Initialize two typed tables in the unused tail after `prefix_bytes`.
    ///
    /// Both types receive exactly their requested entry count. This is used for a timeline's order map and row
    /// marks after an SPMI image. Both tables are plain immutable playback data.
    ///
    /// # Safety
    ///
    /// The caller must prove nothing still borrows any part of this buffer from a prior
    /// use. The returned tables are exclusively writable during preparation, then must
    /// remain immutable after publication until that borrower has been definitively
    /// retired. `web.rs` discharges both obligations with its per-buffer module ownership
    /// token.
    pub unsafe fn initialize_tail_tables<Fixed: Copy + Default, Remainder: Copy + Default>(
        &mut self, prefix_bytes: usize, fixed_count: usize, remainder_count: usize,
    ) -> Option<(&'static mut [Remainder], &'static mut [Fixed])> {
        if size_of::<Fixed>() == 0 || size_of::<Remainder>() == 0 || prefix_bytes > self.capacity {
            return None;
        }
        let base = self.start as usize;
        let end = base.checked_add(self.capacity)?;
        let tail = base.checked_add(prefix_bytes)?;
        let fixed_start = align_up(tail, align_of::<Fixed>())?;
        let fixed_bytes = fixed_count.checked_mul(size_of::<Fixed>())?;
        let fixed_end = fixed_start.checked_add(fixed_bytes)?;
        let remainder_start = align_up(fixed_end, align_of::<Remainder>())?;
        let remainder_bytes = remainder_count.checked_mul(size_of::<Remainder>())?;
        let remainder_end = remainder_start.checked_add(remainder_bytes)?;
        if remainder_end > end {
            return None;
        }

        // SAFETY: the arithmetic above proves both disjoint ranges fit this uniquely
        // owned buffer with each element's required alignment. Every element is written
        // before the initialized slices are constructed.
        unsafe {
            let fixed_pointer = fixed_start as *mut Fixed;
            for index in 0..fixed_count {
                fixed_pointer.add(index).write(Fixed::default());
            }
            let remainder_pointer = remainder_start as *mut Remainder;
            for index in 0..remainder_count {
                remainder_pointer.add(index).write(Remainder::default());
            }
            Some((
                slice::from_raw_parts_mut(remainder_pointer, remainder_count),
                slice::from_raw_parts_mut(fixed_pointer, fixed_count),
            ))
        }
    }
}

fn align_up(address: usize, alignment: usize) -> Option<usize> {
    let mask = alignment.checked_sub(1)?;
    address.checked_add(mask).map(|value| value & !mask)
}
