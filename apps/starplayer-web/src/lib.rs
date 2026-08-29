//! The browser player: a responsive web UI over the WASM build of the engine.
//!
//! Apps depend only on the facade. The AudioWorklet plumbing lives in
//! `starplayer-host-wasm`, which is the crate that is actually compiled to wasm and
//! loaded by the page.
//!
//! There is no Rust in the page itself yet, and after M0-task-A4 there may never need to
//! be: the page is hand-written HTML and JavaScript under `www/`, packaged into `dist/`
//! by `cargo xtask wasm` and served by `cargo xtask serve`. This crate stays in the
//! workspace as the place any future page-side Rust would go, and so the app has a
//! manifest the workspace can see.
//!
//! ```text
//! www/index.html            the page
//! www/app.js                main thread: compile the wasm, own the command transport,
//!                           read the telemetry transport, report which is live
//! www/ring.js               the SharedArrayBuffer wire protocol, used by both ends
//! www/worklet-processor.js  the AudioWorkletProcessor, bundled with the wasm-bindgen
//!                           glue and `ring.js` into dist/starplayer-worklet.js
//! dev-server.mjs            Node dev server, sets the COOP/COEP headers
//!                           SharedArrayBuffer requires
//! ```

#![forbid(unsafe_code)]
