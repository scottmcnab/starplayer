//! Explicit loaded-module live preparation. Legacy rack constructors remain sample-only.
use alloc::boxed::Box;
use crate::{core::Error, engine::instrument::{InstrumentRack, live::{RackRuntime, RackScope}}, model::{Module, ModuleFormat}, rt::Arc};

/// Build one native format runtime per rack, sized for the actual physical voice pool.
#[cfg(feature = "interactive-audition")]
pub fn prepare_live_rack(module: &Arc<Module>, sample_rate_hz: u32, voice_capacity: usize) -> Result<InstrumentRack, Error> {
    prepare_instrument_rack(module, sample_rate_hz, voice_capacity, RackScope::Live)
}

/// Explicit song/live scope; only prepared interactive hosts use the native rack.
pub fn prepare_instrument_rack(module: &Arc<Module>, sample_rate_hz: u32, voice_capacity: usize, scope: RackScope) -> Result<InstrumentRack, Error> {
    #[allow(unreachable_patterns)]
    let runtime: Option<Box<dyn RackRuntime>> = match module.header().format {
        #[cfg(feature = "xm")]
        ModuleFormat::Xm => Some(Box::new(crate::xm::processor::live::XmLiveRuntime::try_new(voice_capacity, sample_rate_hz).map_err(|_| Error::Unsupported("live XM allocation failed"))?)),
        #[cfg(feature = "it")]
        ModuleFormat::It => Some(Box::new(crate::it::processor::live::ItLiveRuntime::try_new(voice_capacity, sample_rate_hz).map_err(|_| Error::Unsupported("live IT allocation failed"))?)),
        ModuleFormat::Mod | ModuleFormat::Mtm | ModuleFormat::S3m => None,
        _ => return Err(Error::Unsupported("native live format is not compiled")),
    };
    InstrumentRack::try_prepared(module, sample_rate_hz, voice_capacity, runtime, native_map(module), scope).map_err(|_| Error::Unsupported("live rack allocation failed"))
}

fn native_map(module: &Module) -> bool {
    match module.header().format {
        ModuleFormat::Xm => true,
        #[cfg(feature = "it")]
        ModuleFormat::It => crate::it::ItFormatExtra::from_header(module.header()).is_instrument_mode(),
        _ => false,
    }
}

/// Pure metadata query: no allocation or runtime preparation required.
pub fn live_instrument_capability(module: &Module, instrument: crate::core::InstrumentId) -> crate::engine::instrument::live::InstrumentCapability {
    crate::engine::instrument::live::instrument_capability_filtered(module, instrument, native_map(module), |note, sample| {
        #[cfg(feature = "xm")]
        if module.header().format == ModuleFormat::Xm {
            return module.sample(sample).is_some_and(|sample| crate::xm::processor::live::live_note_table_index(sample, crate::core::Note::new(note), 0).is_some());
        }
        let _ = (note, sample);
        true
    })
}
