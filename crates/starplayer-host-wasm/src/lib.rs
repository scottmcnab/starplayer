//! AudioWorklet glue for the browser: moves rendered quanta across the worklet boundary
//! and carries commands and telemetry the other way.
//!
//! Must always build for `wasm32-unknown-unknown`; `xtask ci --job wasm-build` checks it.
//!
//! Allowed dependency edges: `starplayer`, plus `wasm-bindgen` once A4 lands.

#![forbid(unsafe_code)]
