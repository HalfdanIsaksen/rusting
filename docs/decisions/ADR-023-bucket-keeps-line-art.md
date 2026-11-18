# ADR-023: The fill bucket keeps same-colour line art

**Status:** Proposed (amends ADR-012)

## Context
ADR-012 let a new bucket fill absorb the touching strokes of its own colour, so fill and outline became one fill. With black line art, filling an area black (shadows, hair, pupils) swallowed the line: it stopped being a stroke, could no longer be edited as one, and stopped bounding later bucket fills on its other side.

## How other tools handle it
| Tool | Model | Filling an area with the line's colour |
|---|---|---|
| Adobe Animate | Vector, "merge drawing" | Fills merge with touching fills of the same colour. Strokes are a separate kind of item and are never merged into fills by the bucket. |
| Toon Boom Harmony | Vector, lines and colour zones | The bucket paints the zone. Lines (and invisible strokes) stay lines, often on a separate Line Art / Colour Art layer. |
| TVPaint | Raster | The flood region comes from the existing pixels (optionally another layer as the source), not the new colour. The line pixels are left as they are, and filling on its own layer under the line is common. |
| Photoshop | Raster | Contiguous flood from the clicked pixel within a tolerance. The boundary is decided by what is already there, so a same-colour fill never moves the line. |

The common rule: painting a region never consumes the line art that bounds it.

## Decision
- The bucket runs `MergeSameColour::new(..).fills_only(true)`, which calls `VectorDrawing::merge_same_colour_fills`. The new fill still merges with touching fills of the same colour, opacity and tier (ADR-012), but no existing stroke is absorbed.
- The brush, rectangle and ellipse keep the ADR-012 behaviour: a brush stroke merges into a same-colour fill it touches, and a shape's own outline merges with its own fill when they share a colour.
- No data model or file format change.

## Consequences
- Fill and same-colour outline stay two items: the outline keeps bounding later fills, stays editable as a stroke and is what a click on the line selects.
- Selecting or moving the filled shape picks the fill alone; select the outline too to move both.
- At partial opacity, the overlap between fill and outline (where the fill tucks under the line) looks darker, as with any two overlapping items.
