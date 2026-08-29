//! Test harness shared by the workspace: golden comparison, the libxmp / libopenmpt diff
//! harness, and the trace differ.
//!
//! Allowed dependency edges: `starplayer`, `starplayer-offline`, plus the reference
//! player bindings once M1 lands.

#![forbid(unsafe_code)]
