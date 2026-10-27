# Working in parallel (feature sessions)

The main session is the **lead architect and integrator**. Each feature session:

1. Works on its own branch `feat/<name>` in its own worktree, created from `main`.
2. Modifies **only** the crates it owns (see `docs/roadmap.md`). If it needs a change in another crate's public API, it stops and asks the main session instead of editing that crate.
3. Never redefines shared types. Frames, IDs, colors, and interpolation come from `animation_core`. Layers come from `scene`. Drawings come from `drawing`.
4. Keeps the workspace green: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
5. Commits to its branch only. No merging into `main`, no pushing.
6. Finishes with a report:
   - What was implemented
   - Files changed
   - Public APIs introduced or changed
   - Tests run (with results)
   - Known limitations
   - Integration requirements (what other crates or the UI need to do)

The main session reviews each branch and merges in dependency order: `drawing` → `timeline` → `document`/`editor`/`file_format` → `renderer`/`desktop` → `ui`.
