# Rusting

Free, open-source 2D animation software for macOS, Windows and Linux. Draw vector frames and animate them frame by frame on a timeline with layers and onion skin, then export MP4, GIF, PNG or sprite sheets. Written in Rust, with a GPU-accelerated canvas (wgpu) and our own animation, timeline, drawing, and document engines.

Website, downloads and docs: [rusting-animation.org](https://rusting-animation.org)

## Install

macOS, with Homebrew:

```bash
brew tap HalfdanIsaksen/rusting https://github.com/HalfdanIsaksen/rusting
brew install HalfdanIsaksen/rusting/rusting
```

Then run `rusting`. `brew upgrade rusting` updates it.

macOS, as an app: download `rusting-<version>-universal-apple-darwin.dmg` from [Releases](https://github.com/HalfdanIsaksen/rusting/releases) (from 0.1.8), open it and drag Rusting to Applications. The app is signed and notarized by Apple.

Windows, with winget (from 0.2.2):

```bash
winget install HalfdanIsaksen.Rusting
```

Then open a new terminal and run `rusting`. `winget upgrade HalfdanIsaksen.Rusting` updates it.

Windows and Linux, by hand: download the zip or tarball from [Releases](https://github.com/HalfdanIsaksen/rusting/releases).

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
| `crates/export` | Encoding exported frames: PNG/JPEG images, image sequences, H.264 video and animated GIF (no GPU) |
| `crates/renderer` | wgpu device, surface, canvas and stroke rendering, offscreen rendering for export |
| `crates/ui` | The editing session and its egui panels |
| `apps/desktop` | The executable: window, event loop, file dialogs, export wiring |
| `assets` | WGSL shaders, the built-in brushes, the app logo and the macOS app icon |
| `tests` | Cross-crate integration tests (`integration_tests` crate) |
| `Formula`, `scripts` | The Homebrew formula, the scripts that regenerate it and the winget manifests on release, and the macOS DMG script |

## Documentation

| Document | What it covers |
|---|---|
| [docs/architecture.md](docs/architecture.md) | Crate dependency graph, data model, coordinate spaces, frame flow |
| [docs/decisions/](docs/decisions/) | Architecture decision records (ADR-001 to ADR-024) |
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
| Selection | `V` | Select strokes, fills and imported pictures (click, or drag a box from anywhere), then move or delete them |
| Lasso | `L` | Select strokes, fills and pictures by drawing a loop around them |
| Transform | `Q` | Scale and rotate the selection with box handles |
| Brush | `B` | Vector strokes with size, opacity, colour, a 0–100% smoothing slider and a draw order (see below). Pick the brush in Tool Settings: **Vector (solid)** or a textured brush (see Brushes below). Hold `Shift` to draw a straight line |
| Rectangle | `R` | Drag out a rectangle with a stroke and a fill. Set stroke width, stroke colour and fill colour (or turn either off) in Tool Settings. The fill can be solid or a gradient into a second colour, running left to right, top to bottom or diagonally. `Shift` makes a square, `Alt` draws from the centre. Draw order applies to both fill and stroke |
| Ellipse | `O` | Like Rectangle, for ellipses; `Shift` makes a circle |
| Text | `T` | Click the canvas and type, with a live preview and caret. `Enter` starts a new line, `Backspace` deletes, `Esc` cancels; `Cmd/Ctrl+Enter`, a click elsewhere or another tool finishes. Set font size, colour, opacity and draw order in Tool Settings. The text becomes vector shapes (one fill per text block, letters keep their holes), so it selects, moves, transforms, erases and exports like any fill, but can't be edited as text afterwards. Tool and canvas shortcuts are off while you type. Font: Ubuntu Light |
| Eraser | `E` | Erase part of a stroke or fill, or whole strokes and fills. Partial erasing cuts holes in fills or splits them into separate fills, with a live preview; every drag is one undo step |
| Paint Bucket | `K` | Fill areas enclosed by strokes, with optional gap closing. Click a stroke to recolour it. Recolouring a fill also recolours the strokes along its edge that had the fill's colour, and keeps its draw order |
| Eyedropper | `I` | Click the canvas to pick the colour (and opacity) of the topmost stroke or fill on the active layer; a gradient fill gives its colour at that point, and empty canvas gives the background colour. Sets the brush and paint bucket colour. `Alt`+click with the Brush, Paint Bucket, Rectangle or Ellipse picks a colour without switching tools (shapes take it as their stroke or fill colour; an `Alt`+drag still draws from the centre) |

**Merge same colour** (Brush, Rectangle, Ellipse, Paint Bucket; on by default): a new stroke or fill that touches fills of the same colour, opacity and draw order becomes one fill with them, so the shape selects, moves and erases as one item. The Paint Bucket only merges with fills: filling an area with the colour of its outline leaves the outline as a stroke, so it stays line art and keeps bounding later fills. Turn it off in Tool Settings to keep items separate.

### Brushes
- Tool Settings → **Brush** picks what the Brush tool draws with: **Vector (solid)**, a built-in brush (**Pencil (HB)**, **Soft Pencil (6B)**, **Charcoal**, **Chalk**, **Ink Pen**, **Marker**) or a brush in the project's Library.
- Textured brushes stamp a tip picture along the line on a paper grain, for a drawn-on-paper look. Overlapping passes build up darker, and the paper's dips stay white. The stroke is still a vector line: it selects, moves, transforms, erases, recolours and exports like any other.
- The first stroke drawn with a built-in brush adds the brush to the Library, so the project file keeps it and opens anywhere.
- **Import** a `.rustbrush` file (File → Import…) to add a brush, and right-click a brush in the Library → **Export Brush…** to share one. A `.rustbrush` file is a zip with `brush.json` (name and stamp settings), `tip.png` and an optional `grain.png`.
- Textured strokes don't merge with same-colour fills, and pen pressure isn't read yet: lines are drawn at full pressure.

**Draw order** (Brush, Rectangle, Ellipse, Text, Paint Bucket): **Behind** puts the new item under everything already in the frame, fills included, and **Normal** puts it on top. The live preview shows where it lands. Erasing, moving and transforming keep an item's draw order.

### Canvas
- Pan with middle-drag or by holding Space, zoom with the wheel or pinch, press `F` to fit.
- **Canvas → Resize Canvas…**: set a new width and height and pick an anchor (corners, edge middles or center) to grow or shrink from. The artwork keeps its place relative to the anchor.

### Timeline and layers
- Frame-by-frame keys with stepped exposure. Icon buttons add a drawing key (filled dot), add an empty key that ends the exposure (hollow dot) or clear the frame (dot with a cross: wipes the drawing shown at the playhead and keeps its key); hover for a description. Right-click a key to remove it.
- **Duplicate frame** (button, Edit → Duplicate Frame, or `Cmd/Ctrl+D`) copies the current frame to the next frame on the active layer only (click a frame cell to pick its layer). An empty frame gives an empty key. If the next frame already has a key, that layer's later keys move one frame right. The animation keeps its length; on the last frame, make the animation longer first.
- **Insert frame** (`F5`, Edit → Insert Frame) holds the current drawing one frame longer on the active layer only, moving that layer's later keys right. The animation keeps its length. **Delete frames** (`Shift+F5`, Edit → Delete Frames, or `Delete`/`Backspace` with frames selected) removes the selected frames, or the current one, on the selected layers only: their later keys move left to close the gap, so those layers end that many frames earlier. Other layers and the animation length stay the same.
- **Animation length**: drag the handle at the end of the frames (the line with a grip in the frame-number ruler) right to make the animation longer or left to make it shorter. The new length shows while dragging and is set on release, as one undo step. Keys past a shortened end are kept, so making it longer again brings them back. The Length field above the timeline does the same.
- Select a block of frames by dragging across the frame cells: the drag covers the frames and the layers it passes over, and selects those layers. `Shift`+click selects a block from the playhead and the active layer. `Cmd/Ctrl`+click in the layer column adds or removes a layer from the block. Drag inside the selection to move its keys on the selected layers only (the drop outline turns red if a key is in the way). A plain click moves the playhead and clears the selection, `Esc` clears it, and dragging in the frame-number ruler scrubs.
- **Frame tags**: select frames and press the tag button (or right-click the frames) to name them, for example "walk" or "jump". Tags show in a strip above the frame numbers. Click a tag to select its frames, double-click it to rename it, and right-click it to rename or remove it. Playback started inside a tag loops that tag.
- Playback with loop, first/previous/next/last frame. `Space` plays and pauses, and `←/→` and `↑/↓` step frames and keys.
- Layer controls in the timeline's layer column: add, duplicate, remove, rename (`F2` or double-click), drag to reorder, show/hide and lock/unlock, per layer or all at once.
- **Layer opacity**: each layer row has an opacity field (0–100%). Drag it sideways or click it to type a value; one drag is one undo step.
- **Layer groups**: select layers and press the folder button to put them in a group (a folder). The group gets a header row with its own show/hide, lock and opacity, which apply to all its layers, and an arrow to fold it. Click the header to select the group's layers, double-click to rename it, drag it to move the whole group, and right-click it (or use the ungroup button) to ungroup. Drag a layer between a group's rows to put it in the group, or out of them to take it out.
- Select several layers with `Cmd/Ctrl`+click (toggle) or `Shift`+click (range); duplicate and remove act on the whole selection.
- **Audio layers**: File → Import… also takes MP3 and WAV sounds. Each goes onto a new audio layer on top (speaker icon), starting at frame 1, and into the Library. Its row shows the clip with its waveform; drag the clip along the row to change where the sound starts. The sound plays in sync with playback, and scrubbing or stepping while paused plays a short snippet of the frame. The eye button mutes the layer. With an audio layer selected, Tool Settings sets the clip's volume and start frame. Inserting, deleting and moving frames move clips with them. Exported videos have no sound yet.
- Onion skin (View → Onion skin).

### Objects
- **Convert to Object**: select layers in the timeline and drag across frames, then right-click the frames or a layer and choose Convert to Object (or press `F8`). Those frames become an object that holds their animation. It is listed in the Library under Objects, and a new layer above plays it where the frames were. Without a frame selection, all frames are used. Items selected on the canvas become a one-frame object with `F8` (or Edit → Convert to Object), placed where they were.
- An object plays its own animation wherever it is placed, until the next key on its layer. A 12-frame object placed on 18 frames plays one and a half times. In Tool Settings, set it to **Loop** or **Play once** (holds its last frame), pick the object frame it starts on, and set its opacity.
- Drag an object from the Library onto a frame cell in the timeline to place it on that layer and frame, or onto the canvas to place it on a new layer where you drop it.
- With the Selection or Transform tool, click an object to select it as one unit, then drag, nudge, scale or rotate it. Moving it on a later frame of its exposure adds a new key there, so the object keeps playing from the new place.
- The timeline shows object exposures in blue, with a diamond on the key and a tick where each loop starts. Rename or delete objects in the Library (Delete works once the object is not placed anywhere). Objects can contain other objects.
- **Motion tweens**: place an object on one key, go to a later frame and move, scale, rotate or fade it there (that adds a second key of the same object). Select the frames from the first key to the second, right-click → Create Motion Tween → Linear, Ease In, Ease Out or Ease In-Out. The object then glides between the two keys, blending position, scale, rotation and opacity, while it keeps playing its own animation. The timeline shows the tween as a purple band with an arrow. Change the ease in Tool Settings (Motion tween) or remove it with right-click → Remove Tween. Moving the object on a frame inside a tween adds a key there, starting from where it is drawn, and the tween runs through it. Only objects can be tweened, and only between two keys of the same object. Rotation turns the direct way between the two angles, not the shortest one.
- Editing an object's contents in place is not supported yet. Drawing tools refuse a frame where the layer shows an object.
- Objects hold drawing layers only. Convert to Object leaves selected audio layers out (the status bar says so), and an object can't be placed on an audio layer.

### Look
- Uses the colours of the Rusting website (warm dark browns with an orange accent). The light or dark palette follows the operating system setting.
- The app icon (window, taskbar and Dock) and the start menu show the website's logo: an orange brush stroke with an amber dot. Its source is `assets/logo.svg`.

### Panels
- The tool strip, Tool Settings, Library, timeline and canvas are dockable panels. Drag a panel by its tab onto another panel to put it beside, above or below it, or behind it as another tab, for example the Library under Tool Settings in one column. Drag the line between two panels to resize them.
- Resizing the window, or hiding a panel, resizes the canvas and leaves the other panels as they are.
- The layout, and which panels are shown, are kept between launches. At launch, a panel saved smaller than its default size (timeline height, tool strip, Tool Settings and Library width) grows back to it, taking the room from the canvas, so every panel shows its contents.

### View menu
- Toggle the **Toolbar** (`Cmd/Ctrl+1`), **Timeline** (`Cmd/Ctrl+2`), **Tool Settings** (`Cmd/Ctrl+3`) and **Library** (`Cmd/Ctrl+4`) to free up space for the canvas. A panel you show again comes back where it was.
- **Reset Panel Layout** puts every panel back in its original place and size, and shows them all.
- **Preview**: a smaller floating window that shows only the canvas at the current frame. It has a close button and a Play button that loops the animation without moving the editor's playhead.

### Help menu
- Lists the keyboard shortcuts.

### Files and export
- Install with Homebrew on macOS (`brew install HalfdanIsaksen/rusting/rusting`) or winget on Windows (`winget install HalfdanIsaksen.Rusting`).
- Save and open versioned `.rusting` project files.
- macOS app: `.rusting` files show the Rusting icon in Finder, and double-clicking one (or dropping it on the Dock icon) opens it, asking first if the open project has unsaved changes.
- Undo/redo for every edit (up to 500 steps).
- Export the current frame as PNG/JPG, an image sequence, or an H.264 video as MP4 or MOV (QuickTime).
- **Export GIF**: the frame range as a looping animated GIF at the document frame rate, for animations up to 15 seconds. A longer range is refused with a message to export it as a video or image sequence instead.
- **Export Sprite Sheet**: every frame tag becomes its own row of frames in one PNG/JPG image. Without tags, the chosen frame range goes on one row.
- **Import** (File → Import…, below Export): pick one or more PNG or JPEG pictures, MP3 or WAV sounds (see Audio layers), or `.rustbrush` brushes (see Brushes). Each picture goes onto the current layer and frame, centred on the canvas and shrunk to fit if it is larger, and is selected so you can move it right away. Pictures move, scale, rotate and delete with the selection tools like any other item, paint under the strokes and fills you draw on top of them (handy for tracing), show in the preview and in every export, and are saved inside the project file. Each imported picture is also added to the Library.
- **Library** (a panel, by default on the right of the canvas): lists every imported file, under Images and Audio, by file name, the project's brushes under Brushes (right-click → Use Brush or Export Brush…), and every object under Objects. Drag a picture onto the canvas to place another copy where you drop it, or right-click it → Place on Canvas to put one in the centre. Drag a sound onto the timeline's frames to start a new audio layer at that frame, or right-click it → Add to Timeline to start one at the current frame. Double-click a row to rename it. Right-click → Delete removes it from the library, but only once nothing uses it (a frame, an object or an audio layer). The project file stores each library picture and sound once, however many times it is placed, and keeps unused library pictures and sounds.

## Contributing workflow

Each feature is built on its own branch (`feat/<name>`) and only touches the crates it owns. Before merging, a branch must pass `fmt`, `clippy -D warnings`, and `test`. Integration happens in dependency order. When a branch adds a user-visible feature, it also updates the feature list above. See [docs/roadmap.md](docs/roadmap.md) and [docs/agents.md](docs/agents.md).

## License

MIT. See [LICENSE](LICENSE).
