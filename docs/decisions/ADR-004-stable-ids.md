# ADR-004: Stable identifiers

**Status:** Accepted (Phase 0)

## Decision
All entity IDs (`DocumentId`, `LayerId`, `DrawingId`, `StrokeId`) are `#[serde(transparent)]` newtypes over `uuid::Uuid` v4, defined only in `animation_core`. IDs are assigned on creation and never reassigned, including by save/load and undo/redo. Commands capture IDs when they are built, so redo recreates the same IDs.

## Alternatives
Generational indices (slotmap) are faster and smaller, but not stable across files or across future copy/paste and merge between documents. We can add an index cache later if profiling shows a need.
