# ADR-015: Imported images (File → Import)

**Status:** Proposed (feat/import-image branch)

## Context
Users want to bring PNG and JPEG pictures into a frame, for example a reference to trace, a background or a logo. Until now every drawing was pure vector data (ADR-001): strokes and fills, tessellated into triangle meshes and drawn in one flat colour or a gradient.

## Options
| | Image item in `VectorDrawing` (chosen) | Raster `DrawingContent` variant | Trace into vector fills |
|---|---|---|---|
| Mix with strokes in one frame | Yes | No: one drawing is raster *or* vector | Yes |
| Select, move, scale, rotate, delete | The existing tools, through `ItemId::Image` | New tools | Yes |
| Looks like the original | Yes | Yes | No |
| Renderer work | One textured pipeline | One textured pipeline | None |

## Decision
- **Model.** `drawing::Image { id: ImageId, data: ImageData, transform: Affine2, opacity, placement }` lives in `VectorDrawing.images`. `ImageId` is a new UUID newtype in `animation_core`. `transform` maps the unit square (picture top-left at the origin, y down) to document coordinates, so moving and transforming an image composes matrices and never resamples pixels.
- **Pixels.** `ImageData` holds the original encoded PNG/JPEG bytes in an `Arc<[u8]>` plus the pixel size, not decoded pixels. Undo snapshots and duplicated frames share the bytes. Core crates stay decoder-free; `renderer` decodes with the `image` crate.
- **Paint order.** Images use the same draw-order tiers as strokes and fills (ADR-010, ADR-011). Within a tier, images paint under fills and fills under strokes, so lines drawn over an imported reference stay visible. Imports go into the normal tier.
- **Commands.** `AddImage` (creates the key and drawing if the frame is empty, like `AddStroke`) and `RemoveImage`. `TranslateItems`, `TransformItems` and `remove_items` handle `ItemId::Image`. The eraser, paint bucket, merge-same-colour and onion skin ignore images; the eyedropper does not pick from them.
- **Import.** File → Import… (below Export) asks the host for a file dialog (`Request::ImportDialog`). The desktop app checks that each file is a PNG or JPEG that decodes, then `Session::import_image` centres it on the canvas, shrinks it to fit if it is larger (keeping proportions), adds it as one undo step and selects it with the Selection tool.
- **Rendering.** An image is a four-vertex quad in the mesh cache (`MeshId::Image`) whose texture coordinates come from the vertex index (`image.wgsl`). `StrokeCache::ensure_image` decodes the bytes once into RGBA8 mip levels (shrunk to the device's maximum texture size); `CanvasRenderer` uploads pending textures at the next render as `Rgba8UnormSrgb` with linear, mipmapped filtering. The same path serves the canvas, the preview window and export.
- **Files.** Format version 9. `images` is omitted when empty, so v8 files load unchanged. Bytes are saved as base64 text. Older builds must reject v9 files, because they would drop the pictures. Validation rejects duplicate image IDs, empty images, non-finite transforms and opacity outside `0..=1`.

## Consequences
- Project files grow by about 4/3 of each imported file's size, once per image item. Duplicating a frame duplicates the item and, in the file, its bytes.
- A texture is cached per image item, so a duplicated frame decodes the picture again on first display.
- Images cannot be erased partially, filled into or recoloured. A raster editing model would need its own ADR.
