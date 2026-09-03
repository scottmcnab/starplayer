//! Real-time plumbing shared by every host: the SPSC ring, the garbage channel, the
//! telemetry snapshot channel, and the `portable-atomic` shim used on targets that lack
//! native compare-and-swap.
//!
//! Everything here is wait-free on the audio side. No allocation, no locks and no panics
//! may occur on a path reachable from `render()`.
//!
//! Allowed dependency edges: `starplayer-core`.
//!
//! # `Arc`, once, for the whole workspace
//!
//! `alloc::sync::Arc` **does not exist** on a target without atomic compare-and-swap, and
//! `riscv32imc-unknown-none-elf` — which CI checks on every commit — is exactly such a
//! target: it implements neither the `A` extension nor any of `core::sync::atomic`. So
//! this crate re-exports [`Arc`] from `portable-atomic-util`, and every other crate in
//! the workspace names *that* one. Writing `alloc::sync::Arc` anywhere in a `no_std`
//! crate is a portability bug that only the bare-metal CI job would catch.
//!
//! What each target pays:
//!
//! | Target | Atomics used | Cost |
//! |---|---|---|
//! | x86-64, aarch64, wasm32 | native CAS | none; `Arc` is the same lock-free refcount as `alloc`'s |
//! | riscv32imc, thumbv6m, Xtensa | `portable-atomic/critical-section` | the refcount RMW runs inside a critical section |
//!
//! The `critical-section` feature is enabled by a **target** condition in this crate's
//! manifest rather than by a cargo feature, because a cargo feature would have to be on
//! by default for the bare-metal CI job to pass, and would then be on for desktop and
//! browser builds that do not need it. Embedded users of those targets have to provide a
//! `critical-section` implementation, which is the standard arrangement there and is what
//! architecture §10 already specifies.
//!
//! Note that the SPSC ring itself needs **no** compare-and-swap: a single-producer,
//! single-consumer ring only ever loads and stores its two indices, and those are native
//! even on riscv32imc.
//!
//! # Why the ring is a dependency and not fifty lines here
//!
//! A wait-free SPSC ring cannot be written in safe Rust. Its whole point is that the
//! producer and the consumer hold *disjoint mutable views of one buffer*, disjoint by an
//! invariant the borrow checker cannot see (the two index atomics), and there is no safe
//! primitive in `core` that grants interior mutability over an arbitrary `T` shared
//! between threads. Every core crate in this workspace is `#![forbid(unsafe_code)]`, so
//! the choice is between weakening that rule for the single most concurrency-sensitive
//! file in the tree, or taking a dependency whose unsafe is audited by more people than
//! this project has. [`ringbuf`] is that dependency: `no_std` + `alloc`, capacity fixed
//! at construction, wait-free `try_push` / `try_pop`, and — decisively — it supports
//! `portable-atomic`, which almost nothing else in this space does, so it builds for the
//! bare-metal target at all.
//!
//! Its API is wrapped rather than re-exported: [`Producer`] and [`Consumer`] are the
//! shapes the engine wants, and the dependency stays replaceable.
//!
//! # The telemetry snapshot channel
//!
//! [`snapshot`] builds on the same ring to publish one whole `Copy` payload per tick,
//! which is the architecture §9(a) "triple buffer or seqlock" slot. A real triple buffer
//! needs `UnsafeCell` for the same reason the ring does, and the one obvious dependency
//! (`triple_buffer`) is `std`-only, so it is a bounded channel of whole snapshots
//! instead — see that module for the full argument.
//!
//! # The scope taps
//!
//! [`tap`] is the other half — architecture §9(b). Where a snapshot must never tear, a
//! scope tap may: it is a picture of a waveform, so both sides are `Relaxed` and the
//! audio thread never waits for anyone. One fixed ring per channel, downsampled in the
//! audio thread to [`TAP_BUCKETS_PER_QUANTUM`] values per render quantum.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod garbage;
pub mod snapshot;
pub mod spsc;
pub mod tap;

pub use garbage::{GarbageChannel, GarbageCollector, garbage_channel};
pub use snapshot::{DEFAULT_SNAPSHOT_DEPTH, SnapshotPublisher, SnapshotReader, snapshot_channel};
pub use spsc::{Consumer, Producer, channel};
pub use tap::{TAP_BUCKETS_PER_QUANTUM, TAP_BUCKET_FRAMES, TAP_RING_BUCKETS, TapReader, TapRing, TapWriter};

/// The workspace's one `Arc`.
///
/// `portable-atomic-util`'s, not `alloc`'s, because `alloc::sync::Arc` is absent on
/// targets without compare-and-swap. See the crate documentation.
pub use portable_atomic_util::Arc;

/// Atomics that exist on every target StarPlayer builds for, including the ones whose
/// `core` has none.
pub use portable_atomic as atomic;
