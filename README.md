# Rusting

Open-source 2D animation software written in Rust. It is vector-first and frame-by-frame, with a GPU-accelerated canvas (wgpu) and our own animation, timeline, drawing, and document engines.

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
| `crates/export` | Encoding exported frames: PNG/JPEG images, image sequences and H.264 video (no GPU) |
| `crates/renderer` | wgpu device, surface, canvas and stroke rendering, offscreen rendering for export |
| `crates/ui` | The editing session and its egui panels |
| `apps/desktop` | The executable: window, event loop, file dialogs, export wiring |
| `assets` | WGSL shaders and the app logo |
| `tests` | Cross-crate integration tests (`integration_tests` crate) |
| `Formula`, `scripts` | The Homebrew formula and the script that regenerates it on release |

## Documentation

| Document | What it covers |
|---|---|
| [docs/architecture.md](docs/architecture.md) | Crate dependency graph, data model, coordinate spaces, frame flow |
| [docs/decisions/](docs/decisions/) | Architecture decision records (ADR-001 to ADR-009) |
| [docs/roadmap.md](docs/roadmap.md) | Development phases and their acceptance criteria |
| [docs/smoke-test.md](docs/smoke-test.md) | Manual end-to-end test checklist |
| [docs/releasing.md](docs/releasing.md) | How to tag a release and update the Homebrew tap |
| [docs/agents.md](docs/agents.md) | Rules for parallel feature sessions |

## Features

### Start menu
- Opens on launch: name the document, set canvas width and height, frame rate and number of frames.
- Templates: HD 1080p, 4K UHD, Square social, Vertical / Story and GIF / small, or custom values.
- Open an existing `.rusting` project from the same screen.

### Drawing tools
The tool strip runs down the side of the canvas:

| Tool | Key | What it does |
|---|---|---|
| Selection | `V` | Select strokes and fills (click, or drag a box from anywhere), then move or delete them |
| Lasso | `L` | Select strokes and fills by drawing a loop around them |
| Transform | `Q` | Scale and rotate the selection with box handles |
| Brush | `B` | Vector strokes with size, opacity, colour, a 0–100% smoothing slider, and paint in front of or behind the existing strokes. Hold `Shift` to draw a straight line |
| Rectangle | `R` | Drag out a rectangle with a stroke and a fill. Set stroke width, stroke colour and fill colour (or turn either off) in Tool Settings. The fill can be solid or a gradient into a second colour, running left to right, top to bottom or diagonally. `Shift` makes a square, `Alt` draws from the centre |
| Ellipse | `O` | Like Rectangle, for ellipses; `Shift` makes a circle |
| Eraser | `E` | Erase part of a stroke or whole strokes |
| Paint Bucket | `K` | Fill areas enclosed by strokes, with optional gap closing |

### Canvas
- Pan with middle-drag or by holding Space, zoom with the wheel or pinch, press `F` to fit.
- **Canvas → Resize Canvas…**: set a new width and height and pick an anchor (corners, edge middles or center) to grow or shrink from. The artwork keeps its place relative to the anchor.

### Timeline and layers
- Frame-by-frame keys with stepped exposure. Icon buttons add a drawing key (filled dot), add an empty key that ends the exposure (hollow dot) or clear the frame (dot with a cross: wipes the drawing shown at the playhead and keeps its key); hover for a description. Right-click a key to remove it.
- **Duplicate frame** (button, Edit → Duplicate Frame, or `Cmd/Ctrl+D`) inserts a copy of the current frame right after it on every layer.
- **Insert frame** (`F5`, Edit → Insert Frame) adds a blank frame after the playhead on every layer, holding the current drawing one frame longer. **Delete frames** (`Shift+F5`, Edit → Delete Frames, or `Delete`/`Backspace` with frames selected) removes the selected frames, or the current one, on every layer.
- Select a range of frames by dragging across the frame cells, or `Shift`+click to select from the playhead. Drag inside the selection to move its keys on every layer (the drop outline turns red if a key is in the way). A plain click moves the playhead and clears the selection, `Esc` clears it, and dragging in the frame-number ruler scrubs.
- **Frame tags**: select frames and press the tag button (or right-click the frames) to name them, for example "walk" or "jump". Tags show in a strip above the frame numbers. Click a tag to select its frames, double-click it to rename it, and right-click it to rename or remove it. Playback started inside a tag loops that tag.
- Playback with loop, first/previous/next/last frame. `Space` plays and pauses, and `←/→` and `↑/↓` step frames and keys.
- Layer controls in the timeline's layer column: add, duplicate, remove, rename (`F2` or double-click), drag to reorder, show/hide and lock/unlock, per layer or all at once.
- Select several layers with `Cmd/Ctrl`+click (toggle) or `Shift`+click (range); duplicate and remove act on the whole selection.
- Onion skin (View → Onion skin).

### Look
- Uses the colours of the Rusting website (warm dark browns with an orange accent). The light or dark palette follows the operating system setting.
- The app icon (window, taskbar and Dock) and the start menu show the website's logo: an orange brush stroke with an amber dot. Its source is `assets/logo.svg`.

### View menu
- Toggle the **Toolbar** (`Cmd/Ctrl+1`), **Timeline** (`Cmd/Ctrl+2`) and **Tool Settings** (`Cmd/Ctrl+3`) to free up space for the canvas.
- **Preview**: a smaller floating window that shows only the canvas at the current frame. It has a close button and a Play button that loops the animation without moving the editor's playhead.

### Help menu
- Lists the keyboard shortcuts.

### Files and export
- Save and open versioned `.rusting` project files.
- Undo/redo for every edit (up to 500 steps).
- Export the current frame as PNG/JPG, an image sequence, or an H.264 video as MP4 or MOV (QuickTime).
- **Export Sprite Sheet**: every frame tag becomes its own row of frames in one PNG/JPG image. Without tags, the chosen frame range goes on one row.

## Contributing workflow

Each feature is built on its own branch (`feat/<name>`) and only touches the crates it owns. Before merging, a branch must pass `fmt`, `clippy -D warnings`, and `test`. Integration happens in dependency order. When a branch adds a user-visible feature, it also updates the feature list above. See [docs/roadmap.md](docs/roadmap.md) and [docs/agents.md](docs/agents.md).

## License

MIT. See [LICENSE](LICENSE).
