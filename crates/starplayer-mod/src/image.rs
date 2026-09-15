use alloc::vec::Vec;
use starplayer_core::fixed::unit_from_ratio;
use starplayer_core::quirks::FormatDialect;
use starplayer_core::{Error, U0F16};
use starplayer_model::{
    DecodeBudget, ImageDecodeError, ImageDecodeStatus, InstrumentDef, LoopMode, ModuleFlags,
    ModuleFormat, ModuleHeader, ModuleImagePlan, ORDER_END, SampleSpec,
    MINIMUM_DECODE_INPUT_BUDGET,
};

use crate::loader::{
    HEADER_BYTES, LoadOptions, MAGIC_OFFSET, ModLayout, ORDER_OFFSET, SAMPLE_COUNT,
    SAMPLE_HEADER_BYTES, SONG_LENGTH_OFFSET, be_u16, default_pan, layout, logical_order,
};
use crate::pattern::{CELL_BYTES, ModCell, ROWS};
use crate::tables::FINETUNE_REFERENCE_RATES;
use crate::timing::{ModTimingEvidence, encode_evidence, high_fxx_only_at_end};

const METADATA_RESOURCE: Error = Error::Resource("not enough memory for module image metadata");

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase { Header, ScanPatterns, Samples, FinishPlan, Layout, Metadata, Patterns, Pcm, Complete }

pub struct ImageDecoder<'buffers> {
    source: &'buffers [u8],
    destination: &'buffers mut [u8],
    workspace: &'buffers mut [u8],
    options: LoadOptions,
    phase: Phase,
    layout: Option<ModLayout>,
    plan: ModuleImagePlan,
    pattern_count: usize,
    pattern_data_offset: usize,
    sample_data_offset: usize,
    scan_position: usize,
    scan_pattern: usize,
    scan_last_high: u8,
    scan_row_low: bool,
    scan_row_high: bool,
    mixed_row: bool,
    amiga_limits: bool,
    last_high: Vec<u8>,
    oversized_sample: bool,
    declared_sample_offset: usize,
    sample_sources: Vec<usize>,
    item: usize,
    metadata_bytes: usize,
    blob_start: usize,
    pcm_start: usize,
    pattern_position: usize,
    pcm_sample: usize,
    pcm_position: usize,
    image_length: Option<usize>,
    metadata_part: usize,
    metadata_position: usize,
}

impl<'buffers> ImageDecoder<'buffers> {
    pub fn new(source: &'buffers [u8], destination: &'buffers mut [u8], workspace: &'buffers mut [u8]) -> ImageDecoder<'buffers> {
        Self::with_options(source, destination, workspace, LoadOptions::default())
    }

    pub fn with_options(source: &'buffers [u8], destination: &'buffers mut [u8], workspace: &'buffers mut [u8], options: LoadOptions) -> ImageDecoder<'buffers> {
        ImageDecoder {
            source, destination, workspace, options, phase: Phase::Header, layout: None,
            plan: ModuleImagePlan::new(), pattern_count: 0, pattern_data_offset: 0,
            sample_data_offset: 0, scan_position: 0, scan_pattern: 0, scan_last_high: 0,
            scan_row_low: false, scan_row_high: false, mixed_row: false, amiga_limits: true,
            last_high: Vec::new(), oversized_sample: false, declared_sample_offset: 0,
            sample_sources: Vec::new(), item: 0, metadata_bytes: 0, blob_start: 0,
            pcm_start: 0, pattern_position: 0, pcm_sample: 0, pcm_position: 0,
            image_length: None,
            metadata_part: 0, metadata_position: 0,
        }
    }

    pub fn image_length(&self) -> Option<usize> { self.image_length }

    pub fn step(&mut self, budget: DecodeBudget) -> Result<ImageDecodeStatus, ImageDecodeError> {
        if budget.max_input_bytes < MINIMUM_DECODE_INPUT_BUDGET || budget.max_pcm_frames == 0 {
            return Err(ImageDecodeError::BudgetTooSmall { minimum_input_bytes: MINIMUM_DECODE_INPUT_BUDGET, minimum_pcm_frames: 1 });
        }
        let budget = DecodeBudget { max_input_bytes: budget.max_input_bytes.min(4096), max_pcm_frames: budget.max_pcm_frames.min(1024) };
        let _workspace_capacity = self.workspace.len();
        match self.phase {
            Phase::Header => self.prepare_header()?,
            Phase::ScanPatterns => self.scan_patterns(budget.max_input_bytes)?,
            Phase::Samples => self.prepare_sample()?,
            Phase::FinishPlan => self.finish_plan()?,
            Phase::Layout => self.prepare_layout()?,
            Phase::Metadata => self.write_metadata_part()?,
            Phase::Patterns => self.write_patterns(budget.max_input_bytes)?,
            Phase::Pcm => self.write_pcm(budget)?,
            Phase::Complete => {}
        }
        Ok(match self.image_length {
            Some(image_length) if self.phase == Phase::Complete => ImageDecodeStatus::Complete { image_length },
            _ => ImageDecodeStatus::Pending,
        })
    }

    fn format_layout(&self) -> Result<ModLayout, Error> { self.layout.ok_or(Error::Invalid("MOD decoder layout is unavailable")) }

    fn prepare_header(&mut self) -> Result<(), ImageDecodeError> {
        let fixed = self.source.get(..HEADER_BYTES).ok_or(Error::Truncated { offset: 0, needed: HEADER_BYTES })?;
        let module_layout = layout(fixed.get(MAGIC_OFFSET..MAGIC_OFFSET + 4).unwrap_or_default()).ok_or(Error::BadMagic)?;
        let orders = fixed.get(ORDER_OFFSET..ORDER_OFFSET + 128).ok_or(Error::Truncated { offset: ORDER_OFFSET, needed: 128 })?;
        self.pattern_count = orders.iter().copied().filter(|order| *order < 0x80)
            .map(|order| logical_order(order, module_layout) as usize).max().map(|value| value + 1).unwrap_or(0);
        let pattern_bytes = self.pattern_count.checked_mul(ROWS as usize)
            .and_then(|value| value.checked_mul(module_layout.channels as usize))
            .and_then(|value| value.checked_mul(CELL_BYTES)).ok_or(Error::TooLarge("MOD pattern data"))?;
        self.pattern_data_offset = HEADER_BYTES + module_layout.extra_header_bytes;
        self.sample_data_offset = self.pattern_data_offset.checked_add(pattern_bytes).ok_or(Error::TooLarge("MOD data offset"))?;
        if self.sample_data_offset > self.source.len() {
            return Err(Error::Truncated { offset: self.pattern_data_offset, needed: pattern_bytes }.into());
        }
        self.last_high.try_reserve_exact(self.pattern_count).map_err(|_| METADATA_RESOURCE)?;
        for _ in 0..self.pattern_count {
            self.plan.try_add_pattern(ROWS as usize * module_layout.channels as usize * CELL_BYTES, ROWS, module_layout.channels)?;
        }
        self.layout = Some(module_layout);
        self.declared_sample_offset = self.sample_data_offset;
        self.phase = Phase::ScanPatterns;
        Ok(())
    }

    fn scan_patterns(&mut self, max_input_bytes: usize) -> Result<(), ImageDecodeError> {
        let module_layout = self.format_layout()?;
        let pattern_stride = ROWS as usize * module_layout.channels as usize * CELL_BYTES;
        let total = self.pattern_count * pattern_stride;
        let mut consumed = 0usize;
        while self.scan_position + CELL_BYTES <= total && consumed + CELL_BYTES <= max_input_bytes {
            let output_cell = self.scan_position / CELL_BYTES;
            let source_cell = if module_layout.paired_four_channel_patterns {
                let within_pattern = output_cell % (ROWS as usize * 8);
                let row = within_pattern / 8;
                let channel = within_pattern % 8;
                let stored_cell = if channel < 4 { row * 4 + channel } else { ROWS as usize * 4 + row * 4 + channel - 4 };
                (output_cell / (ROWS as usize * 8)) * (ROWS as usize * 8) + stored_cell
            } else { output_cell };
            let offset = self.pattern_data_offset + source_cell * CELL_BYTES;
            let bytes = self.source.get(offset..offset + CELL_BYTES).ok_or(Error::Truncated { offset, needed: CELL_BYTES })?;
            let cell = ModCell::from_bytes(bytes).ok_or(Error::Invalid("bad MOD cell"))?;
            self.amiga_limits &= cell.period == 0 || (113..=856).contains(&cell.period);
            if cell.effect == 0xF {
                if cell.param >= 0x20 { self.scan_last_high = cell.param; self.scan_row_high = true; }
                else { self.scan_row_low = true; }
            }
            self.scan_position += CELL_BYTES;
            consumed += CELL_BYTES;
            let cell_in_pattern = self.scan_position / CELL_BYTES % (ROWS as usize * module_layout.channels as usize);
            if cell_in_pattern.is_multiple_of(module_layout.channels as usize) {
                self.mixed_row |= self.scan_row_low && self.scan_row_high;
                self.scan_row_low = false;
                self.scan_row_high = false;
            }
            if cell_in_pattern == 0 {
                self.last_high.push(self.scan_last_high);
                self.scan_last_high = 0;
                self.scan_pattern += 1;
            }
        }
        if self.scan_position == total { self.phase = Phase::Samples; }
        Ok(())
    }

    fn prepare_sample(&mut self) -> Result<(), ImageDecodeError> {
        if self.item >= SAMPLE_COUNT { self.item = 0; self.phase = Phase::FinishPlan; return Ok(()); }
        let fixed = self.source.get(..HEADER_BYTES).ok_or(Error::Truncated { offset: 0, needed: HEADER_BYTES })?;
        let header_offset = 20 + self.item * SAMPLE_HEADER_BYTES;
        let header = fixed.get(header_offset..header_offset + SAMPLE_HEADER_BYTES)
            .ok_or(Error::Truncated { offset: header_offset, needed: SAMPLE_HEADER_BYTES })?;
        let name = starplayer_model::try_decode_cp437(header.get(..22).unwrap_or_default())?;
        let declared_words = be_u16(header, 22)?;
        self.oversized_sample |= declared_words >= 0x8000;
        let declared_length = declared_words as usize * 2;
        let available = self.source.len().saturating_sub(self.declared_sample_offset).min(declared_length);
        let loop_start = (be_u16(header, 26)? as usize * 2).min(declared_length).min(available);
        let loop_length = be_u16(header, 28)? as usize * 2;
        let loop_end = loop_start.saturating_add(loop_length).min(available);
        let loops = loop_length >= 4 && loop_start < loop_end;
        let specification = SampleSpec {
            name: starplayer_model::try_clone_text(&name)?, loop_mode: if loops { LoopMode::Forward } else { LoopMode::None },
            loop_start: if loops { loop_start as u32 } else { 0 }, loop_end: if loops { loop_end as u32 } else { 0 },
            default_volume: unit_from_ratio(header.get(25).copied().unwrap_or(0).min(64) as u32, 64),
            reference_rate_hz: FINETUNE_REFERENCE_RATES[(header.get(24).copied().unwrap_or(0) & 15) as usize],
            ..SampleSpec::default()
        };
        let sample = self.plan.try_add_sample(available, specification)?;
        self.plan.try_add_instrument(InstrumentDef::try_from_sample(&name, sample, U0F16::MAX)?)?;
        self.sample_sources.try_reserve(1).map_err(|_| METADATA_RESOURCE)?;
        self.sample_sources.push(self.declared_sample_offset.min(self.source.len()));
        self.declared_sample_offset = self.declared_sample_offset.checked_add(declared_length).ok_or(Error::TooLarge("MOD sample data"))?;
        self.item += 1;
        Ok(())
    }

    fn finish_plan(&mut self) -> Result<(), ImageDecodeError> {
        let module_layout = self.format_layout()?;
        let fixed = self.source.get(..HEADER_BYTES).ok_or(Error::Truncated { offset: 0, needed: HEADER_BYTES })?;
        let song_length = fixed.get(SONG_LENGTH_OFFSET).copied().unwrap_or(0).min(128) as usize;
        let order_bytes = fixed.get(ORDER_OFFSET..ORDER_OFFSET + 128).ok_or(Error::OutOfRange)?;
        let mut played_orders = Vec::new();
        played_orders.try_reserve_exact(song_length).map_err(|_| METADATA_RESOURCE)?;
        for order in order_bytes.iter().take(song_length) { played_orders.push(logical_order(*order, module_layout) as u16); }
        for raw in order_bytes.iter().take(song_length) {
            self.plan.try_push_order(if *raw == 255 { ORDER_END } else { logical_order(*raw, module_layout) as u16 })?;
        }
        if order_bytes.get(song_length.saturating_sub(1)).copied() != Some(255) { self.plan.try_push_order(ORDER_END)?; }
        let evidence = ModTimingEvidence {
            timing_detection: matches!(module_layout.dialect, FormatDialect::ProTracker) && !self.oversized_sample,
            vblank_only_tag: matches!(module_layout.dialect, FormatDialect::Noisetracker),
            has_high_fxx: self.last_high.iter().any(|value| *value != 0), mixed_row: self.mixed_row,
            high_fxx_only_at_end: high_fxx_only_at_end(&played_orders, &self.last_high),
        };
        self.plan.set_header(ModuleHeader {
            title: starplayer_model::try_decode_cp437(fixed.get(..20).unwrap_or_default())?.into_boxed_str(),
            format: ModuleFormat::Mod, channel_count: module_layout.channels, initial_speed: 6,
            initial_tempo: 125, global_volume: U0F16::MAX, master_volume: U0F16::MAX,
            default_pan: default_pan(module_layout.channels, self.options.stereo_separation),
            flags: ModuleFlags { amiga_limits: self.amiga_limits, linear_slides: false, fast_volume_slides: false, stereo: self.options.stereo_separation.get() != 0 },
            dialect: module_layout.dialect, format_extra: encode_evidence(evidence),
            default_channel_volume: Vec::new().into_boxed_slice(), format_data: Vec::new().into_boxed_slice(),
        });
        self.phase = Phase::Layout;
        Ok(())
    }

    fn prepare_layout(&mut self) -> Result<(), ImageDecodeError> {
        self.metadata_bytes = self.plan.metadata_bytes()?;
        self.blob_start = self.metadata_bytes;
        let after_blob = self.blob_start.checked_add(self.plan.blob_bytes()).ok_or(Error::TooLarge("module image"))?;
        let pcm_length_offset = after_blob.checked_add((4 - after_blob % 4) % 4).ok_or(Error::TooLarge("module image"))?;
        self.pcm_start = pcm_length_offset.checked_add(4).ok_or(Error::TooLarge("module image"))?;
        let image_length = self.pcm_start.checked_add(self.plan.pcm_frames().checked_mul(2).ok_or(Error::TooLarge("module image PCM"))?).ok_or(Error::TooLarge("module image"))?;
        if image_length > self.destination.len() { return Err(ImageDecodeError::DestinationTooSmall { required: image_length, available: self.destination.len() }); }
        self.destination.get_mut(after_blob..pcm_length_offset).ok_or(Error::OutOfRange)?.fill(0);
        self.destination.get_mut(pcm_length_offset..self.pcm_start).ok_or(Error::OutOfRange)?.copy_from_slice(&(self.plan.pcm_frames() as u32).to_le_bytes());
        self.image_length = Some(image_length);
        self.phase = Phase::Metadata;
        Ok(())
    }

    fn write_metadata_part(&mut self) -> Result<(), ImageDecodeError> {
        if self.metadata_part >= self.plan.metadata_part_count() {
            if self.metadata_position != self.metadata_bytes { return Err(Error::Invalid("module image metadata length changed").into()); }
            self.phase = Phase::Patterns; return Ok(());
        }
        let written = self.plan.write_metadata_part(self.metadata_part, self.destination.get_mut(self.metadata_position..self.metadata_bytes).ok_or(Error::OutOfRange)?)?;
        self.metadata_position += written; self.metadata_part += 1; Ok(())
    }

    fn write_patterns(&mut self, max_input_bytes: usize) -> Result<(), ImageDecodeError> {
        let module_layout = self.format_layout()?;
        let total = self.plan.blob_bytes();
        let amount = (total - self.pattern_position).min(max_input_bytes);
        for relative in 0..amount {
            let output_byte = self.pattern_position + relative;
            let output_cell = output_byte / CELL_BYTES;
            let byte_in_cell = output_byte % CELL_BYTES;
            let source_cell = if module_layout.paired_four_channel_patterns {
                let within_pattern = output_cell % (ROWS as usize * 8);
                let row = within_pattern / 8;
                let channel = within_pattern % 8;
                let stored = if channel < 4 { row * 4 + channel } else { ROWS as usize * 4 + row * 4 + channel - 4 };
                output_cell / (ROWS as usize * 8) * (ROWS as usize * 8) + stored
            } else { output_cell };
            let value = self.source.get(self.pattern_data_offset + source_cell * CELL_BYTES + byte_in_cell).copied().ok_or(Error::OutOfRange)?;
            if let Some(target) = self.destination.get_mut(self.blob_start + output_byte) { *target = value; }
        }
        self.pattern_position += amount;
        if self.pattern_position == total { self.phase = Phase::Pcm; }
        Ok(())
    }

    fn write_pcm(&mut self, budget: DecodeBudget) -> Result<(), ImageDecodeError> {
        if self.pcm_sample >= self.plan.samples().len() { self.phase = Phase::Complete; return Ok(()); }
        let sample = self.plan.samples().get(self.pcm_sample).ok_or(Error::OutOfRange)?;
        let source_start = self.sample_sources.get(self.pcm_sample).copied().ok_or(Error::OutOfRange)?;
        let total = sample.stored_frames();
        let mut emitted = 0usize;
        let mut scanned = 0usize;
        while self.pcm_position < total && emitted < budget.max_pcm_frames {
            let body_start = starplayer_core::PRE_ROLL_FRAMES;
            let body_end = body_start + sample.length_frames() as usize;
            let (frame, reads) = if self.pcm_position < body_start { (0, 0) }
                else if self.pcm_position < body_end {
                    let position = self.pcm_position - body_start;
                    ((self.source.get(source_start + position).copied().unwrap_or(0) as i8 as i16) * 256, 1)
                } else if sample.loop_mode() == LoopMode::Forward {
                    let guard = self.pcm_position - body_end;
                    let position = sample.loop_start() as usize + guard % (sample.loop_end() - sample.loop_start()) as usize;
                    ((self.source.get(source_start + position).copied().unwrap_or(0) as i8 as i16) * 256, 1)
                } else { (0, 0) };
            if scanned + reads > budget.max_input_bytes { break; }
            let offset = self.pcm_start + (sample.pcm_offset() as usize - starplayer_core::PRE_ROLL_FRAMES + self.pcm_position) * 2;
            self.destination.get_mut(offset..offset + 2).ok_or(Error::OutOfRange)?.copy_from_slice(&frame.to_le_bytes());
            self.pcm_position += 1; emitted += 1; scanned += reads;
        }
        if self.pcm_position == total { self.pcm_position = 0; self.pcm_sample += 1; }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use super::*;

    #[test]
    fn incremental_image_matches_owned_loader() {
        let mut source = vec![0u8; HEADER_BYTES + ROWS as usize * 4 * CELL_BYTES + 4];
        source[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(b"M.K.");
        source[SONG_LENGTH_OFFSET] = 1;
        source[20 + 22..20 + 24].copy_from_slice(&2u16.to_be_bytes());
        let sample_offset = HEADER_BYTES + ROWS as usize * 4 * CELL_BYTES;
        source[sample_offset..sample_offset + 4].copy_from_slice(&[0x80, 0, 0x7f, 0xff]);
        let expected = crate::load(&source).expect("MOD loads").to_image();
        let mut destination = vec![0u8; expected.len()];
        let mut workspace = vec![0u8; 4096];
        let length = {
            let mut decoder = ImageDecoder::new(&source, &mut destination, &mut workspace);
            loop {
                match decoder.step(DecodeBudget { max_input_bytes: 4096, max_pcm_frames: 3 }).expect("decode") {
                    ImageDecodeStatus::Pending => {}
                    ImageDecodeStatus::Complete { image_length } => break image_length,
                }
            }
        };
        assert_eq!(length, expected.len());
        assert_eq!(destination, expected);
    }
}
