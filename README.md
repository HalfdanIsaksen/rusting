# Rusting

Open-source 2D animation software written in Rust. It is vector-first and frame-by-frame, with a GPU-accelerated canvas (wgpu) and our own animation, timeline, drawing, and document engines.

> Status: **Phase 0**, the workspace foundation. The app opens a window and clears it with wgpu. See [docs/roadmap.md](docs/roadmap.md).

## Install

macOS, with Homebrew:

```bash
brew tap HalfdanIsaksen/rusting https://github.com/HalfdanIsaksen/rusting
brew install HalfdanIsaksen/rusting/rusting
```

Then run `rusting`. `brew upgrade rusting` updates it.

Windows and Linux: download the zip or tarball from [Releases](https://github.com/HalfdanIsaksen/rusting/releases).

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

## Features

### Start menu
- Opens on launch: name the document, set canvas width and height, frame rate and number of frames.
- Templates: HD 1080p, 4K UHD, Square social, Vertical / Story and GIF / small, or custom values.
- Open an existing `.rusting` project from the same screen.

### Drawing tools
The tool strip runs down the side of the canvas:

| Tool | Key | What it does |
|---|---|---|
| Selection | `V` | Select strokes and fills, then move or delete them |
| Transform | `Q` | Scale and rotate the selection with box handles |
| Brush | `B` | Vector strokes with size, opacity, colour and a 0–100% smoothing slider |
| Eraser | `E` | Erase part of a stroke or whole strokes |
| Paint Bucket | `K` | Fill areas enclosed by strokes, with optional gap closing |

### Canvas
- Pan with middle-drag or by holding Space, zoom with the wheel or pinch, press `F` to fit.
- **Canvas → Resize Canvas…**: set a new width and height and pick an anchor (corners, edge middles or center) to grow or shrink from. The artwork keeps its place relative to the anchor.

### Timeline and layers
- Frame-by-frame keys with stepped exposure: new blank drawing, end exposure, remove key.
- **Duplicate frame** (button, Edit → Duplicate Frame, or `Cmd/Ctrl+D`) inserts a copy of the current frame right after it on every layer.
- Playback with loop, first/previous/next/last frame, and `←/→` and `↑/↓` to step frames and keys.
- Layer controls in the timeline's layer column: add, remove, rename (`F2` or double-click), drag to reorder, show/hide and lock/unlock, per layer or all at once.
- Onion skin (View → Onion skin).

### View menu
- Toggle the **Toolbar** (`Cmd/Ctrl+1`) and **Timeline** (`Cmd/Ctrl+2`) to free up space for the canvas.
- **Preview**: a smaller floating window that shows only the canvas at the current frame. It has a close button and a Play button that loops the animation without moving the editor's playhead.

### Files and export
- Save and open versioned `.rusting` project files.
- Undo/redo for every edit (up to 500 steps).
- Export the current frame as PNG/JPG, an image sequence, or an H.264 MP4 video.

## Contributing workflow

Each feature is built on its own branch (`feat/<name>`) and only touches the crates it owns. Before merging, a branch must pass `fmt`, `clippy -D warnings`, and `test`. Integration happens in dependency order. See [docs/roadmap.md](docs/roadmap.md).
