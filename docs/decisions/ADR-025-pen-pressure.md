# ADR-025: Pen pressure from drawing tablets

**Status:** Proposed (follows ADR-022 and ADR-024)

## Context
Strokes have stored a pressure per point since the first drawing engine: tessellation, hit testing, erasing and stamp brushes (ADR-022) all scale width by it, and the Brush Editor (ADR-024) has a Pressure → opacity setting. But input always gave full pressure, so a tablet drew like a mouse.

winit 0.30 does not report a tablet pen the same way on every platform:

| Platform | What winit gives | Pressure |
|---|---|---|
| Windows | `WM_POINTER` pen input as `WindowEvent::Touch` with `Force::Normalized` | yes |
| macOS | a tablet pen as ordinary mouse events (`TouchpadPressure` is only for Force Touch trackpads) | no, but AppKit has it on each `NSEvent` |
| Linux (X11, Wayland) | mouse events; X11 valuators only as raw `DeviceEvent::Motion` axes | no |

egui-winit turns each winit touch into an `egui::Event::Touch` (with `force`) followed by emulated pointer events.

## Decision
- **One path into the canvas: egui touch events.** The canvas (`apps/desktop/src/shell.rs`, `pointer_steps`) gives every pointer event the force of the touch before it, until that touch ends. Pointer events outside a touch are a mouse: full pressure. Windows works through this path with no extra code.
- **macOS adds the touches itself.** `apps/desktop/src/pen.rs` installs a local `NSEvent` monitor for left-button and tablet events. It stores the event's `pressure` when the event is a tablet event (`NSEventSubtypeTabletPoint` or `NSEventTypeTabletPoint`), and NaN otherwise. AppKit runs the monitor right before it hands the same event to winit's view, and winit reports it to the app at once. So when the app gets the winit event, the stored pressure belongs to it. `PenInput` then pushes a touch (Start on press, Move while down, End on release) into egui's input before egui-winit handles the event. This is the only `unsafe` code it adds: two AppKit calls, each with a `SAFETY` comment.
- **The session takes pressure per point.** `ToolController::set_pressure(Option<f32>)` sets the pressure of the next brush points. `None` (mouse) and non-finite values mean `1.0`. A pen never goes below `MIN_PEN_PRESSURE` (0.05), so a very light touch still leaves a thin line. Tool Settings → **Pen pressure** (on by default) turns it off, and then a pen draws at full pressure.
- Only the Brush tool uses pressure. Shapes, the eraser and Shift straight lines stay at full pressure.
- No document or file format change. `StrokePoint.pressure` was already saved.

## Consequences
- Lighter pressure draws thinner vector strokes. Textured brushes get smaller stamps, and lighter ones when their Pressure → opacity is above 0.
- The brush smoothing window averages pressure along with position, so pressure changes are smoothed too.
- Linux tablets still draw at full pressure. Supporting them means reading X11 XInput2 valuators or the Wayland tablet protocol, which winit 0.31 adds. Revisit when the workspace moves to winit 0.31.
- Tilt, rotation, the eraser end of the pen and pressure → size curves are still to come.
