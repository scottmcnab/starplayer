use alloc::vec::Vec;
use starplayer_core::fixed::unit_from_ratio;
use starplayer_core::{Error, U0F16};
use starplayer_model::{
    DecodeBudget, ImageDecodeError, ImageDecodeStatus, InstrumentDef, LoopMode, ModuleFlags,
    ModuleFormat, ModuleHeader, ModuleImagePlan, ORDER_END, ORDER_MARKER, SampleSpec,
    MINIMUM_DECODE_INPUT_BUDGET,
};
use starplayer_mod::FINETUNE_REFERENCE_RATES;

use crate::loader::{
    FORMAT_NATIVE_TEMPO_RESETS, FORMAT_TRACK_MASK, HEADER_BYTES, ORDER_BYTES,
    PATTERN_TABLE_BYTES, SAMPLE_HEADER_BYTES, TRACK_BYTES, TempoMode,
    le_u16, le_u32, mtm_pan,
};
use crate::pattern::{CELL_BYTES, ROWS};

const METADATA_RESOURCE: Error = Error::Resource("not enough memory for module image metadata");

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase { Header, PatternPlans, TempoTracks, TempoRows, Samples, Orders, FinishPlan, Layout, Metadata, Patterns, Pcm, Complete }

pub struct ImageDecoder<'buffers> {
    source: &'buffers [u8], destination: &'buffers mut [u8], workspace: &'buffers mut [u8],
    phase: Phase, plan: ModuleImagePlan, track_count: u16, pattern_count: usize,
    last_order: usize, comment_length: usize, sample_count: usize, channel_count: u8,
    order_offset: usize, track_offset: usize, pattern_table_offset: usize,
    sample_data_offset: usize, item: usize, position: usize, tempo_low: bool, tempo_high: bool,
    tempo_mode: TempoMode, sample_sources: Vec<usize>, metadata_bytes: usize, blob_start: usize,
    pcm_start: usize, pcm_sample: usize, pcm_position: usize, image_length: Option<usize>,
    metadata_part: usize, metadata_position: usize,
}

impl<'buffers> ImageDecoder<'buffers> {
    pub fn new(source: &'buffers [u8], destination: &'buffers mut [u8], workspace: &'buffers mut [u8]) -> ImageDecoder<'buffers> {
        ImageDecoder {
            source, destination, workspace, phase: Phase::Header, plan: ModuleImagePlan::new(),
            track_count: 0, pattern_count: 0, last_order: 0, comment_length: 0,
            sample_count: 0, channel_count: 0, order_offset: 0, track_offset: 0,
            pattern_table_offset: 0, sample_data_offset: 0, item: 0, position: 0,
            tempo_low: false, tempo_high: false, tempo_mode: TempoMode::MultiTracker,
            sample_sources: Vec::new(), metadata_bytes: 0, blob_start: 0, pcm_start: 0,
            pcm_sample: 0, pcm_position: 0, image_length: None,
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
            Phase::Header => self.prepare_header()?, Phase::PatternPlans => self.prepare_pattern_plan()?,
            Phase::TempoTracks => self.scan_tempo_tracks(budget.max_input_bytes)?,
            Phase::TempoRows => self.scan_tempo_row()?, Phase::Samples => self.prepare_sample()?,
            Phase::Orders => self.prepare_order()?, Phase::FinishPlan => self.finish_plan()?,
            Phase::Layout => self.prepare_layout()?, Phase::Patterns => self.write_patterns(budget.max_input_bytes)?,
            Phase::Metadata => self.write_metadata_part()?,
            Phase::Pcm => self.write_pcm(budget)?, Phase::Complete => {}
        }
        Ok(match self.image_length {
            Some(image_length) if self.phase == Phase::Complete => ImageDecodeStatus::Complete { image_length },
            _ => ImageDecodeStatus::Pending,
        })
    }

    fn prepare_header(&mut self) -> Result<(), ImageDecodeError> {
        let header = self.source.get(..HEADER_BYTES).ok_or(Error::Truncated { offset: 0, needed: HEADER_BYTES })?;
        if header.get(..4) != Some(b"MTM\x10") { return Err(Error::BadMagic.into()); }
        self.track_count = le_u16(header, 24)?;
        self.pattern_count = header.get(26).copied().unwrap_or(0) as usize + 1;
        self.last_order = (header.get(27).copied().unwrap_or(0) as usize).min(ORDER_BYTES - 1);
        self.comment_length = le_u16(header, 28)? as usize;
        self.sample_count = header.get(30).copied().unwrap_or(0) as usize;
        if self.sample_count > 63 { return Err(Error::Invalid("MTM sample count must be 0..=63").into()); }
        if header.get(32).copied().unwrap_or(0) != ROWS as u8 { return Err(Error::Unsupported("MTM tracks not containing 64 rows").into()); }
        self.channel_count = header.get(33).copied().unwrap_or(0);
        if !(1..=32).contains(&self.channel_count) { return Err(Error::Invalid("MTM channel count must be 1..=32").into()); }
        self.order_offset = HEADER_BYTES.checked_add(self.sample_count * SAMPLE_HEADER_BYTES).ok_or(Error::TooLarge("MTM order table"))?;
        self.track_offset = self.order_offset.checked_add(ORDER_BYTES).ok_or(Error::TooLarge("MTM track data"))?;
        self.pattern_table_offset = self.track_offset.checked_add(self.track_count as usize * TRACK_BYTES).ok_or(Error::TooLarge("MTM pattern table"))?;
        let comment_offset = self.pattern_table_offset.checked_add(self.pattern_count * PATTERN_TABLE_BYTES).ok_or(Error::TooLarge("MTM comment"))?;
        self.sample_data_offset = comment_offset.checked_add(self.comment_length).ok_or(Error::TooLarge("MTM sample data"))?;
        if self.sample_data_offset > self.source.len() {
            return Err(Error::Truncated { offset: comment_offset.min(self.source.len()), needed: self.sample_data_offset.saturating_sub(comment_offset.min(self.source.len())) }.into());
        }
        self.phase = Phase::PatternPlans;
        Ok(())
    }

    fn prepare_pattern_plan(&mut self) -> Result<(), ImageDecodeError> {
        if self.item >= self.pattern_count { self.item = 0; self.phase = Phase::TempoTracks; return Ok(()); }
        self.plan.try_add_pattern(ROWS as usize * self.channel_count as usize * CELL_BYTES, ROWS, self.channel_count)?;
        self.item += 1;
        Ok(())
    }

    fn scan_tempo_tracks(&mut self, max_input_bytes: usize) -> Result<(), ImageDecodeError> {
        let total = self.track_count as usize * TRACK_BYTES;
        let amount = (total - self.position).min(max_input_bytes / CELL_BYTES * CELL_BYTES);
        let bytes = self.source.get(self.track_offset + self.position..self.track_offset + self.position + amount)
            .ok_or(Error::Truncated { offset: self.track_offset + self.position, needed: amount })?;
        for cell in bytes.chunks_exact(CELL_BYTES) {
            if cell[1] & 15 == 0xF { if cell[2] < 0x20 { self.tempo_low = true; } else { self.tempo_high = true; } }
        }
        self.position += amount;
        if self.position == total {
            self.position = 0;
            if self.tempo_low && self.tempo_high { self.phase = Phase::TempoRows; }
            else { self.phase = Phase::Samples; }
        }
        Ok(())
    }

    fn scan_tempo_row(&mut self) -> Result<(), ImageDecodeError> {
        if self.item >= self.pattern_count { self.item = 0; self.position = 0; self.phase = Phase::Samples; return Ok(()); }
        let row = self.position;
        let mut low = false;
        let mut high = false;
        for channel in 0..self.channel_count as usize {
            let reference_offset = self.pattern_table_offset + self.item * PATTERN_TABLE_BYTES + channel * 2;
            let pair = self.source.get(reference_offset..reference_offset + 2).ok_or(Error::Truncated { offset: reference_offset, needed: 2 })?;
            let track = u16::from_le_bytes([pair[0], pair[1]]);
            if track == 0 || track > self.track_count { continue; }
            let cell_offset = self.track_offset + (track as usize - 1) * TRACK_BYTES + row * CELL_BYTES;
            let cell = self.source.get(cell_offset..cell_offset + CELL_BYTES).ok_or(Error::OutOfRange)?;
            if cell[1] & 15 == 0xF { if cell[2] < 0x20 { low = true; } else { high = true; } }
        }
        if low && high { self.tempo_mode = TempoMode::DualModulePlayer; self.item = 0; self.position = 0; self.phase = Phase::Samples; return Ok(()); }
        self.position += 1;
        if self.position == ROWS as usize { self.position = 0; self.item += 1; }
        Ok(())
    }

    fn prepare_sample(&mut self) -> Result<(), ImageDecodeError> {
        if self.item >= self.sample_count { self.item = 0; self.phase = Phase::Orders; return Ok(()); }
        let offset = HEADER_BYTES + self.item * SAMPLE_HEADER_BYTES;
        let header = self.source.get(offset..offset + SAMPLE_HEADER_BYTES).ok_or(Error::Truncated { offset, needed: SAMPLE_HEADER_BYTES })?;
        if header.get(36).copied().unwrap_or(0) & 1 != 0 { return Err(Error::Unsupported("16-bit MTM samples").into()); }
        let declared_length = le_u32(header, 22)? as usize;
        let source_offset = self.sample_sources.last().copied().map(|previous| {
            let previous_header = self.source.get(HEADER_BYTES + (self.item - 1) * SAMPLE_HEADER_BYTES..HEADER_BYTES + self.item * SAMPLE_HEADER_BYTES).unwrap_or_default();
            previous.saturating_add(le_u32(previous_header, 22).unwrap_or(0) as usize)
        }).unwrap_or(self.sample_data_offset);
        if declared_length > self.source.len().saturating_sub(source_offset) {
            return Err(Error::Truncated { offset: source_offset, needed: declared_length }.into());
        }
        let name = starplayer_model::try_decode_cp437(header.get(..22).unwrap_or_default())?;
        let loop_start = (le_u32(header, 26)? as usize).min(declared_length);
        let declared_loop_start = le_u32(header, 26)? as usize;
        let declared_loop_end = le_u32(header, 30)? as usize;
        let loop_end = declared_loop_end.min(declared_length);
        let loops = declared_loop_end.saturating_sub(declared_loop_start) > 4 && loop_start < loop_end;
        let specification = SampleSpec {
            name: starplayer_model::try_clone_text(&name)?, loop_mode: if loops { LoopMode::Forward } else { LoopMode::None },
            loop_start: if loops { loop_start as u32 } else { 0 }, loop_end: if loops { loop_end as u32 } else { 0 },
            default_volume: unit_from_ratio(header.get(35).copied().unwrap_or(0).min(64) as u32, 64),
            reference_rate_hz: FINETUNE_REFERENCE_RATES[(header.get(34).copied().unwrap_or(0) & 15) as usize],
            ..SampleSpec::default()
        };
        let sample = self.plan.try_add_sample(declared_length, specification)?;
        self.plan.try_add_instrument(InstrumentDef::try_from_sample(&name, sample, U0F16::MAX)?)?;
        self.sample_sources.try_reserve(1).map_err(|_| METADATA_RESOURCE)?;
        self.sample_sources.push(source_offset);
        self.item += 1;
        Ok(())
    }

    fn prepare_order(&mut self) -> Result<(), ImageDecodeError> {
        if self.item > self.last_order { self.plan.try_push_order(ORDER_END)?; self.item = 0; self.phase = Phase::FinishPlan; return Ok(()); }
        let raw = self.source.get(self.order_offset + self.item).copied().ok_or(Error::OutOfRange)?;
        self.plan.try_push_order(if raw as usize >= self.pattern_count { ORDER_MARKER } else { raw as u16 })?;
        self.item += 1;
        Ok(())
    }

    fn finish_plan(&mut self) -> Result<(), ImageDecodeError> {
        let header = self.source.get(..HEADER_BYTES).ok_or(Error::OutOfRange)?;
        let mut format_extra = self.track_count as u32 & FORMAT_TRACK_MASK;
        if self.tempo_mode == TempoMode::MultiTracker { format_extra |= FORMAT_NATIVE_TEMPO_RESETS; }
        let mut pans = Vec::new();
        pans.try_reserve_exact(self.channel_count as usize).map_err(|_| METADATA_RESOURCE)?;
        for value in header.get(34..66).unwrap_or_default().iter().take(self.channel_count as usize) { pans.push(mtm_pan(*value)); }
        self.plan.set_header(ModuleHeader {
            title: starplayer_model::try_decode_cp437(header.get(4..24).unwrap_or_default())?.into_boxed_str(),
            format: ModuleFormat::Mtm, channel_count: self.channel_count, initial_speed: 6,
            initial_tempo: 125, global_volume: U0F16::MAX, master_volume: U0F16::MAX,
            default_pan: pans.into_boxed_slice(), flags: ModuleFlags { amiga_limits: false, linear_slides: false, fast_volume_slides: false, stereo: true },
            dialect: starplayer_core::quirks::FormatDialect::MultiTracker, format_extra,
            default_channel_volume: Vec::new().into_boxed_slice(), format_data: Vec::new().into_boxed_slice(),
        });
        self.phase = Phase::Layout;
        Ok(())
    }

    fn prepare_layout(&mut self) -> Result<(), ImageDecodeError> {
        self.metadata_bytes = self.plan.metadata_bytes()?; self.blob_start = self.metadata_bytes;
        let after_blob = self.blob_start.checked_add(self.plan.blob_bytes()).ok_or(Error::TooLarge("module image"))?;
        let pcm_length_offset = after_blob + (4 - after_blob % 4) % 4; self.pcm_start = pcm_length_offset + 4;
        let image_length = self.pcm_start.checked_add(self.plan.pcm_frames().checked_mul(2).ok_or(Error::TooLarge("module image PCM"))?).ok_or(Error::TooLarge("module image"))?;
        if image_length > self.destination.len() { return Err(ImageDecodeError::DestinationTooSmall { required: image_length, available: self.destination.len() }); }
        self.destination.get_mut(after_blob..pcm_length_offset).ok_or(Error::OutOfRange)?.fill(0);
        self.destination.get_mut(pcm_length_offset..self.pcm_start).ok_or(Error::OutOfRange)?.copy_from_slice(&(self.plan.pcm_frames() as u32).to_le_bytes());
        self.image_length = Some(image_length); self.phase = Phase::Metadata; self.item = 0; self.position = 0;
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
        if self.item >= self.pattern_count { self.item = 0; self.position = 0; self.phase = Phase::Pcm; return Ok(()); }
        let cells = ROWS as usize * self.channel_count as usize;
        let amount = (cells - self.position).min(max_input_bytes / (CELL_BYTES + 2));
        for relative in 0..amount {
            let cell = self.position + relative; let row = cell / self.channel_count as usize; let channel = cell % self.channel_count as usize;
            let reference_offset = self.pattern_table_offset + self.item * PATTERN_TABLE_BYTES + channel * 2;
            let pair = self.source.get(reference_offset..reference_offset + 2).ok_or(Error::OutOfRange)?;
            let track = u16::from_le_bytes([pair[0], pair[1]]);
            let target = self.blob_start + (self.item * cells + cell) * CELL_BYTES;
            if track == 0 || track > self.track_count { self.destination.get_mut(target..target + CELL_BYTES).ok_or(Error::OutOfRange)?.fill(0); }
            else {
                let source = self.track_offset + (track as usize - 1) * TRACK_BYTES + row * CELL_BYTES;
                let bytes = self.source.get(source..source + CELL_BYTES).ok_or(Error::OutOfRange)?;
                self.destination.get_mut(target..target + CELL_BYTES).ok_or(Error::OutOfRange)?.copy_from_slice(bytes);
            }
        }
        self.position += amount;
        if self.position == cells { self.position = 0; self.item += 1; }
        Ok(())
    }

    fn write_pcm(&mut self, budget: DecodeBudget) -> Result<(), ImageDecodeError> {
        if self.pcm_sample >= self.plan.samples().len() { self.phase = Phase::Complete; return Ok(()); }
        let sample = self.plan.samples().get(self.pcm_sample).ok_or(Error::OutOfRange)?;
        let source_start = self.sample_sources.get(self.pcm_sample).copied().ok_or(Error::OutOfRange)?;
        let total = sample.stored_frames(); let mut emitted = 0; let mut scanned = 0;
        while self.pcm_position < total && emitted < budget.max_pcm_frames {
            let body_start = starplayer_core::PRE_ROLL_FRAMES; let body_end = body_start + sample.length_frames() as usize;
            let (frame, reads) = if self.pcm_position < body_start { (0, 0) }
                else if self.pcm_position < body_end { (((self.source[source_start + self.pcm_position - body_start] as i16 - 128) * 256), 1) }
                else if sample.loop_mode() == LoopMode::Forward {
                    let source_frame = sample.loop_start() as usize + (self.pcm_position - body_end) % (sample.loop_end() - sample.loop_start()) as usize;
                    (((self.source[source_start + source_frame] as i16 - 128) * 256), 1)
                } else { (0, 0) };
            if scanned + reads > budget.max_input_bytes { break; }
            let target = self.pcm_start + (sample.pcm_offset() as usize - starplayer_core::PRE_ROLL_FRAMES + self.pcm_position) * 2;
            self.destination.get_mut(target..target + 2).ok_or(Error::OutOfRange)?.copy_from_slice(&frame.to_le_bytes());
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
        let order_offset = HEADER_BYTES;
        let track_offset = order_offset + ORDER_BYTES;
        let pattern_offset = track_offset + TRACK_BYTES;
        let mut source = vec![0u8; pattern_offset + PATTERN_TABLE_BYTES];
        source[..4].copy_from_slice(b"MTM\x10");
        source[24..26].copy_from_slice(&1u16.to_le_bytes());
        source[32] = ROWS as u8; source[33] = 2; source[34] = 3; source[35] = 12;
        source[track_offset..track_offset + 3].copy_from_slice(&[0x30, 0x1f, 0x7d]);
        source[pattern_offset..pattern_offset + 2].copy_from_slice(&1u16.to_le_bytes());
        let expected = crate::load(&source).expect("MTM loads").to_image();
        let mut destination = vec![0u8; expected.len()]; let mut workspace = vec![0u8; 4096];
        let length = {
            let mut decoder = ImageDecoder::new(&source, &mut destination, &mut workspace);
            loop { match decoder.step(DecodeBudget { max_input_bytes: 4096, max_pcm_frames: 7 }).expect("decode") {
                ImageDecodeStatus::Pending => {}, ImageDecodeStatus::Complete { image_length } => break image_length,
            }}
        };
        assert_eq!(length, expected.len()); assert_eq!(destination, expected);
    }
}
