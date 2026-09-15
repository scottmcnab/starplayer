# StarPlayer firmware

The `embedded/` workspace: StarPlayer running on real hardware. Two boards:

* the AI-Thinker **ESP32-Audio-Kit** (ESP32-A1S module, classic ESP32, ES8388 codec) —
  the one that makes sound (M8-I3);
* the **ESP32-C5 devkit** (RISC-V, no audio hardware) — proves the second architecture by
  rendering the same golden fixtures and comparing their SHA-256 and cycle counts against
  the A1S's (M8-I4).

This is **its own cargo workspace**, deliberately (M8 master-plan decision 1). The A1S is
built with the Xtensa fork of rustc and needs `-Z build-std`; both board crates carry
per-target dependency tables full of `esp-*` crates that only compile for their own
triple. None of that may be visible to the main workspace's `cargo xtask ci`.

---

## 1. Toolchain

**Two toolchains, one per board — `embedded/xtask` picks the right one for you.**

* The **A1S** is Xtensa, an out-of-tree LLVM target that only exists inside the named
  espup toolchain **`esp-1.97`** (rustc 1.97.0-nightly), pinned in `rust-toolchain.toml`
  and built with `-Z build-std` (no target has a prebuilt `core`/`alloc` for it — there is
  nothing to add with `rustup target add`).
* The **C5** (`riscv32imac-unknown-none-elf`, M8-I4) is a plain LLVM target with a prebuilt
  `core`/`alloc` component, so it is built under the **main workspace's own pinned stable
  toolchain** (`1.97`) instead — the same one that already builds
  `riscv32imc-unknown-none-elf` for `cargo xtask ci --job no-std-check`. No `-Z build-std`,
  no espup, no Xtensa fork anywhere in the C5's build. This is M8-I4 research point 2:
  plain `rustup` was tried first and it just worked, so the "simpler" `esp-1.97` fallback
  the task allowed was never needed. See `plans/reference/embedded-budget.md` §8 for the
  finding in full.

`embedded/xtask` names the toolchain explicitly per board (`cargo +esp-1.97 …` for the
A1S, `cargo +1.97 …` for the C5) — you never choose it yourself.

The A1S's `esp-1.97` toolchain is already installed. If it ever has to be installed again:

```sh
espup install --name esp-1.97 --toolchain-version 1.97.0.0
```

The C5's plain `1.97` toolchain needs only its target added once, if it is not already:

```sh
rustup target add riscv32imac-unknown-none-elf --toolchain 1.97
```

(`rust-toolchain.toml` at the repository root lists it under `targets`, so a bare `rustup
show` from the repository root installs it automatically on a fresh clone.)

**Every shell that builds the A1S must first source the Xtensa toolchain's environment**,
which puts the Xtensa GCC linker on `PATH` and sets `LIBCLANG_PATH`:

```sh
. ~/export-esp-1.97.sh
```

Forget it and `cargo xtask build --board a1s` stops before compiling anything and tells you
to run exactly that. **The C5 needs none of this** — `riscv32imac-unknown-none-elf` links
with rustc's self-contained `rust-lld`, so `cargo xtask build --board c5` never checks for
an external linker at all. Flashing needs `espflash` (3.3.0) and `cargo-espflash`, both
already installed.

## 2. Build

Everything goes through `cargo xtask`, from **this** directory:

```sh
cd embedded
. ~/export-esp-1.97.sh    # needed for --board a1s; harmless (and unnecessary) for c5

cargo xtask build  --board a1s                     # the audio firmware, six keys, no display
cargo xtask build  --board a1s --features lcd      # the audio firmware, five keys, the ST7789 screen
cargo xtask build  --board a1s --features bench    # the bench firmware (no audio)
cargo xtask build  --board a1s --features web      # the audio firmware plus WiFi, a page and uploads
cargo xtask build  --board a1s --features web,lcd  # both
cargo xtask build  --board a1s --features tone     # gated direct-tone listening diagnostic
cargo xtask build  --board a1s --features engine-tone # standalone native-S3M engine-path diagnostic
cargo xtask build  --board a1s --features matched-tone # strict post-render A/B against engine-tone
cargo xtask build  --board a1s --features swapped-tone # matched A/B with only L/R peaks exchanged
cargo xtask build  --board a1s --features reference-rate-tone # matched A/B at 48 kHz
cargo xtask image  --board a1s [--merge]           # an espflash image under target/
cargo xtask size   --board a1s                     # the image against its partition
cargo xtask assets [--force]                       # regenerate the module images and gzip the page
cargo xtask build  --board a1s --dev               # release codegen, debug assertions on

cargo xtask build  --board c5                      # boot-and-idle smoke build, no module
cargo xtask build  --board c5 --features bench     # the RISC-V bench (M8-I4)
cargo xtask size   --board c5 --features bench     # the bench image against its partition
```

`--board a1s` is also the default, so a bare `cargo xtask build` (no `--board`) builds the
A1S — `--board c5` always has to be spelled out.

The A1S default, `lcd`, `web` and `web,lcd` production personalities render and transmit at
48 000 Hz. `reference-rate-tone` uses that accepted audible rate too. The `bench` build and the
historical `tone`, `engine-tone`, `matched-tone` and `swapped-tone` diagnostics remain at
44 100 Hz so their golden hashes and recorded listening comparisons stay reproducible.

`cargo xtask assets` runs the **main** workspace's `cargo xtask module-images`, which
writes `embedded/assets/*.spmi` — the module images the firmware links with
`include_bytes!` — and gzips `www/index.html` into `embedded/assets/index.html.gz`, which
the `web` build links the same way. All of them are git-ignored build products, and
`build`, `image` and `size` generate them automatically when they are missing or stale, so
a fresh clone builds. The gzipped page has a hard 12 KiB budget and the asset step fails
the build if it is exceeded (it is 6.6 KiB today).

Host-testable logic lives in `firmware-common`, which builds for the host too:

```sh
cargo test -p starplayer-firmware-common
cargo test -p starplayer-embedded-xtask
```

### Why `xtask` and not `cargo build`

Cargo reads `.cargo/config.toml` from the directory it is **invoked in**. The target
triple, `-Tlinkall.x` and `-Z build-std` live in `boards/starplayer-a1s/.cargo/config.toml`,
so a firmware build is only correct with cargo's current directory set to that crate.
`cargo xtask` does that for you. A bare `cargo build` from `embedded/` will quietly try to
build the firmware for the host and fail on the first `esp_hal` name.

There is exactly one target directory, `embedded/target`. Do not create a second — a
`build-std` tree for one Xtensa target is already 1.6 GB.

## 3. Flash and monitor — **owner only**

Agents build images and hand them over. **An agent must never run `espflash flash`,
`espflash erase` or `cargo xtask flash`.** The two commands below are for the owner:

```sh
cd embedded
. ~/export-esp-1.97.sh

# the bench firmware: renders every golden fixture, prints hashes and cycles, no audio
cargo xtask flash --board a1s --features bench
cargo xtask monitor --board a1s

# the audio firmware: plays REFLEX.S3M out of the headphone jack
cargo xtask flash --board a1s
cargo xtask monitor --board a1s

# the owner-only direct-tone comparison: ~1 second sine, ~1 second digital silence
cargo xtask flash --board a1s --features tone
cargo xtask monitor --board a1s

# the owner-only engine-path comparison: native-S3M 125 Hz sine looped by B00
cargo xtask flash --board a1s --features engine-tone
cargo xtask monitor --board a1s

# the owner-only matched A/B: same engine work, 125 Hz and measured stereo peaks
cargo xtask flash --board a1s --features matched-tone
cargo xtask monitor --board a1s

# the owner-only channel discriminator: matched A/B with only its L/R peaks exchanged
cargo xtask flash --board a1s --features swapped-tone
cargo xtask monitor --board a1s

# the owner-only rate discriminator: original matched A/B with only output changed to 48 kHz
cargo xtask flash --board a1s --features reference-rate-tone
cargo xtask monitor --board a1s

# the C5 (M8-I4): one flash, no audio, no listening check — see the "C5" note below
cargo xtask flash --board c5 --features bench
cargo xtask monitor --board c5
```

When the A1S is connected through the Mac RFC2217 bridge, the owner can build and flash the fixed
`web` personality from the repository root with one command:

```sh
embedded/flash_image.sh
```

The wrapper resolves `embedded/` from its own path, so invoking it by any valid relative or
absolute path does not depend on the caller's working directory. It sources
`~/export-esp-1.97.sh`, deletes its checkout's old merged image, asks that checkout's
`embedded/xtask` package to clean its own stale binary, asks that checkout's xtask to
rebuild the image, and uses `esptool.py` to write
`target/starplayer-a1s-current-merged.bin` at address zero. The wrapper refuses to run esptool unless
that build creates a non-empty image at the expected checkout-local path. It defaults to the `web`
feature and the proven
`rfc2217://192.168.0.151:8086?ign_set_control` endpoint at 460800 flash baud. Override either for another
bridge without changing the fixed firmware personality:

```sh
STARPLAYER_RFC2217_ENDPOINT='rfc2217://192.168.0.152:8086?ign_set_control' STARPLAYER_FLASH_BAUD=115200 embedded/flash_image.sh
```

Owner tooling may select another personality and image path through
`STARPLAYER_FEATURES` and `STARPLAYER_IMAGE_PATH`. A Cargo/link failure returns status 20;
an esptool failure returns status 30. Both abort the I9 controller: only a structured
`START`/`END reason=setup` response from running firmware is capacity evidence.

`flash` builds a merged image (bootloader + partition table + app) with `--skip-padding`,
so a reflash leaves the `modules` and `config` partitions alone, and attaches the monitor
when it is done.

The Audio Kit's USB port is a CP2102; on Linux it appears as `/dev/ttyUSB0` and needs the
user to be in the `dialout` group. If espflash cannot get the board into the bootloader,
hold **BOOT** (KEY1 area, the button marked `IO0`) while tapping **EN/RST**.

### The C5

There is no "audio" build to flash — the chip has no codec and no DAC, so `--features
bench` is the only build worth flashing at all. `espflash` autodetects the chip
(`esp32c5`) over USB the same way it does for the A1S; hold the board's **BOOT** button
while tapping **RESET** if it does not enter the bootloader on its own. The transcript's
`BENCH … linear flash sha256=…` lines are the exit criterion (they must equal the same
`goldens/` hashes the A1S's do), and the full log is what fills
`plans/reference/embedded-budget.md`'s C5 rows — its §6 has the line-by-line mapping.

### Before listening

The first hardware run confirmed advancing transport with `underruns=0` and `dma_errors=0`,
but unity master volume was very loud and clipped repeatedly. In the second run the owner found
1/16 was the loudest setting they wanted and the next 1/16 step was already very loud. That run
still sounded noisy or clipped because it discarded about four digital bits while driving the
ES8388 headphone pair at analog 0 dB. The A1S now boots and caps its engine master at exact 1/4,
uses 1/64 button steps, and sets the enabled ES8388 pair-2 headphone drivers (`0x0c`) to −12 dB
(`LOUT2VOL = ROUT2VOL = 0x16`). The combined nominal maximum remains about −24 dB with two more
digital signal bits. The disabled speaker pair stays at its −45 dB analog minimum and GPIO21
holds the speaker-amplifier enable low. Start a listening check with headphones off the
listener's ears and raise the level only after the boot line confirms those settings.

The owner still heard grainy output at that gain on the 16-bit-slot build. The next isolated
image therefore keeps every gain, routing, module and refill decision above while changing the
ES8388 and esp-hal transport to 32-bit Philips slots. Each signed engine `i16` is sign-extended
to `i32`, shifted into the high 16 bits and written as explicit little-endian bytes. This tests
CPU-generated sample framing without changing nominal loudness.

Star FX's clean ADC-to-DAC path on the same device narrows the fault without settling this test.
That path receives and transmits the same native 32-bit representation, so it proves the codec,
analog path, clocks and wiring can be clean while leaving CPU-generated slot alignment open.

The owner still heard grain in the 32-bit music image even though its transport cadence was
correct. The first isolated comparison used `--features tone`. It continues the bundled
module render so CPU load and scheduling stay representative, then replaces every outgoing
quantum immediately before the existing 32-bit packer. The replacement is a compile-time-table,
dual-mono 172.265625 Hz sine at signed amplitude 4 096, followed by the same duration of exact
digital zero. Each interval is 44 032 frames (about 998.46 ms) and 172 complete 256-frame table
cycles, so the gate changes at a zero crossing. Its integer phase and gate state
continue from the initial ring prefill into steady refill. `tone` is off by default and is
deliberately incompatible with the no-audio `bench` build. A clean tone would place the grain
upstream in module/sample rendering; a grainy tone would leave it in the I2S/codec/analog output path.

The first, higher-frequency 1 033.59375 Hz fresh-reset tone run used the verified 478 944-byte
merged image with SHA-256
`cecdd7c752dcce50a8be37731500c74e273cf61010a3890a9efcf4910679cbb6`. Boot confirmed every
transport setting and its original 44 032-frame gates. From engine `0:00` to `0:10`, `written`
advanced from 360 448 to 3 930 112 bytes and `pushes` from 119 to 1 280 while engine time tracked
wall time; `underruns=0` and the recovered startup `dma_errors=1` stayed fixed. This accepts the
tone image's configuration and cadence only. The owner judged that tone apparently clean but too
high to assess the remaining grain confidently. The 172.265625 Hz refinement now needs a
clean/grainy report and confirmation that the gated intervals are silent.

The lower-frequency fresh-reset run used the verified 478 944-byte merged image with SHA-256
`77451cbadf3aea86942ef5315778e2c277a485e81cba55e8ca240123893f5522`. Boot confirmed
172.265625 Hz, amplitude 4 096, both 44 032-frame gates, 32-bit Philips codec mode, headphone
analog −12 dB, 44 100 Hz stereo 32-bit I2S and the six-quantum ring. From engine `0:00` to `0:06`,
`written` advanced from 360 448 to 2 500 608 bytes and `pushes` from 119 to 818 while engine time
tracked wall time; `underruns=0` and the recovered startup `dma_errors=1` stayed fixed. The owner
accepted the 172.265625 Hz tone as clean and every gated interval as silent. Together with the
objective cadence, that accepts CPU-generated signed samples, high-aligned 32-bit packing, DMA,
I2S, codec configuration and the headphone analog path. The remaining music grain is upstream
of the post-render override.

`REFLEX.S3M` is weak material for distinguishing an engine fault: its four audible sources are
8-bit mono samples only 34, 130, 1 978 and 34 frames long. The standalone default-off
`--features engine-tone` comparison therefore constructs a controlled one-channel native S3M
before playback. Row zero plays C-4 at volume 64, rows 1 through 62 are empty fixed-stride S3M
cells, and row 63 carries `B00`. Its single signed-16 sample is the existing 256-entry sine scaled
to amplitude 28 672, has a full forward loop and a 32 kHz reference rate. `B00` returns the one-pattern
order list to order zero, producing an uninterrupted 125 Hz tone through the real
`EmbeddedPlayer<Linear>` path at 44.1 kHz under its default `AtEnd::Continue` policy. Board
master remains 1/4 and no post-render override is installed. `engine-tone` is incompatible with
`bench`, `tone`, `matched-tone`, `swapped-tone` and `web`; it may be combined with `lcd`. A clean
result would place the music
grain in REFLEX's source material, while a grainy result would keep the sequencer, mixer,
interpolator or master path under study.

The allocation-safe engine-tone hardware run used the flash-verified 426 416-byte merged image
with SHA-256 `2efdbd30c05b34f9175fb704d9eeb9b154079772cdd4e824727bb20dfa79a60d`.
Fresh boot reported the expected 125 Hz/32 kHz/signed-16 source, amplitude 28 672, 256-frame
sample loop, native `B00`, linear interpolation and 1/4 master. `EmbeddedPlayer` open left
94 948 of the 120 KiB heap free. Telemetry reached row 59 at `0:07`, wrapped to row 03 at `0:00`
and continued through row 29 at `0:03`, proving the bounded scan and native song loop on the
board. Across the capture, `written` advanced from 356 352 to 4 286 464 and `pushes` from 118 to
1 402; peak held around 5 326–5 327, `underruns=0`, and the recovered startup `dma_errors=1`
stayed fixed. Image identity, configuration, cadence and looping are accepted. The owner reports
that this engine-rendered 125 Hz tone buzzes; clean normal music also remains pending.

The strict follow-up is the default-off `--features matched-tone` image. It constructs and renders
that same native engine-tone S3M at 1/4 master, then replaces each completed quantum immediately
before packing with a continuous 125 Hz integer-Q15 sine. A wrapping full-turn phase accumulator
advances by the exact rounded 12 173 944 units per 44.1 kHz frame and scales the channels to the
host engine capture's signed peaks, 4 796 left and 5 327 right. Phase state continues from the
initial ring prefill through muted handoffs and steady descriptor refill; there is no silence
gate. `matched-tone` is incompatible with `bench`, `tone`, `engine-tone`, `swapped-tone`,
`reference-rate-tone` and `web`, but may be combined with `lcd`.

The matched 125 Hz image also buzzed. In the owner's trimmed phone recording, unwanted lines
begin at 297.363 Hz and repeat about every 86.13 Hz, exactly the cadence of 512 output frames or
two DMA descriptors. The earlier clean direct tone hid this splice because its waveform repeats
every 256 frames, exactly one descriptor, and its gates are descriptor aligned.

After the heap-backed constrained handoff passed its objective transport checks, its matched tone
still buzzed irregularly. The default-off `--features swapped-tone` discriminator kept that exact
module, 1/4 master, 125 Hz Q15 phase and post-render placement, but exchanged only the generator
peaks to 5 327 left and 4 796 right. The owner heard one fixed-right buzz and one centred buzz while
the engine played. KEY1 stopped the engine underlay and removed only the centred buzz; the right
buzz and a very occasional centred pop remained. Resuming restored the centred buzz. Since the
fixed component stayed right after the larger numeric peak moved left, numeric level, signedness
and slot order do not explain it. The centred component follows engine work despite identical
post-render samples.

The default-off `--features reference-rate-tone` discriminator kept `matched-tone`'s controlled
underlay, original 4 796-left/5 327-right peaks, 1/4 master, post-render placement, codec sequence,
DMA geometry and KEY1 mapping. It changed only the complete A1S output path to 48 000 Hz: player
open, I2S setup, timing text and now-playing elapsed conversion all share that selected rate. Its
rounded full-turn phase increment is 11 184 811, retaining 125 Hz at the new rate. The historical
`matched-tone` and `swapped-tone` images remain at 44.1 kHz and retain the 12 173 944 increment.
`reference-rate-tone` is incompatible with `bench`, `tone`, `engine-tone`, `matched-tone`,
`swapped-tone` and `web`, but may be combined with `lcd`. The listening run compared the centred
buzz and intermittent pop while playing and after one KEY1 press, independently of the previously
isolated fixed-right buzz.

That reference image passed its objective run and the owner heard no buzzing at all, with no
audible change on KEY1. Production A1S output therefore adopts 48 kHz. Normal music is the final
listening check; the controlled historical diagnostic rates remain unchanged.

The same run confirmed the DMA remediation from
`plans/engine/complete/M8-task-I3a-dma-refill-remediation.md`: an `available()` error is counted and the
refill falls through to `push_with`, which can recover the descriptor accounting. A fixed
startup `dma_errors` count identifies a recovered transient; a count that keeps climbing still
means the transport is unhealthy. A second run showed that recovery alone did not establish a
steady lead: music had the right pitch but was heavily gated, and song time advanced at about
half wall speed with both counters still zero. A real-audio pre-roll in core 0 then tripped the
ProCpu stack guard. Replacing its closure with silence still panicked: the exact backtrace ended
in the existing `audio::start` → `fill` → `RenderHalf::render` prefill, showing that adding any
async prime call had enlarged `play`'s future enough for that older scratch frame to cross the
guard.

Moving the transfer to core 1 still left the underlying interrupt on core 0. The next 50-second
monitor run advanced only about 24 seconds of song, with `underruns=0` and `dma_errors=0`.
esp-hal 1.1.2 made interrupt affinity a concrete next hypothesis: `ChannelTx::into_async`
disables the DMA interrupt on `Cpu::other()` and binds it to `Cpu::current()`. Constructing the
driver on core 0 therefore left core 1's DMA futures dependent on wakeups from the core whose
UART logging masks interrupts for 7–12 ms. Star FX avoids that affinity mismatch by constructing
its whole I2S driver inside core 1.

The firmware now moves the untouched I2S peripheral parts and renderer to core 1. That core calls
`audio::start`, so driver construction, real-audio prefill and transfer start share the core that
owns the DMA interrupt. Its refill task then completes eight `push_with` descriptor handoffs of
silence and publishes a Release/Acquire startup result. Core 0 spins for at most one second with
the codec muted, reports a start failure distinctly from a timeout, logs only after readiness,
and then unmutes. An early recovery offer may be empty, while every non-empty offer is zero-filled
in place without a render scratch or larger stack. StarPlayer's 32-bit-slot descriptors are about
5.8 ms each, making this a bounded roughly 46 ms startup pre-roll; eight is the count Star FX
soaked for 40 minutes on the same board and HAL version. DMA progress and audible stereo output remain
separate acceptance checks; `plans/reference/embedded-budget.md` §4a/§4b records the underlying
findings.

A timestamped run after moving construction to core 1 unmuted at +0.57 seconds but still showed
only 0:24 engine elapsed at +49.86 seconds, with both error counters at zero. Interrupt affinity
was therefore not the main gating cause, although the ownership fix remains. The next diagnostic
run found the actual refill cadence: at +1.59 seconds `offered=written=88748`, after which both
grew by only 89–90 KB/s, `pushes` grew by about 43/s, and `empty` stayed zero. Stereo 16-bit at
44.1 kHz requires 176 400 B/s.

esp-hal ignores the macro's requested chunk when `DescriptorChain::new` constructs the ring. It
split the old 4 096-byte small circular ring into three ragged 1 366 / 1 366 / 1 364-byte
descriptors; steady `push_with` then accepted variable contiguous regions while returning
descriptor ownership, producing only about half a ring of refill per physical ring cycle. The
16-bit remediation used nine quanta = 4 608 bytes, with three equal 1 536-byte descriptors of
three render quanta or 384 stereo frames.

The first fixed-geometry build booted and made exactly one 1 536-byte steady push. Its transport
line then stayed at `offered=written=1536 pushes=1` while `dma_errors` climbed by about 115/s and
engine time stayed at 0:00. esp-hal's plain `push` performs a second `available()` internally;
after the outer check and render delay that check returned `Late`, and the outer error path then
skipped the descriptor handoff on every later iteration.

For the 32-bit framing experiment, each stereo frame doubles from four to eight DMA bytes. The
ring is six quanta = 6 144 bytes, split into three equal 2 048-byte descriptors of two render
quanta or 256 stereo frames. The whole ring is about 17.4 ms and each descriptor about 5.8 ms;
compile-time assertions preserve the geometry and the 8 184-byte small-ring limit. Required DMA
throughput is now 352 800 B/s.

The first staged-descriptor image used plain async `push`, but two fresh resets panicked after
`PLAY` and before the first telemetry line. Its internal second `available()` remained a fallible
gap even though rendering and packing had moved before the explicit outer check.

Steady refill therefore renders and packs exactly one 2 048-byte descriptor into a static `i16`
render buffer and a fixed boxed byte buffer before waiting for space, then preserves those bytes
across every availability or handoff failure. The byte buffer is allocated once inside audio
start, after `EmbeddedPlayer::open` and before DMA construction or prefill; ownership moves through
`AudioTransfer` into refill, where it is never allocated, resized or freed. Its explicit outer
`available()` validates and counts a whole-descriptor offer; the following `push_with` is
immediate, with no rendering, packing, logging or other await in between. The closure copies and
returns exactly the first staged 2 048 bytes, even when its contiguous offer contains two
descriptors. This differs from the rejected steady closure, which rendered and consumed every
offered descriptor and commonly batched the 512-frame cadence exposed by the recording. Fixed
one-descriptor work keeps the write offset and descriptor pointer advancing together. The engine
advances only after the handoff reports exactly 2 048 bytes; errors or short offers retry the same
staged data.

The packed buffer was previously another `ConstStaticCell`. Its 2 048 bytes enlarged `.bss` and
reduced ProCpu's main stack enough that the constrained-handoff matched-tone image tripped the
stack guard inside `EmbeddedPlayer::open`, before `HEAP after open`. Allocating those bytes only
after open consumes the heap region already reserved by `HEAP_BYTES`; it does not move `_bss_end`
or reduce main-stack address space. Allocation is fallible and reported as an audio-start error.

Both startup recovery and steady state use `push_with`, for distinct reasons. The eight muted
startup closures deliberately return descriptor ownership through `Late` and may receive an empty
offer. The steady closure runs only after a valid outer offer and has a strict one-descriptor copy
and complete-descriptor invariant. The cumulative transport fields `offered`, `written` and `pushes`
count one staged descriptor per valid outer offer, bytes accepted by the constrained handoff, and
steady calls, excluding muted pre-roll. Counting only the descriptor being submitted prevents a
second free descriptor from being counted again on the next iteration.

### What a good boot looks like

The **bench** build prints, in order:

```text
=== StarPlayer A1S bench ===
BUILD profile=release kernels=nearest,linear,cubic,sinc
CPU  240 MHz  (240000000 cycles/s)
SIZE voice=176 bytes  render_half=8344 bytes
HEAP [boot] …
STAGING dram=ok psram=ok
IMAGE synthetic-mod bytes=6072 (5.9 KiB)
BENCH synthetic-mod nearest flash sha256=… frames=441000 … cycles_per_frame=… core_load=…%
…
=== bench complete ===
```

Every `BENCH … linear flash` line's `sha256=` must equal the committed hash in
`goldens/<format>/<stem>__i16_mono_44100_linear.sha256`. That equality **is** M8's exit
criterion. `grep BENCH` over a captured log is the whole extraction tool.

The **C5** bench prints the same line shape from the same shared code
(`firmware_common::bench`), so the two boards' transcripts can be compared line for line:

```text
=== StarPlayer C5 bench (RISC-V, no audio hardware) ===
BUILD profile=release kernels=nearest,linear,cubic,sinc
CPU  240 MHz  (240000000 cycles/s)
SIZE voice=176 bytes  render_half=8344 bytes
HEAP [boot] …
STAGING dram=ok psram=unavailable (this devkit has no PSRAM)
IMAGE synthetic-mod bytes=6072 (5.9 KiB)
BENCH synthetic-mod nearest flash sha256=… frames=441000 … cycles_per_frame=… core_load=…%
…
BENCH petri-s3m nearest dram skipped=no staging buffer large enough
…
idle
```

Its `sha256=` lines must equal the **same** `goldens/` hashes the A1S's do — that equality
on *both* boards is M8's exit criterion, not just the A1S's half of it. `petri-s3m`'s
`dram` rows are always `skipped=`, on this board as on the A1S: its 88 036-byte image does
not fit the 32 KiB `DRAM_STAGING_BYTES` buffer, and there is no `psram` row at all — this
devkit has none fitted.

The **audio** build prints its boot sequence and then one transport line a second:

```text
StarPlayer 0.1.0 on the ESP32-A1S Audio Kit
CPU  240 MHz   heap 120.0 KiB
PSRAM 4194304 bytes (4096.0 KiB) mapped at 0x3f800000
I2C  device at 0x10 (ES8388)
JACK headphone_detect=inserted
CODEC ES8388 at 0x10: DAC up, 32-bit Philips slave, MCLK/LRCK 256, headphone -12 dB, speaker minimum, muted
MODULE image=88036 bytes (85.9 KiB) channels=8 samples=5
HEAP after open: …
HEAP after audio start: …
I2S  48000 Hz stereo 32-bit slots, MCLK on GPIO0, DMA ring 6 quanta (768 frames, 16 ms)
CORE1 audio refill running; pre-roll 8 silent descriptor handoffs complete (~42 ms)
PLAY master_volume=1/4 max=1/4 output=headphone speaker_pa=off
ord 000 pat 000 row 00/06 125 bpm   3/ 8 voices  0:01/2:47  peak=… underruns=0 dma_errors=0 offered=… written=… pushes=… …
```

The `TONE` line appears only in a `--features tone` image; normal audio images continue to play
the module and omit it. An `engine-tone` image replaces the `MODULE` line with:

```text
ENGINE-TONE output=125 Hz source=32000 Hz signed16 amplitude=28672 loop=256 frames song_loop=B00 interpolation=linear master=1/4
```

A `matched-tone` image renders the same controlled module underneath and reports:

```text
MATCHED-TONE output=125 Hz peaks=4796/5327 generator=integer-Q15 continuous underlay=engine-tone interpolation=linear master=1/4
```

A `swapped-tone` image changes only those generator peaks and reports:

```text
SWAPPED-TONE output=125 Hz peaks=5327/4796 generator=integer-Q15 continuous underlay=engine-tone interpolation=linear master=1/4
```

A `reference-rate-tone` image restores the original peaks and changes the selected rate:

```text
REFERENCE-RATE-TONE rate=48000 Hz output=125 Hz peaks=4796/5327 generator=integer-Q15 continuous underlay=engine-tone interpolation=linear master=1/4
I2S  48000 Hz stereo 32-bit slots, MCLK on GPIO0, DMA ring 6 quanta (768 frames, 16 ms)
```

An `lcd` build's boot log has one more line before `MODULE` — `LCD  ST7789
found` or `LCD  ST7789 not found — continuing headless`, from `LcdDisplay::take`'s probe.
"Not found" is not fatal: the audio still plays and the six-vs-five-key map is unaffected
either way (`lcd` always drops KEY2, whether or not a panel actually answered).

### The I2C scan expectation

The boot log's first interesting line is the bus scan, and it is **research point 1's
instrument**:

* `I2C  device at 0x10 (ES8388)` — the expected board, the v2.2+ revision. Play on.
* `I2C  device at 0x1a (AC101 — this revision is out of scope)` — the older Audio Kit
  revision. This firmware cannot drive it; M8 declared it out of scope.
* `I2C  no device answered` — check SDA on GPIO33 and SCL on GPIO32, and check that the
  board is powered from USB rather than from a battery header.

Other addresses may appear and are harmless; the scan probes `0x08`–`0x77` with a
zero-length write, which changes no register on any device.

## 4. The board

This section is the **A1S**'s pin map. The C5 touches no peripheral beyond the CPU clock,
the heap and, in the `bench` build, the `mcycle` CSR — there is no pin map for it because
there is nothing wired to describe.

| Function | GPIO | Note |
|---|---|---|
| I2C SDA / SCL | 33 / 32 | ES8388 control, 7-bit address `0x10` |
| I2S MCLK | 0 | `CLK_OUT1`; a strapping pin — MCLK only appears after boot |
| I2S BCLK / LRCK | 27 / 25 | |
| I2S DOUT (ESP → codec DSDIN) | 26 | |
| I2S DIN (codec ASDOUT → ESP) | 35 | unused; input-only pin |
| PA enable (speaker amplifier) | 21 | held low; the default firmware is headphone-only |
| Headphone detect | 39 | input-only |
| KEY1–KEY6 | 36, 13, 19, 23, 18, 5 | KEY1 is on an input-only pin; six keys by default, five (no KEY2) when `--features lcd` is built |
| LED4 / LED5 | 22 / 19 | LED5 shares KEY3 |
| Display (`--features lcd`, HSPI on the SD-card pin group) | 14 SCK, 13 MOSI, 15 CS, 2 DC, 4 RST | 12 and 15 are strapping pins; **13 is also KEY2**; GPIO12 is never wired to the panel and carries no pull |

The map lives in exactly one place in code: `boards/starplayer-a1s/src/board.rs`.

### Keys (M8-I5)

Six push-buttons, KEY1–KEY6, debounced (two agreeing 10 ms samples) and polled by
`boards/starplayer-a1s/src/keys.rs`; the debounce/edge/hold-repeat state machine itself is
`firmware-common`'s `keys` module — host-tested, fed `(now_ms, [bool; 6])`.

Six-key map (default build):

| Key | GPIO | Press | Hold ≥ 1 s |
|---|---|---|---|
| KEY1 | 36 | play / pause | — |
| KEY2 | 13 | stop (rewind to order 0) | — (I6: re-provision) |
| KEY3 | 19 | previous order | seek back one order every 200 ms |
| KEY4 | 23 | next order | seek forward one order every 200 ms |
| KEY5 | 18 | volume − (1/64 step) | repeat every 200 ms |
| KEY6 | 5 | volume + (1/64 step) | repeat every 200 ms |

Five-key map (`--features lcd`, GPIO13 is the display's MOSI so KEY2 is unavailable):

| Key | GPIO | Press | Hold ≥ 1 s |
|---|---|---|---|
| KEY1 | 36 | play / pause | stop (rewind to order 0) |
| KEY3 | 19 | previous order | seek back one order every 200 ms |
| KEY4 | 23 | next order | seek forward one order every 200 ms |
| KEY5 | 18 | volume − (1/64 step) | repeat every 200 ms |
| KEY6 | 5 | volume + (1/64 step) | repeat every 200 ms |

Volume is applied through `ControlHalf::set_master_volume` and lives in RAM only. The A1S
firmware starts at and caps every control path to exact **1/4**; KEY5/KEY6 move in exact
**1/64** steps. HTTP and WebSocket requests above 1/4 are clamped to the same board maximum.
The web slider exposes 0–1/4 and reports that engine scale as 0–25%. The ES8388 contributes
another −12 dB in its headphone analog registers. This board policy does not change the engine
or other hosts, whose default and available range remain unity.

### The display (M8-I5, `--features lcd`)

An ST7789 SPI IPS panel, 1.69" 240×280 (a 240×320 ST7789 RAM windowed with a 20-row
y-offset), on HSPI: SCK 14, MOSI 13, CS 15, DC 2, RST 4, 20 MHz to start (research point 3
— raise towards the panel's 40 MHz ceiling once the owner has measured a clean redraw on
the board in hand). Off by **default**; `cargo xtask build --board a1s --features lcd`
builds it in. `boards/starplayer-a1s/src/lcd.rs` is the only file that names `mipidsi` or
`embedded-graphics`'s driver types; what is actually drawn is
`firmware-common::screen::Screen`, host-tested against `embedded_graphics::mock_display`.

The screen never panics if the panel is loose or absent: `LcdDisplay::take` goes headless
on the first SPI/init error (the same fail-soft shape ampkeeper's `display.rs` uses for its
I2C LCD backpack) and every draw call after that is a no-op.

`mipidsi = "0.9.0"` and `embedded-graphics = "=0.8.1"` — **not** the newer `mipidsi 0.10.0`
/ `embedded-graphics 0.8.2` (M8-I5 research point 2). The newer pair tightens its
`fixed`/`az` version requirements to a range that cannot be satisfied alongside
`starplayer-core`'s `fixed = "1.31"` at all — `cargo` refuses to resolve a single `az`
version for the whole graph. See the long comment beside `embedded-graphics` in
`embedded/Cargo.toml`'s `[workspace.dependencies]` for the exact ranges. Neither mipidsi
release needs `display-interface-spi`: both carry their own
`mipidsi::interface::SpiInterface`.

### DIP switches

The Audio Kit carries a five-way DIP switch block beside the SD slot. It multiplexes the
SD-card pin group between the slot, the JTAG header and the key matrix, and the silkscreen
labelling differs between board revisions. For **this** firmware:

* nothing here uses the SD card or JTAG, so any switch position that does not route the
  group to the SD slot boots and plays;
* the six-key default build reads KEY1–KEY6 as plain GPIO inputs — no SD card, no display,
  the SD-card pin group's DIP position does not matter to it;
* the `--features lcd` build takes the SD-card pin group (GPIO14/13/15/2/4) for the ST7789
  display, which means the SD slot must be switched **off** — a display and an SD card
  cannot both have those pins, and GPIO13 being both KEY2 and the display's MOSI is exactly
  why the `lcd` build drops to five keys.

Leave them as the board shipped for the six-key default build; switch the SD-card group off
before flashing an `lcd` build, and record what the board in hand is actually set to when
you do.

### Partition table — 4 MB, no OTA

`boards/starplayer-a1s/partitions.csv`:

| Name | Type | Offset | Size | Used for |
|---|---|---|---|---|
| `nvs` | data | `0x9000` | 24 KB | — |
| `phy_init` | data | `0xf000` | 4 KB | — |
| `factory` | app | `0x10000` | 2.5 MB | the firmware |
| `modules` | data | `0x290000` | 1.375 MB | M8-I6's uploaded modules |
| `config` | data | `0x3f0000` | 64 KB | M8-I6's WiFi credentials |

No OTA slots: this is a bring-up firmware flashed over USB, and an `ota_0`/`ota_1` pair
would halve the app budget for a feature the milestone does not need. `modules` and
`config` are declared now rather than later because adding a partition row moves every row
after it and invalidates whatever a device had stored.

## 5. Layout

```text
embedded/
  Cargo.toml              the workspace and every [profile.*]
  rust-toolchain.toml     channel = "esp-1.97"  (the A1S's; the C5 is built under +1.97 instead)
  .cargo/config.toml      the `xtask` alias and nothing else
  firmware-common/        board-independent: the bench runner, the now-playing view model,
                          the key debounce state machine, the screen renderer, the
                          formatting helpers — builds and tests on the host
  boards/starplayer-a1s/  the Xtensa firmware
    .cargo/config.toml    target, runner, -Tlinkall.x, build-std  (directory-scoped!)
    partitions.csv
    src/board.rs          the pin map, once
    src/es8388.rs         the codec driver
    src/audio.rs          I2S + circular DMA + the refill task (runs on core 1)
    src/keys.rs           the six push-buttons as GPIO inputs, always built
    src/lcd.rs             the ST7789 SPI driver, #[cfg(feature = "lcd")] only
    src/images.rs         the aligned include_bytes! wrappers
    src/bench.rs          the `bench` build's runner
    src/psram.rs          the PSRAM arena, #[cfg(feature = "web")] only (M8-I6/I8)
    src/psram_task.rs     PSRAM picoserve futures with internal Embassy headers (`web` only)
    src/store.rs          the config and modules partitions, `web` only
    src/net.rs            station mode, embassy-net and mDNS, `web` only
    src/provisioning.rs   the captive portal personality, `web` only
    src/web.rs            picoserve, the JSON/WebSocket API and module upload, `web` only
    src/main.rs           boot, core pinning, the control/keys/display tasks
    ld/stack-floor.x      a linker ASSERT that fails the build if the main stack drops
                          under 32 KiB (see §6)
    build.rs              adds that fragment to the link
  www/index.html          the page the `web` build serves, gzipped into assets/ by xtask
  boards/starplayer-c5/   the RISC-V bench firmware (M8-I4) — no audio hardware
    .cargo/config.toml    target, runner, -Tlinkall.x — no build-std (research point 2)
    partitions.csv
    src/images.rs         the aligned include_bytes! wrappers (bench-only)
    src/bench.rs          the `mcycle`-backed board glue over firmware-common's runner
    src/main.rs           boot — synchronous `#[esp_hal::main]`, no embassy/esp-rtos
  assets/                 git-ignored build products: *.spmi module images, index.html.gz
  xtask/                  build / image / size / assets / flash / monitor
```

## 6. Web control (M8-I6, `--features web`)

Off by default. A build without it links no radio, no TCP/IP stack and no HTTP server, and
is byte-for-byte the firmware M8-I5 shipped.

### Getting the player onto a network

There are no build-time credentials (M8 master-plan decision 5). The first boot with no
stored network — and any boot where the re-provision key is held — comes up as a **captive
portal** instead:

1. Flash `--features web` (or `web,lcd`) and power the board. It plays the compiled-in
   module in both personalities, so the music is how you know it is alive.
2. Join the open network **`StarPlayer-XXXX`** from a phone or laptop (the suffix is the
   last two bytes of the board's SoftAP MAC, so two boards on one desk are
   distinguishable). The phone's own captive-portal detection should open the page; if it
   does not, browse to **http://192.168.4.1/**.
3. Pick the network from the list the board scanned before it became an access point, type
   the passphrase, and press **Save and restart**. The board writes the credentials to the
   `config` partition and soft-resets two seconds later — the delay is what lets the
   confirmation page reach the phone before the radio goes.
4. It comes back in station mode and answers at **http://starplayer.local/** (mDNS). The
   UART log prints the DHCP address too, for a network where mDNS does not work.

**To provision it again**, either hold the re-provision key at boot for five seconds, or
`curl -X POST http://starplayer.local/api/reprovision` — both end with the portal. The key
is **KEY2** in the six-key build and **KEY1** in the `lcd` build, where KEY2's GPIO is the
display's MOSI and there is no KEY2 at all. A boot that nobody is touching costs one GPIO
read; the key must be held from power-on, and the music plays while you hold it.

There is no password on the portal and none on the API. The threat model is a music player
on a desk; anyone already on the network can change the song, which is the point.

### The API

Port 80. Everything is JSON in and JSON or plain text out; every refusal carries a sentence
written for a person, not a code.

| Method, path | Body | Answer |
|---|---|---|
| `GET /` | — | the page, gzipped, `Content-Encoding: gzip` |
| `GET /api/status` | — | the transport, the module and up to 16 channel rows |
| `GET /api/modules` | — | the compiled-in module plus every stored slot |
| `GET /api/upload-limits` | — | current raw/image buffer limits and the 252 KiB flash-slot limit |
| `POST /api/play`, `/api/stop`, `/api/next`, `/api/previous` | — | 204 |
| `POST /api/seek` | `{"order":12}` | 204 |
| `POST /api/volume` | `{"level":16384}` | 204 (0–16384; higher values clamp to 16384) |
| `POST /api/mute` | `{"channel":3,"muted":true}` | 204 |
| `POST /api/modules` | raw module bytes, or a `.spmi` image | 201, or 409/413/507 with the reason |
| `POST /api/modules/select` | `{"id":2}` | 204 — `0` is the compiled-in module, `1..5` a flash slot |
| `POST /api/modules/store` | `{"id":3}` | 204 — **pauses playback**, see below |
| `POST /api/reprovision` | — | 204, then a reset into the portal |
| `GET /ws` | WebSocket | binary telemetry at 10 Hz; accepts 9-byte command frames |

```sh
curl http://starplayer.local/api/status
curl -X POST http://starplayer.local/api/play
curl -X POST --data-binary @PETRI.S3M http://starplayer.local/api/modules
curl -X POST -H 'content-type: application/json' -d '{"id":1}' http://starplayer.local/api/modules/store
```

The WebSocket's wire format is the **same flat word layout the browser player uses**
(`crates/starplayer-host-wasm/src/lib.rs`): 22 header words then eight words per channel,
little-endian `i32`. `firmware-common/src/api.rs` owns this end of it and its host tests
are what keep the two in step.

### Uploading a module, and what actually fits

Two kinds of file are accepted at `POST /api/modules`:

* **A module image** (`.spmi`, what `cargo xtask module-image` writes in the main
  workspace). It is streamed into PSRAM, copied to the free image buffer in 4 KiB steps
  and played **borrowed in place**.
* **A raw module file** (`.mod`, `.s3m`, `.mtm`, `.xm`, `.it`). The device decodes it with
  an incremental caller-buffer decoder. Canonical pattern bytes and decoded PCM go
  directly from the staging buffer into the free PSRAM image buffer; the reusable 256 KiB
  PSRAM workspace carries bulk decode state. A step scans at most 4 KiB of input or emits
  at most 1,024 PCM frames before yielding.

At boot the firmware reserves 64 KiB for network futures and 256 KiB for the decoder,
then divides all remaining PSRAM into three equal, four-byte-aligned buffers: staging,
current image and replacement image. A 4 MiB part gives each buffer 1,288,872 bytes (with
8 bytes left over). `GET /api/upload-limits` reports those detected capacities; on a board
without usable PSRAM the raw and image limits are zero. Saving remains limited by the
252 KiB flash-slot format even when the playing image is larger.

Only small image index tables, processor state and the sequencer use internal DRAM. Every
allocation in replacement preparation is fallible, timeline tables live in the unused
tail of the image buffer, and a replacement is refused if it would leave less than 8 KiB
of internal heap free. A failed or cancelled request leaves the current playback intact.
The web firmware admits the full 64-channel native tracker width and provides 64
simultaneous voices through the compact master-only engine layout. Traditional formats
therefore have one voice available per channel. IT New Note Action activity can need more
than 64 simultaneous voices; when it does, the established voice-stealing policy applies.
Modules declaring more than 11 channels remain playable and raise a persistent browser
warning plus one UART warning when adopted, because 11 is the measured audio-only
unfiltered voice limit rather than an independently certified S3M channel limit.
Each image buffer has a preallocated DRAM `Arc` token; reuse waits until its strong and
weak reference counts prove that the command queue, render engine and retired source have
all released it. A timeout returns busy and never overwrites the buffer.

The owner's `ARMANI.S3M` acceptance file measures 17,554 bytes raw and 28,812 bytes as a
byte-identical SPMI image. Host allocation instrumentation measured a 5,158-byte decoder
peak with nothing retained, 4,034 bytes for fallible image adoption, 1,380 bytes for a
scan using caller-owned tables, and 420 bytes retained by the live processor. Its scan is
672 row marks over 122.88 seconds. These are 64-bit host allocation figures; the linked
Xtensa measurements from the 2026-09-15 acceptance image are 55,948 bytes of main stack
(`web`), 54,500 bytes (`web,lcd`), and 1,309,104 bytes of application flash (49.94% of
the factory partition). The lowest free internal heap at logged upload checkpoints was
45,136 bytes. ARMANI uploaded in 0.71 seconds; a 300,304-byte raw S3M producing a
600,856-byte image uploaded in 3.02 seconds, and that SPMI uploaded in 5.46 seconds.
All five formats, malformed/oversized refusal, interrupted/concurrent uploads, repeated
replacement, and oversized flash-store refusal passed `tests/web_upload_smoke.py`.
The 60-second HTTP/WebSocket soak delivered 542 telemetry frames. About 220 seconds
of UART capture showed zero underruns, no crashes, and a DMA error count unchanged
from its startup baseline of one. Owner listening acceptance remains separate.

The `Arc` that owns an uploaded module, and everything else with an atomic in it, stays in
**DRAM**: on the classic ESP32 the atomic instructions do not work on PSRAM, so this
firmware never registers PSRAM with the allocator at all (`src/psram.rs` has the full
argument).

### The flash-write pause — the one thing that is audible

On the classic ESP32 an erase or a write **turns the instruction and data caches off** for
its duration. Everything mapped through that cache becomes unreadable: code executing from
flash, the compiled-in module image, and PSRAM, which shares the cache. esp-storage refuses
a write outright while the second core is running, and this firmware runs the audio refill
on core 1 — so a write parks it.

`POST /api/modules/store` therefore **stops the transport first**, waits for the 64-frame
ramp and the 17 ms DMA ring to drain to silence, writes, and plays again. Storing a 90 KB
module is a few seconds of silence, and that is the designed behaviour, not a fault; the
page warns about it beside the button. `POST /api/modules/select` for a flash slot does the
same for a long read, which is safe but contends for the flash bus badly enough to cost
underruns. Saving WiFi credentials pauses playback the same way, and is followed by a reset
anyway.

### The DRAM budget

The `web` build is close to the chip's limit and the limit is **not** flash (the verified
I8a `web,lcd` image uses 49.6% of its partition) — it is the 192 KiB of internal DRAM, out
of which core 0's main stack is whatever `.bss` leaves behind. Three things keep it in
bounds, and all are easy to undo by accident:

* **The 144 KiB web heap is split across two Internal regions** (`main.rs`'s
  `WEB_RECLAIMED_HEAP_BYTES` and `WEB_INTERNAL_HEAP_BYTES`). The 96 KiB `dram2_seg`
  region is registered first; the 48 KiB `.bss` reserve supplies the radio's dynamic
  internal allocations after I8 recovered the necessary stack space. PSRAM is not an
  allocator region.
* **The picoserve worker bodies are in PSRAM, not static Embassy pools**
  (`psram_task.rs`). The pre-I8 image linked both mutually exclusive pools into `.bss`:
  68 560 bytes for the two station workers and 10 088 bytes for the one portal worker.
  I8 keeps each 48-byte Embassy header/pointer proxy in the internal heap and constructs
  only the selected personality's large future bodies directly in the claim-only PSRAM
  arena: 25 584 bytes for station after I8c, or 10 048 bytes for the portal. The arena is still not
  registered with the global allocator.
* **Routing is one flat `match`, and every JSON answer is one body type** (`web.rs`).
  picoserve monomorphises `write_to` per response type, and a `.route()` chain costs a
  stack frame per route. The first draft's two web workers were 97 KiB; the compact pair
  is 26 864 bytes in PSRAM after I10. Each worker retains one 2 048-byte response
  buffer and passes a borrowed handle through the response code. This reduces the two
  largest nested request frames from 38 912 / 27 824 bytes to 8 496 / 2 992 bytes;
  putting future state in PSRAM alone did not bound execution-stack usage.
* **The web engine has 64 channels and 64 voices, with master-only routing, scope sample
  rings disabled and both scalar telemetry rings one snapshot deep.** Scalar status and
  WebSocket telemetry remain available. The channel capacity admits every native tracker
  width. The separate voice capacity covers one foreground voice per IT channel; additional
  IT NNA voices use the normal stealing policy.

`ld/stack-floor.x` fails the build if the main stack drops under 32 KiB. The exact linked
core-0 stack is logged once at boot. The I8c links leave 57 548 bytes in `web` and 56 116
bytes in `web,lcd`; default leaves 35 608 bytes and `lcd` leaves 34 200 bytes. If the
assertion fires, shrink or move the new static — do not lower the floor.
The I10 64/64 release links retain 55 940 bytes in `web` and 54 492 bytes in
`web,lcd`, so both continue to clear that floor.

### A1S voice-capacity benchmark (M8-I9)

`voice-bench` keeps the accepted audible path intact: fixed-point stereo at 48 kHz, six
128-frame quanta in the three-descriptor DMA ring, and therefore 256 frames and a
5,333.33 µs deadline per descriptor. Its 20% headroom gate is 4,266 µs. The workload is
a generated native instrument-mode IT module whose looping PCM and pattern data are
borrowed from the claim-only PSRAM arena. The engine uses the explicit master-only
layout, one-entry scalar telemetry and no scope rings. Normal `web` and `web,lcd` use
the compact 64-channel/64-voice settings selected in I10. The benchmark reserves a fixed
32 KiB
PSRAM image buffer whose maximum workload fit is host-tested. Module construction runs in
a separate Embassy task after async main yields. Player allocation and the fallible
timeline load run in a second separately-polled task; `StaticCell::init_with` constructs
the 11,744-byte render half in place, and only static references return to main. In the
maximum audio-only build this reduces main's poll frame from 34,112 to 544 bytes; its
37,048-byte linked stack leaves 36,504 bytes above main and 11,176 bytes above the
25,872-byte player setup frame. `cargo xtask build` and `image` inspect the ELF and reject
voice-bench builds if main exceeds 8 KiB, player setup exceeds 27 KiB, or either leaves
less than 4 KiB dynamic headroom. The production heap, timeline and 32 KiB linked stack
floor remain intact.

The owner runs the complete four-way search from the repository root:

```sh
python3 embedded/tests/a1s_voice_bench.py run \
  --board-ip starplayer.local \
  --raw-log a1s-voice-bench.log \
  --json-report a1s-voice-bench.json
```

For every audio/web and filtered/unfiltered candidate, the controller exports its exact
channel/voice count and bisection bounds, invokes `embedded/flash_image.sh`, then captures
that reboot through RFC2217. Each candidate's UART is preserved verbatim under
`a1s-voice-bench.log.d/`; the top-level log is the globally sequenced proof assembled
from those runs. The search probes 64 channels/256 voices first, bisects voices from 32
through 256 at 64 channels, and falls back to bisecting channels at 32 voices. If no
channel passes at 32 voices, it holds one channel and bisects voices from 1 through 31,
starting at 31, so the report still records the actual hardware limit and adjacent reject.
When a channel ceiling exists and the repeated voice search reaches 256, the controller
retains and soaks the known adjacent channel reject at its measured 32-voice baseline.
After opening the firmware console at 115200 baud, the benchmark runner
uses DTR/RTS to reset the attached app, then begins its capture deadline. The firmware
waits three seconds after its human-readable boot banner before workload construction or
any machine record, so capture is ready after that attached reset. The wrapper's
460800 baud setting applies only while esptool writes the image. Each candidate build sets
`RUST_MIN_STACK=268435456` for stable Xtensa thin-LTO worker threads during the repeated
search. The controller requires
`--board-ip` before starting any candidate because all four searches include web-loaded
cases. The web cases keep two HTTP clients issuing status/module/upload-limit reads and
invalid upload requests while a WebSocket telemetry client remains connected.
Invalid uploads exercise staging and decoding without replacing the stress module. The
host records locked success/failure counts per workload class, while boot-reset firmware
counters in every `SAMPLE`/`END` prove successful HTTP reads, upload-parser responses and
WebSocket telemetry came from the candidate board and continued throughout the case.

After an interrupted run, add `--resume` with the same artifact directory and durations.
The runner strictly validates each existing `<case>.uart.log` and web-load sidecar against the current mode,
filter, capacity, duration, phase, axis and bisection bounds before reusing it. Missing
artifacts run normally; malformed or mismatched artifacts abort without being overwritten.
Without `--resume`, every candidate is rebuilt, flashed and captured as before.

Qualification lasts 60 seconds. The controller then rebuilds, flashes and captures the
highest pass and its adjacent rejection for 600 seconds, so both soaks execute. If the
provisional pass rejects during its long soak, the controller retains the completed
qualification outcomes and tries the highest qualified lower capacity. It performs a
bounded qualification/long-soak bisection when needed until the final long pass and long
reject are numerically adjacent. The failed provisional soak remains reusable under its
original case name and may become the final reject proof. A
firmware setup failure is a structured capacity rejection, including a fallible timeline
scan that keeps the shipping host's timeline memory in the measured capacity. Build,
link, flash and serial failures abort the search. Replay is hardware-free:

Before a web benchmark starts either station or provisioning networking, it requires
56 KiB of total free internal heap. The web heap's added 48 KiB exists for radio and
esp-rtos dynamic allocations; the remaining 8 KiB is the benchmark's retained heap gate.
This voice-bench-only preflight catches an already consumed or fragmented heap before the
radio reaches its infallible task-stack allocation. Normal web startup is unchanged, and
the preflight does not replace the post-start 8 KiB pass criterion.

```sh
python3 embedded/tests/a1s_voice_bench.py verify a1s-voice-bench.log \
  --json-report a1s-voice-bench-replay.json
```

The verifier requires all four final `RESULT` records and rejects wrong production DMA
geometry, missing or reordered records, a post-START reset, inconsistent counters,
truncated duration and incomplete or incorrectly bound boundary soaks. RESULT records must
name the exact mode/filter search result and its expected voice- or channel-axis reject.
After long-soak fallback, the verifier replays the bounded refinement order and requires
the named stable pass and immediately higher long reject; the reject's historical case
name does not need to say `reject`.
A rejecting minimum 1-channel/1-voice qualification is represented explicitly as a
zero-voice limit with `pass_case=none`; the exact 1x1 case remains the rejection proof.
A claimed pass must
also have correct transport rate, voice plateau and heap reserve with no warnings, DMA
growth, underruns or deadline misses. Those predicate failures are retained as legitimate
`reason=criteria` rejection evidence instead of aborting bisection.
Periodic voice checks after a five-second workload settle must remain at the requested
plateau. Web-load counters may remain zero or unchanged during their own five-second
connection settle. Starting with the first sample at or after that boundary, all three
must be positive and each must have advanced within the preceding five seconds. A counter
exactly five seconds past its last observed increase still passes; a longer stall fails.
`END` applies the same window. The firmware updates the heap low-water mark every 10 ms control tick and around
web control polling; the verifier requires that minimum never recover in later records.
The capture path streams each decoded UART line to its artifact before validation, so
panic, reboot, malformed, sequence and wrong-case failures retain the evidence that caused them.

The complete 2026-09-15 run and independent replay produced these reliable limits:

| Personality | IT filter | Limit | Adjacent rejection | Passing p50 / p95 / max |
|---|---:|---:|---:|---:|
| audio-only | off | 1 channel / 11 voices | 1 / 12 at 4,530 µs | 4,266 / 4,266 / 4,189 µs |
| audio-only | on | 1 channel / 5 voices | 1 / 6 at 4,620 µs | 4,266 / 4,266 / 4,225 µs |
| web-loaded | off | 1 channel / 2 voices | 1 / 3 at 4,343 µs | 2,200 / 3,200 / 4,185 µs |
| web-loaded | on | zero voices | 1 / 1 at 4,672 µs | no passing case |

All accepted cases had zero deadline misses, underruns, DMA-error growth and engine
warnings. The web result retained at least 61,568 bytes of internal heap, so CPU timing
is the limiting resource. The measured production recommendation is two voices with IT
filters disabled while the web personality is active. I10 deliberately selects a
64-channel/64-voice production pool so all native widths load and each IT channel has one
foreground voice available; the status/UI warning above 11 makes the measured timing limit
visible without turning it into an admission limit.

## 7. Notes for the next person

* **`-C force-frame-pointers` is deliberately absent** from the board's rustflags. With it
  on, this firmware does not compile: the Xtensa LLVM register allocator fails with
  `Cannot scavenge register without an emergency spill slot`. Ampkeeper hit the same bug
  from the other direction on the S3. The comment in `.cargo/config.toml` has the detail.
* **The heap is 120 KiB and the number is a balance against the stack.** On the classic
  ESP32 core 0's main stack is whatever DRAM is left between `_bss_end` and `0x3ffe_0000`,
  so every heap byte (and every other `.bss`/`.data` byte — core 1's 8 KiB stack included,
  since M8-I5) is a byte core 0's stack does not get. At 160 KiB the firmware links and
  leaves 6 472 bytes of stack, which will not survive a boot; at 120 KiB the current
  default build's stack is 35 608 bytes and the `lcd` build's is 34 200 bytes. Both clear
  the 32 KiB linker floor, but check after any change that moves a large static:

  ```sh
  xtensa-esp32-elf-nm target/xtensa-esp32-none-elf/release/starplayer-a1s \
    | grep -E " (_bss_end|_stack_start)$"
  ```

  The non-web heap array reserves capacity in `.bss`; allocations consume that fixed region and
  do not move `_bss_end`. Audio's 2 048-byte packed descriptor is deliberately allocated from it
  only after player open. The `web` build's same allocation comes from its fixed two-region
  Internal heap; both reserves already exist by then, so that allocation does not move `_bss_end`.

* **PSRAM is mapped but not added to the general heap** in the audio build. The ESP32's
  atomic instructions do not work correctly on PSRAM, and this engine puts atomics on the
  heap (`Arc` refcounts, the host's seqlocks, the telemetry ring). `src/main.rs` has the
  full reasoning; M8-I6 inherits the constraint. I8's picoserve workers use direct,
  aligned monotonic arena claims instead: their task headers and every shared atomic stay
  in internal DRAM. A structural 64 KiB prefix holds one portal future (10 048 bytes) or
  two station futures (26 864 bytes total); the remainder holds the decoder workspace and
  three equal dynamic module buffers described above.
* **The whole async audio driver and refill task run on core 1** — the refill moved there in
  M8-I5, and the driver construction followed after the hardware result above. M8-I3's write-up
  blamed `RenderHalf` not being `Send` on the engine's `Box<dyn EventSource>` and kept everything
  on core 0 as a result; that diagnosis did not hold once actually checked (`EventSource`
  and `Insert` both already carry a `Send` supertrait bound, so `RenderHalf` was already
  `Send` — see the compile-time assertion beside its definition in
  `crates/starplayer-host-embedded/src/player.rs`). The real, and genuinely unavoidable,
  `Send` obstacle is esp-hal's untouched peripheral-token bundle (`audio::Parts`), which
  main.rs crosses with a small `unsafe impl Send` wrapper (`SendAudioParts`) whose safety
  argument is a one-time ownership handoff into core 1's entry closure, the same shape
  `esp-rtos`'s own internal `SecondCoreStack` wrapper uses. `audio::start` calls `into_async`
  only after that handoff, binding the DMA interrupt to core 1. Core 0 now runs the keys task,
  the control task and (under `lcd`) the display task; see the "Core pinning" section of
  `src/main.rs`.
* The measured sizes live in `plans/reference/embedded-budget.md`, which is where any new
  number belongs.
* **The C5 needs no `-Z build-std` and no Xtensa toolchain at all** — it is built under
  the main workspace's own pinned stable `1.97`, not `esp-1.97` (M8-I4 research point 2,
  `plans/reference/embedded-budget.md` §8). This is a **per-board** choice
  (`embedded/xtask`'s `Board::toolchain`), not a workspace-wide one: `esp-1.97` still pins
  `embedded/rust-toolchain.toml` at the root for the A1S, and `cargo +1.97` on the C5's
  invocation overrides that per build.
* **The C5's `bench` heap is 176 KiB, not the A1S's 120 KiB**, because this board has no
  PSRAM to take `DRAM_STAGING_BYTES` off the internal heap the way the A1S's `External`
  region does. `boards/starplayer-c5/src/main.rs`'s `HEAP_BYTES` doc comment has the
  arithmetic; `plans/reference/embedded-budget.md` §2 has the budget it is sized against.

The web request hardware regression test is `python3 embedded/tests/web_smoke.py <board-ip>`.
Close other StarPlayer tabs first so both workers are available. It validates HTTP responses
and sixty seconds of WebSocket telemetry alongside HTTP, and reports bounded connection-refused
retries while listeners cycle. Capture UART separately to check audio and panic diagnostics.
