//! The ST7789 now-playing screen, `#[cfg(feature = "lcd")]` only.
//!
//! This is the **only** file in the board crate that names `mipidsi` or
//! `embedded-graphics`'s SPI/driver types (M8-I5 deliverable 3): everything about *what*
//! is drawn lives in `firmware_common::screen`, host-tested against `MockDisplay`; this
//! module is the esp-hal SPI bus plus the mipidsi driver that turns `Screen::render`
//! calls into real pixels, and the fail-soft wrapper that keeps a loose or absent panel
//! from ever panicking the firmware — the same shape ampkeeper's
//! `esp32/firmware/src/display.rs` uses for its I2C LCD backpack (`LcdDisplay`, a
//! `NoopDisplay` counterpart, an `Option`-wrapped driver that goes headless on the first
//! error rather than ever unwrapping).
//!
//! # Pins (HSPI, `board.rs`)
//!
//! SCK 14, MOSI 13, CS 15, DC 2, RST 4 — the SD-card pin group (master-plan decision 4).
//! GPIO12 is never touched (research point 1's "do not put a pull-up on GPIO12"; nothing
//! in this wiring needs it).
//!
//! # Versions (M8-I5 research point 2)
//!
//! `mipidsi = "0.9.0"` and `embedded-graphics = "=0.8.1"`, **not** the newer `mipidsi
//! 0.10.0` / `embedded-graphics 0.8.2` — see the long comment in `embedded/Cargo.toml`'s
//! `[workspace.dependencies]`: the newer pair tightens its `az`/`fixed` requirements to a
//! range that cannot be satisfied alongside `starplayer-core`'s `fixed = "1.31"` at all
//! (`cargo` refuses to resolve a single `az` version). Both mipidsi releases carry their
//! **own** SPI interface (`mipidsi::interface::SpiInterface`), so `display-interface-spi`
//! is not a dependency here either way.
//!
//! The panel is the 1.69" 240×280 ST7789 module: a 240×320 ST7789 RAM windowed to
//! 240×280 with a 20-row y-offset. The offset and the orientation flags are the owner's
//! to confirm with a test pattern (research point 2) — [`Y_OFFSET`] and [`ORIENTATION`]
//! are the two constants to change if the picture is shifted or mirrored.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::peripherals::{GPIO2, GPIO4, GPIO13, GPIO14, GPIO15, SPI2};
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::Blocking;
use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::options::{ColorOrder, Orientation};
use mipidsi::{Builder, Display};
use static_cell::StaticCell;

/// SPI clock. 20 MHz rather than the panel's documented 40 MHz ceiling — research point 3
/// says jumper leads (rather than a soldered connection) may need the lower rate; the
/// owner's full-frame-redraw timing measurement is what settles whether 40 MHz is safe on
/// the board in hand.
pub const SPI_FREQUENCY_HZ: u32 = 20_000_000;

/// The panel's y-offset into the ST7789's 240×320 RAM (research point 2).
pub const Y_OFFSET: u16 = 20;

/// The panel's orientation. Landscape/mirroring is a wiring-dependent guess until the
/// owner runs a test pattern (research point 2); this is the one constant to flip if the
/// picture comes up rotated or mirrored.
pub const ORIENTATION: Orientation = Orientation::new();

/// The SPI buffer `mipidsi::interface::SpiInterface` batches pixel writes into. Sized for
/// one channel row's worth of `Rgb565` pixels (`ROW_HEIGHT` × a comfortable row width ×
/// 2 bytes) so a single dirty row's fill does not fragment across many small SPI
/// transactions; the header and full-screen clears simply flush it more than once.
const SPI_BUFFER_BYTES: usize = 512;

static SPI_BUFFER: StaticCell<[u8; SPI_BUFFER_BYTES]> = StaticCell::new();

type Bus = Spi<'static, Blocking>;
type ChipSelect = Output<'static>;
type SpiDevice = ExclusiveDevice<Bus, ChipSelect, Delay>;
type DataCommand = Output<'static>;
type Interface = SpiInterface<'static, SpiDevice, DataCommand>;
type Reset = Output<'static>;
type Panel = Display<Interface, ST7789, Reset>;

/// The four pins and the SPI peripheral the display needs, claimed together the same way
/// `audio::Parts` claims I2S — one struct so nothing else can build a second driver over
/// the same peripheral.
pub struct Parts {
    /// The HSPI (`SPI2`) controller.
    pub spi2: SPI2<'static>,
    /// GPIO14 — SCK.
    pub sck: GPIO14<'static>,
    /// GPIO13 — MOSI. Also KEY2 in the six-key build; unavailable to it here.
    pub mosi: GPIO13<'static>,
    /// GPIO15 — CS.
    pub cs: GPIO15<'static>,
    /// GPIO2 — DC.
    pub dc: GPIO2<'static>,
    /// GPIO4 — RST.
    pub rst: GPIO4<'static>,
}

/// The now-playing screen: a `mipidsi` panel when one answered at boot, silently absent
/// otherwise. Every [`LcdDisplay`] method that touches the panel is infallible and drops
/// the driver on the first bus error, so a loose lead never panics the firmware (design
/// goal 5 in spirit — this is not the audio path, but a display task that can wedge the
/// board is just as unwelcome).
pub struct LcdDisplay {
    panel: Option<Panel>,
}

impl LcdDisplay {
    /// Bring the SPI bus and the panel up. Never panics: any failure — the SPI controller
    /// rejecting its configuration, the panel not acknowledging its init sequence — leaves
    /// [`LcdDisplay`] with no panel, and every draw call becomes a no-op.
    pub fn take(parts: Parts) -> LcdDisplay {
        let Parts { spi2, sck, mosi, cs, dc, rst } = parts;

        let Ok(spi) = Spi::new(spi2, SpiConfig::default().with_frequency(Rate::from_hz(SPI_FREQUENCY_HZ))) else {
            return LcdDisplay { panel: None };
        };
        let spi = spi.with_sck(sck).with_mosi(mosi);

        // Idle high: the panel only listens to MOSI while CS is low.
        let cs_pin = Output::new(cs, Level::High, OutputConfig::default());
        // Software-driven CS through `embedded-hal-bus`'s `ExclusiveDevice`
        // (`esp_hal::spi::master::Spi` implements `SpiBus`, not `SpiDevice`, directly —
        // see the module docs in `embedded/Cargo.toml`). `ExclusiveDevice::new` sets CS
        // inactive as part of construction; `esp_hal::gpio::Output`'s `Error` is
        // `Infallible`, so this cannot actually fail — `expect` says so rather than
        // threading a headless fallback through a branch that can never be taken.
        let spi_device = ExclusiveDevice::new(spi, cs_pin, Delay::new()).expect("esp_hal::gpio::Output's Error is Infallible");

        let dc_pin = Output::new(dc, Level::Low, OutputConfig::default());
        // GPIO4 has no strapping role, so RST starts high (inactive) with no ceremony —
        // `Builder::init` below pulses it low itself.
        let rst_pin = Output::new(rst, Level::High, OutputConfig::default());

        let buffer = SPI_BUFFER.init([0u8; SPI_BUFFER_BYTES]);
        let interface = SpiInterface::new(spi_device, dc_pin, buffer);

        let mut delay = Delay::new();
        let panel = Builder::new(ST7789, interface)
            .display_size(firmware_common::screen::SCREEN_WIDTH as u16, firmware_common::screen::SCREEN_HEIGHT as u16)
            .display_offset(0, Y_OFFSET)
            .orientation(ORIENTATION)
            .color_order(ColorOrder::Rgb)
            .reset_pin(rst_pin)
            .init(&mut delay)
            .ok();

        LcdDisplay { panel }
    }

    /// Whether a panel answered at boot.
    pub fn is_present(&self) -> bool { self.panel.is_some() }
}

impl OriginDimensions for LcdDisplay {
    fn size(&self) -> Size { Size::new(firmware_common::screen::SCREEN_WIDTH as u32, firmware_common::screen::SCREEN_HEIGHT as u32) }
}

impl DrawTarget for LcdDisplay {
    type Color = Rgb565;
    /// Infallible on purpose: a bus error drops the panel (below) rather than surfacing an
    /// `Err` `Screen::render` would have to decide what to do with.
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        let Some(panel) = self.panel.as_mut() else { return Ok(()) };
        if panel.draw_iter(pixels).is_err() {
            self.panel = None;
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &embedded_graphics::primitives::Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let Some(panel) = self.panel.as_mut() else { return Ok(()) };
        if panel.fill_solid(area, color).is_err() {
            self.panel = None;
        }
        Ok(())
    }
}
