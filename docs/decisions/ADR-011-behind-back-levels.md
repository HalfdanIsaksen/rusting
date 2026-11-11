# ADR-011: "Behind" goes under everything (back levels)

**Status:** Proposed (brush-behind-fill branch). Amends ADR-010.

## Context
README promise for draw order **Behind**: it "puts the new item under everything already in the frame".

ADR-010 made *Behind* mean "normal tier, at the bottom of its list". Paint order per drawing was normal fills → normal strokes → front fills → front strokes, so every normal stroke painted above every normal fill. A Behind brush stroke only went to the bottom of the *stroke* list and still covered every fill. A Behind rectangle's outline did the same. Users saw Behind strokes drawn on top of coloured areas.

## Options
| | Single `Back` tier | Back levels (chosen) | One mixed item list |
|---|---|---|---|
| Behind stroke under existing fills | Yes | Yes | Yes |
| Behind stroke under an earlier Behind *fill* | No: back strokes paint above back fills | Yes: each commit gets a deeper level | Yes |
| A shape's outline above its own fill | Yes | Yes: fill and outline share one level | Needs care |
| Data change | One variant | One variant with a number | Replaces `strokes` + `fills` in every crate |

## Decision
- **Data.** `drawing::Placement` gains `Back(u32)`: `Back(level) | Normal | Front`. Larger levels paint further back.
- **Paint order** per drawing, bottom to top: back levels from the deepest up (each level's fills, then its strokes), then normal fills, normal strokes, front fills, front strokes. Each group keeps stored list order. `VectorDrawing::paint_order` sorts the items by (tier, kind) with a stable sort. Everything that already followed `paint_order` (canvas, export, live preview, onion skin, hit-testing, selection) follows the new order with no change.
- **Choosing the level.** `VectorDrawing::behind_placement()` returns `Back(deepest + 1)` (or `Back(1)`). `ui::PaintOrder::placement(drawing)` uses it for *Behind*, given the frame's drawing. The level is fixed when the tool builds the item, so the command stays deterministic on redo. A rectangle or ellipse gives its fill and outline the same level, so the outline stays above its own fill while both go under everything already there. `ui::paint_order::with_preview` resolves the level from the same drawing, so the live preview shows the final stacking.
- **Keeping the level.** Erase pieces, moves, transforms, duplicates and bucket recolours copy `placement`, so they keep the level (a recoloured Behind fill now keeps its place, which ADR-010 could not).
- **File format v7.** `FORMAT_VERSION = 7`. A back item writes `"placement": {"Back": n}`. Normal is still omitted and Front is still `"Front"`, so v6 files load unchanged with no migration step. The bump makes v6 builds reject v7 files instead of failing on an unknown tier. Serde rejects malformed levels as a malformed file.

## Consequences
- Behind items from files saved before v7 stay where they were (normal tier, bottom of the list); only new Behind items use back levels.
- Levels only grow. They are per drawing and u32 (saturating), so running out is not a concern.
- There is still no "Bring to front / Send to back" for existing items; it would be a `SetPlacement` command using the same levels.
- Fill-region finding (ADR-007) still treats every stroke as a boundary, whatever its tier or level.
