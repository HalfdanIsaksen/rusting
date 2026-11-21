# ADR-024: Brush Editor and brushes from artwork

**Status:** Proposed (follows ADR-022)

## Context
ADR-022 added stamp brushes, but the only way to get a new one was to build a `.rustbrush` zip by hand. Its eleven settings could not be changed in the app. Users want to draw a tip with the drawing tools and tune how it stamps, as they can in other painting tools.

## How other tools handle it
| Tool | Making a tip | Editing |
|---|---|---|
| Photoshop | Edit → Define Brush Preset turns the selection into a grayscale tip: dark pixels paint. | A Brush Settings panel (shape dynamics, scattering, texture, transfer) with a live preview stroke. Changes apply to the current brush; saving makes a preset. |
| Procreate | Brush Studio: a "shape source" (tip) and a "grain source" (paper), each imported, pasted or picked from a library. | A drawing pad tries the brush while you edit. Editing a built-in brush makes a duplicate. |
| Krita, Clip Studio | A brush tip from the canvas or a file; Clip Studio registers a selection as material. | A brush editor with a scratchpad. |

## Decision
- **Editing is a draft.** `ui::brush_editor::BrushEditor` holds the brush being edited, its name and a `BrushData` draft. Sliders change only the draft, so dragging doesn't fill the undo history.
- **Apply is one undo step.** It runs `Batch[SetBrush, RenameAsset]`. `editor::commands::SetBrush` replaces the asset's tip, grain and settings, clamped, and refuses an asset that isn't a brush. Every stroke drawn with the brush restyles. That is the point of editing a library brush.
- **Built-in brushes are never changed.** Applying a draft of a built-in brush adds it as a new library brush ("<name> copy" unless renamed), picks it, and the editor carries on with the copy. Strokes drawn with the built-in keep their look, and the built-in stays the same in every project.
- **Pictures come from the host.** The session can't render or decode, so a new tip or grain is a `BrushCapture` with a `BrushPictureSource`:
  - artwork to render: the selection pasted into a throwaway document, or a copy of the document with only one layer visible
  - a picture file to pick
  - a generated ellipse tip or a generated noise grain

  The session pushes `Request::CaptureBrushPicture`. `apps/desktop/src/brush_pictures.rs` makes the picture with `OffscreenRenderer` and `image`, and hands it to `Session::finish_brush_capture`.
- **Tips from artwork.** Coverage = alpha × (1 − luminance), so dark marks paint (Photoshop's convention). A picture whose border is mostly "paint" is light on dark, like the built-in tips, and is read as brightness instead. The marks are cropped, centred, and scaled so that their bounding box fits inside 92% of the tip's inscribed circle. They then fade to zero at the circle (the ADR-022 rule). The tip is at most 256 px across and saved as a gray PNG.
- **Grains from artwork** are the art composited over white paper, cropped to what is drawn and at most 1024 px across, so dark marks become pits that stay empty. Generated grains tile; grains from artwork don't, unless the artwork tiles.
- **Scratchpad.** The host renders a tapered S-stroke drawn with the draft, plus tip and grain thumbnails, whenever the editor's revision changes, and gives the window egui textures.
- **Renderer cache.** A brushed stroke's mesh key now includes every brush setting (`StrokeKey::of_brushed`), so applying new settings rebuilds the stamps. Tip and grain textures are still keyed by buffer address: a new picture is a new buffer, and undo keeps the old one alive.
- No file format change: `BrushData` was already saved in full.

## Consequences
- Create Brush from Selection (Edit menu, canvas right-click, Tool Settings) makes a brush in one click and opens it in the editor.
- The two-layer workflow: draw a tip on one layer and a paper texture on another, then use **From Layer at Current Frame** for each.
- An applied edit is visible at once on every stroke with the brush, including in other frames and objects. Undo puts the old brush back.
- The editor does not add new dynamics. Taper, minimum size, colour jitter, a second tip and pen pressure are still to come. Photoshop ABR and Procreate import are still to come.
