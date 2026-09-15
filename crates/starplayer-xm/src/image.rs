use alloc::string::String;
use alloc::vec::Vec;

use starplayer_core::fixed::{bipolar_from_ratio, unit_from_ratio};
use starplayer_core::{Error, U0F16};
use starplayer_model::{
    DecodeBudget, Envelope, EnvelopePoint, EnvelopeSpan, ImageDecodeError, ImageDecodeStatus,
    InstrumentDef, LoopMode, ModuleFlags, ModuleFormat, ModuleHeader, ModuleImagePlan,
    SampleSpec, MINIMUM_DECODE_INPUT_BUDGET,
};

use crate::header::{self, XmFormatExtra, XmHeader};
use crate::instrument::{self, XmEnvelope, XmInstrumentHeader};
use crate::pattern::{self, XmCell};
use crate::sample::{self, XmSampleHeader};

const FALLBACK_SPEED: u8 = 6;
const FALLBACK_TEMPO: u16 = 125;
const MINIMUM_TEMPO: u16 = 32;
const MAXIMUM_TEMPO: u16 = 255;
const PATTERN_HEADER_LENGTH: usize = 9;
const PATTERN_HEADER_LENGTH_1_02: usize = 8;
const MINIMUM_PATTERN_HEADER_LENGTH: usize = 8;
const MINIMUM_PATTERN_BUDGET_BYTES: usize = 8 * 1024 * 1024;
const PATTERN_BUDGET_PER_FILE_BYTE: usize = 64;
const MODPLUG_INSTRUMENT_HEADER_SIZE: usize = 0x107;
const MODPLUG_CHUNK_SCAN_LIMIT: usize = 256;
const MODPLUG_CHUNK_TAGS: [&[u8; 4]; 6] = [b"text", b"MIDI", b"PNAM", b"CNAM", b"CHFX", b"XTPM"];
const MODPLUG_PLUGIN_TAG_BITS: u32 = u32::from_be_bytes([b'F', b'X', 0, 0]);
const METADATA_RESOURCE: Error = Error::Resource("not enough memory for module image metadata");

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    Header, Patterns, Instruments, SampleHeaders, InlineOffsets, OldOffsets, PlanSamples,
    PlanInstruments, Orders, FinishPlan, Layout, Metadata, PatternFill, PatternUnpack, Pcm,
    Complete,
}

#[derive(Copy, Clone, Debug)]
struct PatternSource { rows: u16, packed_start: usize, packed_length: usize }

#[derive(Debug)]
struct PendingInstrument {
    name: String,
    header: XmInstrumentHeader,
    sample_start: usize,
    sample_count: usize,
}

#[derive(Debug)]
struct PendingSample {
    name: String,
    header: XmSampleHeader,
    data_offset: usize,
    decoded_frames: usize,
}

/// Incrementally convert an XM directly into caller-owned SPMI image storage.
pub struct ImageDecoder<'buffers> {
    source: &'buffers [u8],
    destination: &'buffers mut [u8],
    workspace: &'buffers mut [u8],
    phase: Phase,
    file_header: Option<XmHeader>,
    plan: ModuleImagePlan,
    cursor: usize,
    item: usize,
    pattern_readable: bool,
    decoded_pattern_bytes: usize,
    patterns: Vec<PatternSource>,
    instruments: Vec<PendingInstrument>,
    samples: Vec<PendingSample>,
    sample_headers_remaining: usize,
    offset_sample: usize,
    modplug_tell: bool,
    dialect: Option<starplayer_core::quirks::FormatDialect>,
    empty_pattern: Option<u16>,
    metadata_bytes: usize,
    metadata_part: usize,
    metadata_position: usize,
    blob_start: usize,
    pcm_start: usize,
    pattern_fill: usize,
    pattern_cell: usize,
    packed_position: usize,
    pending_cell: XmCell,
    pending_mask: u8,
    pending_field: u8,
    pcm_sample: usize,
    pcm_position: usize,
    left_accumulator: i16,
    right_accumulator: i16,
    adpcm_table: [i8; sample::ADPCM_TABLE_BYTES],
    adpcm_table_loaded: usize,
    adpcm_byte: u8,
    image_length: Option<usize>,
}

impl<'buffers> ImageDecoder<'buffers> {
    pub fn new(source: &'buffers [u8], destination: &'buffers mut [u8], workspace: &'buffers mut [u8]) -> ImageDecoder<'buffers> {
        ImageDecoder {
            source, destination, workspace, phase: Phase::Header, file_header: None,
            plan: ModuleImagePlan::new(), cursor: 0, item: 0, pattern_readable: true,
            decoded_pattern_bytes: 0, patterns: Vec::new(), instruments: Vec::new(),
            samples: Vec::new(), sample_headers_remaining: 0, offset_sample: 0,
            modplug_tell: false, dialect: None, empty_pattern: None, metadata_bytes: 0,
            metadata_part: 0, metadata_position: 0,
            blob_start: 0, pcm_start: 0, pattern_fill: 0, pattern_cell: 0,
            packed_position: 0, pending_cell: XmCell::EMPTY, pending_mask: 0,
            pending_field: 0, pcm_sample: 0, pcm_position: 0, left_accumulator: 0,
            right_accumulator: 0, adpcm_table: [0; sample::ADPCM_TABLE_BYTES],
            adpcm_table_loaded: 0, adpcm_byte: 0, image_length: None,
        }
    }

    pub fn image_length(&self) -> Option<usize> { self.image_length }

    pub fn step(&mut self, budget: DecodeBudget) -> Result<ImageDecodeStatus, ImageDecodeError> {
        if budget.max_input_bytes < MINIMUM_DECODE_INPUT_BUDGET || budget.max_pcm_frames == 0 {
            return Err(ImageDecodeError::BudgetTooSmall {
                minimum_input_bytes: MINIMUM_DECODE_INPUT_BUDGET, minimum_pcm_frames: 1,
            });
        }
        let budget = DecodeBudget {
            max_input_bytes: budget.max_input_bytes.min(4096),
            max_pcm_frames: budget.max_pcm_frames.min(1024),
        };
        let _workspace_capacity = self.workspace.len();
        match self.phase {
            Phase::Header => self.prepare_header()?,
            Phase::Patterns => self.prepare_pattern()?,
            Phase::Instruments => self.prepare_instrument()?,
            Phase::SampleHeaders => self.prepare_sample_header()?,
            Phase::InlineOffsets => self.assign_inline_offset()?,
            Phase::OldOffsets => self.assign_old_offset()?,
            Phase::PlanSamples => self.plan_sample()?,
            Phase::PlanInstruments => self.plan_instrument()?,
            Phase::Orders => self.prepare_order()?,
            Phase::FinishPlan => self.finish_plan()?,
            Phase::Layout => self.prepare_layout()?,
            Phase::Metadata => self.write_metadata_part()?,
            Phase::PatternFill => self.fill_pattern()?,
            Phase::PatternUnpack => self.unpack_pattern(budget.max_input_bytes)?,
            Phase::Pcm => self.write_pcm(budget)?,
            Phase::Complete => {}
        }
        Ok(match self.image_length {
            Some(image_length) if self.phase == Phase::Complete => ImageDecodeStatus::Complete { image_length },
            _ => ImageDecodeStatus::Pending,
        })
    }

    fn header(&self) -> Result<XmHeader, Error> { self.file_header.ok_or(Error::Invalid("XM decoder header is unavailable")) }

    fn prepare_header(&mut self) -> Result<(), ImageDecodeError> {
        let bytes = self.source.get(..header::FIXED_HEADER_LENGTH)
            .ok_or(Error::Truncated { offset: 0, needed: header::FIXED_HEADER_LENGTH })?;
        let mut fixed = [0u8; header::FIXED_HEADER_LENGTH];
        fixed.copy_from_slice(bytes);
        let file_header = XmHeader::parse(&fixed)?;
        let minimum_body = (file_header.song_length as usize).saturating_add(
            4usize.saturating_mul(file_header.pattern_count as usize + file_header.instrument_count as usize),
        );
        if self.source.len().saturating_sub(header::FIXED_HEADER_LENGTH) < minimum_body {
            return Err(Error::Truncated { offset: header::FIXED_HEADER_LENGTH, needed: minimum_body }.into());
        }
        self.cursor = file_header.body_offset();
        self.phase = if file_header.patterns_precede_instruments() { Phase::Patterns } else { Phase::Instruments };
        self.file_header = Some(file_header);
        Ok(())
    }

    fn prepare_pattern(&mut self) -> Result<(), ImageDecodeError> {
        let header = self.header()?;
        if self.item >= header.pattern_count as usize {
            self.item = 0;
            self.phase = if header.patterns_precede_instruments() { Phase::Instruments } else { Phase::OldOffsets };
            return Ok(());
        }
        let mut rows = pattern::DEFAULT_ROWS;
        let mut packed_start = self.cursor;
        let mut packed_length = 0usize;
        if self.pattern_readable && self.source.len().saturating_sub(self.cursor) >= MINIMUM_PATTERN_HEADER_LENGTH {
            let mut fixed = [0u8; PATTERN_HEADER_LENGTH];
            let readable = PATTERN_HEADER_LENGTH.min(self.source.len().saturating_sub(self.cursor));
            if let Some(bytes) = self.source.get(self.cursor..self.cursor + readable)
                && let Some(target) = fixed.get_mut(..readable)
            {
                target.copy_from_slice(bytes);
            }
            let header_size = read_u32(&fixed, 0) as usize;
            if header_size < MINIMUM_PATTERN_HEADER_LENGTH || self.source.len().saturating_sub(self.cursor) < header_size {
                self.pattern_readable = false;
            } else {
                let (declared_rows, size_offset) = if header.rows_are_a_biased_byte() {
                    (fixed.get(5).copied().unwrap_or(0) as u16 + 1, PATTERN_HEADER_LENGTH_1_02 - 2)
                } else {
                    (read_u16(&fixed, 5), PATTERN_HEADER_LENGTH - 2)
                };
                let declared_length = read_u16(&fixed, size_offset) as usize;
                packed_start = self.cursor.saturating_add(header_size);
                packed_length = declared_length.min(self.source.len().saturating_sub(packed_start));
                self.cursor = packed_start.saturating_add(declared_length);
                rows = if declared_rows == 0 { pattern::DEFAULT_ROWS } else { declared_rows.min(pattern::MAX_ROWS) };
            }
        } else {
            self.pattern_readable = false;
        }
        if packed_start > self.source.len() {
            return Err(Error::Truncated { offset: packed_start, needed: 0 }.into());
        }
        let length = rows as usize * header.channel_count as usize * pattern::CELL_BYTES;
        self.decoded_pattern_bytes = self.decoded_pattern_bytes.saturating_add(length);
        if self.decoded_pattern_bytes > self.source.len().saturating_mul(PATTERN_BUDGET_PER_FILE_BYTE).max(MINIMUM_PATTERN_BUDGET_BYTES) {
            return Err(Error::TooLarge("XM pattern data").into());
        }
        self.patterns.try_reserve(1).map_err(|_| METADATA_RESOURCE)?;
        self.patterns.push(PatternSource { rows, packed_start, packed_length });
        self.plan.try_add_pattern(length, rows, header.channel_count as u8)?;
        self.item += 1;
        Ok(())
    }

    fn prepare_instrument(&mut self) -> Result<(), ImageDecodeError> {
        let file_header = self.header()?;
        if self.item >= file_header.instrument_count as usize {
            self.item = 0;
            self.phase = if file_header.patterns_precede_instruments() { Phase::PlanSamples } else { Phase::Patterns };
            return Ok(());
        }
        let start = self.cursor;
        let readable = instrument::MAX_HEADER_LENGTH.min(self.source.len().saturating_sub(start));
        let (name, instrument_header, declared, raw_sample_header_size) = if readable < 4 {
            (String::new(), XmInstrumentHeader::parse(&[]), 0usize, None)
        } else {
            let raw = self.source.get(start..start + readable).unwrap_or_default();
            let parsed = XmInstrumentHeader::parse(raw);
            let declared = parsed.header_size as usize;
            let visible = declared.min(raw.len());
            let parsed = XmInstrumentHeader::parse(raw.get(..visible).unwrap_or_default());
            let name = starplayer_model::try_decode_cp437(raw.get(instrument::NAME_OFFSET..instrument::NAME_OFFSET + instrument::NAME_LENGTH).unwrap_or_default())?;
            (name, parsed, declared, raw.get(0x1D..0x21).map(|bytes| read_u32(bytes, 0)))
        };
        if declared == MODPLUG_INSTRUMENT_HEADER_SIZE && instrument_header.sample_count == 0 && raw_sample_header_size == Some(0) {
            self.modplug_tell = true;
        }
        if readable >= 4 { self.cursor = start.saturating_add(declared); }
        self.instruments.try_reserve(1).map_err(|_| METADATA_RESOURCE)?;
        self.instruments.push(PendingInstrument {
            name, header: instrument_header, sample_start: self.samples.len(), sample_count: 0,
        });
        self.sample_headers_remaining = if readable < 4 { 0 } else { instrument_header.sample_count as usize };
        self.item += 1;
        self.phase = Phase::SampleHeaders;
        Ok(())
    }

    fn prepare_sample_header(&mut self) -> Result<(), ImageDecodeError> {
        if self.sample_headers_remaining == 0 {
            let header = self.header()?;
            if header.patterns_precede_instruments() {
                self.offset_sample = self.instruments.last().map(|instrument| instrument.sample_start).unwrap_or(self.samples.len());
                self.phase = Phase::InlineOffsets;
            } else {
                self.phase = Phase::Instruments;
            }
            return Ok(());
        }
        if self.source.len().saturating_sub(self.cursor) < sample::HEADER_LENGTH {
            self.sample_headers_remaining = 0;
            return Ok(());
        }
        let bytes = self.source.get(self.cursor..self.cursor + sample::HEADER_LENGTH).ok_or(Error::OutOfRange)?;
        let name = starplayer_model::try_decode_cp437(bytes.get(sample::NAME_OFFSET..sample::NAME_OFFSET + sample::NAME_LENGTH).unwrap_or_default())?;
        self.samples.try_reserve(1).map_err(|_| METADATA_RESOURCE)?;
        self.samples.push(PendingSample { name, header: XmSampleHeader::parse(bytes), data_offset: 0, decoded_frames: 0 });
        if let Some(instrument) = self.instruments.last_mut() { instrument.sample_count += 1; }
        self.cursor = self.cursor.saturating_add(sample::HEADER_LENGTH);
        self.sample_headers_remaining -= 1;
        Ok(())
    }

    fn assign_inline_offset(&mut self) -> Result<(), ImageDecodeError> {
        let end = self.instruments.last().map(|instrument| instrument.sample_start + instrument.sample_count).unwrap_or(self.offset_sample);
        if self.offset_sample >= end {
            self.phase = Phase::Instruments;
            return Ok(());
        }
        let sample = self.samples.get_mut(self.offset_sample).ok_or(Error::OutOfRange)?;
        sample.data_offset = self.cursor;
        self.cursor = self.cursor.saturating_add(sample.header.encoded_bytes());
        self.offset_sample += 1;
        Ok(())
    }

    fn assign_old_offset(&mut self) -> Result<(), ImageDecodeError> {
        if self.offset_sample >= self.samples.len() {
            self.item = 0;
            self.phase = Phase::PlanSamples;
            return Ok(());
        }
        let sample = self.samples.get_mut(self.offset_sample).ok_or(Error::OutOfRange)?;
        sample.data_offset = self.cursor;
        self.cursor = self.cursor.saturating_add(sample.header.encoded_bytes());
        self.offset_sample += 1;
        Ok(())
    }

    fn plan_sample(&mut self) -> Result<(), ImageDecodeError> {
        if self.item >= self.samples.len() {
            self.item = 0;
            self.phase = Phase::PlanInstruments;
            return Ok(());
        }
        let pending = self.samples.get_mut(self.item).ok_or(Error::OutOfRange)?;
        let available_bytes = self.source.len().saturating_sub(pending.data_offset);
        let available_frames = if pending.header.is_adpcm() {
            available_bytes.saturating_sub(sample::ADPCM_TABLE_BYTES).saturating_mul(2)
        } else {
            available_bytes / (pending.header.bytes_per_channel_frame() * pending.header.channels())
        };
        let frames = pending.header.frames().min(available_frames);
        pending.decoded_frames = frames;
        let loop_end = pending.header.loop_end_frames().min(frames);
        let loops = pending.header.loops() && pending.header.loop_start_frames() < loop_end;
        let specification = SampleSpec {
            name: core::mem::take(&mut pending.name),
            loop_mode: match (loops, pending.header.is_ping_pong()) {
                (false, _) => LoopMode::None, (true, false) => LoopMode::Forward, (true, true) => LoopMode::PingPong,
            },
            loop_start: if loops { pending.header.loop_start_frames() as u32 } else { 0 },
            loop_end: if loops { loop_end as u32 } else { 0 },
            default_volume: unit_from_ratio(pending.header.volume as u32, 64),
            reference_rate_hz: starplayer_model::DEFAULT_REFERENCE_RATE_HZ,
            relative_note: pending.header.relative_note,
            finetune: pending.header.finetune,
            default_pan: Some(bipolar_from_ratio(pending.header.pan as i32 - 128, 128)),
            auto_vibrato: self.instrument_for_sample(self.item).map(|instrument| instrument.header.auto_vibrato).unwrap_or_default(),
            sustain_loop: None,
            rate_scale_log2: 0,
        };
        self.plan.try_add_sample(frames, specification)?;
        self.item += 1;
        Ok(())
    }

    fn instrument_for_sample(&self, sample: usize) -> Option<&PendingInstrument> {
        self.instruments.iter().find(|instrument| sample >= instrument.sample_start && sample < instrument.sample_start + instrument.sample_count)
    }

    fn plan_instrument(&mut self) -> Result<(), ImageDecodeError> {
        if self.item >= self.instruments.len() {
            self.item = 0;
            self.phase = Phase::Orders;
            return Ok(());
        }
        let pending = self.instruments.get_mut(self.item).ok_or(Error::OutOfRange)?;
        let mut note_sample_map = [0u16; starplayer_model::NOTE_MAP_LENGTH];
        for (note, local) in pending.header.note_sample_map.iter().enumerate() {
            if (*local as usize) < pending.sample_count
                && let Some(entry) = note_sample_map.get_mut(note)
            {
                *entry = (pending.sample_start + *local as usize + 1).min(u16::MAX as usize) as u16;
            }
        }
        let volume_envelope = try_envelope(&pending.header.volume_envelope)?;
        let panning_envelope = try_envelope(&pending.header.panning_envelope)?;
        let instrument = InstrumentDef {
            name: core::mem::take(&mut pending.name).into_boxed_str(), sample: None,
            default_volume: U0F16::MAX, note_sample_map, volume_envelope, panning_envelope,
            fadeout: pending.header.fadeout, ..InstrumentDef::default()
        };
        self.plan.try_add_instrument(instrument)?;
        self.item += 1;
        Ok(())
    }

    fn prepare_order(&mut self) -> Result<(), ImageDecodeError> {
        let header = self.header()?;
        if self.dialect.is_none() {
            let mut dialect = header.dialect();
            if dialect == starplayer_core::quirks::FormatDialect::FastTracker2
                && header.claims_fast_tracker_2()
                && (self.modplug_tell || has_modplug_extension_chunk(self.source, self.cursor))
            {
                dialect = starplayer_core::quirks::FormatDialect::ModPlugXm;
            }
            let order_bytes = self.source.get(header::FIXED_HEADER_LENGTH..header::FIXED_HEADER_LENGTH + header.song_length as usize).unwrap_or_default();
            let needs_default_order = order_bytes.is_empty() && dialect != starplayer_core::quirks::FormatDialect::OpenMptXm;
            let needs_empty_pattern = if needs_default_order { header.pattern_count == 0 }
                else { order_bytes.iter().any(|order| *order as usize >= header.pattern_count as usize) };
            if needs_empty_pattern {
                let length = pattern::DEFAULT_ROWS as usize * header.channel_count as usize * pattern::CELL_BYTES;
                let id = self.plan.try_add_pattern(length, pattern::DEFAULT_ROWS, header.channel_count as u8)?;
                self.patterns.try_reserve(1).map_err(|_| METADATA_RESOURCE)?;
                self.patterns.push(PatternSource { rows: pattern::DEFAULT_ROWS, packed_start: 0, packed_length: 0 });
                self.empty_pattern = Some(id.0);
            }
            self.dialect = Some(dialect);
        }
        let order_count = if header.song_length == 0 && self.dialect != Some(starplayer_core::quirks::FormatDialect::OpenMptXm) { 1 } else { header.song_length as usize };
        if self.item >= order_count {
            self.item = 0;
            self.phase = Phase::FinishPlan;
            return Ok(());
        }
        let raw = if header.song_length == 0 { 0 } else {
            self.source.get(header::FIXED_HEADER_LENGTH + self.item).copied().ok_or(Error::OutOfRange)?
        };
        let order = if (raw as usize) < header.pattern_count as usize { raw as u16 } else { self.empty_pattern.unwrap_or(0) };
        self.plan.try_push_order(order)?;
        self.item += 1;
        Ok(())
    }

    fn finish_plan(&mut self) -> Result<(), ImageDecodeError> {
        let header = self.header()?;
        let title = starplayer_model::try_decode_cp437(self.source.get(header::TITLE_OFFSET..header::TITLE_OFFSET + header::TITLE_LENGTH).unwrap_or_default())?;
        self.plan.set_header(ModuleHeader {
            title: title.into_boxed_str(), format: ModuleFormat::Xm, channel_count: header.channel_count as u8,
            initial_speed: if header.initial_speed == 0 { FALLBACK_SPEED } else { header.initial_speed.min(u8::MAX as u16) as u8 },
            initial_tempo: if header.initial_tempo == 0 { FALLBACK_TEMPO } else { header.initial_tempo.clamp(MINIMUM_TEMPO, MAXIMUM_TEMPO) },
            global_volume: U0F16::MAX, master_volume: U0F16::MAX,
            default_pan: Vec::new().into_boxed_slice(),
            flags: ModuleFlags { amiga_limits: false, linear_slides: header.linear_slides(), fast_volume_slides: false, stereo: true },
            dialect: self.dialect.unwrap_or_else(|| header.dialect()),
            format_extra: XmFormatExtra { restart_position: header.restart_position, flags: header.flags }.encode(),
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
        let image_length = self.pcm_start.checked_add(self.plan.pcm_frames().checked_mul(2).ok_or(Error::TooLarge("module image PCM"))?)
            .ok_or(Error::TooLarge("module image"))?;
        if image_length > self.destination.len() {
            return Err(ImageDecodeError::DestinationTooSmall { required: image_length, available: self.destination.len() });
        }
        self.destination.get_mut(after_blob..pcm_length_offset).ok_or(Error::OutOfRange)?.fill(0);
        let pcm_frames = u32::try_from(self.plan.pcm_frames()).map_err(|_| Error::TooLarge("module image PCM"))?;
        self.destination.get_mut(pcm_length_offset..self.pcm_start).ok_or(Error::OutOfRange)?.copy_from_slice(&pcm_frames.to_le_bytes());
        self.image_length = Some(image_length);
        self.phase = Phase::Metadata;
        Ok(())
    }

    fn write_metadata_part(&mut self) -> Result<(), ImageDecodeError> {
        if self.metadata_part >= self.plan.metadata_part_count() {
            if self.metadata_position != self.metadata_bytes {
                return Err(Error::Invalid("module image metadata length changed").into());
            }
            self.phase = Phase::PatternFill;
            return Ok(());
        }
        let target = self.destination.get_mut(self.metadata_position..self.metadata_bytes).ok_or(Error::OutOfRange)?;
        let written = self.plan.write_metadata_part(self.metadata_part, target)?;
        self.metadata_position += written;
        self.metadata_part += 1;
        Ok(())
    }

    fn fill_pattern(&mut self) -> Result<(), ImageDecodeError> {
        if self.item >= self.patterns.len() {
            self.item = 0;
            self.phase = Phase::Pcm;
            return Ok(());
        }
        let index = self.plan.patterns().get(self.item).ok_or(Error::OutOfRange)?;
        let length = index.length_bytes() as usize;
        let amount = (length - self.pattern_fill).min(4096);
        let start = self.blob_start + index.blob_offset() as usize + self.pattern_fill;
        self.destination.get_mut(start..start + amount).ok_or(Error::OutOfRange)?.fill(0);
        self.pattern_fill += amount;
        if self.pattern_fill == length {
            self.pattern_fill = 0;
            self.pattern_cell = 0;
            self.packed_position = 0;
            self.pending_mask = 0;
            self.pending_field = 0;
            self.pending_cell = XmCell::EMPTY;
            self.phase = Phase::PatternUnpack;
        }
        Ok(())
    }

    fn unpack_pattern(&mut self, max_input_bytes: usize) -> Result<(), ImageDecodeError> {
        let pattern_source = *self.patterns.get(self.item).ok_or(Error::OutOfRange)?;
        let cell_count = pattern_source.rows as usize * self.header()?.channel_count as usize;
        let mut consumed = 0usize;
        while self.pattern_cell < cell_count {
            if self.pending_field == 0 {
                if self.packed_position >= pattern_source.packed_length || consumed >= max_input_bytes { break; }
                let first = self.source.get(pattern_source.packed_start + self.packed_position).copied().unwrap_or(0);
                self.packed_position += 1;
                consumed += 1;
                self.pending_cell = XmCell::EMPTY;
                self.pending_mask = if first & pattern::MASK_IS_MASK != 0 { first } else {
                    self.pending_cell.note = first;
                    pattern::MASK_INSTRUMENT | pattern::MASK_VOLUME | pattern::MASK_EFFECT | pattern::MASK_PARAMETER
                };
                self.pending_field = 1;
            }
            while self.pending_field <= pattern::MASK_PARAMETER {
                let field = self.pending_field;
                self.pending_field <<= 1;
                if self.pending_mask & field == 0 { continue; }
                let value = if self.packed_position < pattern_source.packed_length {
                    if consumed >= max_input_bytes {
                        self.pending_field = field;
                        break;
                    }
                    let value = self.source.get(pattern_source.packed_start + self.packed_position).copied().unwrap_or(0);
                    self.packed_position += 1;
                    consumed += 1;
                    value
                } else { 0 };
                match field {
                    pattern::MASK_NOTE => self.pending_cell.note = value,
                    pattern::MASK_INSTRUMENT => self.pending_cell.instrument = value,
                    pattern::MASK_VOLUME => self.pending_cell.volume = value,
                    pattern::MASK_EFFECT => self.pending_cell.effect = value,
                    pattern::MASK_PARAMETER => self.pending_cell.parameter = value,
                    _ => {}
                }
            }
            if self.pending_field <= pattern::MASK_PARAMETER { break; }
            let index = self.plan.patterns().get(self.item).ok_or(Error::OutOfRange)?;
            let target = self.blob_start + index.blob_offset() as usize + self.pattern_cell * pattern::CELL_BYTES;
            self.destination.get_mut(target..target + pattern::CELL_BYTES).ok_or(Error::OutOfRange)?
                .copy_from_slice(&self.pending_cell.to_bytes());
            self.pattern_cell += 1;
            self.pending_field = 0;
        }
        if self.pattern_cell >= cell_count || (self.packed_position >= pattern_source.packed_length && self.pending_field == 0) {
            self.item += 1;
            self.phase = Phase::PatternFill;
        }
        Ok(())
    }

    fn write_pcm(&mut self, budget: DecodeBudget) -> Result<(), ImageDecodeError> {
        if self.pcm_sample >= self.plan.samples().len() {
            self.phase = Phase::Complete;
            return Ok(());
        }
        let sample_index = self.plan.samples().get(self.pcm_sample).ok_or(Error::OutOfRange)?;
        let pending = self.samples.get(self.pcm_sample).ok_or(Error::OutOfRange)?;
        let total = sample_index.stored_frames();
        let body_start = starplayer_core::PRE_ROLL_FRAMES;
        let body_end = body_start + sample_index.length_frames() as usize;
        let mut emitted = 0usize;
        let mut scanned = 0usize;
        while self.pcm_position < total && emitted < budget.max_pcm_frames {
            let frame = if self.pcm_position < body_start {
                0
            } else if self.pcm_position < body_end {
                let frame = self.pcm_position - body_start;
                if pending.header.is_adpcm() {
                    while self.adpcm_table_loaded < sample::ADPCM_TABLE_BYTES && scanned < budget.max_input_bytes {
                        let value = self.source.get(pending.data_offset + self.adpcm_table_loaded).copied().unwrap_or(0) as i8;
                        if let Some(entry) = self.adpcm_table.get_mut(self.adpcm_table_loaded) { *entry = value; }
                        self.adpcm_table_loaded += 1;
                        scanned += 1;
                    }
                    if self.adpcm_table_loaded < sample::ADPCM_TABLE_BYTES { break; }
                    let low_nybble = frame.is_multiple_of(2);
                    if low_nybble {
                        if scanned >= budget.max_input_bytes { break; }
                        self.adpcm_byte = self.source.get(pending.data_offset + sample::ADPCM_TABLE_BYTES + frame / 2).copied().unwrap_or(0);
                        scanned += 1;
                    }
                    let index = if low_nybble { self.adpcm_byte & 0x0F } else { self.adpcm_byte >> 4 };
                    self.left_accumulator = (self.left_accumulator as i8)
                        .wrapping_add(self.adpcm_table.get(index as usize).copied().unwrap_or(0)) as i16;
                    self.left_accumulator * 256
                } else {
                    let bytes = pending.header.bytes_per_channel_frame() * pending.header.channels();
                    if scanned.saturating_add(bytes) > budget.max_input_bytes { break; }
                    let left = decode_delta(self.source, pending.data_offset, frame, pending.header.is_sixteen_bit(), &mut self.left_accumulator);
                    scanned += pending.header.bytes_per_channel_frame();
                    if pending.header.is_stereo() {
                        let right_start = pending.data_offset.saturating_add(pending.decoded_frames * pending.header.bytes_per_channel_frame());
                        let right = decode_delta(self.source, right_start, frame, pending.header.is_sixteen_bit(), &mut self.right_accumulator);
                        scanned += pending.header.bytes_per_channel_frame();
                        ((left as i32 + right as i32) / 2) as i16
                    } else { left }
                }
            } else {
                let guard = self.pcm_position - body_end;
                let source_frame = match sample_index.loop_mode() {
                    LoopMode::Forward => {
                        let length = (sample_index.loop_end() - sample_index.loop_start()) as usize;
                        Some(sample_index.loop_start() as usize + guard % length)
                    }
                    LoopMode::PingPong => Some(starplayer_model::ping_pong_reflect(
                        sample_index.loop_start(), sample_index.loop_end(), sample_index.loop_end().saturating_add(guard as u32),
                    ) as usize),
                    LoopMode::None => None,
                };
                match source_frame {
                    Some(source_frame) => read_destination_frame(self.destination, self.pcm_start + (sample_index.pcm_offset() as usize + source_frame) * 2),
                    None => 0,
                }
            };
            let output = self.pcm_start + (sample_index.pcm_offset() as usize - starplayer_core::PRE_ROLL_FRAMES + self.pcm_position) * 2;
            self.destination.get_mut(output..output + 2).ok_or(Error::OutOfRange)?.copy_from_slice(&frame.to_le_bytes());
            self.pcm_position += 1;
            emitted += 1;
        }
        if self.pcm_position == total {
            self.pcm_sample += 1;
            self.pcm_position = 0;
            self.left_accumulator = 0;
            self.right_accumulator = 0;
            self.adpcm_table_loaded = 0;
            self.adpcm_byte = 0;
        }
        Ok(())
    }
}

fn decode_delta(source: &[u8], start: usize, frame: usize, sixteen_bit: bool, accumulator: &mut i16) -> i16 {
    if sixteen_bit {
        let offset = start.saturating_add(frame.saturating_mul(2));
        let delta = i16::from_le_bytes([
            source.get(offset).copied().unwrap_or(0), source.get(offset + 1).copied().unwrap_or(0),
        ]);
        *accumulator = accumulator.wrapping_add(delta);
        *accumulator
    } else {
        let delta = source.get(start.saturating_add(frame)).copied().unwrap_or(0) as i8;
        *accumulator = (*accumulator as i8).wrapping_add(delta) as i16;
        *accumulator * 256
    }
}

fn read_destination_frame(destination: &[u8], offset: usize) -> i16 {
    i16::from_le_bytes([
        destination.get(offset).copied().unwrap_or(0), destination.get(offset + 1).copied().unwrap_or(0),
    ])
}

fn try_envelope(source: &XmEnvelope) -> Result<Option<Envelope>, Error> {
    let count = (source.point_count as usize).min(instrument::MAX_ENVELOPE_POINTS);
    if source.flags & instrument::ENVELOPE_ENABLED == 0 || count == 0 { return Ok(None); }
    let mut points = Vec::new();
    points.try_reserve_exact(count).map_err(|_| METADATA_RESOURCE)?;
    for (tick, value) in source.points.iter().take(count) { points.push(EnvelopePoint { tick: *tick, value: *value as i16 }); }
    let sustain = if source.flags & instrument::ENVELOPE_SUSTAIN != 0 && (source.sustain as usize) < instrument::MAX_ENVELOPE_POINTS {
        Some(EnvelopeSpan { start: source.sustain, end: source.sustain })
    } else { None };
    let loop_span = if source.flags & instrument::ENVELOPE_LOOP != 0
        && (source.loop_end as usize) < instrument::MAX_ENVELOPE_POINTS && source.loop_end >= source.loop_start
    {
        Some(EnvelopeSpan { start: source.loop_start, end: source.loop_end })
    } else { None };
    Ok(Some(Envelope { points: points.into_boxed_slice(), sustain, loop_span, carry: false }))
}

fn has_modplug_extension_chunk(source: &[u8], mut cursor: usize) -> bool {
    for _ in 0..MODPLUG_CHUNK_SCAN_LIMIT {
        let Some(bytes) = source.get(cursor..cursor.saturating_add(8)) else { return false };
        let Some(tag) = bytes.get(..4).and_then(|tag| <[u8; 4]>::try_from(tag).ok()) else { return false };
        let size = read_u32(bytes, 4);
        if size > i32::MAX as u32 { return false; }
        if MODPLUG_CHUNK_TAGS.contains(&&tag)
            || u32::from_be_bytes(tag) & MODPLUG_PLUGIN_TAG_BITS == MODPLUG_PLUGIN_TAG_BITS
        {
            return true;
        }
        cursor = cursor.saturating_add(8).saturating_add(size as usize);
    }
    false
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes.get(offset).copied().unwrap_or(0), bytes.get(offset + 1).copied().unwrap_or(0)])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes.get(offset).copied().unwrap_or(0), bytes.get(offset + 1).copied().unwrap_or(0),
        bytes.get(offset + 2).copied().unwrap_or(0), bytes.get(offset + 3).copied().unwrap_or(0),
    ])
}
