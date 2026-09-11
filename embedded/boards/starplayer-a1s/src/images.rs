//! The module images linked into flash.
//!
//! `include_bytes!` yields a `&'static [u8]` of **alignment 1**, and
//! [`Module::from_image`](starplayer_model::Module::from_image) refuses an image that is
//! not 4-byte aligned in memory rather than silently copying its PCM (M8-I2 research point
//! 1). The `#[repr(C, align(4))]` newtype below is the wrapper that makes the strict
//! constructor usable, and this module exists so there is exactly one of it.
//!
//! The files come from `cargo xtask module-images` in the **main** workspace, which writes
//! `embedded/assets/*.spmi`. They are git-ignored build products: `embedded/xtask` runs
//! that command before every firmware build, so a fresh clone builds.
//!
//! # What each build links
//!
//! The audio firmware links `REFLEX.S3M` alone — the module it plays at boot, 14 984 bytes
//! of image. The `bench` build links all six golden fixtures, 117 040 bytes together
//! (`PETRI.S3M`, at 88 036 bytes and 73 % PCM, is the largest of them). Nothing is ever copied out of flash to play
//! it: the mixer reads the PCM in place through the instruction cache, which is the whole
//! point of M8-I2 and the `flash` row of the budget document's CPU table.

/// Declare a `#[repr(C, align(4))]` static holding one module image.
///
/// A distinct type per image, because the array length is part of the type and there is
/// no way to name "an aligned byte array of whatever length that file happens to be"
/// without one. The array is the struct's only field, so it sits at offset 0 and inherits
/// the struct's 4-byte alignment — which is precisely the guarantee `from_image` checks.
macro_rules! flash_image {
    ($(#[$documentation:meta])* $type_name:ident, $name:ident, $path:literal) => {
        #[repr(C, align(4))]
        struct $type_name([u8; include_bytes!($path).len()]);

        $(#[$documentation])*
        static $name: $type_name = $type_name(*include_bytes!($path));
    };
}

flash_image!(
    /// `REFLEX.S3M` — four channels, ten patterns, 4 480 bytes of PCM. The module the
    /// audio firmware plays at boot.
    ReflexImage, REFLEX_S3M, "../../../assets/reflex-s3m.spmi"
);

/// The boot module's image — `REFLEX.S3M`, 4-byte aligned and `'static`.
///
/// The `bench` build reaches it through [`GOLDEN_FIXTURES`] instead, so this accessor has
/// no caller there.
#[cfg_attr(feature = "bench", allow(dead_code))]
pub fn boot_module() -> &'static [u8] { &REFLEX_S3M.0 }

/// The boot module's name, as the web module list shows it.
#[cfg(feature = "web")]
pub const BOOT_MODULE_NAME: &str = "REFLEX";

#[cfg(feature = "bench")]
mod fixtures {
    flash_image!(
        /// `PETRI.S3M` — eight channels, five samples, 64 156 bytes of PCM; the largest
        /// fixture, and the one the budget document's PSRAM/DRAM rows are about.
        PetriImage, PETRI_S3M, "../../../assets/petri-s3m.spmi"
    );
    flash_image!(
        /// The synthetic MOD fixture: 31 instruments, two patterns.
        SyntheticModImage, SYNTHETIC_MOD, "../../../assets/synthetic-mod.spmi"
    );
    flash_image!(
        /// The synthetic MTM fixture.
        SyntheticMtmImage, SYNTHETIC_MTM, "../../../assets/synthetic-mtm.spmi"
    );
    flash_image!(
        /// The synthetic XM fixture.
        SyntheticXmImage, SYNTHETIC_XM, "../../../assets/synthetic-xm.spmi"
    );
    flash_image!(
        /// The synthetic IT fixture.
        SyntheticItImage, SYNTHETIC_IT, "../../../assets/synthetic-it.spmi"
    );

    /// Every golden fixture, named the way `goldens/` and the budget document name them.
    ///
    /// The order is the order `starplayer_offline::golden_fixtures()` lists them in, so a
    /// device transcript and a host one can be compared line for line.
    pub static ALL: [(&str, &[u8]); 6] = [
        ("synthetic-mod", &SYNTHETIC_MOD.0),
        ("synthetic-mtm", &SYNTHETIC_MTM.0),
        ("synthetic-xm", &SYNTHETIC_XM.0),
        ("synthetic-it", &SYNTHETIC_IT.0),
        ("petri-s3m", &PETRI_S3M.0),
        ("reflex-s3m", &super::REFLEX_S3M.0),
    ];
}

#[cfg(feature = "bench")]
pub use fixtures::ALL as GOLDEN_FIXTURES;
