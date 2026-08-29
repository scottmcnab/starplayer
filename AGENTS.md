# StarPlayer — repo-wide agent instructions

StarPlayer is a **reusable Rust tracked-music engine**: a modern reimplementation of a
1990s DOS tracker player, generalised into a real-time, event-driven synthesis engine.
It plays MOD/S3M/MTM (and later XM/IT), runs in the browser via WASM, on native
desktop, and on `no_std` embedded targets, and is designed to be embedded in other
projects rather than to be one application.

The original 80386 assembly sources live in `STARPLAY/`. **They are a read-only
historical reference and must never be modified.** They are also *gutted* — most of the
engine sits inside TASM `comment %` blocks, so the tree does not assemble as-is; the
text is intact and complete, and is the specification for MOD/S3M/MTM effect semantics.

## Non-negotiable design goals

These are architectural invariants, not preferences. A change that breaks one of them
needs a plan document, not a commit.

1. **Sample-exact event timing.** Tracker ticks land on exact output sample frames. The
   render loop splits each block at event boundaries; it never renders across an event.
2. **Split for voice mixing, quantise for DSP.** Voice accumulation splits within a
   fixed `RENDER_QUANTUM` of 128 frames; the DSP graph and master bus only ever see
   whole quanta. Arbitrary host block sizes are adapted by an output ring.
3. **Buffer-size-independent output is a testable invariant.** Rendering the same module
   at host block sizes 1, 3, 64, 128, 4096 and 8191 must produce byte-identical output.
   This test exists from M0 and must never be weakened.
4. **`no_std` + `alloc` core.** Core crates are `#![no_std]`. No default feature may
   transitively enable `std`. CI checks a bare-metal target on every commit.
5. **Real-time safety in the audio path.** No allocation, no locks, no panics inside
   `render()`. Retired `Arc<Module>` values go back over a garbage channel to be
   dropped off the audio thread. No transcendental functions in the RT path — tables
   only, so output is bit-identical across x86/ARM/WASM on the fixed-point path.
6. **Canonical fidelity, deviations documented.** For MOD/S3M/MTM the original assembly
   is the primary specification, but where it deviates from ST3/ProTracker behaviour
   through an outright defect, implement the canonical behaviour and record the
   deviation in `plans/product/03-accuracy-policy.md`.
7. **Format identity is preserved.** Each format keeps its native pattern data and its
   own effect processor. Do not lower one format into another — the original's
   MOD/MTM → S3M conversion is exactly why its MOD playback was inaccurate.
8. **No trait is committed until its second real implementation exists.** Concepts are
   designed up front in the architecture document; Rust trait boundaries follow the
   implementations.

## Layout

```
crates/     starplayer-{core,rt,dsp,mixer,model,engine}   no_std + alloc
            starplayer-{mod,s3m,mtm,xm,it}                one crate per format: loader + effects
            starplayer-{midi,telemetry}                   no_std + alloc
            starplayer                                    facade / public crate
            starplayer-{host-cpal,host-wasm,offline,testkit}   std
apps/       starplayer-{web,cli,tui}
xtask/      build orchestration, wasm packaging, golden regeneration
plans/      design and implementation plans — see plans/README.md
STARPLAY/   original DOS sources (read-only)
```

## Plans & docs

Any change that affects design must be captured in the relevant section under `plans/`.
Plans live in `plans/<area>/` (`product/`, `reference/`, `engine/`, `apps/`); a plan
moves to that area's `complete/` archive once its deliverable has landed and only
owner-acceptance items remain — the files directly under a `plans/<area>/` are the
outstanding work. See `plans/README.md` for the index.

`plans/product/` documents are the foundation documents and the source of truth for
direction. Task files record *how*; foundation docs record *what and why*.

## Delegation workflow

1. Write a detailed task spec under `plans/engine/` or `plans/apps/`, following the
   existing task-file shape (header table, then Context for a fresh agent / Deliverables
   / Research points / Verification / Out of scope). Write it so an agent with no access
   to conversation history can execute it from the file alone.
2. Create a branch and a **sibling worktree** (`../starplayer-<branch>`) so features
   proceed independently.
3. Delegate implementation to a **GPT-5.6-sol** worker against the task file. The task
   file's header table names the least-capable model tier safe to hand it to.
4. Review the diff and run the task's Verification section before committing.
5. Move the plan to the area's `complete/` once landed and only owner acceptance remains.

## Working agreements

- Avoid making formatting changes to unrelated code, unless the task is about
  formatting.
- Don't run "cargo fmt" unless requested.
- Prefer full variable names aimed at readability instead of abbreviations, for
  example "surface_area" instead of "sa", unless the variable is a commonly used
  idiom such as loop index "i".
- Don't use unsafe code blocks unless absolutely necessary, prefer safe code.
- Prefer compact Rust formatting where it improves readability: keep function
  parameters on one line when practical, even if the line is longer than rustfmt
  would usually choose.
- For method chains, keep the receiver and first action method on the same line
  where readable, then continue the chain on following lines as needed.
- In unit tests, keep assert calls on one line where practical, even if long.
- Use vertical layout when it is visually clearer, such as matrix or array literals,
  multiline JSON/text, or other deliberately structured data.
- Don't add attribution to commit messages.
- Never stage with `git add -A`, `git add .`, `git add -u`, or `git commit -a`. The
  working tree routinely has stray files (flash images, device logs, scratch files)
  that must not be committed. Always stage explicit paths — `git add <path> ...` —
  for exactly the files the change touches, and run `git status` before committing to confirm nothing unintended is staged. (This is how the 4 MB flash images and
  LibreOffice lock files were committed by accident and later had to be scrubbed from history.)
