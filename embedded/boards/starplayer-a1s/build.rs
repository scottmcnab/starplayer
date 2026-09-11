//! Adds the stack-floor assertion to the link (M8-I6).
//!
//! `ld/stack-floor.x` is a linker fragment rather than anything Rust can check: the number
//! it guards — the DRAM left between the end of `.bss` and the top of the data segment —
//! only exists once every static in the firmware has been laid out.

fn main() {
    println!("cargo:rustc-link-search={}/ld", env!("CARGO_MANIFEST_DIR"));
    println!("cargo:rustc-link-arg=-Tstack-floor.x");
    println!("cargo:rerun-if-changed=ld/stack-floor.x");
}
