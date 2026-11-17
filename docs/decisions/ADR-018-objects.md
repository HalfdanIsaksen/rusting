# ADR-018: Objects (symbols and instance keys)

**Status:** Proposed (feat/symbols branch)

## Context
Users want to turn a set of layers and frames into a reusable object that holds its own animation, keep it in the library, and place it on any timeline. A 12-frame object placed on 18 frames should play 1.5 times, or play once and hold its last frame. A selection on the canvas should be able to become a one-frame object too. Motion tweens (next branch) will blend the placement of an object between two keys, so the placement must be easy to interpolate.

## Options
| | Symbol + instance key (chosen) | Nested `Document` per object | Layer-level "clip" property |
|---|---|---|---|
| Plays on any track, from any frame | Yes: a key, exposed like any other | Yes | Only whole layers |
| Reuses timeline rules (stepped exposure, insert/remove/move frames) | Yes, unchanged | Needs its own | Partly |
| Drawings stored once, in one pool | Yes | No: one pool per object | Yes |
| Tween-friendly placement | Decomposed transform on the key | Same | Per layer only |

## Decision
- **Model.**
  - `SymbolId` is a new UUID newtype in `animation_core`.
  - `document::Symbol { id, name, layers: LayerStack, tracks: BTreeMap<LayerId, Track>, frame_count }` lives in `Document.symbols`, a map of its own next to the asset pool. Its drawings stay in the document's shared drawing pool, in the symbol's own coordinates.
  - A new key type, `timeline::Keyframe::Instance(InstanceKey)`, places a symbol. `InstanceKey { symbol, transform: InstanceTransform, opacity, mode: InstanceMode, first_frame }`. `InstanceMode` is `Loop` or `Once`.
  - `InstanceTransform { translation, scale, rotation }` is kept decomposed and has `to_affine`, `from_affine` (drops shear) and `lerp`, so a tween can blend two keys part by part. `Keyframe` stays `Copy` but loses `Eq`.
- **Timing.**
  - An instance key is exposed like any other key: from its frame until the next key or the document end.
  - Frame `f` shows inner frame `first_frame + (f − key)`. `Loop` wraps it modulo the symbol's length, and `Once` clamps it to the last frame (`InstanceKey::inner_frame`).
  - `InstanceKey::advanced(n, len)` is the same playback started `n` frames later. Splitting an exposure (Convert to Object, Duplicate Frame, moving an object on a later frame) uses it, so playback continues seamlessly.
- **Evaluation.**
  - `Document::evaluate` expands instance keys recursively, up to `MAX_SYMBOL_DEPTH` (16) levels. Deeper nesting, which only a damaged file with a cycle can cause, draws nothing.
  - Transforms compose as parent × instance × symbol layer. Opacities multiply.
  - The symbol's layers come out as more `RenderLayer`s carrying the outer layer's `layer_id` and `blend`, so the renderer's per-layer blend compositing treats the object as part of that layer. Blend modes of the symbol's own layers are ignored.
  - `RenderLayer.transform` is now an `Affine2`, and `RenderLayer.in_object` marks content from inside an object. `evaluate_symbol` and `instance_bounds` evaluate one object for previews and hit testing.
- **Library.**
  - Objects are listed in the library's Objects section. The section reads `Document.symbols` directly instead of filtering the asset pool, because a symbol is document structure (layers and tracks) rather than an imported file. This also keeps `AssetKind` limited to imported files.
  - Like assets, symbols stay until the user deletes them. Their drawings count as referenced, so saving never drops them. `RemoveSymbol` is refused while an instance key places the symbol, on a document track or inside another symbol.
- **Commands** (`editor::commands::symbols`):
  - `ConvertToSymbol { layers, range }` copies the keys inside the range into a new symbol starting at frame 0. Drawings get fresh copies, and a drawing or object exposed at the range start by an earlier key gets a key at 0. It clears the range on the original layers and keeps what they showed before and after it. A new layer above them gets an instance key at `range.start` and an `Empty` key at `range.end`.
  - `ConvertItemsToSymbol { layer, frame, items }` makes a one-frame symbol from canvas items. The items are moved so the centre of their bounds is the origin, and the instance sits on a new layer translated to that centre, over the same frames the drawing was exposed.
  - `PlaceInstance` places a key on a layer or on a new layer. `SetInstanceTransform` changes the key on its own frame, and on a later frame creates a new instance key that continues playback. `SetInstanceProps` sets the mode, first frame and opacity; changes in one drag gesture merge.
  - `RenameSymbol` and `RemoveSymbol` manage the library entry.
  - Drawing commands refuse a frame that shows an object with `CommandError::ObjectFrame`, so a stroke never cuts an object short with a new drawing key. Resize Canvas moves instances, not the drawings inside objects.
- **UI.**
  - Convert to Object is in the timeline and layer context menus, in Edit, and on `F8`.
  - The Selection and Transform tools select an object as one unit by its bounds. Moves, nudges, scales and rotations commit `SetInstanceTransform`.
  - Tool Settings shows Loop / Play once, the first frame and the opacity of the object under the playhead.
  - The timeline draws object exposures in their own colour, with a diamond key marker and a tick where each loop starts.
  - Library rows drag an `ObjectDrag` payload onto a timeline cell or onto the canvas, where the object goes on a new layer centred on the drop point.
- **Files.** Format version 11.
  - `symbols` is saved as a map keyed by ID and left out when empty. v10 files need no migration. Older builds must reject v11 files, because they cannot read instance keys.
  - Validation rejects:
    - a key placing a missing object
    - a `first_frame` past the object's end
    - objects that contain themselves
    - an object layer without a track, or a track without a layer
    - an object with no frames, or stored under another ID
    - layer IDs shared between objects and the document
    - an unusable opacity or placement

## Consequences
- An object's contents can't be edited in place yet (no "double-click to enter"). To change one, place its frames back by hand or make a new object. Drawing tools refuse object frames with a message.
- Onion skin ignores objects: it shows neighbouring drawing keys only.
- Converting copies the drawings, so the originals stay in memory for undo and are dropped on the next save.
- The layer transform is still the identity in practice. The tools apply object moves and transforms in document space, like they do for items.
- `from_affine` drops shear. A rotate followed by a non-uniform scale on the canvas is stored as the closest scale and rotation.
- **For motion tweens:** add `tween: Option<Interpolation>` to `InstanceKey`. In `push_instance` (`document/src/evaluate.rs`), when the exposed key has a tween and the next key on the same track is an instance of the same symbol, blend `transform` (with `InstanceTransform::lerp`) and `opacity` by the eased factor. The inner frame stays the first key's.
