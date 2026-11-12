# ADR-013: Layer groups and layer opacity

**Status:** Proposed (feat/layer-opacity-groups branch)

## Context
Users want to set how see-through each layer is, and to keep related layers (for example a character's line art, colour and shading) together in a folder they can hide, lock, fade, fold and move as one.

`Layer.opacity` already existed in the data model and the renderer already multiplied it into every stroke and fill colour, but nothing in the UI could change it.

## Options for groups
| | Flat stack + group records (chosen) | Tree of layers and groups | Tags on layers |
|---|---|---|---|
| Timeline rows, tracks, `RenderList` | Unchanged: still one flat bottom-to-top list | Every consumer walks a tree | Unchanged |
| Nesting | One level | Any depth | None (not a folder) |
| Fold, move as a block | Yes (members are adjacent) | Yes | No (members can be anywhere) |
| Data change | `Layer.group: Option<GroupId>` + `LayerStack.groups` | New stack shape, migration | `Layer.tags` |

## Decision
- **Model.** `scene::LayerGroup { id: GroupId, name, visible, locked, opacity }` lives in `LayerStack.groups`; a layer is in a group when `Layer.group` is that group's ID. `GroupId` is a new UUID newtype in `animation_core`. Groups do not nest.
- **Invariant.** A group's layers are always adjacent in the stack. Every `LayerStack` method that inserts or moves a layer keeps it: a layer that lands between two layers of one group joins that group, and a group layer moved away from the rest of its group leaves it. `move_group` refuses to land inside another group. `check_groups` verifies it, and file loading rejects files that break it.
- **Effect.** Hidden group → its layers are not rendered or onion-skinned. Locked group → its layers cannot be drawn on (`LayerStack::is_editable`). The renderer gets `layer.opacity * group.opacity` per layer (`LayerStack::effective_opacity`) through `RenderLayer.opacity`; nothing in `renderer` changed. Groups have no blend mode of their own (pass-through).
- **Empty groups.** Removing a group's last layer keeps the group record, so undo can put the layer back into it. Empty groups are not shown and are dropped when saving.
- **Folding.** Whether a group is folded is view state (`TimelineView.collapsed_groups`), not a document change, so it is not undoable and not saved.
- **Commands.** `GroupLayers`, `UngroupLayers`, `MoveGroup`, `SetGroup` (rename / visible / locked), `SetOpacity` (layer or group) and `MoveLayer::into_group`. Reordering commands undo by restoring the stack's `Arrangement` (layer order plus group per layer).
- **Opacity edits.** `Command::merge` (default: no merge) lets the history fold a command into the previous one. `SetOpacity` commands of the same slider drag (`in_gesture`) merge, so one drag is one undo step while the canvas updates live.
- **Files.** Format version 8. `groups` and `group` are omitted when empty, so v7 files load unchanged. Older builds must reject v8 files, because dropping group visibility and opacity would change the picture.

## Consequences
- Layer and group opacity are applied per stroke and fill, not to the flattened layer, so where strokes of one semi-transparent layer overlap, the overlap is darker. Flattened opacity would need each such layer drawn into its own texture and composited, like non-Normal blend modes are (`renderer::canvas`).
- Groups cannot contain groups. Nesting would need a tree or a parent link per group and new drop rules.
- Dropping a layer right below a group's bottom layer puts it outside the group; `Move Down` on the bottom member keeps it in the group once before it leaves.
