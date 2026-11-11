# ADR-012: Merging same-colour strokes and fills

**Status:** Proposed (merge-same-colour branch)

## Context
Users paint a shape with the bucket and then extend or touch it up with the brush in the same colour, expecting one solid shape. Before this, the brush stroke stayed its own item: selecting, moving or erasing the shape only acted on one part. Below 100% opacity the overlap also looked darker, because each item blends on its own.

## Options
| | Look only (render same-colour items as a union) | Merge into one fill (chosen) | Group items (shared group ID) |
|---|---|---|---|
| Select / move / erase as one | No | Yes | Yes, but every tool must learn groups |
| Darker overlap at partial opacity | Fixed in the renderer | Gone (one fill) | Still there |
| Data change | None | None: the result is an ordinary `Fill` | New field and file format bump |
| Keeps the stroke editable as a stroke | Yes | No (it becomes fill area) | Yes |

## Decision
- **Rule.** A new item merges with every *fill* of the same colour, opacity and draw-order tier (ADR-010) that it overlaps or touches, directly or through other such fills. The new item can be a brush stroke, a bucket fill, or a rectangle or ellipse's fill and outline. Gradient fills never merge. Existing strokes are never absorbed, and strokes never merge with strokes. Line art therefore stays strokes and keeps bounding later bucket fills, since `fill_region` only sees strokes (ADR-007).
- **Geometry.** `VectorDrawing::merge_same_colour(item) -> Option<Merge>` in `drawing::merge` rasterizes the candidates on one grid of `MERGE_CELL_SIZE` (0.5 unit) cells, capped at `MAX_GRID_CELLS`, and grows the merged area from the item. Candidates that overlap it or touch it within one cell join. The union is traced and simplified with the bucket's `trace_contours`. This is the same raster approach as the bucket and the fill eraser, with no new dependency. The merged outline is accurate to about one cell.
- **Identity.** The merged fill keeps the ID, colour, opacity and tier of the lowest merged fill (by stored index) and takes its place in the fill list.
- **Command.** `editor::commands::MergeSameColour::new(layer, frame, item)` computes the merge when applied, so redo recomputes the same result. Undo puts every original item back with its ID at its old index. If nothing merges, it does nothing. The UI runs it in the same `Batch` as the `AddStroke` / `AddFill`, so adding and merging are one undo step.
- **Option.** "Merge same colour" in Tool Settings for the Brush, Paint Bucket, Rectangle and Ellipse (one shared setting, on by default).
- **Files.** No data model or format change. Older `.rusting` files load unchanged, and existing items are only merged when a new item touches them.

## Consequences
- A merged stroke can no longer be edited as a stroke (width, smoothing). Undo or turning the option off are the ways to keep it separate.
- A merged stroke drops into the fill layer of its tier, so it paints under that tier's strokes. A brown stroke painted over a black outline lets the outline show again once it merges.
- Each merge re-traces the whole merged fill, so its outline is rounded to the grid. Exact rectangle or ellipse edges become accurate to about 0.5 units once something merges into them.
- Bucket recolouring a fill does not merge it with neighbours that now share its colour. Partial erasing does not merge pieces back.
