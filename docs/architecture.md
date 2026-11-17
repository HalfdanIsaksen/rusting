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
| `LayerId`, `GroupId`, `DrawingId`, `StrokeId`, `FillId`, `ImageId`, `AssetId`, `SymbolId`, `DocumentId` | UUID v4 newtypes, stable across save and load |
| `Rgba` | Straight-alpha color, f32 components |
| `Interpolation` | `Step`, `Linear`, `EaseIn`, `EaseOut`, `EaseInOut` (cubic; Bezier later) |
| `Vec2` | `glam::Vec2`, re-exported |

## Data model
```
Document
├─ canvas: CanvasSize, background: Option<Rgba>, fps, frame_count
├─ layers: LayerStack            (bottom-to-top Vec<Layer>; scene crate)
│     + groups: Vec<LayerGroup>; Layer.group: Option<GroupId>, members adjacent
│     (one level, no nesting; group visibility/lock/opacity apply to members; ADR-013)
├─ tracks: BTreeMap<LayerId, Track>    (timeline crate)
│     Track = BTreeMap<FrameIndex, Keyframe>
│     Keyframe = Drawing(DrawingId) | Empty | Instance(InstanceKey)
│     InstanceKey { symbol, transform: InstanceTransform { translation, scale,
│       rotation }, opacity, mode: Loop | Once, first_frame,
│       tween: Option<Interpolation> }  (ADR-018, ADR-019)
│     A tween is active while the next key places the same symbol
│     (Track::tween_from → TweenSpan { start, end, ease }).
├─ drawings: BTreeMap<DrawingId, DrawingContent>  (drawing crate)
│     DrawingContent::Vector(VectorDrawing { strokes: Vec<Stroke>, fills: Vec<Fill>,
│                                            images: Vec<Image> })
│     Image = encoded PNG/JPEG bytes + Affine2 placement (ADR-015)
│     Stroke/Fill/Image.placement: Back(level) | Normal | Front (draw order; ADR-010, ADR-011)
│     paint order: back levels deepest first (images → fills → strokes each) → normal
│     images → normal fills → normal strokes → front images → front fills → front
│     strokes, list order within each group
│     (VectorDrawing::paint_order; ADR-007, ADR-010, ADR-011, ADR-015)
│     Image.asset: Option<AssetId> = the library entry it was placed from (ADR-017)
├─ assets: BTreeMap<AssetId, Asset>    (the library; document crate, ADR-017)
│     Asset { id, name, kind: AssetKind };  AssetKind = Image(ImageData)
│     An asset stays until the user deletes it (RemoveAsset is refused while a
│     drawing a keyframe shows uses it). Files store a placed image's bytes
│     only in its asset.
└─ symbols: BTreeMap<SymbolId, Symbol>   (objects; document crate, ADR-018)
      Symbol { id, name, layers: LayerStack, tracks, frame_count }
      Its drawings live in the shared pool, in the symbol's own coordinates,
      and are never garbage collected while the symbol is in the library.
      Instance keys place it: frame f shows inner frame
      first_frame + (f − key), wrapped (Loop) or held at the end (Once).
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
`evaluate` is cheap and allocates only one `Vec`. Instance keys are expanded recursively (at most `MAX_SYMBOL_DEPTH` levels): each visible symbol layer becomes a `RenderLayer` of the document layer holding the instance, with `transform: Affine2` = layer × instance × symbol layer, opacities multiplied, and `in_object` set so editing tools leave it alone. Inside an active motion tween, `Track::displayed_instance` blends the key's transform and opacity toward the next key by the eased factor, and the inner frame keeps counting from the first key (ADR-019). The renderer caches stroke, fill and image meshes by `MeshId` (a `StrokeId`, `FillId` or `ImageId`) and rebuilds a mesh only when its item changes. Image textures are decoded once per image and uploaded on the next render.

## Error handling
- Library crates return typed `thiserror` errors (`FileFormatError`, `CommandError`, `RendererError`).
- No `unwrap` on recoverable paths. Clippy's `unwrap_used` lint is on, and tests are exempt.
- The desktop app logs with `tracing` and exits with a message instead of panicking on startup failure.
