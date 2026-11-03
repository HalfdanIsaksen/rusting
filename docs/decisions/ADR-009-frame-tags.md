# ADR-009: Frame tags and sprite sheets

**Status:** Proposed (frame-tags branch)

## Context
Game and UI animators keep several actions (walk, jump, idle) in one document. They need to play one action on a loop while they work on it, and to export the actions as a sprite sheet with one row per action.

## Decision
- **Model:** `Document.tags: Vec<FrameTag>`, where `FrameTag { name, range: FrameRange }`. Tags are document data. They are saved, and they change through the undoable `SetTags` command, which replaces the whole list. Tags may overlap. When they do, the first tag containing a frame wins.
- **File format v5:** `tags` is left out when empty and defaults to empty, so v4 files load unchanged and need no migration. Validation rejects a tag that covers no frames. A tag may reach past `frame_count`, the same as keys, so shortening the animation and then lengthening it again keeps the tag.
- **Frame edits:** `DuplicateFrame` moves tags that start after the inserted frame and grows a tag that contains the duplicated frame. Undo reverses both.
- **Playback:** when play starts, the play range is the tag that holds the start frame. If no tag holds it, the play range is the whole animation. The range stays fixed while playing, so playing through a tag does not get stuck looping inside it.
- **Sprite sheet:** `export::ExportTarget::SpriteSheet { rows }` builds the sheet in memory and writes it as one PNG or JPEG.
  - Row `i` holds tag `i`'s frames, left to right.
  - The sheet is as wide as the longest tag, and the cells after a shorter row stay transparent.
  - With no tags, the sheet is one row of the chosen frame range.

## Consequences
- The whole sheet is held in memory. A very large sheet (4K frames, hundreds of them) can need gigabytes.
- No atlas metadata (JSON) is written yet. Add it when a game engine import needs it.
- Tags are not yet moved by edits other than Duplicate Frame (none of the current edits insert or remove frames).
