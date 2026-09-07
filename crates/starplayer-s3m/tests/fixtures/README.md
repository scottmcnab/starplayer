# S3M test fixtures

The `.S3M` files here were written by the repository owner, **Scott McNab**, in
Scream Tracker 3 between 1994 and 1996. They are included as loader and effect-processor
test fixtures: they are the same modules the original DOS StarPlayer was written to play,
so they are the closest thing this project has to a reference recording.

Licensing is **deferred** and will be settled with the project licence
(`plans/product/00-vision.md` decision 7). Until then, treat them as the owner's own
work, included in this repository for testing only.

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
