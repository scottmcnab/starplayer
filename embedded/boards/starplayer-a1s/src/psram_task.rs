//! PSRAM-backed Embassy task futures with internal-DRAM task headers.
//!
//! Embassy's ordinary `#[task]` macro places the whole [`TaskStorage`] in a static
//! pool. For picoserve that includes tens of kilobytes of future state, TCP/HTTP
//! buffers and response bodies, all in `.bss`; on the classic ESP32 every one of those
//! bytes shortens core 0's main stack. This helper keeps only
//! `TaskStorage<ExternalFuture<F>>` in the internal global heap. The proxy is one pointer;
//! the large `F` is initialized directly in a claim from [`crate::psram::Arena`].
//!
//! The split is also an atomic-safety boundary. `TaskStorage` owns Embassy's atomic task
//! state and executor pointer and therefore cannot live in PSRAM. The migrated web and
//! portal futures directly own no atomics or cross-core synchronization: their
//! `embassy_net::Stack` and picoserve configuration values are handles to internal
//! statics, and StarPlayer's channels, signals, mutexes and counters remain internal
//! statics too. Picoserve's inline `Cell` and waker state is private to the owning
//! future, polled exclusively by core 0, so that non-atomic state may live in PSRAM.
//!
//! Flash writes temporarily make PSRAM inaccessible because flash and external RAM share
//! the cache. Every write is performed synchronously by the separate core-0 control task
//! after a worker has sent a job and yielded for its reply. The executor therefore cannot
//! poll either PSRAM future while the cache is off; shared request data is copied to the
//! internal staging described by `store.rs` before the write begins.
//!
//! The arena never frees a claim. Each future is initialized once, pinned at that stable
//! address before its first poll, and exclusively reached through one Embassy task. If a
//! task completes, [`ExternalFuture::drop`] drops the future exactly once while leaving
//! its arena storage claimed for the program's lifetime. This helper is therefore only
//! for the fixed number of workers created once at boot.

use alloc::boxed::Box;
use core::{
    alloc::Layout,
    future::Future,
    pin::Pin,
    ptr::{self, NonNull},
    task::{Context, Poll},
};
use embassy_executor::{Spawner, raw::TaskStorage};

use crate::psram::Arena;

/// Why a boot-time PSRAM task could not be spawned.
pub enum SpawnError {
    /// The monotonic arena had no suitably aligned region for the future body.
    OutOfPsram,
    /// A freshly allocated Embassy header unexpectedly reported itself as busy.
    TaskStorageBusy,
}

/// Pointer-sized future proxy stored beside Embassy's task header in internal DRAM.
struct ExternalFuture<F: Future> {
    inner: NonNull<F>,
}

// Moving the proxy never moves the pointed-to future, whose arena address is stable.
impl<F: Future> Unpin for ExternalFuture<F> {}

impl<F: Future> Future for ExternalFuture<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        // SAFETY: `initialize_external_future` initialized one `F` at this stable arena
        // address and no other pointer can poll it. The Embassy task owns this proxy and
        // polls it serially on core 0. The pointee is never moved, so pinning it here
        // preserves every `Future` pinning invariant.
        unsafe { Pin::new_unchecked(this.inner.as_mut()) }.poll(context)
    }
}

impl<F: Future> Drop for ExternalFuture<F> {
    fn drop(&mut self) {
        // SAFETY: the pointer was initialized exactly once and this proxy is its unique
        // owner. Embassy drops the proxy at most once when the task completes. The arena
        // allocation itself remains claimed, so dropping `F` cannot leave a dangling
        // allocation or permit reuse.
        unsafe { ptr::drop_in_place(self.inner.as_ptr()) };
    }
}

/// Construct `F` directly in the aligned PSRAM claim and return its small proxy.
///
/// Keeping this out of line and accepting a constructor are load-bearing: this is the
/// same NRVO boundary used by ampkeeper's proven helper, preventing the large async
/// future from first being materialized in the caller's constrained core-0 stack.
#[inline(never)]
fn initialize_external_future<F: Future>(storage: NonNull<u8>, constructor: impl FnOnce() -> F) -> ExternalFuture<F> {
    let inner = storage.cast::<F>();
    // SAFETY: `spawn_in_psram` obtained `storage` with `Layout::new::<F>()`, so it is
    // large enough and correctly aligned. The monotonic arena granted this region once
    // and never frees or aliases it. The constructor is invoked once and its result is
    // written once; the pointer is exposed only through `ExternalFuture`, which pins it
    // before polling and drops it exactly once.
    unsafe { inner.as_ptr().write(constructor()) };
    ExternalFuture { inner }
}

/// Spawn a constructor-produced future whose body is pinned in PSRAM.
///
/// The leaked [`TaskStorage`] is intentionally allocated through the ordinary global
/// allocator, which contains internal DRAM only. It must live forever by Embassy's
/// contract and carries the scheduler atomics; only the pointer proxy is stored beside
/// it. Returns the arena bytes consumed, including any alignment padding, for the boot
/// log.
pub fn spawn_in_psram<F>(arena: &mut Arena, spawner: &Spawner, constructor: impl FnOnce() -> F) -> Result<usize, SpawnError>
where
    F: Future + 'static,
{
    let remaining_before = arena.remaining();
    let storage = arena.claim_raw(Layout::new::<F>()).ok_or(SpawnError::OutOfPsram)?;
    let external_future = initialize_external_future(storage, constructor);

    let task_storage: &'static TaskStorage<ExternalFuture<F>> = Box::leak(Box::new(TaskStorage::new()));
    let token = task_storage.spawn(move || external_future).map_err(|_| SpawnError::TaskStorageBusy)?;
    spawner.spawn(token);
    Ok(remaining_before - arena.remaining())
}
