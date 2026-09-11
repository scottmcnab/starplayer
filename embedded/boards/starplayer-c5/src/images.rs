//! The six golden-fixture module images linked into flash, `bench`-only.
//!
//! Mirrors `starplayer-a1s/src/images.rs` exactly — same macro, same six files, same
//! `#[repr(C, align(4))]` wrapper for the same reason: [`Module::from_image`] requires a
//! 4-byte-aligned `&'static [u8]` and refuses to borrow a misaligned one rather than
//! silently copying (M8-I2 research point 1). There is no `petri_s3m()` standalone
//! accessor here — this board has no "audio" build to link it into, only `bench`.

macro_rules! flash_image {
    ($(#[$documentation:meta])* $type_name:ident, $name:ident, $path:literal) => {
        #[repr(C, align(4))]
        struct $type_name([u8; include_bytes!($path).len()]);

        $(#[$documentation])*
        static $name: $type_name = $type_name(*include_bytes!($path));
    };
}

flash_image!(
    /// `PETRI.S3M` — eight channels, five samples, 64 156 bytes of PCM. The module the
    /// milestone's exit criterion is stated in terms of.
    PetriImage, PETRI_S3M, "../../../assets/petri-s3m.spmi"
);
flash_image!(
    /// `REFLEX.S3M` — four channels, ten patterns.
    ReflexImage, REFLEX_S3M, "../../../assets/reflex-s3m.spmi"
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
/// The order matches `starplayer_offline::golden_fixtures()`, exactly as the A1S's does,
/// so a device transcript from either board can be compared line for line.
pub static GOLDEN_FIXTURES: [(&str, &[u8]); 6] = [
    ("synthetic-mod", &SYNTHETIC_MOD.0),
    ("synthetic-mtm", &SYNTHETIC_MTM.0),
    ("synthetic-xm", &SYNTHETIC_XM.0),
    ("synthetic-it", &SYNTHETIC_IT.0),
    ("petri-s3m", &PETRI_S3M.0),
    ("reflex-s3m", &REFLEX_S3M.0),
];
