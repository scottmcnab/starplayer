# M8 — I5: Controls and the display — six keys and an ST7789 now-playing screen (`lcd` feature)

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](../M8-master-plan.md), decision 4; deliverable 7) |
| Status | Implemented 2026-09-11; builds link; awaiting owner hardware run |
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

## Research resolution

Implemented 2026-09-11 on branch `m8-i5`. Every build below links; nothing has been
flashed — flashing is owner-only.

### Prerequisite: the `Send` root cause, and moving the audio refill to core 1

M8-I3's research resolution blamed `RenderHalf` not being `Send` on `Engine`'s
`Box<dyn EventSource>` having no `+ Send` bound, and kept the audio task on core 0 as a
result. **That diagnosis does not hold**, and did not need to be re-litigated by changing
the engine at all:

* `EventSource: Send` (`crates/starplayer-engine/src/source.rs:171`) and
  `Insert<Sample>: Send` (`crates/starplayer-dsp/src/insert.rs:123`) are both already
  supertrait bounds.
* A trait object erased from a trait whose *definition* carries an auto trait as a
  supertrait is itself that auto trait, with no need to spell `Box<dyn EventSource + Send>`
  at the use site — confirmed directly (`trait Foo: Send {} fn assert_send<T: Send>() {}
  assert_send::<Box<dyn Foo>>();` compiles) rather than assumed from memory, since this is
  a genuinely easy rule to misremember the other way.
* The compile-time check the task file asked for is a permanent one, beside `RenderHalf`'s
  definition in `crates/starplayer-host-embedded/src/player.rs`:

  ```rust
  const fn assert_render_half_is_send<T: Send>() {}
  const _: () = assert_render_half_is_send::<RenderHalf<Linear>>();
  ```

  — the same `const _: () = assert_send_sync::<Module>();` idiom
  `crates/starplayer-model/src/module.rs` already uses for `Module: Send + Sync`. It was
  verified three ways: `cargo test -p starplayer-host-embedded -p starplayer-engine` on the
  host (x86_64, full atomics), `cargo xtask build --board a1s` against the real
  `xtensa-esp32-none-elf` target under `esp-1.97` (the target this milestone actually
  cares about — the host result alone would not have been conclusive, since target-specific
  atomic availability could in principle change the answer), and `cargo xtask ci
  --job no-std-check` in the main workspace (`riscv32imc-unknown-none-elf`, no atomics at
  all). All three pass with no engine change.

* **The real, and genuinely unavoidable, `Send` obstacle was somewhere else entirely**:
  `esp_hal`'s own DMA transfer type. Attempting
  `esp_rtos::start_second_core(.., move || { .. audio::refill_task(transfer, render) .. })`
  failed with three `E0277`s naming `*mut DmaDescriptor`, `*const u8` and
  `PhantomData<*const ()>` (`Async`'s own marker) inside
  `esp_hal::i2s::master::asynch::I2sWriteDmaTransferAsync` — none of esp-hal's DMA/async
  internals carry an `unsafe impl Send`, because nothing in the HAL's own API surface
  normally needs to move a live transfer across a thread boundary. `render` needed no such
  treatment (`&'static mut RenderHalf<Linear>` is `Send` because `RenderHalf` is).
  `main.rs`'s `SendTransfer` is a one-field wrapper with `// SAFETY:` and
  `unsafe impl Send for SendTransfer {}`, whose argument is exactly the pattern
  `esp_rtos::start_second_core`'s own internal `SecondCoreStack` wrapper uses for the same
  reason (`esp-rtos` 0.3.0's `lib.rs`): a value moved **once**, into the closure that
  becomes core 1's entire program, never touched from core 0 again — `Send`'s actual
  safety property (no two threads read or write the same memory concurrently) holds because
  there is only ever one owner at a time. This is the one `unsafe` block M8-I5 added; see
  the inventory below. A second, unrelated trap on the same line: edition 2021's disjoint
  closure capture reaches *through* a wrapper struct and captures the non-`Send` field
  directly, silently defeating the wrapper, unless the closure's first statement is
  `let transfer = transfer;` on the whole value before touching `.0` — documented at the
  call site since it is not an obvious failure mode to hit twice.

* With that resolved, the refill runs on `esp_rtos::start_second_core`'s executor
  (`CORE1_EXECUTOR`, an 8 KiB dedicated stack, `CORE1_STACK_SIZE`), simplified from
  ampkeeper's `CORE1_SPAWNER`/`Signal<SendSpawner>` round-trip: this board only ever runs
  one task on core 1, so it is built and spawned directly inside the second core's own
  entry closure rather than sending a `SendSpawner` back to core 0. Core 0 runs the keys
  task, the control task and (under `lcd`) the display task — exactly the work the task
  file's "M8-I5 and M8-I6 should not be started without settling this" flagged as needing
  to not compete with the refill, and now it does not, on any core.

### 1. The KEY2/GPIO13 conflict and the DIP switches

Not newly confirmed from the schematic beyond what I3 already recorded (no board access
from here either) — the pin map and the "13 is also KEY2" conflict came from I3's own
research and `board.rs`'s existing doc comment. What this task adds is the **resolution**
the task file predicted as the default answer: MOSI keeps GPIO13, KEY2 is sacrificed in the
`lcd` build, and `boards/starplayer-a1s/src/keys.rs::Keys::take` has two `#[cfg(...)]`
variants — six GPIOs in the default build, five (no GPIO13) under `lcd` — selected by
`board::Board::take`'s own two `#[cfg]` variants so `main.rs` never claims GPIO13 twice.
The DIP-switch position that matters is whichever one keeps the SD-card pin group off the
SD slot in an `lcd` build; `embedded/README.md`'s DIP-switches section says so and asks the
owner to record what the board in hand is actually set to, same as I3 did for the codec.

### 2. `mipidsi` version and the 240×280 offset — and a real dependency conflict

**The versions the task file's phrasing implicitly assumed (current `mipidsi 0.10.0` /
`embedded-graphics 0.8.2`) cannot be used together with this workspace's `fixed = "1.31"`
at all.** Adding either to `embedded/Cargo.toml` in isolation — not even together —
produces `error: failed to select a version for` `az`, reproduced in a scratch crate with
no `starplayer` code in it:

* `embedded-graphics 0.8.2` tightened its own dependency on `fixed` from `0.8.1`'s
  `^1.14.0` to `~1.27.0`, and `az` from `^1.2.0` to `~1.2.0`.
* `mipidsi 0.10.0` tightened its `embedded-graphics-core` requirement from `0.9.0`'s
  `^0.4.0` to `^0.4.1`, and `embedded-graphics-core 0.4.1` itself tightened `az` from
  `0.4.0`'s `^1.1` to `~1.2.0`.
* `starplayer-core` depends on `fixed = "1.31"`, which needs `az = "^1.3"`.
* `~1.2.0` (`>=1.2.0, <1.3.0`) and `^1.3` (`>=1.3.0, <2.0.0`) do not overlap. Cargo unifies
  same-major dependency versions by default, so there is no version of `az` that satisfies
  both at once, and the resolver refuses rather than picking two.

`embedded-graphics = "=0.8.1"` (`az ^1.2.0`, `fixed ^1.14.0`) and `mipidsi = "0.9.0"`
(`embedded-graphics-core ^0.4.0`, satisfied by `0.4.0`'s own looser `az ^1.1`) both resolve
cleanly to one shared `az 1.3.0` for the whole graph — verified in isolation first, then in
this workspace. Neither step back changes anything the task's code needs: both mipidsi
releases carry their own `mipidsi::interface::SpiInterface` (no `display-interface-spi`
dependency either way), and `embedded_graphics::mock_display::MockDisplay` is unchanged
across `0.8.1`/`0.8.2`. The long version-pin comment lives beside `embedded-graphics` in
`embedded/Cargo.toml`'s `[workspace.dependencies]`, cross-referenced from `lcd.rs` and
`embedded/README.md`.

The 240×280 windowing is `Builder::display_size(240, 280).display_offset(0, 20)` — the
task file's own figures — with `Orientation::new()` (mipidsi's default, no rotation or
mirroring) as the starting point. **Not confirmed against a test pattern**; the owner's
step below says so, and [`lcd::Y_OFFSET`]/[`lcd::ORIENTATION`] are named as the two
constants to change if the picture is shifted or mirrored.

### 3. SPI clock and redraw cost

Set conservatively to 20 MHz (`lcd::SPI_FREQUENCY_HZ`) rather than measured up to the
panel's documented 40 MHz ceiling — no board to measure a bit-error-free redraw on. Dirty-
row redraws are implemented (`firmware_common::screen::Screen::render` diffs the previous
`NowPlaying` field by field, including a per-channel VU peak-hold marker that only forces a
redraw while it is actually decaying) and host-tested for the *count* of draw calls a
changed vs. unchanged frame produces, via a `CountingDisplay` test double — but the
**time** a full-frame or dirty-row redraw takes on real SPI hardware, and whether it stays
under 5 % of core 0, are both for the owner's step below to measure with the clock this
task shipped and to report back with, so the constant can be raised if the wiring supports
it.

### 4. Input-only pins and pull-ups

`Keys::take` asks every key for `InputConfig::default().with_pull(Pull::Up)`, including
KEY1 (GPIO36, input-only, no internal pull on this pin's silicon) — the same "ask for it
anyway; it costs nothing where it is unsupported and is silently ignored there" reasoning
`board.rs`'s pre-existing `headphone_detect` (GPIO39, also input-only) already relies on,
rather than a new pattern invented for this task. Whether KEY1 is actually usable therefore
rests on the board's own external pull-up, same as it did for the headphone-detect pin —
not independently re-verified here, and the owner's hardware run is what actually presses
the keys for the first time.

## What was done differently from the task file, and why

* **One control task owns `ControlHalf`, not three separate tasks contending for it.**
  `ControlHalf` is not `Clone` and the render/control split is deliberately one owner each
  (I1). The keys task (GPIO polling, 10 ms) and a would-be separate display task both want
  to act on `ControlHalf` or read its telemetry; instead, `control_task` alone owns
  `ControlHalf` and runs a single 10 ms loop that drains key events (from `keys_task` over
  an `embassy_sync::Channel`), applies them, collects garbage, and — at 20 Hz — computes a
  `NowPlaying` and hands it to the display task over a `Signal` (latest-value, not a queue:
  a display that falls behind must never draw a stale frame). `display_task` therefore
  never touches `ControlHalf` at all and never reads faster than `control_task` signals it,
  which is where "never from the audio task" ends up being enforced structurally rather
  than by convention — the audio task (now on core 1) touches neither the keys nor the
  screen at all.
* **`NowPlaying` gained `title: FixedStr<28>`, `volume: U0F16` and `channels: [ChannelRow;
  16]`, not `heapless::String`/`heapless::Vec`.** The deliverable's own words ask for "a
  fixed-capacity, `Copy`-able struct (heapless strings)" — but `heapless::String<N>` and
  `heapless::Vec<T, N>` are not `Copy` (their buffers are `[MaybeUninit<T>; N]` plus a
  length, and the crate does not derive `Copy` for either), so literally following "heapless
  strings" would have broken "`Copy`-able", which the same sentence also asks for and which
  M8-I3's `NowPlaying` already was. `firmware_common::now_playing::FixedStr<N>` is a small
  hand-rolled fixed-capacity UTF-8 buffer (`[u8; N]` + a length) that *is* `Copy`, needs no
  new dependency, and keeps `#![forbid(unsafe_code)]`. `from_snapshot` kept its existing
  `sample_rate_hz` parameter (needed for the elapsed/total-seconds fields I3's tests
  already covered) and added `title`/`volume` as new trailing parameters, rather than the
  task file's literal `(snapshot, title, volume)` — dropping `sample_rate_hz` would have
  broken the one-line UART transport string `main.rs`'s `control_task` already printed
  every second.
* **A channel row's "instrument" is the numeric one-based id (`ChannelState::instrument:
  u8`), not a fetched sample/instrument *name*.** The layout sketch's "`nn name(8)`" reads
  as an 8-character name field, but `Snapshot` carries no instrument names — only the
  engine's per-channel numeric id — and giving `NowPlaying` a name would mean plumbing a
  live `&Module` reference through the view model, breaking its `Copy`, no-heap, no-`Module`
  shape and requiring a real `Module` in every host test. The screen's row shows `nn Innn
  note vu-bar effect` instead (`nn` the channel slot, `Innn` the instrument number); adding
  a name lookup is a reasonable follow-up if the owner wants it, and does not need this
  task's architecture to change to add later.
* **The "star logo on stop" is a schematic six-line star burst, not a reproduction of
  `STARPLAY/STAR.ASM`'s `DrawStarAnsi`.** That is ANSI art with no practical pixel mapping
  onto a 240×280 panel, and `plans/reference/original-star-ui.md` §12 asks to keep the star
  as a *motif*, not to reproduce it exactly. `embedded-graphics` 0.8 has no polygon
  primitive; six `Line`s from a fixed-point (×1000) sine/cosine table stand in for it. A
  fair-use nod, and a reasonable place for a follow-up to spend more visual polish.
* **The VU quantisation to a 16-cell bar rounds up at full scale rather than truncating.**
  `U0F16::MAX.to_bits() = 65535`; a plain `bits * 16 / 65536` truncates to 15 there, which a
  test caught (`full-scale VU quantises to the top of a 16-cell bar`) — the formula is
  `((bits + 1) * 16) / 65536`, clamped with `.min(16)` as the safety net rather than the
  rounding itself.
* **`Board::take` grew a `take_common` helper returning a `keys`-less `BoardWithoutKeys`**,
  rather than the `Board { keys: Keys::take(..), ..Board::take_common(..)? }`
  functional-update shape tried first and reverted: constructing the intermediate `Board`
  inside `take_common` would have needed a placeholder `keys` field that is *eagerly*
  evaluated as part of that struct literal (Rust does not lazily defer struct-literal
  fields), so an `unreachable!()` placeholder there would panic on every call rather than
  merely documenting an invariant. `BoardWithoutKeys` (a private struct with no `keys`
  field at all) sidesteps the problem instead of working around it with a comment.

## `unsafe` inventory (M8-I5's additions)

One new site, in the board crate (master-plan decision 6 tolerates `unsafe` only under
`embedded/`, each site with a `// SAFETY:` comment):

| Site | Why |
|---|---|
| `src/main.rs`, `unsafe impl Send for SendTransfer` | Carries `audio::AudioTransfer` — not `Send`, because esp-hal's DMA transfer type holds raw pointers with no `unsafe impl Send` of its own — across the one-time ownership handoff into core 1's entry closure. See "Prerequisite" above for the full argument; the same shape `esp_rtos::start_second_core`'s own `SecondCoreStack` wrapper uses internally. |

`firmware-common`, `boards/starplayer-a1s/src/keys.rs`, `boards/starplayer-a1s/src/lcd.rs`
and `boards/starplayer-a1s/src/board.rs`'s M8-I5 additions contain no `unsafe` of their own.

## Verification run

Every command below was run in this worktree; `. ~/export-esp-1.97.sh` was sourced first
for the firmware ones.

| Command | Result |
|---|---|
| `cd embedded && cargo test -p starplayer-firmware-common` | pass — 33 tests (keys, screen, now_playing, plus I3's bench/format) |
| `cd embedded && cargo test -p starplayer-embedded-xtask` | pass — 4 tests, unchanged |
| `cd embedded && cargo xtask build --board a1s` | **links**, no warnings — 475 312 B (18.1 %), up from I3's 464 944 B (+10 368 B: keys, second-core plumbing) |
| `cd embedded && cargo xtask build --board a1s --features lcd` | **links**, no warnings — 499 152 B (19.0 %), +23 840 B over the default audio build |
| `cd embedded && cargo xtask build --board a1s --features bench` | **links**, no warnings — 552 560 B (21.0 %), unchanged from I3 (the `bench` build excludes `keys`/`lcd`) |
| `cd embedded && cargo xtask size --board a1s --features lcd` | pass — 499 152 / 2 621 440 B = 19.0 % |
| `xtensa-esp32-elf-nm … \| grep _bss_end\|_stack_start` (default, `lcd`) | core 0 stack 38 712 B (default), 37 312 B (`lcd`) — both clear of the 6 472 B failure point I3 found at 160 KiB heap |
| `cd embedded && cargo clippy -p starplayer-firmware-common -p starplayer-embedded-xtask -- -D warnings` | pass, clean (three findings fixed: a `needless_range_loop`, a `collapsible_if` — rewritten as a `&&`-chained `if let`, a `useless_format`) |
| `cargo test -p starplayer-host-embedded -p starplayer-engine` (main workspace) | pass — unaffected by the `RenderHalf` `Send` assertion |
| `cargo xtask ci --job no-std-check` (main workspace) | pass — every bare-metal check, including `starplayer-host-embedded` |

Not run: `cargo xtask flash`/`monitor` (owner-only, never run by an agent).

## Owner steps

In this order. **Flashing is owner-only; no agent has run any of it.**

1. **Confirm the DIP-switch position** keeps the SD-card pin group off the SD slot before
   an `lcd` build (`embedded/README.md`'s DIP-switches section), and record the board's
   actual switch positions for both the six-key and `lcd` builds.

2. **Flash the default (six-key) audio build and try every key**, per
   `embedded/README.md`'s six-key table: KEY1 play/pause, KEY2 stop (back to order 0),
   KEY3/KEY4 previous/next order (press, then hold and confirm the ~200 ms repeat), KEY5/
   KEY6 volume down/up (press, then hold). Confirm KEY1 (GPIO36, no internal pull) actually
   registers — if it does not, the board's external pull-up is missing or different from
   assumed, and `Keys::take`'s pull configuration needs a second look.

   ```sh
   cd embedded && . ~/export-esp-1.97.sh
   cargo xtask flash --board a1s
   cargo xtask monitor --board a1s
   ```

3. **Flash the `lcd` build and confirm the picture.** Check the boot log's `LCD  ST7789
   found`/`not found` line, then look at the panel:
   * if the image is shifted, adjust `lcd::Y_OFFSET`;
   * if it is mirrored or rotated, adjust `lcd::ORIENTATION`;
   * confirm the header (title, `ord/pat/row`, speed, bpm, volume) and the channel rows
     (instrument number, note, VU bar, effect name in English) read correctly against what
     the UART transport line says for the same moment;
   * confirm the five-key map: KEY2 does nothing (GPIO13 is now MOSI), and holding KEY1
     ≥ 1 s stops and rewinds to order 0.

   ```sh
   cargo xtask flash --board a1s --features lcd
   cargo xtask monitor --board a1s
   ```

4. **Measure the redraw cost** research point 3 asks for: watch for underrun lines in the
   transport log while the display is actively redrawing (a changing VU bar, a scrolling
   title if one is added later), and time a full-frame redraw if the hardware allows it.
   Raise `lcd::SPI_FREQUENCY_HZ` from 20 MHz towards the panel's 40 MHz ceiling only once a
   clean redraw is confirmed at 20 MHz, and record whichever value is chosen, with the
   measurement, in this section or in `embedded/README.md`.

5. **Fill in the two `TBD` stack rows** `plans/reference/embedded-budget.md` §2 now
   carries for core 0's peak stack under the keys/control/(`lcd`) display tasks, against
   the 38 712 B / 37 312 B budgets this task measured statically.
