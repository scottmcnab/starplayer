use alloc::vec::Vec;
use starplayer_core::fixed::unit_from_ratio;
use starplayer_core::{Error, U0F16};
use starplayer_model::{
    DecodeBudget, ImageDecodeError, ImageDecodeStatus, InstrumentDef, LoopMode, ModuleFlags,
    ModuleFormat, ModuleHeader, ModuleImagePlan, ORDER_END, ORDER_MARKER, SampleSpec,
    MINIMUM_DECODE_INPUT_BUDGET,
};

use crate::header::{self, MAX_CHANNELS, S3mFormatExtra, S3mHeader};
use crate::pattern::{self, S3mCell};
use crate::sample::{self, S3mSampleHeader};

const MINIMUM_TEMPO: u16 = 32;
const FALLBACK_SPEED: u8 = 6;
const METADATA_RESOURCE: Error = Error::Resource("not enough memory for module image metadata");

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    Header,
    Instruments,
    Patterns,
    Orders,
    Layout,
    Metadata,
    PatternFill,
    PatternUnpack,
    Samples,
    Complete,
}

pub struct ImageDecoder<'buffers> {
    source: &'buffers [u8],
    destination: &'buffers mut [u8],
    workspace: &'buffers mut [u8],
    phase: Phase,
    file_header: Option<S3mHeader>,
    plan: ModuleImagePlan,
    item: usize,
    metadata_bytes: usize,
    blob_start: usize,
    pcm_start: usize,
    image_length: Option<usize>,
    pattern_fill: usize,
    pattern_position: usize,
    pattern_row: u16,
    pcm_sample: usize,
    pcm_position: usize,
    parsed_sample_headers: Vec<S3mSampleHeader>,
    pattern_bodies: Vec<(usize, usize)>,
    metadata_part: usize,
    metadata_position: usize,
}

impl<'buffers> ImageDecoder<'buffers> {
    pub fn new(source: &'buffers [u8], destination: &'buffers mut [u8], workspace: &'buffers mut [u8]) -> ImageDecoder<'buffers> {
        ImageDecoder {
            source,
            destination,
            workspace,
            phase: Phase::Header,
            file_header: None,
            plan: ModuleImagePlan::new(),
            item: 0,
            metadata_bytes: 0,
            blob_start: 0,
            pcm_start: 0,
            image_length: None,
            pattern_fill: 0,
            pattern_position: 0,
            pattern_row: 0,
            pcm_sample: 0,
            pcm_position: 0,
            parsed_sample_headers: Vec::new(),
            pattern_bodies: Vec::new(),
            metadata_part: 0,
            metadata_position: 0,
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
            Phase::Instruments => self.prepare_instrument()?,
            Phase::Patterns => self.prepare_pattern()?,
            Phase::Orders => self.prepare_order()?,
            Phase::Layout => self.prepare_layout()?,
            Phase::Metadata => self.write_metadata_part()?,
            Phase::PatternFill => self.fill_pattern()?,
            Phase::PatternUnpack => self.unpack_pattern(budget.max_input_bytes)?,
            Phase::Samples => self.write_sample_pcm(budget)?,
            Phase::Complete => {}
        }
        Ok(match self.image_length {
            Some(image_length) if self.phase == Phase::Complete => ImageDecodeStatus::Complete { image_length },
            _ => ImageDecodeStatus::Pending,
        })
    }

    fn header(&self) -> Result<S3mHeader, Error> {
        self.file_header.ok_or(Error::Invalid("S3M decoder header is unavailable"))
    }

    fn prepare_header(&mut self) -> Result<(), ImageDecodeError> {
        let fixed = self.source.get(..header::HEADER_LENGTH).ok_or(Error::Truncated { offset: 0, needed: header::HEADER_LENGTH })?;
        let file_header = S3mHeader::parse(fixed)?;
        let channel_count = file_header.channel_count();
        if channel_count == 0 { return Err(Error::Invalid("no enabled channels in the channel-settings array").into()); }
        let tables_end = file_header.tables_end() + if file_header.has_default_pan_block() { MAX_CHANNELS } else { 0 };
        if tables_end > self.source.len() {
            return Err(Error::Truncated { offset: header::HEADER_LENGTH, needed: tables_end - header::HEADER_LENGTH }.into());
        }
        let decoded_pattern_bytes = (file_header.pattern_count as usize)
            .saturating_mul(pattern::ROWS as usize)
            .saturating_mul(file_header.addressed_channels() as usize)
            .saturating_mul(pattern::CELL_BYTES);
        if decoded_pattern_bytes > self.source.len().saturating_mul(64).max(4 * 1024 * 1024) {
            return Err(Error::TooLarge("S3M pattern data").into());
        }
        let pan_block = if file_header.has_default_pan_block() {
            let bytes = self.source.get(file_header.tables_end()..file_header.tables_end() + MAX_CHANNELS)
                .ok_or(Error::Truncated { offset: file_header.tables_end(), needed: MAX_CHANNELS })?;
            let mut block = [0u8; MAX_CHANNELS];
            block.copy_from_slice(bytes);
            Some(block)
        } else {
            None
        };
        let default_pan = if !file_header.is_stereo() && pan_block.is_none() {
            Vec::new()
        } else {
            let nibbles = header::default_pan_nibbles(&file_header.channel_settings, file_header.is_stereo(), pan_block.as_ref());
            let mut pans = Vec::new();
            pans.try_reserve_exact(channel_count as usize).map_err(|_| METADATA_RESOURCE)?;
            for nibble in nibbles.iter().take(channel_count as usize) { pans.push(header::pan_nibble_to_bipolar(*nibble)); }
            pans
        };
        let extra = S3mFormatExtra {
            tracker_version: file_header.tracker_version,
            master_volume: file_header.master_volume,
            general_flags: file_header.general_flags as u8,
        };
        self.plan.set_header(ModuleHeader {
            title: starplayer_model::try_decode_cp437(self.source.get(..28).unwrap_or_default())?.into_boxed_str(),
            format: ModuleFormat::S3m,
            channel_count,
            initial_speed: if file_header.initial_speed == 0 { FALLBACK_SPEED } else { file_header.initial_speed },
            initial_tempo: (file_header.initial_tempo as u16).max(MINIMUM_TEMPO),
            global_volume: unit_from_ratio(file_header.global_volume as u32, 64),
            master_volume: unit_from_ratio(file_header.master_volume_level() as u32, 127),
            default_pan: default_pan.into_boxed_slice(),
            flags: ModuleFlags {
                amiga_limits: file_header.amiga_limits(),
                linear_slides: false,
                fast_volume_slides: file_header.fast_volume_slides(),
                stereo: file_header.is_stereo(),
            },
            dialect: file_header.dialect(),
            format_extra: extra.encode(),
            default_channel_volume: Vec::new().into_boxed_slice(),
            format_data: Vec::new().into_boxed_slice(),
        });
        self.file_header = Some(file_header);
        self.phase = Phase::Instruments;
        Ok(())
    }

    fn instrument_pointer(&self, index: usize) -> Result<usize, Error> {
        let header = self.header()?;
        let offset = header::HEADER_LENGTH + header.order_count as usize + index * 2;
        let Some([low, high]) = self.source.get(offset..offset + 2) else { return Err(Error::Truncated { offset, needed: 2 }) };
        Ok(u16::from_le_bytes([*low, *high]) as usize * 16)
    }

    fn pattern_pointer(&self, index: usize) -> Result<usize, Error> {
        let header = self.header()?;
        let offset = header::HEADER_LENGTH + header.order_count as usize + header.instrument_count as usize * 2 + index * 2;
        let Some([low, high]) = self.source.get(offset..offset + 2) else { return Err(Error::Truncated { offset, needed: 2 }) };
        Ok(u16::from_le_bytes([*low, *high]) as usize * 16)
    }

    fn prepare_instrument(&mut self) -> Result<(), ImageDecodeError> {
        let header = self.header()?;
        if self.item >= header.instrument_count as usize {
            self.item = 0;
            self.phase = Phase::Patterns;
            return Ok(());
        }
        let offset = self.instrument_pointer(self.item)?;
        self.item += 1;
        if offset == 0 || offset + sample::HEADER_LENGTH > self.source.len() {
            self.plan.try_add_instrument(InstrumentDef::default())?;
            return Ok(());
        }
        let bytes = self.source.get(offset..offset + sample::HEADER_LENGTH).ok_or(Error::Truncated { offset, needed: sample::HEADER_LENGTH })?;
        let sample_header = S3mSampleHeader::parse(bytes)?;
        let name = starplayer_model::try_decode_cp437(bytes.get(0x30..0x4C).unwrap_or_default())?;
        if !sample_header.is_pcm() {
            self.plan.try_add_instrument(InstrumentDef { name: name.into_boxed_str(), ..InstrumentDef::default() })?;
            return Ok(());
        }
        if sample_header.packing != 0 { return Err(Error::Unsupported("packed S3M sample data").into()); }
        let available_frames = if sample_header.data_offset == 0 || sample_header.data_offset >= self.source.len() {
            0
        } else {
            (self.source.len() - sample_header.data_offset) / sample_header.bytes_per_frame()
        };
        let frames = (sample_header.length as usize).min(available_frames);
        let loop_end = (sample_header.loop_end as usize).min(frames);
        let loops = sample_header.loops() && (sample_header.loop_start as usize) < loop_end;
        let specification = SampleSpec {
            name: starplayer_model::try_clone_text(&name)?,
            loop_mode: if loops { LoopMode::Forward } else { LoopMode::None },
            loop_start: if loops { sample_header.loop_start } else { 0 },
            loop_end: if loops { loop_end as u32 } else { 0 },
            default_volume: unit_from_ratio(if sample_header.volume > 64 { 0 } else { sample_header.volume as u32 }, 64),
            reference_rate_hz: if sample_header.c2spd == 0 { starplayer_model::DEFAULT_REFERENCE_RATE_HZ } else { sample_header.c2spd },
            ..SampleSpec::default()
        };
        let sample_id = self.plan.try_add_sample(frames, specification)?;
        self.parsed_sample_headers.try_reserve(1).map_err(|_| METADATA_RESOURCE)?;
        self.parsed_sample_headers.push(sample_header);
        self.plan.try_add_instrument(InstrumentDef::try_from_sample(&name, sample_id, U0F16::MAX)?)?;
        Ok(())
    }

    fn prepare_pattern(&mut self) -> Result<(), ImageDecodeError> {
        let header = self.header()?;
        if self.item >= header.pattern_count as usize {
            self.item = 0;
            self.phase = Phase::Orders;
            return Ok(());
        }
        let length = pattern::ROWS as usize * header.addressed_channels() as usize * pattern::CELL_BYTES;
        self.plan.try_add_pattern(length, pattern::ROWS, header.addressed_channels())?;
        let pointer = self.pattern_pointer(self.item)?;
        let body = if pointer == 0 {
            (0, 0)
        } else {
            let Some([low, high]) = self.source.get(pointer..pointer + 2) else { return Err(Error::Truncated { offset: pointer, needed: 2 }.into()) };
            let packed_length = u16::from_le_bytes([*low, *high]) as usize;
            let body_length = packed_length.saturating_sub(2);
            self.source.get(pointer + 2..pointer + 2 + body_length).ok_or(Error::Truncated { offset: pointer, needed: packed_length })?;
            (pointer + 2, body_length)
        };
        self.pattern_bodies.try_reserve(1).map_err(|_| METADATA_RESOURCE)?;
        self.pattern_bodies.push(body);
        self.item += 1;
        Ok(())
    }

    fn prepare_order(&mut self) -> Result<(), ImageDecodeError> {
        let header = self.header()?;
        if self.item >= header.order_count as usize {
            self.item = 0;
            self.phase = Phase::Layout;
            return Ok(());
        }
        let raw = self.source.get(header::HEADER_LENGTH + self.item).copied()
            .ok_or(Error::Truncated { offset: header::HEADER_LENGTH + self.item, needed: 1 })?;
        let order = match raw {
            255 => ORDER_END,
            254 => ORDER_MARKER,
            value if (value as usize) < header.pattern_count as usize => value as u16,
            _ => ORDER_MARKER,
        };
        self.plan.try_push_order(order)?;
        self.item += 1;
        Ok(())
    }

    fn prepare_layout(&mut self) -> Result<(), ImageDecodeError> {
        self.metadata_bytes = self.plan.metadata_bytes()?;
        self.blob_start = self.metadata_bytes;
        let after_blob = self.blob_start.checked_add(self.plan.blob_bytes()).ok_or(Error::TooLarge("module image"))?;
        let padding = (4 - after_blob % 4) % 4;
        let pcm_length_offset = after_blob.checked_add(padding).ok_or(Error::TooLarge("module image"))?;
        self.pcm_start = pcm_length_offset.checked_add(4).ok_or(Error::TooLarge("module image"))?;
        let image_length = self.pcm_start.checked_add(self.plan.pcm_frames().checked_mul(2).ok_or(Error::TooLarge("module image PCM"))?)
            .ok_or(Error::TooLarge("module image"))?;
        if image_length > self.destination.len() {
            return Err(ImageDecodeError::DestinationTooSmall { required: image_length, available: self.destination.len() });
        }
        if let Some(bytes) = self.destination.get_mut(after_blob..pcm_length_offset) { bytes.fill(0); }
        let pcm_frames = u32::try_from(self.plan.pcm_frames()).map_err(|_| Error::TooLarge("module image PCM"))?;
        self.destination.get_mut(pcm_length_offset..self.pcm_start).ok_or(Error::OutOfRange)?.copy_from_slice(&pcm_frames.to_le_bytes());
        self.image_length = Some(image_length);
        self.phase = Phase::Metadata;
        Ok(())
    }

    fn write_metadata_part(&mut self) -> Result<(), ImageDecodeError> {
        if self.metadata_part >= self.plan.metadata_part_count() {
            if self.metadata_position != self.metadata_bytes { return Err(Error::Invalid("module image metadata length changed").into()); }
            self.phase = Phase::PatternFill;
            return Ok(());
        }
        let written = self.plan.write_metadata_part(self.metadata_part, self.destination.get_mut(self.metadata_position..self.metadata_bytes).ok_or(Error::OutOfRange)?)?;
        self.metadata_position += written;
        self.metadata_part += 1;
        Ok(())
    }

    fn fill_pattern(&mut self) -> Result<(), ImageDecodeError> {
        let header = self.header()?;
        if self.item >= header.pattern_count as usize {
            self.item = 0;
            self.phase = Phase::Samples;
            return Ok(());
        }
        let pattern_length = pattern::ROWS as usize * header.addressed_channels() as usize * pattern::CELL_BYTES;
        let amount = (pattern_length - self.pattern_fill).min(4096);
        let start = self.blob_start + self.item * pattern_length + self.pattern_fill;
        let bytes = self.destination.get_mut(start..start + amount).ok_or(Error::OutOfRange)?;
        let empty = S3mCell::EMPTY.to_bytes();
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = empty.get((self.pattern_fill + index) % pattern::CELL_BYTES).copied().unwrap_or(0);
        }
        self.pattern_fill += amount;
        if self.pattern_fill == pattern_length {
            self.pattern_fill = 0;
            self.pattern_position = 0;
            self.pattern_row = 0;
            self.phase = Phase::PatternUnpack;
        }
        Ok(())
    }

    fn unpack_pattern(&mut self, max_input_bytes: usize) -> Result<(), ImageDecodeError> {
        let header = self.header()?;
        let (body_start, body_length) = self.pattern_bodies.get(self.item).copied().ok_or(Error::OutOfRange)?;
        if body_length == 0 {
            self.item += 1;
            self.phase = Phase::PatternFill;
            return Ok(());
        }
        let body = self.source.get(body_start..body_start + body_length).ok_or(Error::OutOfRange)?;
        let mut consumed = 0usize;
        while self.pattern_row < pattern::ROWS {
            let Some(mask) = body.get(self.pattern_position).copied() else { break };
            let event_bytes = 1 + if mask & 0x20 != 0 { 2 } else { 0 } + if mask & 0x40 != 0 { 1 } else { 0 } + if mask & 0x80 != 0 { 2 } else { 0 };
            let available_event = event_bytes.min(body.len().saturating_sub(self.pattern_position));
            if consumed + available_event > max_input_bytes { break; }
            consumed += available_event;
            self.pattern_position = self.pattern_position.saturating_add(1);
            if mask == 0 {
                self.pattern_row += 1;
                continue;
            }
            let channel = mask & 0x1F;
            let mut cell = S3mCell::EMPTY;
            if mask & 0x20 != 0 {
                cell.note = body.get(self.pattern_position).copied().unwrap_or(pattern::NOTE_NONE);
                cell.instrument = body.get(self.pattern_position + 1).copied().unwrap_or(pattern::INSTRUMENT_NONE);
                self.pattern_position = self.pattern_position.saturating_add(2);
            }
            if mask & 0x40 != 0 {
                cell.volume = body.get(self.pattern_position).copied().unwrap_or(pattern::VOLUME_NONE);
                self.pattern_position = self.pattern_position.saturating_add(1);
            }
            if mask & 0x80 != 0 {
                cell.command = body.get(self.pattern_position).copied().unwrap_or(pattern::COMMAND_NONE);
                cell.info = body.get(self.pattern_position + 1).copied().unwrap_or(0);
                self.pattern_position = self.pattern_position.saturating_add(2);
            }
            if channel < header.addressed_channels() {
                let pattern_length = pattern::ROWS as usize * header.addressed_channels() as usize * pattern::CELL_BYTES;
                let offset = self.blob_start + self.item * pattern_length
                    + (self.pattern_row as usize * header.addressed_channels() as usize + channel as usize) * pattern::CELL_BYTES;
                self.destination.get_mut(offset..offset + pattern::CELL_BYTES).ok_or(Error::OutOfRange)?.copy_from_slice(&cell.to_bytes());
            }
        }
        if self.pattern_row >= pattern::ROWS || self.pattern_position >= body.len() {
            self.item += 1;
            self.phase = Phase::PatternFill;
        }
        Ok(())
    }

    fn write_sample_pcm(&mut self, budget: DecodeBudget) -> Result<(), ImageDecodeError> {
        if self.pcm_sample >= self.plan.samples().len() {
            self.phase = Phase::Complete;
            return Ok(());
        }
        let sample = self.plan.samples().get(self.pcm_sample).ok_or(Error::OutOfRange)?;
        let source_header = self.parsed_sample_headers.get(self.pcm_sample).copied().ok_or(Error::OutOfRange)?;
        let total = sample.stored_frames();
        let mut emitted = 0usize;
        let mut scanned = 0usize;
        while self.pcm_position < total && emitted < budget.max_pcm_frames {
            let body_start = starplayer_core::PRE_ROLL_FRAMES;
            let body_end = body_start + sample.length_frames() as usize;
            let (frame, input_bytes) = if self.pcm_position < body_start {
                (0, 0)
            } else if self.pcm_position < body_end {
                let source_frame = self.pcm_position - body_start;
                let reads = if source_header.flags & sample::FLAG_SIXTEEN_BIT != 0 { 2 } else { 1 };
                if scanned + reads > budget.max_input_bytes { break; }
                (decode_source_frame(self.source, &source_header, source_frame), reads)
            } else if sample.loop_mode() == LoopMode::Forward {
                let guard = self.pcm_position - body_end;
                let loop_length = (sample.loop_end() - sample.loop_start()) as usize;
                let source_frame = sample.loop_start() as usize + guard % loop_length;
                let reads = if source_header.flags & sample::FLAG_SIXTEEN_BIT != 0 { 2 } else { 1 };
                if scanned + reads > budget.max_input_bytes { break; }
                (decode_source_frame(self.source, &source_header, source_frame), reads)
            } else {
                (0, 0)
            };
            if scanned + input_bytes > budget.max_input_bytes { break; }
            let output_offset = self.pcm_start + (sample.pcm_offset() as usize - starplayer_core::PRE_ROLL_FRAMES + self.pcm_position) * 2;
            self.destination.get_mut(output_offset..output_offset + 2).ok_or(Error::OutOfRange)?.copy_from_slice(&frame.to_le_bytes());
            scanned += input_bytes;
            emitted += 1;
            self.pcm_position += 1;
        }
        if self.pcm_position == total {
            self.pcm_sample += 1;
            self.pcm_position = 0;
        }
        Ok(())
    }

}

fn decode_source_frame(source: &[u8], header: &S3mSampleHeader, frame: usize) -> i16 {
    if header.flags & sample::FLAG_SIXTEEN_BIT != 0 {
        let offset = header.data_offset.saturating_add(frame.saturating_mul(2));
        let low = source.get(offset).copied().unwrap_or(0);
        let high = source.get(offset + 1).copied().unwrap_or(0);
        i16::from_le_bytes([low, high])
    } else {
        sample::unsigned8_to_i16(source.get(header.data_offset.saturating_add(frame)).copied().unwrap_or(0x80))
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use alloc::vec;
    use super::*;

    #[test]
    fn incremental_image_matches_the_owned_loader() {
        let source = include_bytes!("../tests/fixtures/REFLEX.S3M");
        let expected = crate::load(source).expect("fixture loads").to_image();
        let mut destination = vec![0u8; expected.len()];
        let mut workspace = vec![0u8; 4096];
        let length = {
            let mut decoder = ImageDecoder::new(source, &mut destination, &mut workspace);
            loop {
                match decoder.step(DecodeBudget { max_input_bytes: 4096, max_pcm_frames: 19 }).expect("incremental decode") {
                    ImageDecodeStatus::Pending => {}
                    ImageDecodeStatus::Complete { image_length } => break image_length,
                }
            }
        };
        assert_eq!(length, expected.len());
        assert_eq!(destination, expected);
    }

    #[test]
    fn exact_capacity_is_required() {
        let source = include_bytes!("../tests/fixtures/REFLEX.S3M");
        let expected = crate::load(source).expect("fixture loads").to_image();
        let mut destination = vec![0u8; expected.len() - 1];
        let mut workspace = vec![0u8; 4096];
        let mut decoder = ImageDecoder::new(source, &mut destination, &mut workspace);
        loop {
            match decoder.step(DecodeBudget { max_input_bytes: 4096, max_pcm_frames: 1024 }) {
                Ok(ImageDecodeStatus::Pending) => {}
                Err(ImageDecodeError::DestinationTooSmall { required, available }) => {
                    assert_eq!(required, expected.len());
                    assert_eq!(available + 1, required);
                    break;
                }
                other => panic!("unexpected result: {other:?}"),
            }
        }
    }

}
