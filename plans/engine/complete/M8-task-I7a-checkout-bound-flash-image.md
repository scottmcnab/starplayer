# M8 — I7a: checkout-bound RFC2217 flash image

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](../M8-master-plan.md)) |
| Status | **Complete 2026-09-14.** The wrapper rebuilt xtask and flashed the exact current-checkout image with a verified hash |
| Depends on | [I7](M8-task-I7-rfc2217-flash-script.md), [I8](M8-task-I8-web-boot-stack-remediation.md) |
| Recommended model | GPT-5.6-sol |
| Verified by | Agent shell/harness tests and reviewer inspection, then an authorized RFC2217 flash from `main` and UART boot capture |

## Context for a fresh agent

`embedded/flash_image.sh` resolves its own checkout and invokes `cargo xtask image`, then flashes
`$STARPLAYER_EMBEDDED_DIR/target/starplayer-a1s-web-merged.bin`. The xtask binary derives its
workspace at compile time with `env!("CARGO_MANIFEST_DIR")`. During I8 hardware acceptance, Cargo
reused `embedded/target/debug/starplayer-embedded-xtask` whose embedded manifest path named the I8
sibling worktree. The command built and wrote a fresh merged image in that sibling while the shell
script subsequently selected an older merged image in `main`. This was detected from the printed
paths and esptool was interrupted before it connected or wrote bytes.

This can happen whenever byte-identical xtask sources from another worktree populate/reuse Cargo
artifacts. A flash wrapper must fail closed if its build writes anywhere except its own checkout.
The immediate remediation must also force xtask to be rebuilt with the invoking checkout's
`CARGO_MANIFEST_DIR`.

## Deliverables

1. Update `embedded/flash_image.sh` to remove its checkout's pre-existing merged web image before
   the build, then force a checkout-local rebuild of `starplayer-embedded-xtask` before invoking
   `cargo xtask image --board a1s --features web --merge`. Use Cargo's package-scoped clean; do not
   wipe the full firmware target directory.
2. After the build, require the expected checkout-local image to be a non-empty regular file. A
   path-confused xtask must leave that file absent and make the script exit before `esptool.py`.
   Preserve the endpoint/baud overrides and the fixed A1S `web` personality.
3. Add a shell test or isolated fake-command harness that proves both paths: a build that writes the
   expected image reaches the exact esptool invocation, while a build that writes elsewhere never
   invokes esptool. Also verify the cleanup/build ordering and quoting without touching hardware.
4. Update the adjacent `embedded/README.md` runbook to say the wrapper invalidates the old image and
   rebuilds xtask for its own checkout, so it cannot silently flash a stale artifact after worktree
   use.

## Research points

- Confirm `cargo clean -p starplayer-embedded-xtask` removes only that host package's artifacts and
  forces the compile-time manifest path to be evaluated for the current checkout.
- Confirm deleting only `target/starplayer-a1s-web-merged.bin` cannot touch the persistent board
  partitions; it is a host-side generated file and esptool has not run yet.
- Keep the test independent of the real Xtensa toolchain, RFC2217 endpoint and board.

## Verification

```sh
bash -n embedded/flash_image.sh
embedded/tests/flash_image_test.sh
test -x embedded/flash_image.sh
git diff --check
```

Do not run the real wrapper or access hardware in the implementation worktree. The root reviewer
will rebuild and flash after merge.

## Implementation result

`embedded/flash_image.sh` now removes only its checkout-local merged web image, runs
`cargo clean -p starplayer-embedded-xtask`, and then invokes the existing fixed A1S `web` image
build. It requires the expected checkout-local result to be both a regular file and non-empty
before esptool can run. A path-confused xtask therefore leaves the deliberately invalidated local
path absent and fails closed before any board command.

`embedded/tests/flash_image_test.sh` copies the wrapper beneath a temporary checkout path that
contains spaces and substitutes isolated fake Cargo and esptool commands. Its successful case
proves stale-image removal, package-clean/build ordering, the exact esptool argument vector, and
endpoint/path quoting. Its path-confused case writes an image under a different checkout and
proves the wrapper rejects it without invoking esptool. Both wrapper and harness pass `bash -n`,
the harness passes, both scripts retain their executable bits, and `git diff --check` passes. The
real wrapper was not run and no hardware was accessed during implementation.

Hardware acceptance then ran the merged wrapper from `/home/scott/projects/starplayer`. Its log
showed both the xtask compile and board build rooted in that checkout, wrote the new local merged
image, flashed 1 338 128 bytes at address zero, and verified the flash hash before reset. The
subsequent UART capture booted the intended 144 KiB-heap image.

## Out of scope

- Changing xtask's compile-time workspace discovery throughout the build tool.
- Changing firmware, the web-stack fix, audio, partitions, or the RFC2217 defaults.
- Erasing board flash or installing tools.
