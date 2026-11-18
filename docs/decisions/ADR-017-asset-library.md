# ADR-017: Asset library

**Status:** Proposed (feat/asset-library branch)

## Context
File → Import (ADR-015) copies each picture's bytes into the image item it creates. Nothing lists what has been imported, a picture can only be placed again by importing the file again, and the project file stores the bytes once per placed copy. Objects (reusable animated symbols) and audio layers come next. Both need a list of project-wide things that items on the timeline refer to by ID.

## Options
| | Asset pool in `Document` (chosen) | Library derived from placed images | Files linked by path |
|---|---|---|---|
| Lists files that are not placed (yet, or any more) | Yes | No | Yes |
| Project file stores each picture once | Yes | Only with a separate dedup pass | Yes, as a path |
| Project is one self-contained file | Yes | Yes | No: moving or sharing it breaks links |
| Fits audio and objects later | New `AssetKind` variants | No | Audio only |

## Decision
- **Model.**
  - `Document.assets: BTreeMap<AssetId, Asset>` holds the library. `AssetId` is a new UUID newtype in `animation_core`.
  - `Asset { id, name, kind: AssetKind }` lives in `document::assets`. `AssetKind::Image(ImageData)` is the only kind for now. Audio and objects get their own variants on their branches.
  - `drawing::Image` gets `asset: Option<AssetId>`, the entry it was placed from. Its `data` stays a shared copy of the asset's `ImageData` (an `Arc`), so the renderer, hit testing and export are unchanged.
- **Ownership.**
  - The library owns its assets. They stay until the user deletes them, including when nothing uses them.
  - `RemoveAsset` is refused with `CommandError::AssetInUse` while a drawing that some keyframe shows has an image from the asset (`Document::asset_in_use`). Drawings kept only for undo don't count.
- **Commands.** `AddAsset`, `RemoveAsset` and `RenameAsset` (`editor::commands::assets`). Placing reuses `AddImage`, with the asset ID in the `Image`.
- **Import.** `Session::import_image` runs one `Batch` of `AddAsset` and `AddImage`, so one undo takes back both. The asset is named after the file.
- **Library panel.**
  - A right-side panel (View → Library, `Cmd/Ctrl+4`, on by default) lists the assets in sections by kind. Images is the first section.
  - Double-click renames an asset. The context menu has Place on Canvas, Rename and Delete. Delete is disabled while the asset is in use.
- **Drag to canvas.**
  - A row sets an egui drag-and-drop payload, `ui::AssetDrag(AssetId)`.
  - While the payload is over the canvas, the desktop shell outlines where the picture will land. When it is released there, the shell converts the pointer to document coordinates and calls `Session::place_asset(id, Some(point))`.
  - The picture is placed like an import: centred on that point, shrunk to fit the canvas, one undo step, then selected with the Selection tool. Locked or hidden layers refuse the drop.
- **Files.** Format version 10.
  - `assets` is saved as a map keyed by ID, and left out when empty.
  - An image with an asset is saved without `data`. On load, its data is filled back in from the asset. A file therefore stores each library picture once, however often it is placed.
  - Older builds must reject v10 files, because they would load those images as empty. v9 files load unchanged.
  - Validation rejects:
    - an image whose asset is missing or is not a picture
    - two assets with the same ID
    - an asset stored under another asset's key
    - an empty library picture
  - If an image's asset is gone when saving (only possible for a drawing kept for undo), the image keeps its bytes and loses the reference, so every saved file loads.

## Consequences
- Duplicating a frame no longer duplicates a library picture's bytes in the file, which fixes a cost listed in ADR-015. The renderer still caches one texture per image item.
- Images imported before this change have no asset. They keep their own bytes and are not listed in the library.
- Deleting a placed copy never deletes the asset. Unused library pictures make the file bigger until the user deletes them.
- **Adding a kind:**
  - Add an `AssetKind` variant, and extend `Asset`'s accessors and `AssetKind::label`.
  - Extend `Document::used_assets` with whatever refers to the new kind.
  - Add an entry to `SECTIONS` in `ui/src/panels/library.rs`, with its own icon and empty-state text. Extend `describe` in the same file.
  - Add file-format validation for references to the new kind.
  - Brushes (ADR-022) followed this list: `AssetKind::Brush`, stroke styles refer to them, and they have a Brushes section.
