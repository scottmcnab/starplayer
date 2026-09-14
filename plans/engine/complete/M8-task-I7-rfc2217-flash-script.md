# M8 — I7: RFC2217 remote flash wrapper

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](../M8-master-plan.md)) |
| Status | **Complete 2026-09-14.** The executable fixed-`web` wrapper and adjacent runbook documentation are implemented and pass the non-flashing verification |
| Depends on | [I3a](M8-task-I3a-dma-refill-remediation.md) |
| Recommended model | GPT-5.6-sol |
| Verified by | Agent syntax, executable-bit and diff checks plus command review; the wrapper was deliberately not executed during implementation |

## Context for a fresh agent

The A1S board is attached to a Mac and exposed to this Linux checkout by an RFC2217 bridge at
`192.168.0.151:8086`. `cargo xtask flash` invokes Rust `espflash`, which accepts locally enumerated
serial devices but does not open this RFC2217 URL, producing `espflash::no_serial`. Python
`esptool.py` 3.3.2 and pyserial do support the bridge. The proven command uses
`rfc2217://192.168.0.151:8086?ign_set_control`, chip `esp32`, baud 460800 and writes the merged
image at address zero.

The owner wants one `flash_image.sh` command for the current web-control workflow. Firmware must
still be built and merged through `embedded/xtask`, because that wrapper selects the correct board
crate, Xtensa toolchain, partition table and app partition. Every A1S shell must source
`~/export-esp-1.97.sh` first.

## Deliverables

1. Add executable `embedded/flash_image.sh`. It must locate `embedded/` from its own path, source
   `${HOME}/export-esp-1.97.sh`, build a merged A1S `web` image with `cargo xtask image --board a1s
   --features web --merge`, then flash `target/starplayer-a1s-web-merged.bin` with:
   `esptool.py --chip esp32 --baud 460800 -p
   'rfc2217://192.168.0.151:8086?ign_set_control' write_flash 0x0 <image>`.
2. Use `set -euo pipefail`, quote all paths and the RFC2217 URL, and fail clearly when the export
   script, `cargo`, `esptool.py`, or generated image is missing. Do not erase flash, touch the
   `modules`/`config` partitions, start a monitor, install software or require the caller's current
   directory.
3. Allow the endpoint and baud to be overridden through clearly named environment variables while
   keeping the proven bridge and 460800 as defaults. Keep the firmware personality fixed to `web`
   so the generated filename and flashed content cannot disagree.
4. Document the wrapper in `embedded/README.md` beside the flash commands, including the one-line
   invocation and the endpoint/baud overrides.

## Research points

- Confirm the xtask output stem for `--features web` is exactly
  `target/starplayer-a1s-web-merged.bin`.
- Confirm `--merge` uses `--skip-padding`, so writing the merged image at address zero ends before
  the persistent `modules` and `config` partitions.

## Verification

```sh
bash -n embedded/flash_image.sh
test -x embedded/flash_image.sh
git diff --check
```

Review the script command-by-command against the already verified manual RFC2217 flash command.
Do not run it during implementation: that would write hardware when only the wrapper was requested.

## Implementation result

`embedded/flash_image.sh` is executable, resolves `embedded/` from its own path, sources the
required Xtensa environment and checks each required command and generated image with a clear
failure message. It always builds the A1S `web` personality through `cargo xtask image --board
a1s --features web --merge`, then writes that exact merged image at address zero with the proven
RFC2217 URL and 460800 baud. `STARPLAYER_RFC2217_ENDPOINT` and `STARPLAYER_FLASH_BAUD` provide the
only overrides. The wrapper contains no erase, monitor or installation command.

The xtask source confirms the selected output is
`target/starplayer-a1s-web-merged.bin` and that merged images use `--skip-padding`, leaving the
persistent `modules` and `config` partitions beyond the generated image. The runbook documents the
one-line invocation, defaults, overrides and working-directory-independent path handling.

`bash -n embedded/flash_image.sh`, `test -x embedded/flash_image.sh` and `git diff --check` all
passed. The script itself was not executed, as required, so this implementation performed no
firmware build, flash, erase or monitor operation against the board.

## Out of scope

- Adding RFC2217 support to Rust `espflash` or `embedded/xtask`.
- Changing firmware, partitions, the 48 kHz audio path, Wi-Fi provisioning or the web UI.
- Monitoring the UART after flashing.
