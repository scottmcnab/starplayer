//! Renders a [`NowPlaying`] onto an `embedded-graphics` [`DrawTarget`] — board-independent
//! (M8-I5 deliverable 3), so it is testable on the host against
//! [`embedded_graphics::mock_display::MockDisplay`] with no board and no SPI bus. The
//! board crate's `lcd.rs` is only the driver glue: an `esp_hal` SPI bus wrapped in
//! `mipidsi`, which also implements `DrawTarget<Color = Rgb565>` and is handed to
//! [`Screen::render`] unmodified.
//!
//! # Layout, at 240×280
//!
//! ```text
//! ┌──────────────────────────────────────────┐  y=0
//! │ <title, truncated to the panel width>     │  header line 1
//! │ ord ppp rr/rr  sss bpm   v.vv             │  header line 2
//! ├──────────────────────────────────────────┤  y=HEADER_HEIGHT
//! │ 01 I05 C-5 ▓▓▓▓░░░░░░░░░░░░  change speed │  one row per channel,
//! │ 02 ... ...                                │  ROW_HEIGHT px tall,
//! │ …                                         │  up to MAX_DISPLAY_CHANNELS
//! └──────────────────────────────────────────┘
//! ```
//!
//! # Dirty-row redraws
//!
//! [`Screen`] keeps the last [`NowPlaying`] it drew and only repaints the header when a
//! header field changed and only repaints a channel row when that row's fields (or its
//! peak-hold marker, which decays every frame while a channel is loud) changed — the task
//! file's "only dirty rows are redrawn" budget. A channel that has been silent since the
//! last frame costs nothing.
//!
//! # What is deliberately simplified
//!
//! The original's exit screen (`STARPLAY/STAR.ASM`'s `DrawStarAnsi`) is ANSI art with no
//! practical pixel-for-pixel translation to a 240×280 panel, and reproducing it exactly
//! was not this task's point (`plans/reference/original-star-ui.md` §12 only asks for
//! "the star logo" as a *motif* to keep, alongside the banner). [`draw_stopped_placeholder`]
//! draws a small schematic star instead of the ANSI original — a fair-use nod rather than
//! a reproduction, and a reasonable place for a follow-up to spend more polish.

use alloc::format;

use embedded_graphics::mono_font::ascii::{FONT_5X8, FONT_6X10};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;

use crate::now_playing::{ChannelRow, MAX_DISPLAY_CHANNELS, NowPlaying};

/// The panel's native width, in pixels — the 1.69" ST7789's 240×280 window (M8-I5
/// research point 2's 20-row y-offset is the board driver's business, not this crate's;
/// everything here draws in panel-local coordinates starting at `(0, 0)`).
pub const SCREEN_WIDTH: i32 = 240;
/// The panel's native height, in pixels.
pub const SCREEN_HEIGHT: i32 = 280;
/// One channel row's height. `MAX_DISPLAY_CHANNELS` of these plus [`HEADER_HEIGHT`] must
/// not exceed [`SCREEN_HEIGHT`]: `28 + 16 * 14 = 252`, comfortably inside 280.
pub const ROW_HEIGHT: i32 = 14;
/// Two 14 px text lines: the title and the transport line.
pub const HEADER_HEIGHT: i32 = 28;
/// Cells in a VU bar, matching [`starplayer_telemetry::VuMeter`]'s original 0..64 scale
/// quantised to sixteenths (`ChannelRow::vu` is already `0..=16`).
pub const VU_CELLS: u8 = 16;
const VU_CELL_WIDTH: i32 = 3;
const VU_CELL_GAP: i32 = 1;
const VU_CELL_HEIGHT: i32 = 8;
/// Where a channel row's VU bar starts, in panel-local x.
pub const VU_BAR_X: i32 = 84;

const COLOR_BACKGROUND: Rgb565 = Rgb565::BLACK;
const COLOR_TEXT: Rgb565 = Rgb565::WHITE;
const COLOR_TEXT_DIM: Rgb565 = Rgb565::new(10, 20, 10); // a dim grey-green for a silent channel's row

/// One VU cell's colour, green→yellow→red bottom to top of the bar — the original's
/// gradient (`plans/reference/original-star-ui.md` §12).
pub fn vu_cell_color(cell_index: u8) -> Rgb565 {
    match cell_index {
        0..=10 => Rgb565::GREEN,
        11..=13 => Rgb565::YELLOW,
        _ => Rgb565::RED,
    }
}

/// Draw one 16-cell VU bar at `origin`, with `lit_cells` filled from the bottom (well,
/// left — the bar is horizontal) and an optional single-cell peak-hold marker.
pub fn draw_vu_bar<D>(target: &mut D, origin: Point, lit_cells: u8, peak_cell: Option<u8>) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    for cell in 0..VU_CELLS {
        let x = origin.x + i32::from(cell) * (VU_CELL_WIDTH + VU_CELL_GAP);
        let rectangle = Rectangle::new(Point::new(x, origin.y), Size::new(VU_CELL_WIDTH as u32, VU_CELL_HEIGHT as u32));
        let lit = cell < lit_cells || peak_cell == Some(cell);
        let color = if lit { vu_cell_color(cell) } else { COLOR_BACKGROUND };
        rectangle.into_styled(PrimitiveStyle::with_fill(color)).draw(target)?;
    }
    Ok(())
}

/// A small schematic star, drawn centred at `center` with the given radius — the stopped
/// screen's placeholder for `DrawStarAnsi` (see the module docs' "What is deliberately
/// simplified").
fn draw_stopped_placeholder<D>(target: &mut D, center: Point, radius: i32) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let style = PrimitiveStyle::with_stroke(Rgb565::YELLOW, 1);
    // Six spokes rather than a true five-point star outline: `embedded-graphics` 0.8 has
    // no polygon primitive, and six evenly spaced `Line`s read as a recognisable star
    // burst without hand-rolling one.
    for spoke in 0..6 {
        // Fixed-point sine/cosine table for 0/60/120/180/240/300 degrees, ×1000 — no
        // trigonometry in this crate (it is not the audio path, but there is no reason to
        // reach for `libm` for six constants either).
        const COS_1000: [i32; 6] = [1000, 500, -500, -1000, -500, 500];
        const SIN_1000: [i32; 6] = [0, 866, 866, 0, -866, -866];
        let end = Point::new(center.x + radius * COS_1000[spoke] / 1000, center.y + radius * SIN_1000[spoke] / 1000);
        Line::new(center, end).into_styled(style).draw(target)?;
    }
    Ok(())
}

fn clear<D>(target: &mut D, area: Rectangle) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    area.into_styled(PrimitiveStyle::with_fill(COLOR_BACKGROUND)).draw(target)
}

fn draw_header<D>(target: &mut D, view: &NowPlaying) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    clear(target, Rectangle::new(Point::zero(), Size::new(SCREEN_WIDTH as u32, HEADER_HEIGHT as u32)))?;
    let style = MonoTextStyle::new(&FONT_6X10, COLOR_TEXT);
    Text::new(view.title.as_str(), Point::new(2, 9), style).draw(target)?;

    let elapsed = view.elapsed_seconds;
    let total = match view.total_seconds {
        Some(seconds) => format!("{}:{:02}", seconds / 60, seconds % 60),
        None => alloc::string::String::from("--:--"),
    };
    let volume_percent = (u32::from(view.volume.to_bits()) * 100) / (u32::from(u16::MAX) + 1);
    let line2 = format!(
        "ord{:03} pat{:03} row{:02}/{:02} {:3}bpm vol{:3}% {}:{:02}/{}",
        view.order, view.pattern, view.row, view.speed, view.tempo_bpm, volume_percent,
        elapsed / 60, elapsed % 60, total,
    );
    Text::new(&line2, Point::new(2, HEADER_HEIGHT - 5), style).draw(target).map(|_| ())
}

fn draw_channel_row<D>(target: &mut D, y: i32, slot: usize, row: &ChannelRow, peak_cell: Option<u8>) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    clear(target, Rectangle::new(Point::new(0, y), Size::new(SCREEN_WIDTH as u32, ROW_HEIGHT as u32)))?;
    let style = MonoTextStyle::new(&FONT_5X8, if row.active { COLOR_TEXT } else { COLOR_TEXT_DIM });
    let note = core::str::from_utf8(&row.note).unwrap_or("???");
    let label = format!("{:02} I{:02} {}", slot + 1, row.instrument, note);
    Text::new(&label, Point::new(2, y + ROW_HEIGHT - 4), style).draw(target)?;
    draw_vu_bar(target, Point::new(VU_BAR_X, y + (ROW_HEIGHT - VU_CELL_HEIGHT) / 2), row.vu, peak_cell)?;
    let effect_x = VU_BAR_X + i32::from(VU_CELLS) * (VU_CELL_WIDTH + VU_CELL_GAP) + 4;
    Text::new(row.effect_name, Point::new(effect_x, y + ROW_HEIGHT - 4), style).draw(target).map(|_| ())
}

/// A `Screen` owns the dirty-row comparison and the VU peak-hold state; it holds nothing
/// board-specific, so the same value drives the real panel and a host test's
/// `MockDisplay` alike.
pub struct Screen {
    previous: Option<NowPlaying>,
    peaks: [u8; MAX_DISPLAY_CHANNELS],
}

impl Screen {
    /// A screen that has never drawn anything — the next [`Screen::render`] repaints
    /// everything.
    pub const fn new() -> Screen { Screen { previous: None, peaks: [0; MAX_DISPLAY_CHANNELS] } }

    /// Redraw whatever changed since the last call, and clear the target on the very
    /// first one.
    pub fn render<D>(&mut self, target: &mut D, view: &NowPlaying) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let first_frame = self.previous.is_none();
        if first_frame {
            clear(target, Rectangle::new(Point::zero(), Size::new(SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32)))?;
        }

        let header_dirty = first_frame
            || self.previous.is_some_and(|previous| {
                previous.title != view.title
                    || previous.order != view.order
                    || previous.pattern != view.pattern
                    || previous.row != view.row
                    || previous.speed != view.speed
                    || previous.tempo_bpm != view.tempo_bpm
                    || previous.volume != view.volume
                    || previous.elapsed_seconds != view.elapsed_seconds
                    || previous.total_seconds != view.total_seconds
            });
        if header_dirty {
            draw_header(target, view)?;
        }

        let stopped = view.channel_count == 0;
        let was_stopped = first_frame || self.previous.is_some_and(|previous| previous.channel_count == 0);
        if stopped {
            if !was_stopped || first_frame {
                clear(target, Rectangle::new(Point::new(0, HEADER_HEIGHT), Size::new(SCREEN_WIDTH as u32, (SCREEN_HEIGHT - HEADER_HEIGHT) as u32)))?;
                draw_stopped_placeholder(target, Point::new(SCREEN_WIDTH / 2, HEADER_HEIGHT + (SCREEN_HEIGHT - HEADER_HEIGHT) / 2), 40)?;
            }
        } else {
            let count = view.displayed_channels().len();
            for slot in 0..count {
                let row = &view.channels[slot];
                let previous_row = self.previous.as_ref().map(|previous| previous.channels[slot]);

                let peak = &mut self.peaks[slot];
                *peak = if row.vu >= *peak { row.vu } else { peak.saturating_sub(1) };
                let peak_cell = if *peak > 0 { Some(*peak - 1) } else { None };
                let peak_changed = previous_row.is_none_or(|previous_row| {
                    let previous_peak = previous_row.vu; // approximate: a redraw when the level itself moved covers the marker too
                    previous_peak != row.vu
                });

                let dirty = first_frame || was_stopped || previous_row != Some(*row) || peak_changed;
                if dirty {
                    let y = HEADER_HEIGHT + (slot as i32) * ROW_HEIGHT;
                    draw_channel_row(target, y, slot, row, peak_cell)?;
                }
            }
        }

        self.previous = Some(*view);
        Ok(())
    }
}

impl Default for Screen {
    fn default() -> Screen { Screen::new() }
}

#[cfg(test)]
mod tests {
    use embedded_graphics::mock_display::MockDisplay;
    use embedded_graphics::pixelcolor::Rgb565;

    use super::*;
    use crate::now_playing::NowPlaying;

    #[test]
    fn vu_cells_are_green_then_yellow_then_red() {
        assert_eq!(vu_cell_color(0), Rgb565::GREEN);
        assert_eq!(vu_cell_color(10), Rgb565::GREEN);
        assert_eq!(vu_cell_color(11), Rgb565::YELLOW);
        assert_eq!(vu_cell_color(13), Rgb565::YELLOW);
        assert_eq!(vu_cell_color(14), Rgb565::RED);
        assert_eq!(vu_cell_color(15), Rgb565::RED);
    }

    #[test]
    fn a_vu_bar_at_zero_lights_no_cells() {
        let mut display: MockDisplay<Rgb565> = MockDisplay::new();
        display.set_allow_out_of_bounds_drawing(true);
        draw_vu_bar(&mut display, Point::zero(), 0, None).unwrap();
        // Every drawn pixel is the background colour — nothing reads as "lit".
        for x in 0..(i32::from(VU_CELLS) * (VU_CELL_WIDTH + VU_CELL_GAP)) {
            if let Some(color) = display.get_pixel(Point::new(x, 0)) {
                assert_eq!(color, COLOR_BACKGROUND);
            }
        }
    }

    #[test]
    fn a_vu_bar_lights_exactly_the_requested_cells() {
        let mut display: MockDisplay<Rgb565> = MockDisplay::new();
        display.set_allow_out_of_bounds_drawing(true);
        draw_vu_bar(&mut display, Point::zero(), 3, None).unwrap();
        // Cell 0 (green) is lit; cell 3 (still within the mock's 64px width for these
        // narrow cells) is not.
        assert_eq!(display.get_pixel(Point::new(1, 4)), Some(Rgb565::GREEN));
        let cell_3_x = 3 * (VU_CELL_WIDTH + VU_CELL_GAP) + 1;
        assert_eq!(display.get_pixel(Point::new(cell_3_x, 4)), Some(COLOR_BACKGROUND));
    }

    #[test]
    fn a_peak_marker_lights_its_cell_even_past_the_live_level() {
        let mut display: MockDisplay<Rgb565> = MockDisplay::new();
        display.set_allow_out_of_bounds_drawing(true);
        draw_vu_bar(&mut display, Point::zero(), 2, Some(5)).unwrap();
        let cell_5_x = 5 * (VU_CELL_WIDTH + VU_CELL_GAP) + 1;
        assert_eq!(display.get_pixel(Point::new(cell_5_x, 4)), Some(vu_cell_color(5)));
    }

    /// A `DrawTarget` over the panel's full 240×280 area that only counts how many
    /// `Rectangle`/`Text` draw calls reached it, standing in for the real ST7789 driver.
    /// `MockDisplay`'s internal framebuffer is capped at 64×64 (its own documentation),
    /// too small to hold a whole frame of this screen, so the dirty-row tests below
    /// count fills instead of asserting exact pixel patterns.
    struct CountingDisplay {
        fills: u32,
    }

    impl OriginDimensions for CountingDisplay {
        fn size(&self) -> Size { Size::new(SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32) }
    }

    impl DrawTarget for CountingDisplay {
        type Color = Rgb565;
        type Error = core::convert::Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            self.fills += pixels.into_iter().count() as u32;
            Ok(())
        }
    }

    fn playing_view(vu: u8) -> NowPlaying {
        let mut view = NowPlaying::from_snapshot(&starplayer_telemetry::Snapshot::IDLE, crate::SAMPLE_RATE_HZ, "Petri", starplayer::core::U0F16::MAX);
        view.channel_count = 1;
        view.channels[0].active = true;
        view.channels[0].vu = vu;
        view.channels[0].effect_name = "change speed";
        view
    }

    #[test]
    fn the_first_render_draws_the_whole_screen() {
        let mut display = CountingDisplay { fills: 0 };
        let mut screen = Screen::new();
        screen.render(&mut display, &playing_view(4)).unwrap();
        assert!(display.fills > 0, "the first frame must draw something");
    }

    #[test]
    fn an_unchanged_view_redraws_nothing_once_the_peak_has_settled() {
        let mut display = CountingDisplay { fills: 0 };
        let mut screen = Screen::new();
        let view = playing_view(0);
        screen.render(&mut display, &view).unwrap(); // first frame: draws everything
        screen.render(&mut display, &view).unwrap(); // peak (0) is already settled at 0
        let after_settle = display.fills;
        screen.render(&mut display, &view).unwrap(); // still identical and settled
        assert_eq!(display.fills, after_settle, "a settled, unchanged view must not redraw");
    }

    #[test]
    fn a_changed_row_redraws_but_a_stable_one_does_not() {
        let mut display = CountingDisplay { fills: 0 };
        let mut screen = Screen::new();
        let mut view = playing_view(8);
        // Let the peak settle to the live level over enough frames.
        for _ in 0..20 {
            screen.render(&mut display, &view).unwrap();
        }
        let settled = display.fills;
        screen.render(&mut display, &view).unwrap();
        assert_eq!(display.fills, settled, "an unchanged, settled row must not redraw");

        view.channels[0].vu = 16;
        screen.render(&mut display, &view).unwrap();
        assert!(display.fills > settled, "a channel whose VU actually moved must redraw");
    }

    #[test]
    fn a_stopped_view_clears_the_channel_area_once_and_then_leaves_it_alone() {
        let mut display = CountingDisplay { fills: 0 };
        let mut screen = Screen::new();
        let stopped = NowPlaying::default();
        screen.render(&mut display, &stopped).unwrap();
        let after_first = display.fills;
        screen.render(&mut display, &stopped).unwrap();
        assert_eq!(display.fills, after_first, "a screen that is already showing 'stopped' must not redraw it every frame");
    }
}
