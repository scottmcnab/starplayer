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
