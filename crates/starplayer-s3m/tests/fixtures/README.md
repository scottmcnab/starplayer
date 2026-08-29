# S3M test fixtures

The five `.S3M` files here were written by the repository owner, **Scott McNab**, in
Scream Tracker 3 between 1994 and 1996. They are included as loader and effect-processor
test fixtures: they are the same modules the original DOS StarPlayer was written to play,
so they are the closest thing this project has to a reference recording.

Licensing is **deferred** and will be settled with the project licence
(`plans/product/00-vision.md` decision 7). Until then, treat them as the owner's own
work, included in this repository for testing only.

| File | Bytes | Channels | Notes |
|---|---|---|---|
| `REFLEX.S3M` | 9,634 | 3 | Amiga-limits flag set; 32-byte default-pan block present |
| `ARMANI.S3M` | 17,554 | 5 | default-pan block present, with a `0x80` "keep" entry |
| `PETRI.S3M` | 35,966 | 8 | no pan block; stereo |
| `NICETUNE.S3M` | 54,412 | 16 | 16 channels; Amiga-limits flag set; no pan block |
| `MOVEMENT.S3M` | 58,499 | 2 | **mono** (master-volume bit 7 clear); no pan block |

They are deliberately the five smallest of the owner's collection; the rest of that
collection is loaded by the `#[ignore]`d `loads_every_module_in_the_owners_collection`
test, which reads from outside the repository and so is not part of `cargo xtask ci`.
