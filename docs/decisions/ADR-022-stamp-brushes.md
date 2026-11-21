# ADR-022: Stamp brushes and the `.rustbrush` format

**Status:** Proposed (feat/brushes branch).

## Context
Animators expect a pencil with a paper feel. Tools such as Toon Boom get it from raster pencils: the line breaks up on the paper grain, and passes build up darker. Rusting is vector-first (ADR-001), and select, move, transform, partial erase, bucket fill, objects and tweens all depend on strokes being vector data. Users also want to share brushes and use the same brush in several projects.

## Options
| | Stamp engine on vector strokes (chosen) | Textured ribbon shader | Raster layers |
|---|---|---|---|
| Paper feel (grain, build-up, ragged edges) | Yes | Grain only; no build-up or tip shape | Full |
| Editing tools keep working | Yes | Yes | Need raster versions |
| Different tips (pencil, charcoal, chalk, marker) | Yes | No | Yes |
| File size | Points plus one brush per project | Points | Pixels per frame |
| Work | Medium | Small | Large |

## Decision
- **Model.**
  - `drawing::brush`: `BrushSettings` holds spacing, flow, opacity_pressure, angle, angle_jitter, follow_direction, size_jitter, opacity_jitter, scatter, grain_scale and grain_strength. Every field has a `serde(default)`, a documented range, `validate()` and `clamped()`.
  - `BrushData { tip: ImageData, grain: Option<ImageData>, settings }` reuses the base64 `ImageData` of imported pictures.
  - `BrushStyle` gains `brush: Option<AssetId>` (left out of files when `None`, which means a solid stroke). The type stays `Copy`, so erase splitting, recolouring and transforming keep the brush without changes.
  - `AssetKind::Brush(BrushData)` in the library (ADR-017). `Document::used_assets` counts the brushes of strokes in referenced drawings, so `RemoveAsset` refuses a brush in use.
- **Stamps.** `drawing::stamps(stroke, settings)` is pure:
  - It places a stamp every `spacing × width × pressure` along the path, at most 200,000 per stroke.
  - Jitter comes from a SplitMix64 seeded by the `StrokeId`, so the editor, the preview window and every export draw the same stamps.
  - Size always follows pressure, as for solid strokes. Scatter shrinks the stamp by the same fraction, so every stamp stays inside the circle of radius `width × pressure / 2`. Hit testing, erasing and bucket boundaries therefore use the stroke geometry they always used.
  - Tips must fade to nothing at their inscribed circle.
- **Rendering.**
  - `StampMesh` holds one quad per stamp: position, tip UV and stamp opacity, 20 bytes per vertex. It is cached under `MeshId::Brushed(StrokeId)`, and `StrokeKey` hashes the brush ID.
  - Tip and grain are uploaded once per brush as `Rgba8Unorm` textures (coverage, not colour), cached by `AssetId`. A brush without grain binds its tip twice and sets the strength to 0.
  - `brush.wgsl` computes coverage as `tip.r × tip.a × stamp opacity × colour alpha`. The grain is sampled in layer coordinates with a repeating sampler, so it stays on the paper while strokes move.
  - **Grain is a height threshold, not a multiplier:** `smoothstep(strength − 0.5, strength, grain)`. With a multiplier, dozens of overlapping low-flow stamps filled the valleys and the grain disappeared. With the threshold, valleys stay empty however much paint builds up.
  - **Depth:** the brush pipeline tests `LessEqual` with depth writes on. All stamps of a stroke share its depth, so they build up, and later items, which are nearer, still cover the stroke. Solid strokes keep `Less`, so a pixel is painted once.
  - `DrawListBuilder::with_assets(doc.assets())` resolves brushes. `with_brush` adds a brush that is not in the library yet: the canvas uses it for the stroke being drawn with a built-in brush. A stroke whose brush can't be found is drawn solid.
- **Built-in brushes.** The app ships six, in `assets/brushes/<slug>/` (`brush.json` plus `tip.png` and an optional `grain.png`), compiled into `apps/desktop` with `include_bytes!`:
  - Pencil (HB)
  - Soft Pencil (6B)
  - Charcoal
  - Chalk
  - Ink Pen
  - Marker

  Each has a fixed UUID. `cargo run -p desktop --example generate_brushes` regenerates the PNGs from fixed seeds; the JSON files are written by hand. **The first stroke drawn with a built-in brush adds it to the document's library in the same undo step** (`Batch [AddAsset, AddStroke]`). Projects therefore always contain their brushes, and the file format needs no list of built-ins.
- **`.rustbrush` v1.**
  - A zip with `brush.json` (`{ "format_version": 1, "id"?, "name", "settings" }`), `tip.png` and an optional `grain.png`.
  - Only PNG is accepted, at most 1024×1024, and each entry at most 32 MB uncompressed.
  - Settings out of range are clamped. A newer `format_version` is refused with a message to update.
  - Reading and writing live in `apps/desktop/src/brushes.rs`, using the `zip` crate with deflate only. File → Import… takes `.rustbrush`; the Library's Export Brush… writes one. An imported brush gets a new `AssetId`.
- **UI.**
  - Tool Settings → Brush has a picker listing Vector (solid), the built-ins, then the project's other library brushes. The chosen brush is `EditorState.brush.brush`. It falls back to solid when its asset disappears (File → Open, deleting the brush, or undoing an import).
  - The Library has a Brushes section with Use Brush, Export Brush…, Rename and Delete. Delete is refused while a stroke uses the brush.
- **Merge same colour skips brushed strokes.** `Look::of_stroke` returns `None`: a fill can't carry the texture. The checkbox is greyed out while a textured brush is selected.
- **Files.** Format version 14. Validation rejects a stroke whose brush is missing (`MissingBrushAsset`) or not a brush (`NotABrushAsset`), brush settings out of range (`BadBrushSettings`), and an empty tip or grain (`EmptyAsset`). v13 files load unchanged.

## Consequences
- **No tablet pressure yet.** Live input records pressure 1.0, so pressure-to-opacity and pressure-to-size only matter for imported data. The presets are tuned to look right at full pressure. Capturing pen pressure is a follow-up.
- **No brush editor.** Settings come from `brush.json`. Changing a library brush's settings would restyle every stroke that uses it; that needs an undoable `SetBrushSettings` and UI. *(Done in ADR-024: the Brush Editor and `SetBrush`.)*
- **No per-stroke opacity cap.** Stroke and layer opacity scale each stamp, so a translucent brushed stroke darkens where its own stamps overlap. Capping it would need each stroke rendered to its own target.
- The grain follows the layer and object transform, not the canvas, so it moves with an object that moves.
- Rectangle and Ellipse outlines stay solid.
- A long brushed stroke has more geometry than a solid one: four vertices per stamp, about 10 stamps per stroke width.
- Built-in brushes are stored in every project that uses them: 40–70 KB each with grain, about 4 KB without.
- Procreate and Photoshop ABR import can convert into this format later.
