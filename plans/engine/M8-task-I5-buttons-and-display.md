# M8 — I5: Controls and the display — six keys and an ST7789 now-playing screen (`lcd` feature)

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](M8-master-plan.md), decision 4; deliverable 7) |
| Status | Planned 2026-09-11; pulled |
| Depends on | I3 (board crate, `EmbeddedPlayer` on the device, `firmware-common`) |
| Blocks | — |
| Parallel with | I4, I6 |
| Recommended model | Claude Sonnet (GPIO polling and an embedded-graphics screen over well-trodden crates; the only judgement calls are the pin conflicts) |
| Verified by | agent (host tests for the key state machine and the view model; the `lcd` and default builds both compile), then owner on hardware |

## Context for a fresh agent

After I3 the A1S plays one module from boot with no way to stop it. This task adds the
six push-buttons as transport and volume controls, and — **behind a build-time feature
`lcd`, off by default** (master-plan decision 4) — a now-playing screen on the owner's
1.69" 240×280 ST7789 SPI display, wired to the SD-card pin group. The audio build must
keep working with no display attached, which is why the feature exists; ampkeeper's
`lcd` feature (`../ampkeeper/esp32/firmware/src/{display,lcd,screen}.rs`) is the pattern:
a `NoopDisplay` when the feature is off, one `#[cfg(feature = "lcd")]` driver module,
and a view model that is built and host-tested regardless.

What the screen shows is the engine's telemetry snapshot
(`starplayer_telemetry::Snapshot`): the **sounding** order/pattern/row (the original
STAR.EXE's `_MActual*` latch, already handled engine-side), speed and BPM, and one row per
channel with the instrument, note, volume, a VU bar from `vu_level`, and the effect
**spelled out in English** via `EffectDisplay.name` — the original's most charming idea,
per `plans/reference/original-star-ui.md` §12 and the A1 plan. `VuMeter` already decays at
the original's 2/64 per tick. This is the third consumer of the telemetry API after the
web page and the CLI, which is the point at which it stops being designed for one UI.

Pin conflicts are real on this board and are the task's only design work:

- **KEY2 is GPIO13, which is also the SD group's MOSI.** With the display on HSPI, either
  KEY2 is given up or MOSI moves to another SD-group pin. Research point 1 settles it from
  the schematic and the DIP switches; the default answer is *MOSI on 13, KEY2 sacrificed
  when `lcd` is on* — the key map below has a five-key variant.
- GPIO12 and GPIO15 are strapping pins (12 selects flash voltage; driving it high at reset
  on a 3.3 V module bricks boot). Do not put a pull-up on 12; prefer 15 for CS (weak
  pull-up is what its strap wants) and keep 12 unused or as DC with no pull.
- KEY1 is GPIO36 and headphone-detect is GPIO39: input-only, no internal pull-ups; the
  board provides external ones (verify).

### Code you must read before changing anything

- `embedded/boards/starplayer-a1s/src/{board,main,audio}.rs` (I3) — `Board::take`, the
  task layout, the core pinning; the control task where key events become mailbox
  commands.
- `crates/starplayer-host-embedded/src/lib.rs` (I1) — `ControlHalf`: `play`, `stop`,
  `seek_order`, `set_master_volume`, `telemetry()`.
- `crates/starplayer-telemetry/src/{snapshot,vu}.rs` — `Snapshot`, `ChannelState`,
  `EffectDisplay`, `VuMeter` constants.
- `../ampkeeper/esp32/firmware/src/esp32_main.rs::poll_button` and `BUTTON_POLL_INTERVAL`
  — the polling + edge + hold pattern (100 ms there; 10 ms here for musical controls).
- `../ampkeeper/esp32/firmware/src/{display,lcd,screen}.rs` — the feature-gated display
  arrangement to mirror.
- `plans/reference/original-star-ui.md` §12 and `plans/apps/A1-master-plan.md` "What to
  take from the original" — the layout to echo at 240×280.
- `apps/starplayer-cli/src/play.rs::report` — the one-line status the CLI prints; the
  screen's header row is that.

## Deliverables

### 1. Keys (`boards/starplayer-a1s/src/keys.rs`, always built)

A `Keys` type over six `esp_hal::gpio::Input`s polled every 10 ms from a control task:
per-key debounce (two consecutive agreeing samples), edge detection, hold timing.
Emits `KeyEvent::{Press(Key), Hold(Key, Duration), Release(Key)}` onto an
`embassy_sync::channel`. The state machine is a plain `no_std` struct in
`firmware-common` fed with `(now, [bool; 6])` so it is host-tested.

Key map (six-key build):

| Key | GPIO | Press | Hold ≥ 1 s |
|---|---|---|---|
| KEY1 | 36 | play / pause | — |
| KEY2 | 13 | stop (rewind to order 0) | I6: re-provision (≥ 5 s) |
| KEY3 | 19 | previous order | seek back one order per 200 ms |
| KEY4 | 23 | next order | seek forward one order per 200 ms |
| KEY5 | 18 | volume − (1/16 steps) | repeat |
| KEY6 | 5 | volume + | repeat |

Five-key variant when `lcd` takes GPIO13: KEY1 short = play/pause, KEY1 long = stop.
Volume is applied through `set_master_volume`; the level persists in RAM only.

### 2. The view model (`firmware-common/src/now_playing.rs`, always built)

`NowPlaying::from_snapshot(&Snapshot, title: &str, volume: U0F16) -> NowPlaying`: a
fixed-capacity, `Copy`-able struct (heapless strings) holding the header line
(`title`, `order/pattern/row`, `speed`, `bpm`, `voices`), and up to 16 channel rows
(`instrument`, note name, `vu` 0–16, `effect_name`, `active`). Host-tested against a
synthetic `Snapshot`. Refresh happens at ≤ 20 Hz from a display task reading
`ControlHalf::telemetry()`, never from the audio task.

### 3. The display (`boards/starplayer-a1s/src/lcd.rs`, `#[cfg(feature = "lcd")]`)

- Crates: `mipidsi` (ST7789, 240×280 with the 20-row window offset for this panel;
  research point 2 for the exact version and offset), `display-interface-spi`,
  `embedded-graphics`, esp-hal SPI master on HSPI with DMA. Pins from `board.rs`: SCK 14,
  MOSI 13, CS 15, DC 2, RST 4, backlight tied high or on a spare pin.
- Layout at 240×280: a two-line header (title; `ord pat row  spd bpm`), then channel
  rows of 14 px — 16 rows fit — each `nn name(8) note vu-bar effect`, VU bars in the
  original's green→yellow→red with peak-hold; the star logo on stop. A `screen.rs`
  renders `NowPlaying` to a `DrawTarget`, so it is testable against
  `embedded-graphics`' `MockDisplay` on the host.
- Only dirty rows are redrawn (compare the previous `NowPlaying`), to keep SPI traffic
  well under the 20 Hz budget at the panel's supported clock.
- `NoopDisplay` when the feature is off; `main.rs` spawns the display task only under
  the feature.

### 4. Documentation

`embedded/README.md`: wiring table for the display, the DIP-switch positions, the key
map in both variants, and the `--features lcd` build line.

## Research points

1. **The KEY2/GPIO13 conflict and the DIP switches.** Read the v2.2 schematic: which keys
   the DIP switches route to the SD lines, whether MOSI can sit on another exposed pin
   (GPIO 4 or 2 through the SD header), and whether KEY2 can survive. Record the wiring
   actually used.
2. **`mipidsi` version and the 240×280 offset.** The 1.69" panel is a 240×320 ST7789 RAM
   with a 240×280 window; the y-offset (20) and the orientation flags differ between
   modules. Confirm with a test pattern.
3. **SPI clock the panel tolerates** on this wiring (typically 40 MHz; jumper leads may
   need 20). Measure the full-frame redraw time and confirm dirty-row redraws keep the
   display task under 5 % of core 0.
4. **Input-only pins and pull-ups**: KEY1 (36) — confirm the board's external pull-up;
   otherwise KEY1 is unusable and the map shifts.

## Verification

```text
cd embedded && cargo test -p starplayer-firmware-common       # keys state machine, view model, screen on MockDisplay
cd embedded && cargo xtask build --board a1s                  # no lcd
cd embedded && cargo xtask build --board a1s --features lcd
# owner
cd embedded && cargo xtask flash --board a1s --features lcd && cargo xtask monitor
#   → keys drive transport and volume; the screen shows the sounding row and VU bars;
#     no underrun lines while the display redraws
```

## Out of scope

A pattern view or oscilloscopes on the device (scope taps cost ~2 KB per channel and the
budget must say there is room first). Touch. Rotary encoders. Persisting the volume.
