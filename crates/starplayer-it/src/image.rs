//! Incremental IT to canonical module-image conversion.

use alloc::string::String;
use alloc::vec::Vec;
use starplayer_core::fixed::unit_from_ratio;
use starplayer_core::{Error, GUARD_FRAMES, PRE_ROLL_FRAMES, U0F16};
use starplayer_model::{
    AutoVibrato, DecodeBudget, ImageDecodeError, ImageDecodeStatus, InstrumentDef, LoopMode,
    ModuleFlags, ModuleFormat, ModuleHeader, ModuleImagePlan, ORDER_END, ORDER_MARKER,
    SampleSpec, SustainLoop, MINIMUM_DECODE_INPUT_BUDGET, ping_pong_reflect,
};

use crate::compression::IncrementalDecompressor;
use crate::header::{self, ItFormatExtra, ItHeader, MAX_CHANNELS};
use crate::instrument::{self, ItInstrument};
use crate::pattern::{self, ItCell};
use crate::sample::{self, ItSampleHeader};

const MINIMUM_TEMPO: u16 = 32;
const FALLBACK_SPEED: u8 = 6;
const MAX_VOLUME: u8 = 64;
const MAX_SONG_VOLUME: u8 = 128;
const PATTERN_HEADER_BYTES: usize = 8;
const MINIMUM_PATTERN_BUDGET_BYTES: usize = 32 * 1024 * 1024;
const MINIMUM_PCM_BUDGET_FRAMES: usize = 8 * 1024 * 1024;
const METADATA_RESOURCE: Error = Error::Resource("not enough memory for module image metadata");

#[derive(Copy, Clone, Debug, Default)]
struct PatternExtent { body_offset: usize, body_length: usize, rows: u16 }

#[derive(Copy, Clone, Debug, Default)]
struct SampleInfo {
    header: Option<ItSampleHeader>,
    requested_frames: usize,
    decoded_frames: usize,
    left_frames: usize,
    right_frames: usize,
    left_consumed: usize,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    Header, Offsets, PatternScan, Samples, Instruments, Orders, HeaderPlan, HeaderData,
    Patterns, Layout, Metadata, PatternFill, PatternUnpack, PcmInit, PcmLeft, PcmRight,
    PcmTail, PcmGuard, Complete,
}

pub struct ImageDecoder<'buffers> {
    source: &'buffers [u8],
    destination: &'buffers mut [u8],
    workspace: &'buffers mut [u8],
    phase: Phase,
    header: Option<ItHeader>,
    plan: ModuleImagePlan,
    instrument_offsets: Vec<u32>,
    sample_offsets: Vec<u32>,
    patterns: Vec<PatternExtent>,
    samples: Vec<SampleInfo>,
    sample_volumes: Vec<u8>,
    first_pointer: usize,
    midi_offset: Option<usize>,
    pending_header: Option<ModuleHeader>,
    item: usize,
    subitem: usize,
    channel_count: u8,
    decoded_rows: usize,
    scan_mask: [u8; pattern::MAX_PACKED_CHANNELS],
    scan_position: usize,
    scan_row: u16,
    compressed_header: Option<ItSampleHeader>,
    compressed_requested: usize,
    compressed_channel: u8,
    compressed_left_frames: usize,
    compressed_left_consumed: usize,
    compressed_decoder: Option<IncrementalDecompressor<'buffers>>,
    pcm_remaining: usize,
    metadata_bytes: usize,
    blob_start: usize,
    pcm_start: usize,
    image_length: Option<usize>,
    metadata_part: usize,
    metadata_position: usize,
    pattern_fill: usize,
    unpack_position: usize,
    unpack_row: u16,
    unpack_mask: [u8; pattern::MAX_PACKED_CHANNELS],
    unpack_note: [u8; pattern::MAX_PACKED_CHANNELS],
    unpack_instrument: [u8; pattern::MAX_PACKED_CHANNELS],
    unpack_volume: [u8; pattern::MAX_PACKED_CHANNELS],
    unpack_command: [u8; pattern::MAX_PACKED_CHANNELS],
    unpack_info: [u8; pattern::MAX_PACKED_CHANNELS],
    pcm_position: usize,
    pcm_decoder: Option<IncrementalDecompressor<'buffers>>,
    pcm_channel_position: usize,
    pcm_left_accumulator: i16,
    pcm_right_accumulator: i16,
}

impl<'buffers> ImageDecoder<'buffers> {
    pub fn new(source: &'buffers [u8], destination: &'buffers mut [u8], workspace: &'buffers mut [u8]) -> Self {
        Self {
            source, destination, workspace, phase: Phase::Header, header: None,
            plan: ModuleImagePlan::new(), instrument_offsets: Vec::new(), sample_offsets: Vec::new(),
            patterns: Vec::new(), samples: Vec::new(), sample_volumes: Vec::new(),
            first_pointer: usize::MAX, midi_offset: None, pending_header: None, item: 0,
            subitem: 0, channel_count: 0, decoded_rows: 0,
            scan_mask: [0; pattern::MAX_PACKED_CHANNELS], scan_position: 0, scan_row: 0,
            compressed_header: None, compressed_requested: 0, compressed_channel: 0,
            compressed_left_frames: 0, compressed_left_consumed: 0, compressed_decoder: None,
            pcm_remaining: 0, metadata_bytes: 0, blob_start: 0, pcm_start: 0,
            image_length: None, metadata_part: 0, metadata_position: 0, pattern_fill: 0,
            unpack_position: 0, unpack_row: 0, unpack_mask: [0; pattern::MAX_PACKED_CHANNELS],
            unpack_note: [pattern::NOTE_NONE; pattern::MAX_PACKED_CHANNELS],
            unpack_instrument: [pattern::INSTRUMENT_NONE; pattern::MAX_PACKED_CHANNELS],
            unpack_volume: [pattern::VOLUME_NONE; pattern::MAX_PACKED_CHANNELS],
            unpack_command: [pattern::COMMAND_NONE; pattern::MAX_PACKED_CHANNELS],
            unpack_info: [0; pattern::MAX_PACKED_CHANNELS], pcm_position: 0,
            pcm_decoder: None, pcm_channel_position: 0, pcm_left_accumulator: 0,
            pcm_right_accumulator: 0,
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
            Phase::Offsets => self.prepare_offset()?,
            Phase::PatternScan => self.scan_pattern(budget.max_input_bytes)?,
            Phase::Samples => self.prepare_sample(budget)?,
            Phase::Instruments => self.prepare_instrument()?,
            Phase::Orders => self.prepare_order()?,
            Phase::HeaderPlan => self.prepare_module_header()?,
            Phase::HeaderData => self.copy_header_data(budget.max_input_bytes)?,
            Phase::Patterns => self.prepare_pattern()?,
            Phase::Layout => self.prepare_layout()?,
            Phase::Metadata => self.write_metadata()?,
            Phase::PatternFill => self.fill_pattern()?,
            Phase::PatternUnpack => self.unpack_pattern(budget.max_input_bytes)?,
            Phase::PcmInit => self.initialize_pcm(budget.max_pcm_frames)?,
            Phase::PcmLeft => self.write_pcm_left(budget)?,
            Phase::PcmRight => self.write_pcm_right(budget)?,
            Phase::PcmTail => self.finish_stereo_tail(budget.max_pcm_frames)?,
            Phase::PcmGuard => self.write_guard(budget.max_pcm_frames)?,
            Phase::Complete => {}
        }
        Ok(match self.image_length {
            Some(image_length) if self.phase == Phase::Complete => ImageDecodeStatus::Complete { image_length },
            _ => ImageDecodeStatus::Pending,
        })
    }

    fn file_header(&self) -> Result<ItHeader, Error> { self.header.ok_or(Error::Invalid("IT decoder header is unavailable")) }
    fn tail(&self, offset: usize) -> &'buffers [u8] { self.source.get(offset..).unwrap_or_default() }

    fn prepare_header(&mut self) -> Result<(), ImageDecodeError> {
        let bytes = self.source.get(..header::HEADER_LENGTH).ok_or(Error::Truncated { offset: 0, needed: header::HEADER_LENGTH })?;
        let file_header = ItHeader::parse(bytes)?;
        let orders_end = header::HEADER_LENGTH.checked_add(file_header.order_count as usize).ok_or(Error::TooLarge("IT tables"))?;
        let instruments_end = orders_end.checked_add(file_header.instrument_count as usize * 4).ok_or(Error::TooLarge("IT tables"))?;
        let samples_end = instruments_end.checked_add(file_header.sample_count as usize * 4).ok_or(Error::TooLarge("IT tables"))?;
        let tables_end = samples_end.checked_add(file_header.pattern_count as usize * 4).ok_or(Error::TooLarge("IT tables"))?;
        if tables_end > self.source.len() { return Err(Error::Truncated { offset: header::HEADER_LENGTH, needed: tables_end - header::HEADER_LENGTH }.into()); }
        self.instrument_offsets.try_reserve_exact(file_header.instrument_count as usize).map_err(|_| METADATA_RESOURCE)?;
        self.sample_offsets.try_reserve_exact(file_header.sample_count as usize).map_err(|_| METADATA_RESOURCE)?;
        self.patterns.try_reserve_exact(file_header.pattern_count as usize).map_err(|_| METADATA_RESOURCE)?;
        self.samples.try_reserve_exact(file_header.sample_count as usize).map_err(|_| METADATA_RESOURCE)?;
        self.sample_volumes.try_reserve_exact(file_header.sample_count as usize).map_err(|_| METADATA_RESOURCE)?;
        self.pcm_remaining = self.source.len().saturating_mul(16).max(MINIMUM_PCM_BUDGET_FRAMES);
        self.header = Some(file_header);
        self.phase = Phase::Offsets;
        Ok(())
    }

    fn table_offsets(&self) -> Result<(usize, usize, usize), Error> {
        let header = self.file_header()?;
        let instruments = header::HEADER_LENGTH + header.order_count as usize;
        let samples = instruments + header.instrument_count as usize * 4;
        let patterns = samples + header.sample_count as usize * 4;
        Ok((instruments, samples, patterns))
    }

    fn read_u32(&self, offset: usize) -> Result<u32, Error> {
        let Some([a, b, c, d]) = self.source.get(offset..offset + 4) else { return Err(Error::Truncated { offset, needed: 4 }) };
        Ok(u32::from_le_bytes([*a, *b, *c, *d]))
    }

    fn prepare_offset(&mut self) -> Result<(), ImageDecodeError> {
        let header = self.file_header()?;
        let (instrument_table, sample_table, pattern_table) = self.table_offsets()?;
        if self.item < header.instrument_count as usize {
            let pointer = self.read_u32(instrument_table + self.item * 4)?;
            if pointer > 0 { self.first_pointer = self.first_pointer.min(pointer as usize); }
            self.instrument_offsets.push(pointer); self.item += 1; return Ok(());
        }
        let sample_index = self.item - header.instrument_count as usize;
        if sample_index < header.sample_count as usize {
            let pointer = self.read_u32(sample_table + sample_index * 4)?;
            if pointer > 0 { self.first_pointer = self.first_pointer.min(pointer as usize); }
            self.sample_offsets.push(pointer); self.item += 1; return Ok(());
        }
        let pattern_index = sample_index - header.sample_count as usize;
        if pattern_index < header.pattern_count as usize {
            let pointer = self.read_u32(pattern_table + pattern_index * 4)?;
            if pointer > 0 { self.first_pointer = self.first_pointer.min(pointer as usize); }
            self.patterns.push(self.pattern_extent(pointer as usize)); self.item += 1; return Ok(());
        }
        if header.special & header::SPECIAL_SONG_MESSAGE != 0 {
            self.first_pointer = self.first_pointer.min(header.message_offset as usize);
        }
        let tables_end = pattern_table + header.pattern_count as usize * 4;
        let mut cursor = tables_end;
        if header.special & header::SPECIAL_EDIT_HISTORY != 0
            && let Some([low, high]) = self.source.get(cursor..cursor + 2)
        {
            let entries = u16::from_le_bytes([*low, *high]) as usize;
            let end = cursor.saturating_add(2).saturating_add(entries.saturating_mul(header::EDIT_HISTORY_ENTRY_BYTES));
            if end <= self.source.len() && end <= self.first_pointer { cursor = end; }
        }
        if header.has_midi_configuration() && cursor.saturating_add(header::MIDI_CONFIGURATION_BYTES) <= self.source.len() {
            self.midi_offset = Some(cursor);
        }
        self.item = 0; self.phase = Phase::PatternScan;
        Ok(())
    }

    fn pattern_extent(&self, offset: usize) -> PatternExtent {
        if offset == 0 || offset.saturating_add(PATTERN_HEADER_BYTES) > self.source.len() { return PatternExtent { rows: pattern::DEFAULT_ROWS, ..PatternExtent::default() }; }
        let Some([packed_low, packed_high, rows_low, rows_high, _, _, _, _]) = self.source.get(offset..offset + PATTERN_HEADER_BYTES) else { return PatternExtent { rows: pattern::DEFAULT_ROWS, ..PatternExtent::default() } };
        let packed = u16::from_le_bytes([*packed_low, *packed_high]) as usize;
        let rows = u16::from_le_bytes([*rows_low, *rows_high]);
        if rows == 0 || rows > pattern::MAX_ROWS { return PatternExtent { rows: pattern::DEFAULT_ROWS, ..PatternExtent::default() }; }
        let body_offset = offset + PATTERN_HEADER_BYTES;
        PatternExtent { body_offset, body_length: packed.min(self.source.len().saturating_sub(body_offset)), rows }
    }

    fn packed_channel(byte: u8) -> Option<usize> {
        let value = (byte & 0x7F) as usize;
        let channel = if value == 0 { 0 } else { value - 1 };
        (channel < pattern::MAX_PACKED_CHANNELS).then_some(channel)
    }

    fn scan_pattern(&mut self, max_input_bytes: usize) -> Result<(), ImageDecodeError> {
        if self.item >= self.patterns.len() {
            self.channel_count = self.channel_count.clamp(1, MAX_CHANNELS as u8);
            let bytes = self.decoded_rows.saturating_mul(self.channel_count as usize).saturating_mul(pattern::CELL_BYTES);
            if bytes > self.source.len().saturating_mul(64).max(MINIMUM_PATTERN_BUDGET_BYTES) { return Err(Error::TooLarge("IT pattern data").into()); }
            self.item = 0; self.phase = Phase::Samples; return Ok(());
        }
        let extent = self.patterns.get(self.item).copied().ok_or(Error::OutOfRange)?;
        if extent.body_length == 0 { self.finish_pattern_scan(extent.rows); return Ok(()); }
        let body = self.source.get(extent.body_offset..extent.body_offset + extent.body_length).ok_or(Error::OutOfRange)?;
        let starting = self.scan_position;
        while self.scan_row < extent.rows && self.scan_position.saturating_sub(starting) < max_input_bytes.saturating_sub(7) {
            let Some(byte) = body.get(self.scan_position).copied() else { break };
            self.scan_position += 1;
            if byte == 0 { self.scan_row += 1; continue; }
            let Some(channel) = Self::packed_channel(byte) else { self.scan_position = body.len(); break };
            if byte & pattern::CHANNEL_HAS_MASK != 0 {
                let Some(mask) = body.get(self.scan_position).copied() else { self.scan_position = body.len(); break };
                self.scan_position += 1;
                let slot = self.scan_mask.get_mut(channel).ok_or(Error::OutOfRange)?;
                *slot = mask;
            }
            let low = self.scan_mask.get(channel).copied().unwrap_or(0) & 0x0F;
            if low != 0 && channel < MAX_CHANNELS && self.channel_count <= channel as u8 { self.channel_count = channel as u8 + 1; }
            self.scan_position = self.scan_position.saturating_add(mask_skip(low));
        }
        if self.scan_row >= extent.rows || self.scan_position >= body.len() { self.finish_pattern_scan(extent.rows); }
        Ok(())
    }

    fn finish_pattern_scan(&mut self, rows: u16) {
        self.decoded_rows = self.decoded_rows.saturating_add(rows as usize);
        self.item += 1; self.scan_position = 0; self.scan_row = 0; self.scan_mask = [0; pattern::MAX_PACKED_CHANNELS];
    }

    fn prepare_sample(&mut self, budget: DecodeBudget) -> Result<(), ImageDecodeError> {
        if let Some(header) = self.compressed_header {
            let tracker_version = self.file_header()?.tracker_version;
            let scratch = starplayer_model::image_workspace_i16(self.workspace, budget.max_pcm_frames)?;
            let decoder = self.compressed_decoder.as_mut().ok_or(Error::Invalid("IT compressed preflight state"))?;
            let chunk = decoder.decode_into(scratch, budget.max_input_bytes);
            let total_input_consumed = decoder.total_input_consumed();
            if self.compressed_channel == 0 { self.compressed_left_frames += chunk.written; }
            else { self.subitem += chunk.written; }
            if chunk.finished {
                if self.compressed_channel == 0 && header.is_stereo(tracker_version) {
                    self.compressed_left_consumed = total_input_consumed;
                    self.compressed_channel = 1; self.subitem = 0;
                    let tail = self.tail(header.data_offset as usize + self.compressed_left_consumed);
                    self.compressed_decoder = Some(IncrementalDecompressor::new(tail, self.compressed_requested, header.is_sixteen_bit(), header.is_delta()));
                } else {
                    let right_frames = if self.compressed_channel == 1 { self.subitem } else { 0 };
                    let frames = self.compressed_left_frames.max(right_frames);
                    let info = SampleInfo { header: Some(header), requested_frames: self.compressed_requested, decoded_frames: frames, left_frames: self.compressed_left_frames, right_frames, left_consumed: self.compressed_left_consumed };
                    self.finish_sample(info)?;
                    self.compressed_header = None; self.compressed_decoder = None; self.compressed_channel = 0; self.subitem = 0;
                }
            }
            return Ok(());
        }
        let header_offset = self.sample_offsets.get(self.item).copied().unwrap_or(0) as usize;
        if header_offset == 0 || header_offset.saturating_add(sample::HEADER_LENGTH) > self.source.len() {
            return self.finish_sample(SampleInfo::default());
        }
        let bytes = self.source.get(header_offset..header_offset + sample::HEADER_LENGTH).ok_or(Error::OutOfRange)?;
        let sample_header = ItSampleHeader::parse(bytes)?;
        let data_offset = sample_header.data_offset as usize;
        if sample_header.length == 0 || data_offset == 0 || data_offset >= self.source.len() {
            return self.finish_sample(SampleInfo { header: Some(sample_header), ..SampleInfo::default() });
        }
        if sample_header.is_compressed() {
            let requested = (sample_header.length as usize).min(self.pcm_remaining);
            self.compressed_header = Some(sample_header); self.compressed_requested = requested;
            self.compressed_channel = 0; self.compressed_left_frames = 0; self.compressed_left_consumed = 0;
            self.compressed_decoder = Some(IncrementalDecompressor::new(self.tail(data_offset), requested, sample_header.is_sixteen_bit(), sample_header.is_delta()));
            return Ok(());
        }
        let channels = sample_header.stored_channels(self.file_header()?.tracker_version);
        let frames = (sample_header.length as usize).min((self.source.len() - data_offset) / sample_header.bytes_per_frame(self.file_header()?.tracker_version)).min(self.pcm_remaining);
        self.finish_sample(SampleInfo { header: Some(sample_header), requested_frames: frames, decoded_frames: frames, left_frames: frames, right_frames: if channels == 2 { frames } else { 0 }, left_consumed: frames * if sample_header.is_sixteen_bit() { 2 } else { 1 } })
    }

    fn finish_sample(&mut self, info: SampleInfo) -> Result<(), ImageDecodeError> {
        let header_offset = self.sample_offsets.get(self.item).copied().unwrap_or(0) as usize;
        let (name, global_volume, specification) = if let Some(sample_header) = info.header {
            let name = starplayer_model::try_decode_cp437(self.source.get(header_offset + 0x14..header_offset + 0x2E).unwrap_or_default())?;
            let loop_end = (sample_header.loop_end as usize).min(info.decoded_frames) as u32;
            let loops = sample_header.loops() && sample_header.loop_start < loop_end;
            let sustain_end = (sample_header.sustain_end as usize).min(info.decoded_frames) as u32;
            let sustains = sample_header.sustain_loops() && sample_header.sustain_start < sustain_end;
            let specification = SampleSpec {
                name: starplayer_model::try_clone_text(&name)?,
                loop_mode: match (loops, sample_header.loop_is_ping_pong()) { (false, _) => LoopMode::None, (true, false) => LoopMode::Forward, (true, true) => LoopMode::PingPong },
                loop_start: if loops { sample_header.loop_start } else { 0 }, loop_end: if loops { loop_end } else { 0 },
                default_volume: unit_from_ratio(sample_header.volume.min(MAX_VOLUME) as u32, MAX_VOLUME as u32),
                reference_rate_hz: sample_header.reference_rate_hz(), relative_note: 0, finetune: 0,
                default_pan: sample_header.pan_position().map(header::pan_to_bipolar),
                auto_vibrato: AutoVibrato { waveform: sample_header.auto_vibrato_waveform(), sweep: sample_header.vibrato_sweep, depth: sample_header.vibrato_depth & 0x7F, rate: sample_header.vibrato_speed },
                sustain_loop: if sustains { Some(SustainLoop { mode: if sample_header.sustain_is_ping_pong() { LoopMode::PingPong } else { LoopMode::Forward }, start: sample_header.sustain_start, end: sustain_end }) } else { None },
                rate_scale_log2: 0,
            };
            (name, sample_header.global_volume.min(MAX_VOLUME), specification)
        } else {
            (String::new(), MAX_VOLUME, SampleSpec::one_shot(""))
        };
        self.plan.try_add_sample(info.decoded_frames, specification)?;
        self.samples.push(info); self.sample_volumes.push(global_volume);
        self.pcm_remaining = self.pcm_remaining.saturating_sub(info.decoded_frames);
        let _ = name;
        self.item += 1;
        if self.item >= self.sample_offsets.len() { self.item = 0; self.phase = Phase::Instruments; }
        Ok(())
    }

    fn prepare_instrument(&mut self) -> Result<(), ImageDecodeError> {
        let header = self.file_header()?;
        if !header.is_instrument_mode() {
            if self.item >= self.plan.samples().len() { self.item = 0; self.phase = Phase::Orders; return Ok(()); }
            let name = self.plan.samples().get(self.item).ok_or(Error::OutOfRange)?.name();
            self.plan.try_add_instrument(InstrumentDef::try_from_sample(name, starplayer_core::SampleId(self.item as u16), U0F16::MAX)?)?;
            self.item += 1; return Ok(());
        }
        if self.item >= self.instrument_offsets.len() { self.item = 0; self.phase = Phase::Orders; return Ok(()); }
        let offset = self.instrument_offsets.get(self.item).copied().ok_or(Error::OutOfRange)? as usize; self.item += 1;
        if offset == 0 || offset.saturating_add(instrument::HEADER_LENGTH) > self.source.len() {
            self.plan.try_add_instrument(InstrumentDef::default())?; return Ok(());
        }
        let bytes = self.source.get(offset..offset + instrument::HEADER_LENGTH).ok_or(Error::OutOfRange)?;
        let name = starplayer_model::try_decode_cp437(bytes.get(0x20..0x3A).unwrap_or_default())?;
        let parsed = if header.has_old_instruments() { ItInstrument::try_parse_old(bytes, name)? } else { ItInstrument::try_parse(bytes, name)? };
        self.plan.try_add_instrument(parsed.try_to_model(self.samples.len())?)?;
        Ok(())
    }

    fn prepare_order(&mut self) -> Result<(), ImageDecodeError> {
        let header = self.file_header()?;
        if self.item >= header.order_count as usize { self.item = 0; self.phase = Phase::HeaderPlan; return Ok(()); }
        let raw = *self.source.get(header::HEADER_LENGTH + self.item).ok_or(Error::OutOfRange)?;
        self.plan.try_push_order(match raw { 255 => ORDER_END, 254 => ORDER_MARKER, value if (value as usize) < self.patterns.len() => value as u16, _ => ORDER_MARKER })?;
        self.item += 1; Ok(())
    }

    fn prepare_module_header(&mut self) -> Result<(), ImageDecodeError> {
        let header = self.file_header()?;
        let mut default_pan = Vec::new();
        if header.is_stereo() {
            default_pan.try_reserve_exact(self.channel_count as usize).map_err(|_| METADATA_RESOURCE)?;
            for raw in header.channel_pan.iter().take(self.channel_count as usize) { default_pan.push(header::pan_to_bipolar(*raw)); }
        }
        let mut default_volume = Vec::new();
        default_volume.try_reserve_exact(self.channel_count as usize).map_err(|_| METADATA_RESOURCE)?;
        for raw in header.channel_volume.iter().take(self.channel_count as usize) { default_volume.push(unit_from_ratio((*raw).min(MAX_VOLUME) as u32, MAX_VOLUME as u32)); }
        let format_data_length = header::DATA_FIXED_BYTES + self.sample_volumes.len() + self.midi_offset.map_or(0, |_| header::MIDI_CONFIGURATION_BYTES);
        let mut format_data = Vec::new();
        format_data.try_reserve_exact(format_data_length).map_err(|_| METADATA_RESOURCE)?;
        format_data.extend_from_slice(&header.tracker_version.to_le_bytes());
        format_data.extend_from_slice(&header.format_version.to_le_bytes());
        format_data.push(header.stereo_separation); format_data.push(header.pitch_wheel_depth);
        format_data.push(u8::from(self.midi_offset.is_some())); format_data.push(0);
        format_data.extend_from_slice(&header.channel_pan);
        format_data.extend_from_slice(&(self.sample_volumes.len() as u16).to_le_bytes());
        format_data.extend_from_slice(&self.sample_volumes);
        format_data.resize(format_data_length, 0);
        let extra = ItFormatExtra { flags: header.flags, special: header.special as u8, old_instruments: header.has_old_instruments(), has_midi_configuration: self.midi_offset.is_some() };
        self.pending_header = Some(ModuleHeader {
            title: starplayer_model::try_decode_cp437(self.source.get(0x04..0x1E).unwrap_or_default())?.into_boxed_str(),
            format: ModuleFormat::It, channel_count: self.channel_count,
            initial_speed: if header.initial_speed == 0 { FALLBACK_SPEED } else { header.initial_speed },
            initial_tempo: (header.initial_tempo as u16).max(MINIMUM_TEMPO),
            global_volume: unit_from_ratio(header.global_volume.min(MAX_SONG_VOLUME) as u32, MAX_SONG_VOLUME as u32),
            master_volume: unit_from_ratio(header.mix_volume.min(MAX_SONG_VOLUME) as u32, MAX_SONG_VOLUME as u32),
            default_pan: default_pan.into_boxed_slice(),
            flags: ModuleFlags { amiga_limits: false, linear_slides: header.is_linear_slides(), fast_volume_slides: false, stereo: header.is_stereo() },
            dialect: header.dialect(), format_extra: extra.encode(), default_channel_volume: default_volume.into_boxed_slice(),
            format_data: format_data.into_boxed_slice(),
        });
        self.subitem = 0; self.phase = Phase::HeaderData; Ok(())
    }

    fn copy_header_data(&mut self, max_input_bytes: usize) -> Result<(), ImageDecodeError> {
        let Some(source_offset) = self.midi_offset else {
            self.plan.set_header(self.pending_header.take().ok_or(Error::Invalid("IT header state"))?);
            self.phase = Phase::Patterns; return Ok(());
        };
        let amount = (header::MIDI_CONFIGURATION_BYTES - self.subitem).min(max_input_bytes);
        let source = self.source.get(source_offset + self.subitem..source_offset + self.subitem + amount).ok_or(Error::OutOfRange)?;
        let pending = self.pending_header.as_mut().ok_or(Error::Invalid("IT header state"))?;
        let start = header::DATA_FIXED_BYTES + self.sample_volumes.len() + self.subitem;
        pending.format_data.get_mut(start..start + amount).ok_or(Error::OutOfRange)?.copy_from_slice(source);
        self.subitem += amount;
        if self.subitem == header::MIDI_CONFIGURATION_BYTES {
            self.plan.set_header(self.pending_header.take().ok_or(Error::Invalid("IT header state"))?);
            self.subitem = 0; self.phase = Phase::Patterns;
        }
        Ok(())
    }

    fn prepare_pattern(&mut self) -> Result<(), ImageDecodeError> {
        if self.item >= self.patterns.len() { self.item = 0; self.phase = Phase::Layout; return Ok(()); }
        let extent = self.patterns.get(self.item).copied().ok_or(Error::OutOfRange)?;
        self.plan.try_add_pattern(extent.rows as usize * self.channel_count as usize * pattern::CELL_BYTES, extent.rows, self.channel_count)?;
        self.item += 1; Ok(())
    }

    fn prepare_layout(&mut self) -> Result<(), ImageDecodeError> {
        self.metadata_bytes = self.plan.metadata_bytes()?; self.blob_start = self.metadata_bytes;
        let after_blob = self.blob_start.checked_add(self.plan.blob_bytes()).ok_or(Error::TooLarge("module image"))?;
        let pcm_length_offset = after_blob.checked_add((4 - after_blob % 4) % 4).ok_or(Error::TooLarge("module image"))?;
        self.pcm_start = pcm_length_offset.checked_add(4).ok_or(Error::TooLarge("module image"))?;
        let image_length = self.pcm_start.checked_add(self.plan.pcm_frames().checked_mul(2).ok_or(Error::TooLarge("module image PCM"))?).ok_or(Error::TooLarge("module image"))?;
        if image_length > self.destination.len() { return Err(ImageDecodeError::DestinationTooSmall { required: image_length, available: self.destination.len() }); }
        self.destination.get_mut(after_blob..pcm_length_offset).ok_or(Error::OutOfRange)?.fill(0);
        self.destination.get_mut(pcm_length_offset..self.pcm_start).ok_or(Error::OutOfRange)?.copy_from_slice(&(self.plan.pcm_frames() as u32).to_le_bytes());
        self.image_length = Some(image_length); self.phase = Phase::Metadata; Ok(())
    }

    fn write_metadata(&mut self) -> Result<(), ImageDecodeError> {
        if self.metadata_part >= self.plan.metadata_part_count() {
            if self.metadata_position != self.metadata_bytes { return Err(Error::Invalid("module image metadata length changed").into()); }
            self.phase = Phase::PatternFill; return Ok(());
        }
        let written = self.plan.write_metadata_part(self.metadata_part, self.destination.get_mut(self.metadata_position..self.metadata_bytes).ok_or(Error::OutOfRange)?)?;
        self.metadata_position += written; self.metadata_part += 1; Ok(())
    }

    fn reset_unpack(&mut self) {
        self.unpack_position = 0; self.unpack_row = 0; self.unpack_mask = [0; pattern::MAX_PACKED_CHANNELS];
        self.unpack_note = [pattern::NOTE_NONE; pattern::MAX_PACKED_CHANNELS]; self.unpack_instrument = [0; pattern::MAX_PACKED_CHANNELS];
        self.unpack_volume = [pattern::VOLUME_NONE; pattern::MAX_PACKED_CHANNELS]; self.unpack_command = [0; pattern::MAX_PACKED_CHANNELS];
        self.unpack_info = [0; pattern::MAX_PACKED_CHANNELS];
    }

    fn fill_pattern(&mut self) -> Result<(), ImageDecodeError> {
        if self.item >= self.patterns.len() { self.item = 0; self.phase = Phase::PcmInit; return Ok(()); }
        let index = self.plan.patterns().get(self.item).ok_or(Error::OutOfRange)?;
        let length = index.length_bytes() as usize;
        let amount = (length - self.pattern_fill).min(4096);
        let start = self.blob_start + index.blob_offset() as usize + self.pattern_fill;
        let bytes = self.destination.get_mut(start..start + amount).ok_or(Error::OutOfRange)?;
        let empty = ItCell::EMPTY.to_bytes();
        for (offset, byte) in bytes.iter_mut().enumerate() { *byte = empty.get((self.pattern_fill + offset) % pattern::CELL_BYTES).copied().unwrap_or(0); }
        self.pattern_fill += amount;
        if self.pattern_fill == length { self.pattern_fill = 0; self.reset_unpack(); self.phase = Phase::PatternUnpack; }
        Ok(())
    }

    fn unpack_pattern(&mut self, max_input_bytes: usize) -> Result<(), ImageDecodeError> {
        let extent = self.patterns.get(self.item).copied().ok_or(Error::OutOfRange)?;
        if extent.body_length == 0 { self.item += 1; self.phase = Phase::PatternFill; return Ok(()); }
        let body = self.source.get(extent.body_offset..extent.body_offset + extent.body_length).ok_or(Error::OutOfRange)?;
        let starting = self.unpack_position;
        while self.unpack_row < extent.rows && self.unpack_position.saturating_sub(starting) < max_input_bytes.saturating_sub(7) {
            let Some(byte) = body.get(self.unpack_position).copied() else { break }; self.unpack_position += 1;
            if byte == 0 { self.unpack_row += 1; continue; }
            let Some(channel) = Self::packed_channel(byte) else { self.unpack_position = body.len(); break };
            if byte & pattern::CHANNEL_HAS_MASK != 0 {
                let mask = body.get(self.unpack_position).copied().unwrap_or(0); self.unpack_position += 1;
                *self.unpack_mask.get_mut(channel).ok_or(Error::OutOfRange)? = mask;
            }
            let mask = self.unpack_mask.get(channel).copied().unwrap_or(0);
            let pattern_index = self.plan.patterns().get(self.item).ok_or(Error::OutOfRange)?;
            let cell_offset = self.blob_start + pattern_index.blob_offset() as usize + (self.unpack_row as usize * self.channel_count as usize + channel) * pattern::CELL_BYTES;
            let mut cell = if channel < self.channel_count as usize {
                ItCell::from_bytes(self.destination.get(cell_offset..cell_offset + pattern::CELL_BYTES).unwrap_or_default()).unwrap_or(ItCell::EMPTY)
            } else { ItCell::EMPTY };
            if mask & pattern::MASK_LAST_NOTE != 0 { cell.note = self.unpack_note.get(channel).copied().unwrap_or(pattern::NOTE_NONE); }
            if mask & pattern::MASK_LAST_INSTRUMENT != 0 { cell.instrument = self.unpack_instrument.get(channel).copied().unwrap_or(0); }
            if mask & pattern::MASK_LAST_VOLUME != 0 { cell.volume = self.unpack_volume.get(channel).copied().unwrap_or(pattern::VOLUME_NONE); }
            if mask & pattern::MASK_LAST_COMMAND != 0 {
                cell.command = self.unpack_command.get(channel).copied().unwrap_or(0);
                cell.info = self.unpack_info.get(channel).copied().unwrap_or(0);
            }
            if mask & pattern::MASK_NOTE != 0 {
                cell.note = body.get(self.unpack_position).copied().map(pattern::normalise_note).unwrap_or(pattern::NOTE_NONE); self.unpack_position += 1;
                *self.unpack_note.get_mut(channel).ok_or(Error::OutOfRange)? = cell.note;
            }
            if mask & pattern::MASK_INSTRUMENT != 0 {
                cell.instrument = body.get(self.unpack_position).copied().unwrap_or(0); self.unpack_position += 1;
                *self.unpack_instrument.get_mut(channel).ok_or(Error::OutOfRange)? = cell.instrument;
            }
            if mask & pattern::MASK_VOLUME != 0 {
                cell.volume = body.get(self.unpack_position).copied().unwrap_or(pattern::VOLUME_NONE); self.unpack_position += 1;
                *self.unpack_volume.get_mut(channel).ok_or(Error::OutOfRange)? = cell.volume;
            }
            if mask & pattern::MASK_COMMAND != 0 {
                cell.command = body.get(self.unpack_position).copied().unwrap_or(0); cell.info = body.get(self.unpack_position + 1).copied().unwrap_or(0); self.unpack_position += 2;
                *self.unpack_command.get_mut(channel).ok_or(Error::OutOfRange)? = cell.command;
                *self.unpack_info.get_mut(channel).ok_or(Error::OutOfRange)? = cell.info;
            }
            if channel < self.channel_count as usize { self.destination.get_mut(cell_offset..cell_offset + pattern::CELL_BYTES).ok_or(Error::OutOfRange)?.copy_from_slice(&cell.to_bytes()); }
        }
        if self.unpack_row >= extent.rows || self.unpack_position >= body.len() { self.item += 1; self.phase = Phase::PatternFill; }
        Ok(())
    }

    fn sample_destination_start(&self, sample_index: usize) -> Result<usize, Error> {
        let sample = self.plan.samples().get(sample_index).ok_or(Error::OutOfRange)?;
        Ok(self.pcm_start + (sample.pcm_offset() as usize - PRE_ROLL_FRAMES) * 2)
    }
    fn write_frame(&mut self, frame: usize, value: i16) -> Result<(), Error> {
        let offset = self.sample_destination_start(self.item)? + frame * 2;
        self.destination.get_mut(offset..offset + 2).ok_or(Error::OutOfRange)?.copy_from_slice(&value.to_le_bytes()); Ok(())
    }
    fn read_frame(&self, frame: usize) -> Result<i16, Error> {
        let offset = self.sample_destination_start(self.item)? + frame * 2;
        let Some([low, high]) = self.destination.get(offset..offset + 2) else { return Err(Error::OutOfRange) };
        Ok(i16::from_le_bytes([*low, *high]))
    }

    fn initialize_pcm(&mut self, max_frames: usize) -> Result<(), ImageDecodeError> {
        if self.item >= self.samples.len() { self.phase = Phase::Complete; return Ok(()); }
        let stored = self.plan.samples().get(self.item).ok_or(Error::OutOfRange)?.stored_frames();
        let amount = (stored - self.pcm_position).min(max_frames);
        let start = self.sample_destination_start(self.item)? + self.pcm_position * 2;
        self.destination.get_mut(start..start + amount * 2).ok_or(Error::OutOfRange)?.fill(0);
        self.pcm_position += amount;
        if self.pcm_position == stored {
            self.pcm_position = 0; self.pcm_channel_position = 0; self.pcm_left_accumulator = 0; self.pcm_right_accumulator = 0;
            let info = self.samples.get(self.item).copied().ok_or(Error::OutOfRange)?;
            if let Some(header) = info.header.filter(|header| header.is_compressed()) {
                self.pcm_decoder = Some(IncrementalDecompressor::new(self.tail(header.data_offset as usize), info.requested_frames, header.is_sixteen_bit(), header.is_delta()));
            }
            self.phase = Phase::PcmLeft;
        }
        Ok(())
    }

    fn decode_raw_frame(&mut self, header: ItSampleHeader, channel: usize, frame: usize) -> i16 {
        let width = if header.is_sixteen_bit() { 2 } else { 1 };
        let block = self.samples.get(self.item).map(|info| info.decoded_frames).unwrap_or(0) * width;
        let offset = header.data_offset as usize + channel * block + frame * width;
        if header.is_sixteen_bit() {
            let low = self.source.get(offset).copied().unwrap_or(0); let high = self.source.get(offset + 1).copied().unwrap_or(0);
            let bytes = if header.is_big_endian() { [high, low] } else { [low, high] };
            let raw = u16::from_le_bytes(bytes);
            if header.is_delta() {
                let accumulator = if channel == 0 { &mut self.pcm_left_accumulator } else { &mut self.pcm_right_accumulator };
                *accumulator = accumulator.wrapping_add(raw as i16); *accumulator
            } else if header.is_signed() { raw as i16 } else { (raw ^ 0x8000) as i16 }
        } else {
            let silence = if header.is_signed() || header.is_delta() { 0 } else { 0x80 };
            let raw = self.source.get(offset).copied().unwrap_or(silence);
            if header.is_delta() {
                let accumulator = if channel == 0 { &mut self.pcm_left_accumulator } else { &mut self.pcm_right_accumulator };
                let next = (*accumulator as i8).wrapping_add(raw as i8); *accumulator = next as i16;
                next as i16 * 256
            } else if header.is_signed() { sample::signed8_to_i16(raw) } else { sample::unsigned8_to_i16(raw) }
        }
    }

    fn write_pcm_left(&mut self, budget: DecodeBudget) -> Result<(), ImageDecodeError> {
        let info = self.samples.get(self.item).copied().ok_or(Error::OutOfRange)?;
        let Some(header) = info.header else { self.phase = Phase::PcmGuard; self.pcm_position = 0; return Ok(()); };
        let stored_body = self.plan.samples().get(self.item).ok_or(Error::OutOfRange)?.length_frames() as usize;
        if header.is_compressed() {
            let sample_start = self.sample_destination_start(self.item)?;
            let scratch = starplayer_model::image_workspace_i16(self.workspace, budget.max_pcm_frames)?;
            let chunk = self.pcm_decoder.as_mut().ok_or(Error::Invalid("IT PCM decoder state"))?.decode_into(scratch, budget.max_input_bytes);
            for value in scratch.iter().take(chunk.written) {
                if self.pcm_channel_position < stored_body {
                    let offset = sample_start + (PRE_ROLL_FRAMES + self.pcm_channel_position) * 2;
                    self.destination.get_mut(offset..offset + 2).ok_or(Error::OutOfRange)?.copy_from_slice(&value.to_le_bytes());
                }
                self.pcm_channel_position += 1;
            }
            if !chunk.finished { return Ok(()); }
        } else {
            let amount = (info.left_frames - self.pcm_channel_position).min(budget.max_pcm_frames);
            for _ in 0..amount {
                let value = self.decode_raw_frame(header, 0, self.pcm_channel_position);
                if self.pcm_channel_position < stored_body { self.write_frame(PRE_ROLL_FRAMES + self.pcm_channel_position, value)?; }
                self.pcm_channel_position += 1;
            }
            if self.pcm_channel_position < info.left_frames { return Ok(()); }
        }
        self.pcm_channel_position = 0;
        if header.is_stereo(self.file_header()?.tracker_version) {
            if header.is_compressed() {
                self.pcm_decoder = Some(IncrementalDecompressor::new(self.tail(header.data_offset as usize + info.left_consumed), info.requested_frames, header.is_sixteen_bit(), header.is_delta()));
            }
            self.phase = Phase::PcmRight;
        } else { self.phase = Phase::PcmGuard; self.pcm_position = 0; }
        Ok(())
    }

    fn write_pcm_right(&mut self, budget: DecodeBudget) -> Result<(), ImageDecodeError> {
        let info = self.samples.get(self.item).copied().ok_or(Error::OutOfRange)?; let header = info.header.ok_or(Error::OutOfRange)?;
        let stored_body = self.plan.samples().get(self.item).ok_or(Error::OutOfRange)?.length_frames() as usize;
        if header.is_compressed() {
            let sample_start = self.sample_destination_start(self.item)?;
            let scratch = starplayer_model::image_workspace_i16(self.workspace, budget.max_pcm_frames)?;
            let chunk = self.pcm_decoder.as_mut().ok_or(Error::Invalid("IT PCM decoder state"))?.decode_into(scratch, budget.max_input_bytes);
            for right in scratch.iter().take(chunk.written) {
                if self.pcm_channel_position < stored_body {
                    let offset = sample_start + (PRE_ROLL_FRAMES + self.pcm_channel_position) * 2;
                    let left = if self.pcm_channel_position < info.left_frames {
                        let Some([low, high]) = self.destination.get(offset..offset + 2) else { return Err(Error::OutOfRange.into()) };
                        i16::from_le_bytes([*low, *high])
                    } else { 0 };
                    let value = ((left as i32 + *right as i32) / 2) as i16;
                    self.destination.get_mut(offset..offset + 2).ok_or(Error::OutOfRange)?.copy_from_slice(&value.to_le_bytes());
                }
                self.pcm_channel_position += 1;
            }
            if !chunk.finished { return Ok(()); }
        } else {
            let amount = (info.right_frames - self.pcm_channel_position).min(budget.max_pcm_frames);
            for _ in 0..amount {
                let right = self.decode_raw_frame(header, 1, self.pcm_channel_position);
                if self.pcm_channel_position < stored_body {
                    let left = self.read_frame(PRE_ROLL_FRAMES + self.pcm_channel_position)?;
                    self.write_frame(PRE_ROLL_FRAMES + self.pcm_channel_position, ((left as i32 + right as i32) / 2) as i16)?;
                }
                self.pcm_channel_position += 1;
            }
            if self.pcm_channel_position < info.right_frames { return Ok(()); }
        }
        self.pcm_position = self.pcm_channel_position; self.phase = Phase::PcmTail; Ok(())
    }

    fn finish_stereo_tail(&mut self, max_frames: usize) -> Result<(), ImageDecodeError> {
        let info = self.samples.get(self.item).copied().ok_or(Error::OutOfRange)?;
        let stored_body = self.plan.samples().get(self.item).ok_or(Error::OutOfRange)?.length_frames() as usize;
        let end = info.left_frames.min(stored_body);
        let amount = end.saturating_sub(self.pcm_position).min(max_frames);
        for _ in 0..amount {
            let frame = PRE_ROLL_FRAMES + self.pcm_position; self.write_frame(frame, (self.read_frame(frame)? as i32 / 2) as i16)?; self.pcm_position += 1;
        }
        if self.pcm_position >= end { self.pcm_position = 0; self.phase = Phase::PcmGuard; }
        Ok(())
    }

    fn write_guard(&mut self, max_frames: usize) -> Result<(), ImageDecodeError> {
        let sample = self.plan.samples().get(self.item).cloned().ok_or(Error::OutOfRange)?;
        let amount = (GUARD_FRAMES - self.pcm_position).min(max_frames);
        for _ in 0..amount {
            let value = match (sample.sustain_loop().is_some(), sample.loop_mode()) {
                (true, _) | (false, LoopMode::None) => 0,
                (false, LoopMode::Forward) => {
                    let length = sample.loop_end() - sample.loop_start();
                    self.read_frame(PRE_ROLL_FRAMES + sample.loop_start() as usize + self.pcm_position % length as usize)?
                }
                (false, LoopMode::PingPong) => {
                    let source = ping_pong_reflect(sample.loop_start(), sample.loop_end(), sample.loop_end() + self.pcm_position as u32);
                    self.read_frame(PRE_ROLL_FRAMES + source as usize)?
                }
            };
            self.write_frame(PRE_ROLL_FRAMES + sample.length_frames() as usize + self.pcm_position, value)?;
            self.pcm_position += 1;
        }
        if self.pcm_position == GUARD_FRAMES {
            self.item += 1; self.pcm_position = 0; self.pcm_decoder = None; self.phase = Phase::PcmInit;
        }
        Ok(())
    }
}

const fn mask_skip(low: u8) -> usize {
    ((low & 1 != 0) as usize) + ((low & 2 != 0) as usize) + ((low & 4 != 0) as usize) + 2 * ((low & 8 != 0) as usize)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use alloc::vec;

    const SEEDS: &[&[u8]] = &[
        include_bytes!("../../../fuzz/seeds/it/minimal.it"),
        include_bytes!("../../../fuzz/seeds/it/compressed-8bit.it"),
        include_bytes!("../../../fuzz/seeds/it/compressed-16bit.it"),
        include_bytes!("../../../fuzz/seeds/it/instrument-mode.it"),
        include_bytes!("../../../fuzz/seeds/it/maximum-counts.it"),
        include_bytes!("../../../fuzz/seeds/it/midi-macros.it"),
        include_bytes!("../../../fuzz/seeds/it/old-instruments.it"),
        include_bytes!("../../../fuzz/seeds/it/sustain-ping-pong.it"),
    ];

    fn image(source: &[u8], workspace_bytes: usize) -> Result<Vec<u8>, ImageDecodeError> {
        let expected = crate::load(source).map_err(ImageDecodeError::Module)?.to_image();
        let mut destination = vec![0u8; expected.len()];
        let mut workspace = vec![0u8; workspace_bytes];
        let mut decoder = ImageDecoder::new(source, &mut destination, &mut workspace);
        for _ in 0..2_000_000 {
            if decoder.step(DecodeBudget { max_input_bytes: 4096, max_pcm_frames: 7 })?
                == (ImageDecodeStatus::Complete { image_length: expected.len() })
            {
                drop(decoder);
                assert_eq!(destination, expected);
                return Ok(destination);
            }
        }
        Err(Error::Invalid("incremental decoder did not finish").into())
    }

    fn first_sample_header(source: &[u8]) -> usize {
        let orders = u16::from_le_bytes([source[0x20], source[0x21]]) as usize;
        let instruments = u16::from_le_bytes([source[0x22], source[0x23]]) as usize;
        let table = header::HEADER_LENGTH + orders + instruments * 4;
        u32::from_le_bytes(source[table..table + 4].try_into().expect("sample pointer")) as usize
    }

    fn compressed_block(residuals: &[u8]) -> Vec<u8> {
        let mut bits = Vec::new();
        let mut buffer = 0u32;
        let mut available = 0u32;
        for residual in residuals {
            buffer |= (*residual as u32) << available;
            available += 9;
            while available >= 8 {
                bits.push(buffer as u8);
                buffer >>= 8;
                available -= 8;
            }
        }
        if available > 0 { bits.push(buffer as u8); }
        let mut block = (bits.len() as u16).to_le_bytes().to_vec();
        block.extend_from_slice(&bits);
        block
    }

    #[test]
    fn all_it_seed_images_match_the_owned_loader() {
        for source in SEEDS { image(source, 2049).expect("seed converts"); }
    }

    #[test]
    fn compressed_decode_requires_caller_workspace() {
        let error = image(SEEDS[1], 1).expect_err("one byte cannot hold one PCM frame");
        assert!(matches!(error, ImageDecodeError::WorkspaceTooSmall { .. }));
    }

    #[test]
    fn compressed_stereo_with_a_short_right_stream_matches_the_owned_loader() {
        let mut source = SEEDS[0].to_vec();
        let sample_header = first_sample_header(&source);
        let mut compressed = compressed_block(&[1, 1, 0xFE, 3]);
        compressed.extend_from_slice(&compressed_block(&[4, 0xFC]));
        let data_offset = source.len();
        source.extend_from_slice(&compressed);
        source[sample_header + 0x12] = sample::FLAG_HAS_DATA | sample::FLAG_STEREO | sample::FLAG_COMPRESSED;
        source[sample_header + 0x2E] = 0;
        source[sample_header + 0x30..sample_header + 0x34].copy_from_slice(&4u32.to_le_bytes());
        source[sample_header + 0x48..sample_header + 0x4C].copy_from_slice(&(data_offset as u32).to_le_bytes());

        image(&source, 2049).expect("compressed stereo converts");
    }

    #[test]
    fn delta_sixteen_bit_stereo_and_ping_pong_guards_match_the_owned_loader() {
        let mut source = SEEDS[0].to_vec();
        let sample_header = first_sample_header(&source);
        let data_offset = source.len();
        let left = [100i16, 100, -50, -150];
        let right = [-20i16, 40, 60, -80];
        for delta in left.into_iter().chain(right) { source.extend_from_slice(&delta.to_le_bytes()); }
        source[sample_header + 0x12] = sample::FLAG_HAS_DATA | sample::FLAG_SIXTEEN_BIT | sample::FLAG_STEREO | sample::FLAG_LOOP | sample::FLAG_PING_PONG_LOOP;
        source[sample_header + 0x2E] = sample::CONVERT_SIGNED | sample::CONVERT_DELTA;
        source[sample_header + 0x30..sample_header + 0x34].copy_from_slice(&4u32.to_le_bytes());
        source[sample_header + 0x34..sample_header + 0x38].copy_from_slice(&1u32.to_le_bytes());
        source[sample_header + 0x38..sample_header + 0x3C].copy_from_slice(&4u32.to_le_bytes());
        source[sample_header + 0x48..sample_header + 0x4C].copy_from_slice(&(data_offset as u32).to_le_bytes());

        image(&source, 2049).expect("delta stereo converts");
    }
}
