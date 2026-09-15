//! Runtime format dispatch for bounded caller-buffer module conversion.

pub use starplayer_model::{DecodeBudget, ImageDecodeError, ImageDecodeStatus};

/// Convert a native module directly into a caller-owned SPMI buffer.
/// Construction only identifies the format; call `step` until complete before using the
/// image. Source, destination and scratch storage remain exclusively borrowed meanwhile.
/// The shared work budget requires at least 4096 input bytes and one PCM frame; each
/// step is capped at 4096 input bytes and 1024 PCM frames even when given larger limits.
pub struct ModuleImageDecoder<'buffers> { decoder: Decoder<'buffers> }

// Keep state inline: boxing this enum would introduce an infallible construction allocation.
#[allow(clippy::large_enum_variant)]
enum Decoder<'buffers> {
    #[cfg(feature = "s3m")]
    S3m(starplayer_s3m::image::ImageDecoder<'buffers>),
    #[cfg(feature = "mod")]
    Mod(starplayer_mod::image::ImageDecoder<'buffers>),
    #[cfg(feature = "mtm")]
    Mtm(starplayer_mtm::image::ImageDecoder<'buffers>),
    #[cfg(feature = "xm")]
    Xm(starplayer_xm::image::ImageDecoder<'buffers>),
    #[cfg(feature = "it")]
    It(starplayer_it::image::ImageDecoder<'buffers>),
    #[cfg(not(any(feature = "s3m", feature = "mod", feature = "mtm", feature = "xm", feature = "it")))]
    #[allow(dead_code)]
    Unavailable(core::marker::PhantomData<&'buffers mut [u8]>),
}

impl<'buffers> ModuleImageDecoder<'buffers> {
    #[cfg_attr(not(any(feature = "s3m", feature = "mod", feature = "mtm", feature = "xm", feature = "it")), allow(unreachable_code, unused_variables))]
    pub fn new(source: &'buffers [u8], destination: &'buffers mut [u8], workspace: &'buffers mut [u8]) -> Result<Self, ImageDecodeError> {
        let _ = (&destination, &workspace);
        let decoder = match crate::probe(source) {
            #[cfg(feature = "s3m")]
            Some(starplayer_model::ModuleFormat::S3m) => Decoder::S3m(starplayer_s3m::image::ImageDecoder::new(source, destination, workspace)),
            #[cfg(feature = "mod")]
            Some(starplayer_model::ModuleFormat::Mod) => Decoder::Mod(starplayer_mod::image::ImageDecoder::new(source, destination, workspace)),
            #[cfg(feature = "mtm")]
            Some(starplayer_model::ModuleFormat::Mtm) => Decoder::Mtm(starplayer_mtm::image::ImageDecoder::new(source, destination, workspace)),
            #[cfg(feature = "xm")]
            Some(starplayer_model::ModuleFormat::Xm) => Decoder::Xm(starplayer_xm::image::ImageDecoder::new(source, destination, workspace)),
            #[cfg(feature = "it")]
            Some(starplayer_model::ModuleFormat::It) => Decoder::It(starplayer_it::image::ImageDecoder::new(source, destination, workspace)),
            #[allow(unreachable_patterns)]
            _ => return Err(starplayer_core::Error::BadMagic.into()),
        };
        Ok(Self { decoder })
    }

    pub fn step(&mut self, budget: DecodeBudget) -> Result<ImageDecodeStatus, ImageDecodeError> {
        let _ = budget;
        match &mut self.decoder {
            #[cfg(feature = "s3m")]
            Decoder::S3m(decoder) => decoder.step(budget),
            #[cfg(feature = "mod")]
            Decoder::Mod(decoder) => decoder.step(budget),
            #[cfg(feature = "mtm")]
            Decoder::Mtm(decoder) => decoder.step(budget),
            #[cfg(feature = "xm")]
            Decoder::Xm(decoder) => decoder.step(budget),
            #[cfg(feature = "it")]
            Decoder::It(decoder) => decoder.step(budget),
            #[cfg(not(any(feature = "s3m", feature = "mod", feature = "mtm", feature = "xm", feature = "it")))]
            Decoder::Unavailable(_) => Err(starplayer_core::Error::BadMagic.into()),
        }
    }

    pub fn image_length(&self) -> Option<usize> {
        match &self.decoder {
            #[cfg(feature = "s3m")]
            Decoder::S3m(decoder) => decoder.image_length(),
            #[cfg(feature = "mod")]
            Decoder::Mod(decoder) => decoder.image_length(),
            #[cfg(feature = "mtm")]
            Decoder::Mtm(decoder) => decoder.image_length(),
            #[cfg(feature = "xm")]
            Decoder::Xm(decoder) => decoder.image_length(),
            #[cfg(feature = "it")]
            Decoder::It(decoder) => decoder.image_length(),
            #[cfg(not(any(feature = "s3m", feature = "mod", feature = "mtm", feature = "xm", feature = "it")))]
            Decoder::Unavailable(_) => None,
        }
    }
}
