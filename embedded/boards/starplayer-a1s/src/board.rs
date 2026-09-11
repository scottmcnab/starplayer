//! The AI-Thinker ESP32-Audio-Kit, as pins.
//!
//! Every GPIO number on this board appears **once**, here, as a named constant, and
//! [`Board::take`] is the only place peripherals are claimed. A driver module takes a
//! typed handle out of [`Board`] and never learns a pin number.
//!
//! # The map
//!
//! | Function | GPIO | Note |
//! |---|---|---|
//! | I2C SDA / SCL | 33 / 32 | ES8388 control, 7-bit address `0x10` |
//! | I2S MCLK | 0 | `CLK_OUT1`; a strapping pin, so MCLK only appears after boot |
//! | I2S BCLK / LRCK | 27 / 25 | |
//! | I2S DOUT (ESP → codec DSDIN) | 26 | |
//! | I2S DIN (codec ASDOUT → ESP) | 35 | unused here; input-only pin |
//! | PA enable (speaker amplifier) | 21 | drive high for the two speaker outputs |
//! | Headphone detect | 39 | input-only; low when a jack is inserted |
//! | KEY1–KEY6 | 36, 13, 19, 23, 18, 5 | M8-I5; KEY1 is on an input-only pin |
//! | LED4 / LED5 | 22 / 19 | LED5 shares KEY3 |
//! | SD-card group (the display, M8-I5) | 14 SCK, 13 MOSI, 15 CS, 2, 4, 12 | 12 and 15 are strapping pins; **13 is also KEY2** |
//!
//! This is the map every Arduino and ESP-ADF port of the v2.2 Audio Kit uses. It is
//! **research point 1** to confirm it against the board in hand: the firmware logs an I2C
//! scan at boot for exactly that purpose, and the ES8388 answers at `0x10` where the older
//! AC101 revision of the board — out of scope for this milestone — answers at `0x1a`.
//!
//! # Strapping pins, and why the order of operations matters
//!
//! GPIO0 is the chip's boot-mode strap: held low across a reset, the ROM enters UART
//! download mode and the application never runs. It is safe as an *output* only after
//! boot, which is exactly when [`Board::take`] hands it to the I2S driver as MCLK. Never
//! configure it before `esp_hal::init` returns, and never drive it from a `Board` built in
//! a constructor that could run at reset.
//!
//! GPIO12 and GPIO15 are the flash-voltage and JTAG straps; they are in the SD-card group,
//! and M8-I5's display claims them. Nothing here touches them.

// The pin map is this module's whole purpose and is complete on purpose: the keys, the
// LEDs and the I2S data-in pin are M8-I5's and have no consumer yet, and a map with holes
// in it would be worse than a few unused constants. Same for `AsyncI2c`, which names the
// type the display will want.
#![allow(dead_code)]

use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::peripherals::{GPIO21, GPIO32, GPIO33, GPIO39, I2C0};
use esp_hal::time::Rate;
use esp_hal::{Async, Blocking};

/// The codec's 7-bit I2C address with its `CE` pin tied low, which is how the Audio Kit
/// wires it. (ESP-ADF's `ES8388_ADDR` is `0x20`, the 8-bit write form of the same
/// address; `embedded-hal` takes the 7-bit one.)
pub const ES8388_I2C_ADDRESS: u8 = 0x10;

/// The address the older AC101 revision of this board answers at. Nothing drives it; the
/// boot-time scan names it so a `0x1a` in the scan output identifies the board rather than
/// looking like a mystery.
pub const AC101_I2C_ADDRESS: u8 = 0x1a;

/// I2C bus rate. 100 kHz rather than 400: the codec is configured once at boot, so the
/// slower rate costs nothing and tolerates the board's long control-bus traces.
pub const I2C_RATE_HZ: u32 = 100_000;

/// Codec control — `SDA`.
pub const PIN_I2C_SDA: u8 = 33;
/// Codec control — `SCL`.
pub const PIN_I2C_SCL: u8 = 32;
/// I2S master clock, `CLK_OUT1`. Also the boot-mode strap; see the module docs.
pub const PIN_I2S_MCLK: u8 = 0;
/// I2S bit clock.
pub const PIN_I2S_BCLK: u8 = 27;
/// I2S word select (LRCK).
pub const PIN_I2S_LRCK: u8 = 25;
/// I2S data out, ESP → codec `DSDIN`.
pub const PIN_I2S_DOUT: u8 = 26;
/// I2S data in, codec `ASDOUT` → ESP. Unused: this milestone has no audio input.
pub const PIN_I2S_DIN: u8 = 35;
/// Speaker amplifier enable. High turns the two speaker outputs on.
pub const PIN_PA_ENABLE: u8 = 21;
/// Headphone-jack detect, input only.
pub const PIN_HEADPHONE_DETECT: u8 = 39;

/// The six push-buttons, KEY1 first. M8-I5's business; listed here so the one place that
/// knows GPIO numbers knows all of them.
pub const PIN_KEYS: [u8; 6] = [36, 13, 19, 23, 18, 5];

/// The two user LEDs, LED4 then LED5. LED5 shares GPIO19 with KEY3, so a build that
/// drives it loses that key.
pub const PIN_LEDS: [u8; 2] = [22, 19];

/// The board's own peripherals: the codec control bus and the two discrete signals.
///
/// It deliberately does **not** own I2S. That driver needs `I2S0`, a DMA channel and four
/// pins *together*, and handing them out one at a time would let two callers build two
/// drivers over one peripheral; `audio::start` takes them as a unit instead. Splitting
/// `Peripherals` at the call site — one field per argument — is also what keeps this
/// module free of `unsafe`: nothing here clones a peripheral singleton.
pub struct Board<'d> {
    /// The codec control bus, already configured.
    pub i2c: I2c<'d, Blocking>,
    /// Speaker-amplifier enable, starting **low** — silence until something asks for
    /// sound.
    pub power_amplifier: Output<'d>,
    /// Headphone-jack detect.
    pub headphone_detect: Input<'d>,
}

/// What [`Board::take`] could not do.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BoardError {
    /// The I2C controller rejected its configuration. Unreachable with the constants
    /// above; it exists so board set-up contains no `unwrap`, because a panic on a device
    /// is a reset.
    I2cConfig,
}

impl Board<'static> {
    /// Claim the codec bus, the amplifier enable and the headphone detect.
    ///
    /// Called exactly once, from `main`, **after** `esp_hal::init` — GPIO0 is the
    /// boot-mode strap and GPIO12/15 are the flash-voltage and JTAG straps, so nothing on
    /// this board may be configured before the ROM has finished with them.
    pub fn take(
        i2c0: I2C0<'static>,
        sda: GPIO33<'static>,
        scl: GPIO32<'static>,
        power_amplifier: GPIO21<'static>,
        headphone_detect: GPIO39<'static>,
    ) -> Result<Board<'static>, BoardError> {
        let i2c = I2c::new(i2c0, I2cConfig::default().with_frequency(Rate::from_hz(I2C_RATE_HZ)))
            .map_err(|_| BoardError::I2cConfig)?
            .with_sda(sda)
            .with_scl(scl);

        Ok(Board {
            i2c,
            power_amplifier: Output::new(power_amplifier, Level::Low, OutputConfig::default()),
            // Pulled up: the jack shorts the pin to ground when a plug is inserted, and
            // GPIO39 is input-only with no internal pull of its own on some revisions —
            // asking for one costs nothing where it exists and is ignored where it does not.
            headphone_detect: Input::new(headphone_detect, InputConfig::default().with_pull(Pull::Up)),
        })
    }
}

/// Marker for the async I2C the display (M8-I5) will want. Unused today; named so the
/// type appears in exactly one place when it is.
pub type AsyncI2c<'d> = I2c<'d, Async>;
