# Rusting

Open-source 2D animation software written in Rust. It is vector-first and frame-by-frame, with a GPU-accelerated canvas (wgpu) and our own animation, timeline, drawing, and document engines.

> Status: **Phase 0**, the workspace foundation. The app opens a window and clears it with wgpu. See [docs/roadmap.md](docs/roadmap.md).

## Build and run

Requires stable Rust ≥ 1.95. `rust-toolchain.toml` selects stable automatically when you use rustup.

```bash
cargo run -p desktop          # launch the editor
cargo test --workspace        # run all tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

Set `RUST_LOG=debug` for more log output.

## Layout

| Path | Responsibility |
|---|---|
| `crates/animation_core` | Frames, FPS, time conversion, stable IDs, colors, interpolation |
| `crates/timeline` | Tracks, keyframes, stepped exposure, playback |
| `crates/drawing` | Vector strokes, brushes, drawings, tessellation |
| `crates/scene` | Layers, ordering, visibility, lock, transforms |
| `crates/document` | The project document and per-frame evaluation (`RenderList`) |
| `crates/file_format` | Versioned `.rusting` save/load and validation |
| `crates/editor` | Editor state, commands, undo/redo |
| `crates/renderer` | wgpu device, surface, canvas and stroke rendering |
| `crates/ui` | Editor panels (egui shell for the MVP) |
| `apps/desktop` | The executable: window, event loop, wiring |
| `tests` | Cross-crate integration tests (`integration_tests` crate) |

Read [docs/architecture.md](docs/architecture.md) for the dependency graph and rules, and [docs/decisions/](docs/decisions/) for the architecture decision records.

## Contributing workflow

Each feature is built on its own branch (`feat/<name>`) and only touches the crates it owns. Before merging, a branch must pass `fmt`, `clippy -D warnings`, and `test`. Integration happens in dependency order. See [docs/roadmap.md](docs/roadmap.md).
