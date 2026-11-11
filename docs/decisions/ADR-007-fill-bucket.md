# ADR-007: Fill bucket (raster-assisted, stored as vector)

**Status:** Proposed (fill-bucket branch)

## Context
Animators colour line art by clicking inside closed areas. Hand-drawn lines often leave small gaps, so the tool needs a gap-closing option: No gaps / Small gaps / Medium gaps. ADR-001 keeps drawings as editable vectors and noted that fills would need extra work.

## Options
| | Planar map (exact vector regions) | Raster-assisted, stored as vector | Raster fill layer |
|---|---|---|---|
| Robustness with messy, overlapping, pressure-varying strokes | Hard (curve intersections, numerical edge cases) | Good: works on whatever the strokes cover | Good |
| Gap closing | Hard | Easy: dilate the lines | Easy |
| Stored data | Vector | Vector contours | Pixels per frame |
| Zoom quality | Exact | Accurate to one grid cell (1 unit normally) | Pixelates |

## Decision
Raster-assisted region finding, stored as vector:

- **Data.** `drawing::Fill { id: FillId, contours: Vec<Vec<Vec2>>, color: Rgba, opacity: f32 }` in `VectorDrawing.fills`. Contours are in document coordinates; outer contours have positive signed area (y down) and holes negative, and hit-testing uses even-odd. `FillId` lives in `animation_core`. Fills draw **under** all strokes of the same drawing.
- **Region finder** `drawing::fill_region(drawing, point, gap, canvas_size) -> Result<FillShape, FillError>`, pure and unit-tested:
  1. Rasterize the strokes' painted shape (same geometry as the tessellator and hit test) into an "ink" grid over the padded stroke bounds. Cells are 1 document unit. If that would exceed `MAX_GRID_CELLS` (1.5M), cells grow, so large drawings stay fast (~50 ms release for a full 1920×1080 frame; ~0.7 s in debug builds). Ink is inflated by 0.75 cell, so hairlines always form a closed barrier.
  2. **Gap closing:** cells within the gap radius of ink also block (exact Euclidean distance transform). `SMALL_GAP_RADIUS = 4`, `MEDIUM_GAP_RADIUS = 10` document units close gaps up to about twice that.
  3. Flood fill (4-connected) from the click. Reaching the grid border means the area is not enclosed: `FillError::NotClosed`, shown as "Area not closed — try a larger gap setting". The tool never floods the whole canvas. A click on ink gives `OnLine`. A click inside the gap margin starts from the nearest free cell that can be reached without crossing ink.
  4. Grow back by the gap radius. That rounds off corners and drops thin parts (a morphological opening), so every enclosed pocket of gap-margin cells that touches the fill is added back. The gap itself opens to the outside, so it is not added.
  5. Tuck under the lines: ink cells within half the widest line width (+1 cell) of the fill join it. This leaves no halo, and because only ink cells are added, nothing shows outside the lines.
  6. Keep the component connected to the click, trace the cell boundary (marching squares: midpoints of boundary cracks, turning right at diagonal touches), then simplify with Douglas–Peucker (0.6 cell).
- **Tessellation:** `drawing::tessellate_fill` joins holes into their outer contour with bridges, then ear clips. It is written here with no dependency: contours are already simplified (tens to hundreds of points), so an O(n²) ear clipper is fast. Fill triangles don't overlap, so plain alpha blending is correct at any opacity.
- **Rendering:** the renderer's mesh cache is keyed by `MeshId::{Stroke, Fill}`. `StrokeDraw.id` is now a `MeshId`, and `StrokeId`/`FillId` convert with `.into()`. Per drawing, fills are pushed before strokes, so the existing per-draw depth ordering puts them underneath. Onion skin tints fills like strokes.
- **Editing:** `editor::commands::{AddFill, RemoveFill}` mirror `AddStroke`/`RemoveStroke`: they create a drawing and key on an empty frame and refuse locked layers. Clicking an existing fill with a different colour recolours it as one undo step (`Batch[RemoveFill, AddFill]`). That step also recolours strokes along the fill's edge that had the fill's old colour, so a fill and outline of one colour change together. Clicking a stroke with the bucket recolours the stroke (`ReplaceStroke` with the same ID).
- **File format v2:** `FORMAT_VERSION = 2`. The migration adds `"fills": []` to every v1 vector drawing. `validate` checks fill IDs are unique and that colours, opacity and points are in range.

## Consequences
- Accuracy is one grid cell. With very large drawings (cell > 1 unit) edges are slightly coarser, but they are still under the line art.
- Fills are independent of their strokes: moving or erasing a stroke later does not update the fill (no live planar map). The stroke eraser does not remove fills yet. `VectorDrawing::fill_at(point)` plus `RemoveFill` is the hook for that.
- Gap closing can poke into a closed gap by a fraction of the gap radius. Areas narrower than twice the gap radius that are entirely separate from the click area are not filled, which is intended.
