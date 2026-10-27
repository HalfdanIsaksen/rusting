# ADR-002: Rendering and UI integration

**Status:** Accepted (Phase 0)

## Decision
- **wgpu 30** is the only rendering backend. Shaders are WGSL in `assets/shaders/`.
- **winit 0.30** (the `ApplicationHandler` API) handles windowing and input. Only `apps/desktop` depends on it.
- **egui 0.37** (`egui-winit`, `egui-wgpu`) is a temporary UI shell for panels. It is used only in `ui` and `apps/desktop`. The long-term plan is custom widgets on our renderer.
- The canvas is **not** an egui widget. The renderer draws it into an offscreen texture (4× MSAA, resolved), and egui shows that texture in the central panel. This keeps canvas rendering independent of egui, so swapping egui out later only touches `ui` and `desktop`.

## Version rule
`egui`, `egui-wgpu`, and `egui-winit` must share one release. `wgpu` and `winit` must be the versions those crates require. As of Phase 0: egui 0.37 → wgpu 30.0.x, winit 0.30.13. Check with `cargo tree -i wgpu` when upgrading.

## Consequences
- The workspace MSRV is 1.95 because egui 0.37 requires it.
- One extra texture copy per frame for the canvas. That's negligible at editor resolutions.
