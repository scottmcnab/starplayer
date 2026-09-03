//! Shared module-bytes loading for every subcommand: a plain file, or one entry of a ZIP
//! archive.
//!
//! `--entry N` names the **position** in [`starplayer_archive::list_modules`]'s
//! already-filtered result — zero-based, in the order `starplayer info` lists them — not
//! the archive's raw central-directory index, which also counts directories and entries
//! this crate does not recognise as a module and so is not what a user counting "the
//! second module in the zip" means.

use std::path::Path;

/// Read `path` from disk. The one place every subcommand reports a file-not-found or
/// permission error the same way.
pub fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("could not read `{}`: {error}", path.display()))
}

/// Resolve `bytes` — already read from `path`, for the error messages — to one module's
/// bytes: itself, if it is not a ZIP archive; otherwise the entry `entry` names, or the
/// archive's only recognised entry when `entry` is `None` and there is exactly one,
/// mirroring the web player's picker (`apps/starplayer-web/www/app.js`: a single-entry
/// archive is chosen automatically, and only an archive with more than one asks).
pub fn resolve_entry(path: &Path, bytes: &[u8], entry: Option<usize>) -> Result<Vec<u8>, String> {
    if !starplayer_archive::is_zip(bytes) {
        return Ok(bytes.to_vec());
    }

    let modules = starplayer_archive::list_modules(bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    let chosen = match entry {
        Some(index) => modules.get(index).ok_or_else(|| {
            format!(
                "{}: no entry {index} (the archive has {} recognised module(s); `starplayer info {}` lists them)",
                path.display(),
                modules.len(),
                path.display()
            )
        })?,
        None if modules.len() == 1 => &modules[0],
        None => {
            return Err(format!(
                "{}: contains {} recognised modules; pick one with --entry N (`starplayer info {}` lists them)",
                path.display(),
                modules.len(),
                path.display()
            ));
        }
    };
    starplayer_archive::extract(bytes, chosen.index).map_err(|error| format!("{}: {error}", path.display()))
}

/// [`read_file`] then [`resolve_entry`] — what every subcommand but `info` wants: one
/// module's bytes, from a file path and an optional archive entry.
pub fn load_module_bytes(path: &Path, entry: Option<usize>) -> Result<Vec<u8>, String> {
    let bytes = read_file(path)?;
    resolve_entry(path, &bytes, entry)
}
