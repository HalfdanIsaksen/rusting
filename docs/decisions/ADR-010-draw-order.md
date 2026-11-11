# ADR-010: Draw order (Behind / Normal / In front)

**Status:** Proposed (draw-order branch)

## Context
Users want to draw shapes and brush strokes **behind** or **in front of** what is already on the canvas, and anything set to **in front** must stay on top of everything else in the frame, including what is drawn later.

Before this, a drawing painted all its fills (in list order) and then all its strokes. The brush could already insert a stroke at the bottom of the stroke list ("Paint behind"), but nothing could stay on top: every later item covered it. A shape's fill always sat under every stroke, so an opaque rectangle could never hide existing lines.

## Options
| | Per-item z-index | Two tiers + list order (chosen) | One mixed item list |
|---|---|---|---|
| "Always on top" | Needs a max-z rule kept up by every command | Built in: the front tier paints last | Needs re-sorting on every insert |
| Data change | Number on every item; renumbering | One enum per item, default omitted from files | Replaces `strokes` + `fills` (big migration, touches every crate) |
| Fill-under-stroke rule (ADR-007) | Lost unless encoded in z | Kept within each tier | Lost |

## Decision
- **Data.** `drawing::Placement { Normal, Front }` on `Stroke.placement` and `Fill.placement`, default `Normal` (the old behaviour). Storage stays two lists (`strokes`, `fills`). List position is the order within a tier.
- **Paint order** per drawing, bottom to top: normal fills → normal strokes → front fills → front strokes, each group in list order. `VectorDrawing::paint_order()` yields it, and everything follows it: the canvas (`renderer::CanvasRenderer::prepare_render_list`, `DrawListBuilder::render_list`, so also export), the desktop draw list with the live preview, the onion skin, `item_at` (topmost first, so selection, the eyedropper and the bucket's "click a line" check), `fill_at`, `strokes_hit` and `all_items` (marquee, lasso, select all).
- **Tool option** (`ui::PaintOrder`), in Tool Settings for Brush, Rectangle, Ellipse and the Paint Bucket. For a shape it applies to both fill and outline:
  - *Behind*: normal tier, at the **bottom** of the list (`AddStroke::behind` / `AddFill::behind`). A behind rectangle goes under existing fills and strokes. Its outline still paints above normal fills, because strokes always sit above the fills of their tier.
  - *Normal* (default): normal tier, on top of the list. This is the old default.
  - *In front*: front tier, on top. The item stays above every normal item, also those drawn later. A front fill covers normal lines (the bucket's fills tuck half a line width under the lines, so an in-front bucket fill covers half of its outline).
  The old brush "Paint in front / Paint behind" toggle became this option (its "in front" was *Normal*).
- **Preview.** `ui::paint_order::with_preview` adds the preview items to a copy of the layer's drawing, in the same place a commit would add them. The live preview therefore shows the final stacking, for example a *Behind* rectangle drawn under the lines while you drag.
- **Keeping the tier.** Partial-erase pieces, transforms, moves, duplicates and bucket recolours keep `placement`. Every change is still an undoable command (`AddStroke`, `AddFill`, `Batch`).
- **File format v6.** `FORMAT_VERSION = 6`. `placement` is written only for front items, and an absent field means `Normal`, so v5 files need no migration step. The bump is needed because a v5 build would silently drop `"Front"` and change the picture. `validate` needs no new check: serde rejects unknown tiers as malformed.

## Consequences
- An in-front fill still paints under in-front strokes, and a behind outline still paints over normal fills. The order is "tier, then kind", not a free z-order. Moving existing items between tiers (Bring to front / Send to back on a selection) is not built yet. It would be a small `SetPlacement` command over the selection.
- Bucket recolour swaps the fill for a new one on top of its tier (as before ADR-010). The tier is kept, but the position inside the tier is not.
- Fill-region finding (ADR-007) still treats every stroke as a boundary, whatever its tier.
