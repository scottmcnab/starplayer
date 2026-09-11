//! `cargo xtask` for the firmware workspace — the one entry point for building, imaging,
//! sizing and (for the owner) flashing a StarPlayer board.
//!
//! # The working-directory rule, which is the whole reason this exists
//!
//! Cargo reads `.cargo/config.toml` from the directory it is **invoked in**. The target
//! triple, `-Tlinkall.x`, `-C force-frame-pointers` and `-Z build-std` all live in
//! `boards/<board>/.cargo/config.toml`, so a firmware build is only correct when cargo's
//! current directory is that board's crate. Run `cargo build` from `embedded/` instead and
//! cargo will quietly try to build the firmware for the host and fail on the first
//! `esp_hal` name. [`board_command`] is the one place that gets it right, and every build
//! path goes through it.
//!
//! # Flashing is owner-only
//!
//! An agent builds images and hands them over; the owner flashes. `flash` and `monitor`
//! exist here so the *command* is written down in one place, and they print that rule
//! before they run. Nothing in this file is allowed to erase a device.
//!
//! # One target directory
//!
//! Every build shares `embedded/target`. There is no second one and there must not be: a
//! `build-std` tree for one Xtensa target is already several hundred megabytes.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Everything that differs between one board and the next.
///
/// A table rather than a `match`: M8-I4 adds the ESP32-C5 by adding a row, and the fact
/// that adding a row is the entire change is the point of having the struct.
#[derive(Clone, Copy)]
struct Board {
    /// What the user types after `--board`.
    name: &'static str,
    /// espflash's `--chip`.
    chip: &'static str,
    /// The Rust target triple.
    target: &'static str,
    /// The board crate's directory, relative to `embedded/`.
    directory: &'static str,
    /// The cargo package name, for the binary's path under `target/`.
    package: &'static str,
    /// The partition table, relative to the board crate.
    partition_table: &'static str,
    /// Which app partition the image is sized against and written to.
    app_partition: &'static str,
    /// espflash's `--flash-size`.
    flash_size: &'static str,
    /// The compile-time `ESP_LOG` filter. esp-println reads it at build time, so it is
    /// part of what makes a build reproducible.
    esp_log: &'static str,
    /// The toolchain-provided linker that must be on `PATH`, so a missing
    /// `. ~/export-esp-1.97.sh` is diagnosed by name rather than by a link error.
    linker: &'static str,
}

/// The AI-Thinker ESP32-Audio-Kit (M8-I3).
const A1S: Board = Board {
    name: "a1s",
    chip: "esp32",
    target: "xtensa-esp32-none-elf",
    directory: "boards/starplayer-a1s",
    package: "starplayer-a1s",
    partition_table: "partitions.csv",
    app_partition: "factory",
    flash_size: "4mb",
    esp_log: "info",
    linker: "xtensa-esp32-elf-gcc",
};

/// Every board this workspace knows how to build. M8-I4 appends the C5 here.
const BOARDS: &[Board] = &[A1S];

/// What the command line asked for.
struct Selectors {
    board: Board,
    /// Extra cargo features, comma-joined, on top of the board's default set.
    features: String,
    /// `--dev`: the `dev` profile (release codegen, debug assertions on).
    dev: bool,
}

impl Selectors {
    /// The cargo flags every build, image and size share.
    fn cargo_flags(&self) -> Vec<String> {
        let mut flags = vec!["--target".to_string(), self.board.target.to_string()];
        if !self.dev {
            flags.push("--release".to_string());
        }
        if !self.features.is_empty() {
            flags.push("--features".to_string());
            flags.push(self.features.clone());
        }
        flags
    }

    /// The profile directory under `target/<triple>/`.
    fn profile_directory(&self) -> &'static str { if self.dev { "debug" } else { "release" } }

    /// A stem that names the build, so two images never overwrite each other.
    fn output_stem(&self) -> String {
        let mut stem = format!("starplayer-{}", self.board.name);
        for feature in self.features.split(',').map(str::trim).filter(|feature| !feature.is_empty()) {
            stem.push('-');
            stem.push_str(feature);
        }
        if self.dev {
            stem.push_str("-dev");
        }
        stem
    }

    /// Where `cargo build` puts the ELF.
    fn elf_path(&self) -> PathBuf {
        embedded_root()
            .join("target")
            .join(self.board.target)
            .join(self.profile_directory())
            .join(self.board.package)
    }
}

/// `embedded/`, derived from this crate's manifest directory rather than from the current
/// working directory, so `cargo xtask` behaves the same wherever it is run from.
fn embedded_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("embedded/xtask has a parent").to_path_buf()
}

/// The main StarPlayer workspace, one level above `embedded/`.
fn repository_root() -> PathBuf {
    embedded_root().parent().expect("embedded/ has a parent").to_path_buf()
}

/// A cargo invocation with its current directory set to the board crate — see the
/// working-directory rule in the module docs.
///
/// The toolchain is named explicitly (`+esp-1.97`) as well as pinned by
/// `embedded/rust-toolchain.toml`. Belt and braces: the file is what CI and a bare `cargo`
/// obey, and the argument is what makes an error message say which compiler was asked for.
fn board_command(board: &Board) -> Command {
    let mut command = Command::new("cargo");
    command.arg("+esp-1.97");
    command.current_dir(embedded_root().join(board.directory));
    command.env("ESP_LOG", board.esp_log);
    command
}

/// Refuse early, with the fix, if the Xtensa GCC linker is not on `PATH`.
///
/// Without `. ~/export-esp-1.97.sh` the build gets a long way and then fails with
/// `linker 'xtensa-esp32-elf-gcc' not found`, which is a perfectly good message buried
/// under a screen of cargo output. This one is the first thing printed.
fn require_linker(board: &Board) -> Result<(), String> {
    let found = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|directory| directory.join(board.linker).is_file()))
        .unwrap_or(false);
    if found {
        return Ok(());
    }
    Err(format!(
        "{} is not on PATH.\n  The Xtensa toolchain's environment has not been sourced. Run:\n\n      . ~/export-esp-1.97.sh\n\n  in this shell and try again (it sets PATH for the linker and LIBCLANG_PATH for bindgen).",
        board.linker
    ))
}

/// Make sure `embedded/assets/*.spmi` exists, by running the main workspace's
/// `cargo xtask module-images` when it does not.
///
/// The images are git-ignored build products that the firmware links with
/// `include_bytes!`, so a fresh clone has none and a build would fail on a missing file.
/// Running the generator is cheap and idempotent. `--force` regenerates unconditionally,
/// which is what to do after `IMAGE_VERSION` moves.
fn ensure_assets(force: bool) -> Result<(), String> {
    let assets = embedded_root().join("assets");
    let petri = assets.join("petri-s3m.spmi");
    if petri.is_file() && !force {
        return Ok(());
    }
    eprintln!("xtask: generating module images (cargo xtask module-images in the main workspace)");
    let mut command = Command::new("cargo");
    command.arg("xtask").arg("module-images").current_dir(repository_root());
    run(command)
}

/// `cargo xtask build` — compile the firmware.
fn command_build(selectors: &Selectors) -> Result<(), String> {
    require_linker(&selectors.board)?;
    ensure_assets(false)?;
    let mut command = board_command(&selectors.board);
    command.arg("build");
    for flag in selectors.cargo_flags() {
        command.arg(flag);
    }
    run(command)?;
    eprintln!("xtask: built {}", selectors.elf_path().display());
    Ok(())
}

/// `cargo xtask image` — an espflash image beside the ELF.
///
/// Without `--merge` this is the **app-only** image: the bytes that go into the app
/// partition, and the number `size` measures against the partition's budget. With
/// `--merge` it is the whole flashable image (bootloader, partition table and app), which
/// is what an owner writes to a blank board in one go.
fn command_image(selectors: &Selectors, merge: bool, out: Option<&str>) -> Result<PathBuf, String> {
    require_linker(&selectors.board)?;
    ensure_assets(false)?;

    let out_path = match out {
        Some(path) => PathBuf::from(path),
        None => {
            let suffix = if merge { "-merged.bin" } else { ".bin" };
            embedded_root().join("target").join(format!("{}{suffix}", selectors.output_stem()))
        }
    };
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }

    let mut command = board_command(&selectors.board);
    command.arg("espflash").arg("save-image");
    command.arg("--chip").arg(selectors.board.chip);
    if merge {
        // `--skip-padding` so a reflash does not wipe the `modules` and `config`
        // partitions M8-I6 will be storing things in.
        command.arg("--merge").arg("--skip-padding");
    }
    for flag in selectors.cargo_flags() {
        command.arg(flag);
    }
    // The partition table is passed explicitly rather than left to
    // `[package.metadata.espflash]`, so the table this file names is the single source of
    // truth for both the image and the size gate.
    command.arg("--partition-table").arg(selectors.board.partition_table);
    command.arg("--target-app-partition").arg(selectors.board.app_partition);
    command.arg("--flash-size").arg(selectors.board.flash_size);
    command.arg(&out_path);
    run(command)?;
    eprintln!("xtask: wrote {}", out_path.display());
    Ok(out_path)
}

/// `cargo xtask size` — the app image against its partition's budget.
///
/// Warns at 90 % and fails above 100 %, which is the only threshold that means anything:
/// an image larger than its partition cannot be flashed at all.
fn command_size(selectors: &Selectors) -> Result<ExitCode, String> {
    let image = command_image(selectors, false, None)?;
    let size = std::fs::metadata(&image).map_err(|error| format!("cannot stat {}: {error}", image.display()))?.len();
    let limit = partition_size(selectors)?;

    let permille = size * 1000 / limit;
    println!();
    println!("Firmware size — {} {}", selectors.board.name, if selectors.features.is_empty() { "(default)" } else { selectors.features.as_str() });
    println!("  profile      : {}", selectors.profile_directory());
    println!("  image        : {size} bytes ({})", image.display());
    println!("  {:<13}: {limit} bytes ({})", selectors.board.app_partition, selectors.board.partition_table);
    println!("  usage        : {}.{}%", permille / 10, permille % 10);

    if size > limit {
        eprintln!("error: the image is {size} bytes and the {} partition holds {limit}", selectors.board.app_partition);
        return Ok(ExitCode::FAILURE);
    }
    if permille >= 900 {
        eprintln!("warning: the image is at {}.{}% of its partition", permille / 10, permille % 10);
    }
    Ok(ExitCode::SUCCESS)
}

/// The size of the board's app partition, read out of its CSV.
///
/// A deliberately small parser: strip comments, split on commas, find the row whose name
/// matches, and read its size field. Sizes are hex (`0x280000`), decimal, or a decimal with
/// a `K`/`M` suffix, which is everything the ESP-IDF format allows.
fn partition_size(selectors: &Selectors) -> Result<u64, String> {
    let path = embedded_root().join(selectors.board.directory).join(selectors.board.partition_table);
    let text = std::fs::read_to_string(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        if fields.first() != Some(&selectors.board.app_partition) {
            continue;
        }
        let size = fields.get(4).ok_or_else(|| format!("{} has no size field for {}", path.display(), selectors.board.app_partition))?;
        return parse_partition_size(size).ok_or_else(|| format!("cannot parse the size {size:?} in {}", path.display()));
    }
    Err(format!("{} has no {} row", path.display(), selectors.board.app_partition))
}

/// `0x280000`, `2621440`, `2560K` or `2M` → bytes.
fn parse_partition_size(field: &str) -> Option<u64> {
    let field = field.trim();
    if let Some(hex) = field.strip_prefix("0x").or_else(|| field.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).ok();
    }
    if let Some(kilobytes) = field.strip_suffix(['K', 'k']) {
        return kilobytes.parse::<u64>().ok().map(|value| value * 1024);
    }
    if let Some(megabytes) = field.strip_suffix(['M', 'm']) {
        return megabytes.parse::<u64>().ok().map(|value| value * 1024 * 1024);
    }
    field.parse::<u64>().ok()
}

/// `cargo xtask flash` / `monitor` — **owner only**.
fn command_owner_espflash(selectors: &Selectors, subcommand: &str, passthrough: &[String]) -> Result<(), String> {
    eprintln!("NOTE: flashing and monitoring hardware is OWNER-ONLY. An agent builds images");
    eprintln!("      and hands them over; it must never run this. See embedded/README.md.");
    let mut command = Command::new("espflash");
    command.arg(subcommand);
    command.current_dir(embedded_root());
    if subcommand == "flash" {
        let image = command_image(selectors, true, None)?;
        command.arg("--chip").arg(selectors.board.chip);
        command.arg("--flash-size").arg(selectors.board.flash_size);
        command.arg("--monitor");
        command.arg(image);
    }
    for argument in passthrough {
        command.arg(argument);
    }
    run(command)
}

fn run(mut command: Command) -> Result<(), String> {
    eprintln!("+ {command:?}");
    let status = command.status().map_err(|error| format!("cannot run {command:?}: {error}"))?;
    if status.success() { Ok(()) } else { Err(format!("{command:?} exited with {status}")) }
}

fn parse_selectors(arguments: &mut Vec<String>) -> Result<Selectors, String> {
    let mut board_name: Option<String> = None;
    let mut features = String::new();
    let mut dev = false;
    let mut remaining = Vec::new();

    let taken: Vec<String> = core::mem::take(arguments);
    let mut iterator = taken.into_iter();
    while let Some(argument) = iterator.next() {
        match argument.as_str() {
            "--board" => board_name = Some(iterator.next().ok_or("--board needs a value (a1s)")?),
            "--features" => features = iterator.next().ok_or("--features needs a value")?,
            "--dev" => dev = true,
            "--release" => dev = false,
            _ => remaining.push(argument),
        }
    }
    let name = board_name.unwrap_or_else(|| A1S.name.to_string());
    let board = BOARDS
        .iter()
        .find(|board| board.name == name)
        .copied()
        .ok_or_else(|| format!("unknown board {name:?}; known boards: {}", BOARDS.iter().map(|board| board.name).collect::<Vec<&str>>().join(", ")))?;

    *arguments = remaining;
    Ok(Selectors { board, features, dev })
}

fn usage() {
    eprintln!("cargo xtask <command> [--board a1s] [--features f,g] [--dev]");
    eprintln!();
    eprintln!("  build                compile the firmware");
    eprintln!("  image [--merge]      save an espflash image (--merge = bootloader + table + app)");
    eprintln!("  size                 the app image against its partition's budget");
    eprintln!("  assets [--force]     regenerate embedded/assets/*.spmi from the main workspace");
    eprintln!("  flash                OWNER ONLY — write the merged image and monitor");
    eprintln!("  monitor              OWNER ONLY — attach to the serial console");
    eprintln!();
    eprintln!("Every build needs the Xtensa toolchain's environment:  . ~/export-esp-1.97.sh");
}

fn main() -> ExitCode {
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.is_empty() {
        usage();
        return ExitCode::FAILURE;
    }
    let command = arguments.remove(0);

    let result = match command.as_str() {
        "build" => parse_selectors(&mut arguments).and_then(|selectors| command_build(&selectors)).map(|_| ExitCode::SUCCESS),
        "image" => parse_selectors(&mut arguments).and_then(|selectors| {
            let merge = arguments.iter().any(|argument| argument == "--merge");
            let out = arguments.iter().position(|argument| argument == "--out").and_then(|index| arguments.get(index + 1)).cloned();
            command_image(&selectors, merge, out.as_deref()).map(|_| ExitCode::SUCCESS)
        }),
        "size" => parse_selectors(&mut arguments).and_then(|selectors| command_size(&selectors)),
        "assets" => ensure_assets(arguments.iter().any(|argument| argument == "--force")).map(|_| ExitCode::SUCCESS),
        "flash" | "monitor" => parse_selectors(&mut arguments)
            .and_then(|selectors| command_owner_espflash(&selectors, &command, &arguments))
            .map(|_| ExitCode::SUCCESS),
        "help" | "--help" | "-h" => {
            usage();
            Ok(ExitCode::SUCCESS)
        }
        other => Err(format!("unknown command {other:?}")),
    };

    match result {
        Ok(code) => code,
        Err(message) => {
            eprintln!("xtask: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_sizes_parse_in_every_spelling_the_idf_format_allows() {
        assert_eq!(parse_partition_size("0x280000"), Some(2_621_440));
        assert_eq!(parse_partition_size("2621440"), Some(2_621_440));
        assert_eq!(parse_partition_size("2560K"), Some(2_621_440));
        assert_eq!(parse_partition_size("2M"), Some(2_097_152));
        assert_eq!(parse_partition_size("  0x1000 "), Some(4_096));
        assert_eq!(parse_partition_size("not a size"), None);
    }

    #[test]
    fn the_a1s_app_partition_is_two_and_a_half_megabytes() {
        let selectors = Selectors { board: A1S, features: String::new(), dev: false };
        assert_eq!(partition_size(&selectors).expect("the a1s table parses"), 0x280000);
    }

    #[test]
    fn an_output_stem_names_the_features_and_the_profile() {
        let selectors = Selectors { board: A1S, features: "bench".to_string(), dev: true };
        assert_eq!(selectors.output_stem(), "starplayer-a1s-bench-dev");
        let plain = Selectors { board: A1S, features: String::new(), dev: false };
        assert_eq!(plain.output_stem(), "starplayer-a1s");
    }

    #[test]
    fn every_board_name_is_unique() {
        let mut names: Vec<&str> = BOARDS.iter().map(|board| board.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), BOARDS.len());
    }
}
