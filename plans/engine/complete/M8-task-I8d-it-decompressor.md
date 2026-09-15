# M8 — I8d helper: Incremental IT sample decompression

| Field | Value |
|---|---|
| Parent | M8-task-I8d-psram-module-decoding.md |
| Status | Complete; integrated and verified by I8d |
| Recommended model | GPT-5.6-sol, high effort |

## Context for a fresh agent

The direct PSRAM converter needs a bounded allocation-free IT 2.14/2.15 compressed
sample decoder. Current crates/starplayer-it/src/compression.rs exposes an eager
Vec-based decompress. The core converter worker will consume a resumable helper.
Keep existing semantics, including truncated block handling and consumed byte count.

## Deliverables

- In compression.rs add IncrementalDecompressor constructed from data, frame count,
  wide flag and is_215. decode_into(&mut self, output: &mut [i16],
  max_input_bytes: usize) returns DecodeChunk { written, input_consumed, finished }.
  Agree visibility/API with /root/psram_core before finalizing; incremental converter
  may need public export through format crate.
- Preserve block boundaries, integrators, bit buffers, escape state and width across
  calls. Stop at output capacity and never consume more than the input budget,
  including block headers. Zero budget/output must not spin. Total consumed offset
  permits finding stereo right-channel stream. No allocations in state or decode_into.
- Share primitive bit-width/integrator logic with old decode where practical; old public
  API behavior must remain byte-identical. No whole-block intermediate Vec.

## Research points

Truncated/zero-length blocks, incomplete escapes across budget boundaries, exhausted
output after reading a symbol, invalid widths, 8/16-bit and double delta, block reset.

## Verification

Compare against existing decompress for current tests and fixtures, varied tiny output
and input budgets, multi-block streams, truncations and malformed widths. Add meaningful
step-bound tests and progress assertions. Run IT crate tests. No cargo fmt. Root reviews
before commit; stage only compression.rs and necessary lib export/tests paths explicitly.

## Out of scope

Other loaders, image writer, firmware, hardware, historical references. Coordinate with
core worker to avoid compression.rs overlap. Do not modify shared parent task document.

## Result

The allocation-free incremental helper landed in `45dbe34` and is consumed by the
caller-buffer IT decoder. Tiny-budget, block-boundary, truncation and integration
parity tests pass; I8d also verified IT upload on the A1S.
