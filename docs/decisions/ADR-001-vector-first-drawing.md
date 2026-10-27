# ADR-001: Vector-first drawing representation

**Status:** Accepted (Phase 0)

## Context
The MVP needs editable drawing data that survives frame changes, zoom, and save/load. The choice is between raster (pixel tiles per drawing) and vector (stroke geometry). The project owner asked for vector first.

## Options
| | Vector strokes | Raster tiles |
|---|---|---|
| Zoom quality | Resolution-independent | Pixelates and needs mipmaps |
| File size | Small (points) | Large (pixels per frame) |
| Editing (select, move, recolor, erase a stroke) | Natural | Hard |
| Painterly brushes, smudge | Hard | Natural |
| GPU work | Tessellation plus a mesh cache | Texture uploads and blending |

## Decision
- A drawing is a `VectorDrawing { strokes: Vec<Stroke> }`. A `Stroke` is `{ id: StrokeId, points: Vec<StrokePoint{pos, pressure}>, style: BrushStyle{color, width, opacity} }`, stored in **document coordinates**.
- Strokes are tessellated on the CPU in `drawing` (polyline → triangles with round joins and caps). This is pure and unit-tested. The renderer caches meshes per `StrokeId` and rebuilds only changed strokes.
- The MVP eraser removes whole strokes (hit-test). Splitting strokes comes later.
- Content is wrapped in `enum DrawingContent { Vector(..) }`, so a `Raster` variant can be added later without changing the timeline, document, or file envelope.

## Consequences
- Pressure is stored from day one (mouse = 1.0), so tablet support doesn't need a format change.
- Fills, bucket tools, and painterly brushes need extra work later: planar maps, or a raster layer type.
