//! Adds the stack-floor assertion to the link (M8-I6).
//!
//! `ld/stack-floor.x` is a linker fragment rather than anything Rust can check: the number
//! it guards — the DRAM left between the end of `.bss` and the top of the data segment —
//! only exists once every static in the firmware has been laid out.

fn main() {
    println!("cargo:rustc-link-search={}/ld", env!("CARGO_MANIFEST_DIR"));
    println!("cargo:rustc-link-arg=-Tstack-floor.x");
    println!("cargo:rerun-if-changed=ld/stack-floor.x");
    bench_environment("STARPLAYER_VOICE_BENCH_CHANNELS", "64", 1, 64);
    bench_environment("STARPLAYER_VOICE_BENCH_VOICES", "256", 1, 256);
    bench_environment("STARPLAYER_VOICE_BENCH_SECONDS", "60", 1, 3_600);
    bench_environment("STARPLAYER_VOICE_BENCH_LOW", "32", 1, 256);
    bench_environment("STARPLAYER_VOICE_BENCH_HIGH", "256", 1, 256);
    bench_choice("STARPLAYER_VOICE_BENCH_PHASE", "qualify", &["qualify", "soak"]);
    bench_choice("STARPLAYER_VOICE_BENCH_AXIS", "voices", &["voices", "channels", "boundary"]);
    bench_case();
}

fn bench_choice(name: &str, default: &str, choices: &[&str]) {
    println!("cargo:rerun-if-env-changed={name}");
    let value = std::env::var(name).unwrap_or_else(|_| default.to_owned());
    assert!(choices.contains(&value.as_str()), "{name} must be one of {}", choices.join(", "));
    println!("cargo:rustc-env={name}={value}");
}

fn bench_case() {
    const NAME: &str = "STARPLAYER_VOICE_BENCH_CASE";
    println!("cargo:rerun-if-env-changed={NAME}");
    let value = std::env::var(NAME).unwrap_or_else(|_| "manual".to_owned());
    assert!(!value.is_empty() && value.len() <= 63, "{NAME} must contain 1..=63 characters");
    assert!(value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'), "{NAME} may contain only ASCII letters, digits and '-'");
    println!("cargo:rustc-env={NAME}={value}");
}

fn bench_environment(name: &str, default: &str, minimum: usize, maximum: usize) {
    println!("cargo:rerun-if-env-changed={name}");
    let value = std::env::var(name).unwrap_or_else(|_| default.to_owned());
    let parsed = value.parse::<usize>().unwrap_or_else(|_| panic!("{name} must be an integer"));
    assert!((minimum..=maximum).contains(&parsed), "{name} must be in {minimum}..={maximum}");
    println!("cargo:rustc-env={name}={value}");
}
