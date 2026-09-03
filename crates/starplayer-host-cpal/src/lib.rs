//! Native audio output: the [`AudioBackend`] implementation over `cpal`.
//!
//! Everything that is not platform-specific lives in `starplayer-host` — the transport, the
//! seek mailbox, the engine arms, `Player`. This crate is only the driver: enumerate
//! devices, negotiate a configuration, open a stream, and turn the interleaved `f32` the
//! host produces into whatever sample format the device takes.
//!
//! ```no_run
//! use starplayer::engine::MixerMode;
//! use starplayer_host::{AudioSpec, Player};
//! use starplayer_host_cpal::CpalBackend;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut backend = CpalBackend::new()?;
//! let mut player = Player::open(&mut backend, None, AudioSpec::stereo(48_000), MixerMode::DEFAULT)?;
//! player.load(&std::fs::read("song.s3m")?)?;
//! player.play()?;
//! # Ok(())
//! # }
//! ```
//!
//! Allowed dependency edges: `starplayer`, `starplayer-host`, `cpal`.
//!
//! # Building on Linux
//!
//! `cpal` always builds its ALSA backend on Linux, and that needs the ALSA **headers** —
//! `sudo apt install libasound2-dev`, or the equivalent, which is the normal route and the
//! one CI takes. Without root, build `alsa-lib` from its release tarball into a prefix of
//! your own and point cargo at it:
//!
//! ```sh
//! ./configure --prefix="$PWD/target/alsa-lib" && make && make install
//! PKG_CONFIG_PATH="$PWD/target/alsa-lib/lib/pkgconfig" cargo test -p starplayer-host-cpal
//! ```
//!
//! `PKG_CONFIG_PATH` is *prepended* to pkg-config's own search path, so setting it is
//! harmless on a machine that has the system package. It is deliberately not committed to
//! `.cargo/config.toml`: it names a build output that exists only where somebody built it.
//!
//! # Which host, and why PulseAudio is compiled in
//!
//! [`CpalBackend::new`] takes `cpal::default_host()`, which on Linux takes PulseAudio when a
//! server is reachable and ALSA otherwise. (It would prefer PipeWire ahead of both, but that
//! host is behind a cpal feature that binds `libpipewire`, and a PipeWire server answers on
//! the PulseAudio socket regardless.) The `pulseaudio` feature is on in the workspace pin because it
//! costs no system dependency — the `pulseaudio` crate speaks the protocol itself rather
//! than binding `libpulse` — and because ALSA alone finds no real device on a machine with
//! no sound card, which is every container and every WSL2 box. On WSL2 with WSLg the ALSA
//! host enumerates only the `null` device, while the PulseAudio host finds the real sink.

#![forbid(unsafe_code)]

use std::boxed::Box;
use std::string::ToString;
use std::sync::Arc;
use std::vec::Vec;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, ErrorKind, SampleFormat, StreamConfig, SupportedBufferSize, SupportedStreamConfig};
use starplayer::mixer::{Dither, HostSample};
use starplayer_host::{AudioBackend, AudioSpec, DeviceInfo, HostError, RenderCallback, Stream, StreamControl, StreamHealth};

/// Frames the `i16` conversion scratch holds.
///
/// A device may ask for far more than it advertises — PulseAudio will happily hand over a
/// third of a second — so the conversion walks a long block in chunks of this rather than
/// sizing itself from a promise. 8192 frames is 170 ms at 48 kHz; the allocation is 64 kB
/// and it happens once, when the stream opens.
pub const CONVERSION_SCRATCH_FRAMES: usize = 8_192;

/// Sample formats this backend will open a device in.
///
/// `f32` because the host already produces it and the device then takes it unchanged; `i16`
/// because it is what a great many devices — including WSLg's PulseAudio sink — offer as
/// their default.
pub const SUPPORTED_FORMATS: [SampleFormat; 2] = [SampleFormat::F32, SampleFormat::I16];

/// The rates `--list-devices` probes a device's advertised ranges for.
const COMMON_RATES: [u32; 11] = [8_000, 11_025, 16_000, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

/// Which sample format to try first when a device offers both.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum PreferredFormat {
    /// Hand the device the host's own `f32`, unconverted.
    #[default]
    Float,
    /// Convert to `i16` in the callback, with [`HostSample`].
    Int16,
}

impl PreferredFormat {
    /// The two formats, most wanted first.
    pub const fn order(self) -> [SampleFormat; 2] {
        match self {
            PreferredFormat::Float => [SampleFormat::F32, SampleFormat::I16],
            PreferredFormat::Int16 => [SampleFormat::I16, SampleFormat::F32],
        }
    }
}

fn backend_error(error: cpal::Error) -> HostError { HostError::Backend(error.to_string()) }

/// Whether an error means the stream is dead and has to be rebuilt, as opposed to a glitch
/// it carries on through.
fn is_fatal(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::DeviceNotAvailable
            | ErrorKind::StreamInvalidated
            | ErrorKind::PermissionDenied
            | ErrorKind::UnsupportedConfig
            | ErrorKind::UnsupportedOperation
            | ErrorKind::InvalidInput
    )
}

/// What a device agreed to, in both vocabularies.
struct Chosen {
    spec: AudioSpec,
    config: StreamConfig,
    sample_format: SampleFormat,
}

/// Pick a configuration `device` will actually accept for `requested`.
///
/// The rate is honoured when the device advertises a range containing it, and replaced by
/// the device's own default when it does not — which is the whole reason
/// [`AudioBackend::negotiate`] is a separate call from `open`: the answer decides the rate
/// the engine is built at and the rate the song is scanned at (architecture §4.1), and a
/// host that guessed would have to rebuild both on its first callback.
fn choose(device: &cpal::Device, requested: AudioSpec, preferred: PreferredFormat) -> Result<Chosen, HostError> {
    let default = device.default_output_config().map_err(backend_error)?;

    let mut best: Option<SupportedStreamConfig> = None;
    if let Ok(ranges) = device.supported_output_configs() {
        let candidates: Vec<_> = ranges.collect();
        'search: for format in preferred.order() {
            for range in &candidates {
                if range.channels() == requested.channels
                    && range.sample_format() == format
                    && range.contains_rate(requested.sample_rate_hz)
                {
                    best = range.try_with_sample_rate(requested.sample_rate_hz);
                    if best.is_some() {
                        break 'search;
                    }
                }
            }
        }
    }

    // Nothing matched, so take the device's own default and report what it turned out to
    // be. A caller that cares compares `Stream::spec` with what it asked for.
    let supported = best.unwrap_or(default);
    if !SUPPORTED_FORMATS.contains(&supported.sample_format()) {
        let reason = std::format!("this host renders f32 and i16, the device offers {:?}", supported.sample_format());
        return Err(HostError::UnsupportedSpec { requested, reason });
    }
    if supported.channels() != 1 && supported.channels() != 2 {
        let reason = std::format!("this host renders mono and stereo, the device wants {} channels", supported.channels());
        return Err(HostError::UnsupportedSpec { requested, reason });
    }

    let buffer_size = match (requested.preferred_block_frames, supported.buffer_size()) {
        (Some(frames), SupportedBufferSize::Range { min, max }) if frames >= *min && frames <= *max => BufferSize::Fixed(frames),
        (Some(frames), SupportedBufferSize::Unknown) => BufferSize::Fixed(frames),
        _ => BufferSize::Default,
    };
    let spec = AudioSpec {
        sample_rate_hz: supported.sample_rate(),
        channels: supported.channels(),
        preferred_block_frames: match buffer_size {
            BufferSize::Fixed(frames) => Some(frames),
            BufferSize::Default => None,
        },
    };
    let config = StreamConfig { channels: supported.channels(), sample_rate: supported.sample_rate(), buffer_size };
    Ok(Chosen { spec, config, sample_format: supported.sample_format() })
}

/// Turns the host's interleaved `f32` into the `i16` a device asked for.
///
/// The conversion is [`HostSample`]'s, so it clamps rather than wrapping — a loud module
/// sounds clipped, never inverted — and it runs with dither **off**: the mixer mode's own
/// depth post-stage has already decided what resolution the signal carries, and dithering a
/// second time here would add noise describing precision that is not there.
///
/// Nothing in [`Int16Writer::write`] allocates: the scratch is sized when the stream opens
/// and a block longer than it is walked in chunks.
struct Int16Writer {
    callback: RenderCallback,
    scratch: Vec<f32>,
    dither: Dither,
}

impl Int16Writer {
    fn new(callback: RenderCallback, channels: usize) -> Int16Writer {
        Int16Writer {
            callback,
            scratch: std::vec![0.0; CONVERSION_SCRATCH_FRAMES.saturating_mul(channels.max(1))],
            dither: Dither::OFF,
        }
    }

    fn write(&mut self, output: &mut [i16]) {
        for block in output.chunks_mut(self.scratch.len().max(1)) {
            let Some(scratch) = self.scratch.get_mut(..block.len()) else { break };
            scratch.fill(0.0);
            (self.callback)(scratch);
            for (destination, sample) in block.iter_mut().zip(scratch.iter()) {
                *destination = <i16 as HostSample>::from_unit_f32(*sample, &mut self.dither);
            }
        }
    }
}

/// The open cpal stream, kept alive for as long as the host holds its [`Stream`].
struct CpalStreamControl {
    stream: cpal::Stream,
}

impl StreamControl for CpalStreamControl {
    fn play(&self) -> Result<(), HostError> { self.stream.play().map_err(backend_error) }

    fn pause(&self) -> Result<(), HostError> { self.stream.pause().map_err(backend_error) }
}

/// Native audio output through `cpal`.
pub struct CpalBackend {
    host: cpal::Host,
    preferred_format: PreferredFormat,
}

impl CpalBackend {
    /// The platform's default host — PipeWire, then PulseAudio, then ALSA on Linux; WASAPI
    /// on Windows; CoreAudio on macOS.
    pub fn new() -> Result<CpalBackend, HostError> {
        Ok(CpalBackend { host: cpal::default_host(), preferred_format: PreferredFormat::default() })
    }

    /// A backend on one named host, for a caller that wants ALSA specifically on a machine
    /// where PulseAudio is also running.
    pub fn with_host(id: cpal::HostId) -> Result<CpalBackend, HostError> {
        let host = cpal::host_from_id(id).map_err(backend_error)?;
        Ok(CpalBackend { host, preferred_format: PreferredFormat::default() })
    }

    /// Which host this backend is on: "ALSA", "PulseAudio", "WASAPI".
    pub fn host_name(&self) -> &'static str { self.host.id().name() }

    /// Choose which sample format to try first when a device offers both.
    pub fn set_preferred_format(&mut self, preferred: PreferredFormat) { self.preferred_format = preferred; }

    /// Find `name` among this host's output devices, or the default when `None`.
    ///
    /// An exact name first, then a case-insensitive substring, so `--device rdp` finds
    /// "RDP Sink" without anybody having to quote it.
    fn find(&self, name: Option<&str>) -> Result<cpal::Device, HostError> {
        let Some(name) = name else {
            return self.host.default_output_device().ok_or(HostError::NoDevice);
        };
        let devices: Vec<cpal::Device> = self.host.output_devices().map_err(backend_error)?.collect();
        if let Some(device) = devices.iter().find(|device| device.to_string() == name) {
            return Ok(device.clone());
        }
        let wanted = name.to_lowercase();
        devices
            .iter()
            .find(|device| device.to_string().to_lowercase().contains(&wanted))
            .cloned()
            .ok_or_else(|| HostError::UnknownDevice(name.to_string()))
    }

    fn describe(&self, device: &cpal::Device, default_name: Option<&str>) -> DeviceInfo {
        let name = device.to_string();
        let mut supported_rates: Vec<u32> = Vec::new();
        if let Ok(ranges) = device.supported_output_configs() {
            for range in ranges {
                for rate in COMMON_RATES {
                    if range.contains_rate(rate) && !supported_rates.contains(&rate) {
                        supported_rates.push(rate);
                    }
                }
            }
        }
        supported_rates.sort_unstable();
        DeviceInfo {
            is_default: default_name.is_some_and(|default| default == name),
            name,
            supported_rates,
            backend: self.host_name().to_string(),
        }
    }
}

impl AudioBackend for CpalBackend {
    fn devices(&self) -> Vec<DeviceInfo> {
        let default_name = self.host.default_output_device().map(|device| device.to_string());
        let Ok(devices) = self.host.output_devices() else { return Vec::new() };
        devices.map(|device| self.describe(&device, default_name.as_deref())).collect()
    }

    fn negotiate(&self, device: Option<&str>, requested: AudioSpec) -> Result<AudioSpec, HostError> {
        Ok(choose(&self.find(device)?, requested, self.preferred_format)?.spec)
    }

    fn open(&mut self, device: Option<&str>, spec: AudioSpec, mut callback: RenderCallback) -> Result<Stream, HostError> {
        let device = self.find(device)?;
        let chosen = choose(&device, spec, self.preferred_format)?;

        let health = StreamHealth::new();
        let error_health = Arc::clone(&health);
        let on_error = move |error: cpal::Error| error_health.record(is_fatal(error.kind()));

        let stream = match chosen.sample_format {
            SampleFormat::F32 => device
                .build_output_stream::<f32, _, _>(chosen.config, move |data, _| callback(data), on_error, None)
                .map_err(backend_error)?,
            SampleFormat::I16 => {
                let mut writer = Int16Writer::new(callback, chosen.config.channels as usize);
                device
                    .build_output_stream::<i16, _, _>(chosen.config, move |data, _| writer.write(data), on_error, None)
                    .map_err(backend_error)?
            }
            other => {
                let reason = std::format!("this host renders f32 and i16, not {other:?}");
                return Err(HostError::UnsupportedSpec { requested: spec, reason });
            }
        };
        Ok(Stream::new(chosen.spec, health, Box::new(CpalStreamControl { stream })))
    }
}

/// Every output device on **every** host `cpal` can reach, not just the default one.
///
/// What `--list-devices` prints. A machine commonly has more than one host — PulseAudio and
/// ALSA both, on Linux — and the device somebody wants is often not on the default one.
pub fn all_devices() -> Vec<DeviceInfo> {
    let mut devices = Vec::new();
    for id in cpal::available_hosts() {
        let Ok(backend) = CpalBackend::with_host(id) else { continue };
        devices.extend(backend.devices());
    }
    devices
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The device-format conversion, driven directly. No device is involved, which is the
    /// point: it is the one piece of this crate with arithmetic in it.
    #[test]
    fn the_i16_writer_clamps_rather_than_wrapping() {
        let mut writer = Int16Writer::new(
            Box::new(|output: &mut [f32]| {
                for (index, sample) in output.iter_mut().enumerate() {
                    *sample = match index % 4 {
                        0 => 0.0,
                        1 => 1.0,
                        2 => -1.0,
                        _ => 4.0,
                    };
                }
            }),
            2,
        );

        for length in [2usize, 8, 512] {
            let mut output = std::vec![0i16; length];
            writer.write(&mut output);
            for (index, sample) in output.iter().enumerate() {
                let expected = match index % 4 {
                    0 => 0,
                    1 => 32_767,
                    2 => -32_767,
                    _ => 32_767,
                };
                assert_eq!(*sample, expected, "sample {index} of a {length}-sample block");
            }
        }
    }

    /// A block longer than the scratch is walked in chunks, so a device that asks for a
    /// third of a second at once gets a whole correct block rather than a scratch-sized one.
    #[test]
    fn a_block_longer_than_the_scratch_is_still_filled_end_to_end() {
        let mut next = 0i32;
        let mut writer = Int16Writer::new(
            Box::new(move |output: &mut [f32]| {
                for sample in output.iter_mut() {
                    *sample = next as f32 / 32_767.0;
                    next = (next + 1) % 32_768;
                }
            }),
            1,
        );

        let length = CONVERSION_SCRATCH_FRAMES * 3 + 17;
        let mut output = std::vec![0i16; length];
        writer.write(&mut output);
        for (index, sample) in output.iter().enumerate() {
            assert_eq!(*sample, (index % 32_768) as i16, "sample {index}");
        }
    }

    #[test]
    fn a_dead_device_is_fatal_and_an_underrun_is_not() {
        assert!(is_fatal(ErrorKind::DeviceNotAvailable));
        assert!(is_fatal(ErrorKind::StreamInvalidated));
        assert!(!is_fatal(ErrorKind::Xrun), "an underrun is a glitch, not a dead stream");
        assert!(!is_fatal(ErrorKind::BackendError));
    }

    #[test]
    fn the_preferred_format_orders_the_two_this_host_renders() {
        assert_eq!(PreferredFormat::Float.order(), [SampleFormat::F32, SampleFormat::I16]);
        assert_eq!(PreferredFormat::Int16.order(), [SampleFormat::I16, SampleFormat::F32]);
        assert_eq!(PreferredFormat::default(), PreferredFormat::Float);
    }

    /// Enumeration has to work, or fail cleanly, on a machine with no sound card at all —
    /// which is every CI runner. It must never panic.
    #[test]
    fn enumerating_devices_never_panics_even_with_no_sound_card() {
        for device in &all_devices() {
            assert!(!device.name.is_empty(), "a listed device has a name");
            assert!(!device.backend.is_empty());
        }
        if let Ok(backend) = CpalBackend::new() {
            assert!(!backend.host_name().is_empty());
            // Either there is a device or there is not; both are legal, and asking must not
            // be an error either way.
            let _ = backend.devices();
            // Negotiating against a name nothing has is an error, never a panic.
            assert!(backend.negotiate(Some("\u{1f600} no such device"), AudioSpec::stereo(48_000)).is_err());
        }
    }
}
