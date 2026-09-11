//! Write **module images** — the flash-resident form a device plays without copying a
//! sample frame into RAM (M8-I2).
//!
//! The loader runs here, on the development machine, and the finished `Module` is
//! serialised with `Module::to_image`. `cargo xtask module-image` and
//! `cargo xtask module-images` are the front doors; this binary exists because xtask is
//! deliberately dependency-free and cannot link a loader.
//!
//! ```text
//! starplayer-module-image <module> <out.spmi>   one module
//! starplayer-module-image --fixtures <dir>      the six golden fixtures, into <dir>
//! ```
//!
//! Every image is read back with `Module::from_image_copied` before it is written, so a
//! file this binary produces is one the device's reader accepts — and the PCM share of
//! each image is reported, because that number is what decides whether a module fits a
//! flash partition (M8-I2 research point 2).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use starplayer::model::Module;
use starplayer_offline::{golden_fixtures, module_image_filename};

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let succeeded = match arguments.as_slice() {
        [flag, directory] if flag == "--fixtures" => write_fixture_images(Path::new(directory)),
        [input, output] => write_one_image(Path::new(input), Path::new(output)),
        _ => {
            eprintln!("usage: starplayer-module-image <module> <out.spmi>");
            eprintln!("       starplayer-module-image --fixtures <directory>");
            false
        }
    };
    if succeeded { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn write_one_image(input: &Path, output: &Path) -> bool {
    let bytes = match std::fs::read(input) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("starplayer-module-image: cannot read `{}`: {error}", input.display());
            return false;
        }
    };
    let module = match starplayer::load(&bytes) {
        Ok(module) => module,
        Err(error) => {
            eprintln!("starplayer-module-image: cannot load `{}`: {error}", input.display());
            return false;
        }
    };
    write_image(&module, output)
}

fn write_fixture_images(directory: &Path) -> bool {
    if let Err(error) = std::fs::create_dir_all(directory) {
        eprintln!("starplayer-module-image: cannot create `{}`: {error}", directory.display());
        return false;
    }
    let mut succeeded = true;
    for fixture in golden_fixtures() {
        let path: PathBuf = directory.join(module_image_filename(&fixture));
        let module = match starplayer::load(&fixture.bytes) {
            Ok(module) => module,
            Err(error) => {
                eprintln!("starplayer-module-image: cannot load the {} fixture `{}`: {error}", fixture.format, fixture.stem);
                succeeded = false;
                continue;
            }
        };
        succeeded &= write_image(&module, &path);
    }
    succeeded
}

/// Serialise, read back, and write — in that order, so nothing unreadable reaches a
/// firmware.
fn write_image(module: &Module, output: &Path) -> bool {
    let image = module.to_image();
    match Module::from_image_copied(&image) {
        Ok(reread) if reread == *module => {}
        Ok(_) => {
            eprintln!("starplayer-module-image: `{}` did not read back as the module it came from", output.display());
            return false;
        }
        Err(error) => {
            eprintln!("starplayer-module-image: `{}` did not read back: {error}", output.display());
            return false;
        }
    }
    if let Some(parent) = output.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("starplayer-module-image: cannot create `{}`: {error}", parent.display());
        return false;
    }
    if let Err(error) = std::fs::write(output, &image) {
        eprintln!("starplayer-module-image: cannot write `{}`: {error}", output.display());
        return false;
    }

    let pcm_bytes = module.image_pcm_bytes();
    let pcm_share = match image.is_empty() {
        true => 0.0,
        false => 100.0 * pcm_bytes as f64 / image.len() as f64,
    };
    println!("wrote {:>9} bytes  ({:>9} PCM, {pcm_share:5.1}%)  {}", image.len(), pcm_bytes, output.display());
    true
}
