# Architecture

## Goals
- Animation logic is independent of rendering and UI. Core crates compile and test without a GPU or window.
- One owner per concept: frames, IDs, layers, and drawings are each defined in exactly one crate.
- Every document change goes through an undoable command.

## Crate dependency graph

```
animation_core ─┬─> timeline ─┐
                ├─> drawing ──┼─> document ─┬─> file_format
                └─> scene ────┘             ├─> editor ──> ui ─┐
                                            └─> renderer ──────┴─> apps/desktop
```

Arrows point from a dependency to its dependents. Rules:
- No cycles. Core crates (`animation_core`, `timeline`, `drawing`, `scene`, `document`, `file_format`, `editor`) never depend on `renderer`, `ui`, `winit`, or `egui`.
- `document` re-exports `animation_core`, `drawing`, `scene`, and `timeline`, so downstream crates see one version of each type.
- Only `ui` and `apps/desktop` may depend on egui. Only `apps/desktop` depends on winit.
- `ui` also depends on `file_format` (its `Session` opens and saves projects). Native file dialogs (`rfd`) live in `apps/desktop`.
- `export` (PNG/JPEG/MP4 encoding, ADR-008) depends on no workspace crate and no GPU. Only `apps/desktop` uses it: it renders frames with `renderer::OffscreenRenderer` and hands them to `export::ExportWorker`.
- `renderer` takes anything wgpu can turn into a surface. It does not know about winit.

## Shared types (owned by `animation_core`)
| Type | Meaning |
|---|---|
| `FrameIndex(u32)` | Zero-based frame. UI shows `display_number()` (one-based). |
| `FrameRange` | Half-open `start..end` |
| `Fps` | Rational frames/second (e.g. 24000/1001) |
| `LayerId`, `DrawingId`, `StrokeId`, `FillId`, `DocumentId` | UUID v4 newtypes, stable across save and load |
| `Rgba` | Straight-alpha color, f32 components |
| `Interpolation` | `Step`, `Linear` (Bezier later) |
| `Vec2` | `glam::Vec2`, re-exported |

## Data model
```
Document
├─ canvas: CanvasSize, background: Option<Rgba>, fps, frame_count
├─ layers: LayerStack            (bottom-to-top Vec<Layer>; scene crate)
├─ tracks: BTreeMap<LayerId, Track>    (timeline crate)
│     Track = BTreeMap<FrameIndex, Keyframe>;  Keyframe = Drawing(DrawingId) | Empty
└─ drawings: BTreeMap<DrawingId, DrawingContent>  (drawing crate)
      DrawingContent::Vector(VectorDrawing { strokes: Vec<Stroke>, fills: Vec<Fill> })
      Stroke/Fill.placement: Normal | Front (draw-order tier; ADR-010)
      paint order: normal fills → normal strokes → front fills → front strokes,
      list order within each group (VectorDrawing::paint_order; ADR-007, ADR-010)
```
Drawings live in a pool, separate from keyframes. Later this lets one drawing be exposed on several frames (cycles and holds) without copying it.

## Coordinate spaces
1. **Screen**: physical pixels in the window, origin top-left.
2. **Canvas/document**: units of the document resolution, origin at the canvas top-left, y down. All stroke data is stored here.
3. **Layer-local**: a layer's own space, mapped to document space by `Layer.transform` (identity in the MVP).

Input is converted from screen to document coordinates by `renderer::Viewport` (Phase 1) **before** anything is stored.

## Frame flow
```
input → ui/desktop → editor::Command → History → Document
                                               │
                     EditorState.current_frame ┘→ Document::evaluate(frame) → RenderList → renderer
```
`evaluate` is cheap and allocates only one `Vec`. The renderer caches stroke and fill meshes by `MeshId` (a `StrokeId` or `FillId`) and rebuilds a mesh only when its stroke changes.

## Error handling
- Library crates return typed `thiserror` errors (`FileFormatError`, `CommandError`, `RendererError`).
- No `unwrap` on recoverable paths. Clippy's `unwrap_used` lint is on, and tests are exempt.
- The desktop app logs with `tracing` and exits with a message instead of panicking on startup failure.
