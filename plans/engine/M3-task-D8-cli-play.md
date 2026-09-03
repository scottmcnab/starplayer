# M3 — D8: `starplayer play`

| Field | Value |
|---|---|
| Milestone | M3 ([master plan](M3-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Ready |
| Depends on | D4 (cpal host) and D5 (CLI) — both landed |
| Blocks | M3 exit criterion "`starplayer play foo.s3m` works on Linux" |
| Parallel with | F2, G2, G3 |
| Recommended model | Claude Sonnet (wiring an existing player into an existing CLI) |
| Verified by | agent (`cargo test -p starplayer-cli`, `starplayer play` audible on this machine), then reviewer, then the owner listens |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine; the apps are `std`. Read
`AGENTS.md` first.

Task D4 landed `starplayer-host` (`Player`, `AudioBackend`, `AudioSpec`, `ManualBackend`
for tests) and `starplayer-host-cpal` (the cpal backend and an `examples/play.rs` that
already does everything this command needs: device listing and selection, rate and
buffer negotiation, loading and scanning a module, playing to its natural end or its loop
point, printing order/row/BPM once a second). Task D5 landed `apps/starplayer-cli` with
`info`, `render` and `trace`, and a `play` stub that exits 2. This task replaces the stub
with the example's behaviour, so the M3 exit criterion is met from the CLI itself.

### Code you must read before changing anything

- `crates/starplayer-host-cpal/examples/play.rs` — the behaviour to move, verbatim in
  spirit; `crates/starplayer-host-cpal/src/lib.rs` (`CpalBackend`), `crates/starplayer-host/src/{lib,player,backend}.rs`.
- `apps/starplayer-cli/src/{main,info,render,archive}.rs` and `Cargo.toml` — the argument
  style (clap derive), the zip handling, the error convention (one line, non-zero exit).
- `plans/engine/complete/M3-task-D4-cpal-host.md` research resolution (the WSL2 device
  facts: PulseAudio is what finds a device here; `--buffer` defaults to 1024 for the WSLg
  sink) and `plans/reference/original-star-ui.md` §7 (the original's argument surface: a
  mixing rate, a buffer size, a device selection).

## Deliverables

1. **`starplayer play <file> [--entry N] [--device NAME] [--rate HZ] [--buffer FRAMES] [--repeat] [--list-devices]`**
   in `apps/starplayer-cli/src/play.rs`: build the cpal backend, list or select a device,
   negotiate the spec, open a `Player`, load the module (through `archive`), play; print
   the title and negotiated spec once, then order/pattern/row/speed/BPM/voices/peak once a
   second on one updating line; stop at the natural end or the loop-point fade unless
   `--repeat`; exit 0. `Ctrl-C` stops the transport click-free (the transport ramp) and
   exits 0 — use `ctrlc` pinned in `[workspace.dependencies]` or a small signal handler;
   no busy loop.
2. `starplayer-host-cpal` becomes a dependency of the CLI; the example stays (it is the
   host crate's own acceptance test) but any shared helper moves into `starplayer-host`
   rather than being copied.
3. `--help` reads well; the crate docs' usage block lists `play`.
4. `plans/engine/M3-master-plan.md`: exit criterion status updated.

## Research points

1. **A machine with no device.** Confirm `play` fails with one clear line naming
   `--list-devices` and exits non-zero, by forcing the backend to find none in a test
   (the `ManualBackend` or an env override), never by panicking.
2. **`--repeat` and `AtEnd`.** Map `--repeat` to `AtEnd::Continue` and the default to the
   D1/D2 fade rule the web player uses; note where the fade length comes from.

## Verification

```sh
cargo test -p starplayer-cli
cargo run -p starplayer-cli -- play --list-devices
cargo run -p starplayer-cli -- play crates/starplayer-s3m/tests/fixtures/NICETUNE.S3M           # audible; exits 0 at the end
cargo run -p starplayer-cli -- play crates/starplayer-s3m/tests/fixtures/NICETUNE.S3M --rate 44100 --buffer 512
cargo test --workspace
cargo xtask ci --job clippy
cargo xtask ci --job host-tests
```

Report the exact commands and results. **Do not commit** — the reviewer commits.

## Out of scope

The TUI (A1). MIDI. Any engine or host-crate change beyond moving a helper.

## Research resolution

### 1. A machine with no device — **`play_on` takes `&mut dyn AudioBackend`, so a test double proves the one-line, non-panicking failure without a sound card**

`starplayer play`'s device-opening logic is split out of `run` into a private
`play_on(backend: &mut dyn AudioBackend, backend_name: &str, file_display: &str, bytes:
&[u8], args: &PlayArgs, interrupted: &AtomicBool) -> Result<(), String>` in
`apps/starplayer-cli/src/play.rs`. `run` is the only caller that builds a real
`CpalBackend`; everything downstream of it goes through the `AudioBackend` trait object,
exactly the shape `Player::open` already asks for.

The test `a_backend_with_no_device_fails_with_one_line_naming_list_devices_and_never_panics`
(`apps/starplayer-cli/src/play.rs`, `tests` module) defines a local `NoDeviceBackend`
whose `devices()` returns empty and whose `negotiate`/`open` both return
`Err(HostError::NoDevice)` — the same shape `cpal::default_host()` reports on a machine
with no sound card at all (task D4's research resolution measured this exact case: ALSA
alone finds only `null`). Driving `play_on` with it exercises the real failure path
(`Player::open(...).map_err(...)`) with no `ManualBackend` and no env override needed,
since the trait object is the seam already: `ManualBackend` was considered but it always
negotiates a device (`MANUAL_DEVICE_NAME`), so it cannot express "no device" without a
second, harness-only device name being invented on it — a small purpose-built double is
more direct and does not touch `starplayer-host` at all, keeping this task inside its
stated scope. The assertion checks the returned `String` is one line (`!contains('\n')`)
and names `--list-devices`; it never calls `.unwrap()`/`.expect()` on the failure, so a
regression that turned this into a panic would fail the test rather than aborting it.

The message itself: `could not open an output stream on the {backend_name} backend:
{error}; \`--list-devices\` shows what this machine has`. `main.rs`'s generic `Err(String)`
handling (shared by `info`, `render`, `trace`) prints it as `starplayer: {message}` to
stderr and returns `ExitCode::FAILURE` — the CLI's "one line, non-zero exit" convention,
rather than the example's own two-`eprintln!` shape (which is fine for a standalone
binary that owns its own exit path, but the CLI already has a place for this).

### 2. `--repeat` and `AtEnd` — **`--repeat` maps to `AtEnd::Continue`; the default is `AtEnd::FadeOut` with a five-second fade, matching the web player's `SONG_FADE_SECONDS`, not the example's own six**

`play_on` sets the transport policy right after `player.load`:

* `args.repeat` → `player.set_at_end(AtEnd::Continue)` — wrap and keep playing from the
  loop point or the restart order, repeat-on.
* the default → `player.set_fade_frames(LOOP_FADE_SECONDS.saturating_mul(spec.sample_rate_hz))`
  then `player.set_at_end(AtEnd::FadeOut)`. `AtEnd::FadeOut` covers both endings by
  itself (`crates/starplayer-core/src/event.rs`): a song that runs out of order list has
  no second pass to fade into and stops exactly as `AtEnd::Stop` would (task D2); a song
  that jumps back to a loop point plays one more pass under a fading transport.

**Where the fade length comes from.** `Transport`'s own constructor default
(`DEFAULT_FADE_FRAMES` in `crates/starplayer-host/src/transport.rs`) is a raw frame
count, `10 * 48_000` — correct only at 48 kHz, quietly wrong (9.19 s at 44.1 kHz, 5.12 s
at 96 kHz) at any other negotiated rate, because nothing scales it. Both the example and
this command sidestep it by calling `Player::set_fade_frames` explicitly with a
frame count computed from the negotiated `spec.sample_rate_hz`, which is the only way to
get a fade whose *duration* is independent of the negotiated rate. The example
(`crates/starplayer-host-cpal/examples/play.rs`) picked its own constant, six seconds,
with no stated reason. This task's deliverable ties the CLI's default to "the D1/D2 fade
rule the web player uses" instead: `apps/starplayer-web/www/app.js` defines `const
SONG_FADE_SECONDS = 5` and computes `Math.round(SONG_FADE_SECONDS * sampleRate)` the same
way. `play.rs`'s `LOOP_FADE_SECONDS` is therefore `5`, not the example's `6` — a
deliberate, documented deviation from "verbatim in spirit" for this one constant, so a
module fades over the same wall-clock span whether it is heard through the web player or
the CLI. `crates/starplayer-host-cpal/examples/play.rs` was not changed to match, since it
predates this task and D4 owns it; changing its behaviour is out of this task's scope.

## Deviations from the task file

* **Error message shape.** The example's `run` prints its own `eprintln!` on a failed
  `Player::open` and returns `ExitCode::FAILURE` directly. `play::run`/`play_on` instead
  return `Result<(), String>`, matching `info`, `render` and `trace`, and let `main`'s
  existing generic handler print and set the exit code. This is "moved... verbatim in
  spirit," not verbatim: the CLI already had one convention for reporting an error and
  this command uses it rather than inventing a second one.
* **`Ctrl-C` handler installation moved out of the testable core.** `ctrlc::set_handler`
  is process-global and can only be installed once, so it is called from `run` (once per
  process invocation) rather than from `play_on`. `play_on` instead takes `interrupted:
  &AtomicBool`, which the no-device test passes as a plain, never-installed flag. This
  keeps the research-point-1 test independent of the process-wide signal handler while
  still exercising the exact code path `run` uses.
* **Default fade is five seconds, not the example's six.** See research point 2 above.

## Verification results (this run)

All commands from this file's Verification section were run in
`/home/scott/projects/starplayer-d8` on 2026-09-03:

```
cargo test -p starplayer-cli                 → ok (5 play::tests + 2 golden_reproduction + 2 robustness)
cargo run -p starplayer-cli -- play --list-devices
                                              → exit 0; printed "* [PulseAudio] RDP Sink" and "[ALSA] Discard all samples..."
cargo run -p starplayer-cli -- play crates/starplayer-s3m/tests/fixtures/NICETUNE.S3M
                                              → exit 0; played audibly through PulseAudio's RDP Sink; ran 26.185s wall
                                                (25 s song + ~1 s startup/ring-out) for a 25 s (1 228 800 frame @ 48 kHz) song
cargo run -p starplayer-cli -- play crates/starplayer-s3m/tests/fixtures/NICETUNE.S3M --rate 44100 --buffer 512
                                              → exit 0; negotiated "44100 Hz, 2 ch, 512 frames"; ran ~26.1s wall
cargo test --workspace                       → ok, every crate's test result line reports 0 failed
cargo xtask ci --job clippy                  → 1 job(s) passed, clean after fixing two bool_assert_comparison lints in play.rs's own tests
cargo xtask ci --job host-tests              → 1 job(s) passed
```
