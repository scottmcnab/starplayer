# StarPlayer — repo-wide agent instructions

## Plans & docs

Any change that affects design must be captured in the relevant section under
`plans/`. A plan moves to that area's `complete/` archive once its deliverable has landed and only owner-acceptance items remain — the files directly under a `plans/` are the outstanding work.

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
