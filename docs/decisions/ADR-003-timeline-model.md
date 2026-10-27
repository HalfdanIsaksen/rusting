# ADR-003: Timeline and frame model

**Status:** Accepted (Phase 0)

## Decision
- Frames are `FrameIndex(u32)`, **zero-based** internally and **one-based** in the UI (`display_number`).
- Each layer has one drawing `Track`: `BTreeMap<FrameIndex, Keyframe>`, where `Keyframe = Drawing(DrawingId) | Empty`.
- **Stepped exposure:** the drawing visible at frame *f* is the last key at or before *f*. It holds until the next key, or until `Document.frame_count`. An `Empty` key ends an exposure.
- Drawings are referenced by ID from a document-wide pool, so several keys can expose the same drawing later.
- Property animation (position, rotation, scale, opacity) will use `PropertyTrack<T>` with `Interpolation::{Step, Linear}`, and Bezier later. The `Interpolation` type exists now. Property tracks are added after frame-by-frame animation works.

## Consequences
- `BTreeMap` gives O(log n) exposure lookup and ordered iteration for the timeline UI.
- Inserting or deleting frames means rebuilding keys at shifted indices. That's acceptable at animation scales.
