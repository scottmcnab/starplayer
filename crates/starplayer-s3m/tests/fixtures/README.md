# S3M test fixtures

The `.S3M` files here were written by the repository owner, **Scott McNab**, in
Scream Tracker 3 between 1994 and 1996. They are included as loader and effect-processor
test fixtures: they are the same modules the original DOS StarPlayer was written to play,
so they are the closest thing this project has to a reference recording.

They are licensed under **Creative Commons Attribution-NonCommercial-NoDerivatives 4.0
International** ([CC BY-NC-ND 4.0](https://creativecommons.org/licenses/by-nc-nd/4.0/)),
not under the repository's code licence. You may copy, share and play them unchanged,
with credit to Scott McNab, for non-commercial purposes; running this repository's tests
and demos is exactly that. You may not sell them, sample them, or distribute altered
versions. Rendering them to audio for the golden and perceptual tests is playback, not a
derivative work.

| File | Bytes | Channels | Notes |
|---|---|---|---|
| `REFLEX.S3M` | 9,634 | 3 | Amiga-limits flag set; 32-byte default-pan block present |
| `PETRI.S3M` | 35,966 | 8 | no pan block; stereo |

The set was trimmed to these two on 2026-09-07 to keep the repository compact; the
header-level cases the removed files covered — the mono master-volume bit and a full
16-entry channel-settings table — live in synthetic tests in `fixtures.rs`.

The rest of the owner's collection is loaded by the `#[ignore]`d
`loads_every_module_in_the_owners_collection` test, which reads from outside the
repository and so is not part of `cargo xtask ci`.
