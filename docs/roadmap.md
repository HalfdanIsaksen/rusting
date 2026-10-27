# Roadmap

Each phase lists its acceptance criteria. Phases run in parallel where their crates don't overlap.

| Phase | Branch | Owns | Status |
|---|---|---|---|
| 0 Workspace and architecture | `phase-0/workspace` | everything | ✅ done |
| 1 Window and canvas | `feat/renderer-canvas` | `renderer`, `apps/desktop`, `assets/shaders` | ✅ merged |
| 2 Vector drawing engine | `feat/vector-drawing` | `drawing` | ✅ merged |
| 4/5 Timeline engine and playback | `feat/timeline-engine` | `timeline`, `animation_core` | ✅ merged |
| 6 Document ops, undo, persistence | `feat/document-persistence` | `document`, `scene`, `editor`, `file_format` | ✅ merged |
| 3–5 UI integration (layers, timeline, toolbar, onion skin) | `feat/ui-integration` | `ui`, `apps/desktop` | after 1 + 2 + 4 + 6 merge |
| 7 Refinement | — | — | after MVP |

## Phase 1: Window and canvas (`feat/renderer-canvas`)
1. `renderer::Viewport { canvas_size, zoom, pan, screen_size, scale_factor }` with `screen_to_canvas`, `canvas_to_screen`, and `zoom_about(cursor, factor)`. Pure math, unit-tested: round trips, and the canvas point under the cursor stays fixed through a zoom.
2. A canvas render target sized to the viewport panel, recreated on resize, with 4× MSAA.
3. Pipelines: canvas background (color, or a checkerboard when transparent), canvas-bounds outline, and a stroke pipeline with a mesh cache keyed by `StrokeId`. A view-projection uniform comes from `Viewport`. Shaders live in `assets/shaders/*.wgsl`.
4. Desktop: an egui shell (egui 0.37, egui-wgpu, egui-winit) with the canvas texture in the central panel. Middle-drag or space-drag pans, wheel or pinch zooms about the cursor, and high-DPI and resize are handled.

**Acceptance:** the window opens, the canvas renders with visible bounds, resizing works, and zoom and pan show no drift.

## Phase 2: Vector drawing (`feat/vector-drawing`)
`StrokeBuilder` (collects input, drops near-duplicate points, light smoothing), `tessellate(&Stroke) -> Mesh` (round joins and caps, pressure-aware width), eraser hit-testing (`strokes_hit(point, radius)`), and tests.

**Acceptance (with Phase 1 and UI):** visible strokes survive redraws, zoom, and pan, and are stored in document coordinates.

## Phase 4/5: Timeline engine (`feat/timeline-engine`)
Insert and remove frames (shift keys), `next_key`/`prev_key` navigation, exposure end, a `PlaybackClock` with an injected time source (ADR-005), looping, play ranges, and onion-skin neighbour queries.

**Acceptance:** exposure and playback timing are unit-tested, including frame drops when rendering is slow.

## Phase 6: Document ops and persistence (`feat/document-persistence`)
Commands for AddStroke, RemoveStroke, MoveLayer, Add/RemoveKeyframe, SetLayerVisibility, and SetLayerLocked. Locked layers reject stroke commands. Save/load integration tests in `tests/`, plus validation messages.

**Acceptance:** save → load reproduces the document exactly, every command passes an apply → undo → equals-original test, and invalid files give errors instead of crashes.

## MVP definition
All 11 points of the brief work together in one app: launch, canvas, draw, multiple layers, visibility and order, keyframes, different drawings per frame, scrubbing, playback, save and reopen, undo and redo. See `docs/smoke-test.md`.
