# Rusting

@README.md
@docs/architecture.md

## Before calling work done
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Rules
- Respect the crate dependency graph in `docs/architecture.md`. Core crates never touch egui, winit, or wgpu.
- Every document change is an undoable command in `crates/editor`.
- A design choice that is hard to undo gets an ADR in `docs/decisions/` (`ADR-NNN-slug.md`). Only ADR markdown lives there.
- Parallel feature sessions follow `docs/agents.md`.
- Update the README feature list when you add a user-visible feature.
