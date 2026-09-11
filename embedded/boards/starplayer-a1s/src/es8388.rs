//! A minimal ES8388 driver: DAC playback only, over blocking `embedded-hal` 1.0 I2C.
//!
//! # Why this file exists rather than a dependency
//!
//! **Research point 2, resolved: there is no ES8388 crate.** The crates.io registry API
//! returns `total: 0` for `es8388`; `lib.rs` finds nothing; `esp-rs/esp-hal-community`
//! carries a buzzer and a smart-LED driver and no codecs. The nearest published relatives
//! are `es8311` 0.1.1 and `es7210` 0.1.0 (both `embedded-hal` 1.0, both `no_std`, both
//! different silicon with different register maps). One unpublished driver exists —
//! `github.com/hi-squeaky-things/driver_es8388`, last touched 2024-03-30 — but it names
//! `esp-hal 0.16` as a hard, non-optional dependency of the *library*, which makes it
//! neither portable nor current. So: forty register writes, written out here.
//!
//! # Where the register sequence comes from
//!
//! ESP-ADF's `es8388_init()` on the `release/v2.x` branch —
//! `components/audio_hal/driver/es8388/es8388.c` — cross-checked against the same driver's
//! new home on `master` (`components/esp_codec_dev/device/es8388/es8388.c`, byte-identical
//! init), the register-name map in `es8388.h`, the **ES8388 datasheet Rev 5.0 (July 2018)**
//! for the bit fields, and the **ES8388 User Guide (2011-06-17)** for the vendor's own
//! playback bring-up flow. Four places where the sources disagree are decided below and
//! the decision is recorded beside the write.
//!
//! Note that the path named in the M8-I3 task file —
//! `esp-adf/blob/master/components/audio_hal/driver/es8388/es8388.c` — **404s**: the driver
//! moved out of `audio_hal` on `master`. `release/v2.x` is where the classic file still
//! lives.
//!
//! # The four decisions
//!
//! 1. **`MASTERMODE` (`0x08`) = `0x00`, codec as I2S slave.** The reset default is `0x80`
//!    — *master* — so omitting this write leaves the codec driving BCLK and LRCK against
//!    the ESP32, which is also driving them. ESP-ADF writes `cfg->i2s_iface.mode`, and
//!    `AUDIO_HAL_MODE_SLAVE` is `0x00`.
//! 2. **`DACCONTROL21` (`0x2b`) = `0x80`, not `0xc0`.** Bit 7 `slrck` makes the ADC and
//!    DAC share one LRCK; bit 6 `lrck_sel` chooses *which*, and the datasheet and the User
//!    Guide both say it must be 0 (and that it only has an effect in master mode anyway).
//!    ESP-ADF writes `0xc0` only in its analog line-in bypass path. Ports that write
//!    `0xc0` for playback are "correcting" a misleading ADF comment and are wrong.
//! 3. **`LDACVOL`/`RDACVOL` (`0x1a`/`0x1b`) are written explicitly.** They **reset to
//!    `0xc0` = −96 dB**, so a driver that leaves them alone produces silence and no error.
//!    This is the single most likely way to bring a codec up perfectly and hear nothing.
//! 4. **The output-enable bits come from the datasheet, not from ESP-ADF.** ADF's
//!    `es_dac_output_t` has `DAC_OUTPUT_LOUT1 = 0x04` and `DAC_OUTPUT_ROUT2 = 0x20`
//!    transposed with respect to the datasheet's Register 4 (`bit5 = LOUT1`, `bit4 =
//!    ROUT1`, `bit3 = LOUT2`, `bit2 = ROUT2`). It is invisible for "all four outputs"
//!    (`0x3c` either way) and wrong the moment a build wants the headphone pair alone, so
//!    [`Outputs`] spells the datasheet's bits.
//!
//! The three undocumented writes ADF makes at `0x35`, `0x37` and `0x39` ("disable the
//! internal DLL to improve 8K sample rate") are **omitted**: they lie past the datasheet's
//! documented map, which stops at `0x34`, and this firmware runs at 44 100 Hz.
//!
//! # Clocking
//!
//! The ES8388 has no PLL. MCLK is its only clock source, so the ESP32 must output it —
//! on this board GPIO0 through `CLK_OUT1`, which `audio.rs` sets up. `DACCONTROL2`
//! (`0x18`) is written `0x02`, meaning MCLK/LRCK = 256; in *slave* mode the codec
//! auto-detects the ratio and the write is advisory, but it costs one byte and is correct
//! if the roles are ever swapped.
//!
//! # Real-time
//!
//! Every method here blocks on I2C. None of them may be called from the DMA refill: the
//! codec is configured once at boot, and a volume change from a control task is a
//! millisecond of bus traffic on a thread that can afford it.

// `set_volume_attenuation` and `release` have no caller in the audio build: volume is
// M8-I5's (the keys) and M8-I6's (the web page), and `release` is for a build that puts a
// second device on the control bus. A codec driver that only exposed what today's `main`
// happens to call would have to be reopened for each of them.
#![allow(dead_code)]

use embedded_hal::i2c::I2c;

/// Registers this driver writes, by the names ESP-ADF's `es8388.h` gives them.
///
/// Only the ones used are listed. The numbering is the datasheet's: `CONTROL1` is
/// register 0 and the map runs to `DACCONTROL30` at `0x34`.
mod register {
    /// Chip control 1: `SameFs`, `VMIDSEL`, `EnRef`.
    pub const CONTROL1: u8 = 0x00;
    /// Chip control 2: the analog, bias and Vref power-down bits.
    pub const CONTROL2: u8 = 0x01;
    /// Digital block resets and power-downs.
    pub const CHIPPOWER: u8 = 0x02;
    /// DAC power-down bits and the four output-driver enables.
    pub const DACPOWER: u8 = 0x04;
    /// I2S master/slave select, BCLK divider and inversion.
    pub const MASTERMODE: u8 = 0x08;
    /// DAC serial format: word length and I2S/left/right/DSP.
    pub const DACCONTROL1: u8 = 0x17;
    /// DAC MCLK-to-LRCK ratio.
    pub const DACCONTROL2: u8 = 0x18;
    /// DAC mute and soft-ramp control.
    pub const DACCONTROL3: u8 = 0x19;
    /// Left digital DAC volume, 0.5 dB per step, `0x00` = 0 dB.
    pub const LDACVOL: u8 = 0x1a;
    /// Right digital DAC volume.
    pub const RDACVOL: u8 = 0x1b;
    /// Output mixer input select (`LMIXSEL`/`RMIXSEL`).
    pub const DACCONTROL16: u8 = 0x26;
    /// Left mixer routing: `LD2LO`, `LI2LO`.
    pub const DACCONTROL17: u8 = 0x27;
    /// Right mixer routing: `RD2RO`, `RI2RO`.
    pub const DACCONTROL20: u8 = 0x2a;
    /// Shared-LRCK select and the MCLK/DLL disables.
    pub const DACCONTROL21: u8 = 0x2b;
    /// `VROI`: the Vref-to-output resistance.
    pub const DACCONTROL23: u8 = 0x2d;
    /// `LOUT1VOL` — analog headphone-left output volume.
    pub const LOUT1VOL: u8 = 0x2e;
    /// `ROUT1VOL`.
    pub const ROUT1VOL: u8 = 0x2f;
    /// `LOUT2VOL` — analog speaker-left output volume.
    pub const LOUT2VOL: u8 = 0x30;
    /// `ROUT2VOL`.
    pub const ROUT2VOL: u8 = 0x31;
}

/// `DACPOWER` (`0x04`) output-driver enables, **as the datasheet numbers them**.
///
/// Not ESP-ADF's `es_dac_output_t`, whose `LOUT1` and `ROUT2` are transposed — see the
/// module docs, decision 4.
pub mod outputs {
    /// Headphone left.
    pub const LOUT1: u8 = 0x20;
    /// Headphone right.
    pub const ROUT1: u8 = 0x10;
    /// Speaker left.
    pub const LOUT2: u8 = 0x08;
    /// Speaker right.
    pub const ROUT2: u8 = 0x04;
    /// Headphone pair only.
    pub const HEADPHONE: u8 = LOUT1 | ROUT1;
    /// Speaker pair only.
    pub const SPEAKER: u8 = LOUT2 | ROUT2;
    /// All four — what this firmware enables, so the jack and the on-board amplifier both
    /// work without a build flag.
    pub const ALL: u8 = HEADPHONE | SPEAKER;
}

/// The analog output-volume register value for 0 dB.
///
/// `LOUT1VOL`/`ROUT1VOL` and their speaker siblings run −45 dB … +4.5 dB in 1.5 dB steps,
/// so `0x00` is −45 dB and `0x1e` (30) is unity. ESP-ADF writes exactly this.
const ANALOG_VOLUME_0DB: u8 = 0x1e;

/// `DACCONTROL3`'s mute bit.
const DAC_MUTE_BIT: u8 = 0x04;

/// The digital-volume register value for −96 dB, which is also the reset value of
/// `LDACVOL`/`RDACVOL` and therefore the reason decision 3 exists.
const DIGITAL_VOLUME_SILENT: u8 = 0xc0;

/// What went wrong talking to the codec.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The I2C transfer failed: no acknowledgement, a bus fault, or arbitration lost.
    /// Carries the register the driver was writing, because "the codec did not answer" and
    /// "the codec stopped answering at register `0x2b`" are different problems.
    Bus(u8),
}

impl core::fmt::Display for Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Bus(register) => write!(formatter, "ES8388 I2C write to register 0x{register:02x} failed"),
        }
    }
}

/// A blocking ES8388 on an `embedded-hal` 1.0 bus.
///
/// Holds a shadow of `DACCONTROL3` so [`mute`](Es8388::mute) is a single write rather than
/// a read-modify-write — one fewer bus transaction on a path a control task may call, and
/// one fewer way for a failed read to leave the chip in a state the driver disagrees with.
pub struct Es8388<Bus> {
    bus: Bus,
    address: u8,
    dac_control_3: u8,
}

impl<Bus: I2c> Es8388<Bus> {
    /// Wrap a bus and an address. Touches no register until [`init_dac_only`](Es8388::init_dac_only).
    pub fn new(bus: Bus, address: u8) -> Es8388<Bus> {
        Es8388 { bus, address, dac_control_3: DAC_MUTE_BIT }
    }

    /// Probe every 7-bit address and report which ones acknowledged, as a 128-bit map.
    ///
    /// **Research point 1's instrument.** A zero-length write is the conventional probe: a
    /// device that is present acknowledges its address byte and a device that is absent
    /// does not, and neither case changes any register. The A1S should answer at `0x10`
    /// (ES8388); an answer at `0x1a` instead means the older AC101 revision, which this
    /// milestone does not support.
    ///
    /// The map is `[u64; 2]`, bit `n` of word `n / 64` for address `n`, so a caller can
    /// print it without allocating.
    pub fn scan(bus: &mut Bus) -> [u64; 2] {
        let mut found = [0u64; 2];
        // 0x00-0x07 and 0x78-0x7f are reserved by the I2C specification; probing them
        // upsets some devices (0x00 is the general call) and can find nothing useful.
        for address in 0x08u8..0x78 {
            if bus.write(address, &[]).is_ok() {
                found[usize::from(address) / 64] |= 1u64 << (address % 64);
            }
        }
        found
    }

    /// Configure the codec for DAC playback: I2S slave, Philips, 16-bit, MCLK/LRCK = 256,
    /// both DACs into both mixers, `outputs` enabled, 0 dB everywhere, and **still muted**.
    ///
    /// The chip is left muted deliberately. The DMA ring has nothing in it at the moment
    /// the codec comes up, and an unmuted codec fed an empty ring is how a bring-up
    /// produces a click loud enough to be remembered. The caller unmutes once audio is
    /// flowing — see `audio::start`.
    ///
    /// The sequence is ESP-ADF's `es8388_init()` with the ADC rows and the three
    /// undocumented DLL writes removed, in ADF's order. Order matters at three points:
    /// mute first, take the digital blocks out of reset before configuring them, and enable
    /// the output drivers last.
    pub fn init_dac_only(&mut self, outputs: u8) -> Result<(), Error> {
        // Mute before anything else moves.
        self.dac_control_3 = DAC_MUTE_BIT;
        self.write(register::DACCONTROL3, self.dac_control_3)?;

        // Analog, bias generator and Vref buffer powered up; low-power Vref buffer on.
        // (Reset value is 0x5c.)
        self.write(register::CONTROL2, 0x50)?;
        // Every digital block out of reset and powered.
        self.write(register::CHIPPOWER, 0x00)?;
        // I2S **slave**. The reset default is 0x80 — master — so this write is mandatory.
        self.write(register::MASTERMODE, 0x00)?;
        // DACs and all four output drivers off while the format is configured.
        self.write(register::DACPOWER, 0xc0)?;
        // `SameFs` = 1, `VMIDSEL` = 10 (500 kΩ divider). ESP-ADF's value; the vendor's own
        // flow writes 0x05 (`EnRef` = 1, 50 kΩ) instead, which is the value to try if the
        // output pops or the Vmid ramp is audibly slow.
        self.write(register::CONTROL1, 0x12)?;
        // `DACWL` = 011 (16-bit), `DACFORMAT` = 00 (I2S Philips). ESP-ADF reaches the same
        // byte through `es8388_config_fmt` (mask 0xf9, `ES_I2S_NORMAL` = 0) and
        // `es8388_set_bits_per_sample` (mask 0xc7, `BIT_LENGTH_16BITS` = 3 → 3 << 3).
        self.write(register::DACCONTROL1, 0x18)?;
        // Single speed, MCLK/LRCK = 256.
        self.write(register::DACCONTROL2, 0x02)?;
        // Mixer input select: LIN1/RIN1. Irrelevant while the line-in mix bits are 0, and
        // written for the same reason ADF writes it — so the register is known, not
        // inherited.
        self.write(register::DACCONTROL16, 0x00)?;
        // `LD2LO` = 1: left DAC into the left mixer, line-in not mixed.
        self.write(register::DACCONTROL17, 0x90)?;
        // `RD2RO` = 1: right DAC into the right mixer.
        self.write(register::DACCONTROL20, 0x90)?;
        // `slrck` = 1: one LRCK for ADC and DAC, and it is the DAC's (`lrck_sel` = 0).
        self.write(register::DACCONTROL21, 0x80)?;
        // `VROI` = 0: 1.5 kΩ Vref-to-output.
        self.write(register::DACCONTROL23, 0x00)?;

        // Digital 0 dB. These reset to 0xc0 (−96 dB); see decision 3.
        self.write(register::LDACVOL, 0x00)?;
        self.write(register::RDACVOL, 0x00)?;

        // Analog 0 dB on all four outputs. ESP-ADF leaves the speaker pair at −45 dB
        // (0x00); this board has a speaker amplifier behind `LOUT2`/`ROUT2` and a PA-enable
        // pin that already decides whether the speakers make sound, so the volume register
        // is not the place to also decide it.
        self.write(register::LOUT1VOL, ANALOG_VOLUME_0DB)?;
        self.write(register::ROUT1VOL, ANALOG_VOLUME_0DB)?;
        self.write(register::LOUT2VOL, ANALOG_VOLUME_0DB)?;
        self.write(register::ROUT2VOL, ANALOG_VOLUME_0DB)?;

        // DACs on, and the requested output drivers with them. Last, so nothing is driven
        // while the format is half-configured.
        self.write(register::DACPOWER, outputs & outputs::ALL)?;
        Ok(())
    }

    /// Set the digital DAC volume, in units of −0.5 dB from unity.
    ///
    /// `0` is 0 dB and `0xc0` (192) is −96 dB, which is the register's own scale: the
    /// ES8388's `LDACVOL`/`RDACVOL` are linear in decibels at half a decibel per step.
    /// Values above `0xc0` are clamped to silence rather than wrapping into loud.
    ///
    /// Deliberately **not** ESP-ADF's `es8388_set_voice_volume(0..100)`. That function's
    /// mapping runs through `audio_volume.c`'s board table and subtracts a
    /// `BOARD_PA_GAIN` the Audio Kit — not an official ADF board — has no published value
    /// for; the result never reaches 0 dB at volume 100 and could not be explained in a
    /// budget document. A host-side volume curve belongs to the host.
    pub fn set_volume_attenuation(&mut self, half_decibels: u8) -> Result<(), Error> {
        let value = half_decibels.min(DIGITAL_VOLUME_SILENT);
        self.write(register::LDACVOL, value)?;
        self.write(register::RDACVOL, value)
    }

    /// Mute or unmute the DAC — `DACCONTROL3` bit 2, from the driver's shadow.
    pub fn mute(&mut self, muted: bool) -> Result<(), Error> {
        self.dac_control_3 = if muted { self.dac_control_3 | DAC_MUTE_BIT } else { self.dac_control_3 & !DAC_MUTE_BIT };
        self.write(register::DACCONTROL3, self.dac_control_3)
    }

    /// Give the bus back, for a caller that wants to talk to something else on it.
    pub fn release(self) -> Bus { self.bus }

    fn write(&mut self, register: u8, value: u8) -> Result<(), Error> {
        self.bus.write(self.address, &[register, value]).map_err(|_| Error::Bus(register))
    }
}
