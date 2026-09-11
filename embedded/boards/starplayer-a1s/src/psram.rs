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
//! Three buffers, claimed once at boot and never freed ([`Arena::claim`]):
//!
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
//! # The other thing PSRAM cannot do
//!
//! On the classic ESP32 flash and PSRAM are reached through the same cache. An erase or
//! write that turns the cache off therefore makes PSRAM unreadable as well as flash, so
//! "play out of PSRAM while writing flash" is not a trick that works here. `store.rs`
//! says what is done instead.

use core::slice;

/// How much PSRAM one uploaded module may use, for each of the three buffers.
///
/// 512 KiB is six times `PETRI.S3M`'s 88 036-byte image and comfortably past anything the
/// 4 MB flash could hold as a slot, while three of them together are under half of the
/// smallest PSRAM fitted to an A1S module. It is a policy number, not a hardware one: the
/// real ceiling on an uploaded module is DRAM, because `starplayer::load` decodes into the
/// heap before `web::load_uploaded` serialises the result back into PSRAM.
pub const BUFFER_BYTES: usize = 512 * 1024;

/// What [`Arena::claim`] aligns every buffer to.
///
/// [`Module::from_image`](starplayer_model::Module::from_image) *refuses* an image whose
/// address is not 4-byte aligned rather than silently copying its PCM out of it (M8-I2),
/// and the whole point of these buffers is that it does not copy.
const ALIGNMENT: usize = 4;

/// A bump allocator over the mapped PSRAM region.
///
/// Claim-only: there is no `free`, because the three buffers are claimed at boot and live
/// for the life of the program. That is the entire lifetime story, and it is what makes
/// the `&'static` views [`Buffer::fill`] hands out sound.
pub struct Arena {
    next: usize,
    end: usize,
}

impl Arena {
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

    /// Claim `bytes` of PSRAM, 4-byte aligned, for the life of the program.
    ///
    /// `None` when the region has no room left — which is the normal answer on a module
    /// with no PSRAM fitted, and is reported as "the web build runs without upload"
    /// rather than as a boot failure.
    pub fn claim(&mut self, bytes: usize) -> Option<Buffer> {
        let start = self.next.next_multiple_of(ALIGNMENT);
        let end = start.checked_add(bytes)?;
        if end > self.end {
            return None;
        }
        self.next = end;
        Some(Buffer { start: start as *mut u8, capacity: bytes })
    }
}

/// One claimed PSRAM buffer.
///
/// Deliberately a raw pointer and a length rather than a `&'static mut [u8]`: the buffer
/// is *reused*, and each use hands out a `&'static [u8]` that a [`Module`] borrows from.
/// A `&'static mut` cannot express that — reborrowing it as `&'static [u8]` would freeze
/// it for the rest of the program — so the aliasing obligation is carried by hand, stated
/// on [`Buffer::fill`], and discharged by the ping-pong in `web.rs`.
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

    /// Copy `image` in and hand back the `'static` view a module borrows from.
    ///
    /// # Safety
    ///
    /// The same obligation as [`Buffer::as_mut`]: whatever module borrowed this buffer
    /// last must have been dropped.
    pub unsafe fn fill(&mut self, image: &[u8]) -> Option<&'static [u8]> {
        if image.len() > self.capacity {
            return None;
        }
        // SAFETY: the caller's obligation, forwarded. `image.len() <= capacity`, and the
        // copy initialises every byte the returned view covers.
        let destination = unsafe { slice::from_raw_parts_mut(self.start, image.len()) };
        destination.copy_from_slice(image);
        Some(destination)
    }

    /// The `'static` view of the first `length` bytes, for a body that was streamed
    /// straight into this buffer rather than copied in.
    ///
    /// # Safety
    ///
    /// The same obligation as [`Buffer::fill`], plus: `length` bytes must actually have
    /// been written by a preceding [`Buffer::as_mut`] borrow.
    pub unsafe fn view(&self, length: usize) -> Option<&'static [u8]> {
        if length > self.capacity {
            return None;
        }
        // SAFETY: as `fill`, with the initialisation obligation stated above instead of
        // discharged by a copy here.
        Some(unsafe { slice::from_raw_parts(self.start, length) })
    }
}
