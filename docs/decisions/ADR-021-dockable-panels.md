# ADR-021: Dockable panels

**Status:** Proposed (feat/dockable-panels branch)

## Context
The editor had a fixed layout of egui panels: the tool strip on the left, tool settings and library on the right, and the timeline across the bottom, with the canvas in the central panel. View could hide a panel but not move it. Users want to rearrange the panels, for example to put the Library under Tool Settings in one column, and to move every panel, the timeline included.

## Options
| | `egui_tiles` tree (chosen) | `egui_dock` | Our own docking on egui panels |
|---|---|---|---|
| Drag a tab beside, above, below or onto another panel | Yes | Yes | Lots of new code |
| Same egui release as the workspace (0.37) | Yes: 0.18.0 needs egui ^0.37 | Release lags egui | Yes |
| Extra crates in the build | Only `egui_tiles` (ahash, itertools and log are already in) | More | None |
| Panels can be hidden in place and come back where they were | Yes: per-tile visibility | Close and re-add | Depends |

## Decision
- **Dependency.** `egui_tiles` 0.18.0, a workspace dependency used only by `ui`, without default features. Its `serde` feature would turn on `egui/serde`, which adds a crate (`enumn`, via `accesskit/serde`), and would tie the saved file to egui_tiles' internal shape.
- **Panes.** `ui::panels::dock::Pane` is `Canvas`, `Toolbar`, `ToolSettings`, `Library` or `Timeline`. They live in an `egui_tiles::Tree<Pane>` held by `Session.panels: PanelLayout`. The menu bar and the status bar stay fixed egui panels. Preview and export stay floating windows.
- **Layout is UI state.** It is not part of the document or the project file and has no undo command. `ViewSettings` stays the source of truth for which panels are shown. Each frame, `PanelLayout::sync` makes the tree's per-tile visibility match it.
- **Canvas.** The canvas pane calls the host's closure with the pane's `Ui`, exactly as the central panel used to. The desktop shell already derived everything from that `Ui`'s rect: the viewport size (and so the render target size), screen-to-document conversion, overlays and drop targets. It needs no changes. The canvas cannot be hidden. While it is alone in its place it has no tab bar, so it fills the place and cannot be dragged away, but other panes can be dropped around it or onto it as tabs.
- **Tabs.** Every other pane always has a tab (`all_panes_must_have_tabs`). The tab is the handle to drag the pane by, and it carries the pane's title. Tabs and resize lines use the theme palette. Tabs have no close buttons; View hides panels. Ctrl+Tab tab cycling is off, so the key stays free for the editor.
- **Sizes.**
  - The default layout is the old fixed layout with the old sizes: the tool strip is `toolbar::WIDTH` wide, Library 200 points, Tool Settings 220 points and the timeline 210 points. It is sized to the real area on its first frame.
  - egui_tiles splits space by relative shares, so a window resize would stretch every panel. Instead, when the area or the set of visible panes changes, `keep_sizes` keeps each pane's size from the last frame along the splits that hold the canvas, and gives the difference to the canvas. Hiding a panel gives its room to the canvas, and showing it takes back the same size. Below a 120-point canvas the panes shrink together.
  - Dragging a resize line changes sizes as egui_tiles does.
- **View menu.** The toggles and `Cmd/Ctrl+1…4` hide and show panes in place. View → Reset Panel Layout restores the default layout and shows every panel.
- **Persistence.**
  - `PanelLayout::to_json` and `from_json` use a small format of our own: `{ version: 1, root }`, with nodes `pane { pane, visible }`, `row` and `column` with `{ share, node }` children, and `tabs { children, active }`.
  - A file is rejected (and the current layout kept) if:
    - it is not valid JSON in that shape, or has another version
    - it has no canvas, or repeats a pane
    - it has an empty container, or nests more than 32 levels
  - Bad shares become 1. A hidden canvas is shown. Panes a file lacks are added back on their default side of the window.
  - The desktop app (`layout_file`) reads the file at start and writes it on exit, replacing it with one rename. It is stored in the per-user configuration folder found from environment variables: `~/Library/Application Support/Rusting` on macOS, `%APPDATA%\Rusting` on Windows, and `$XDG_CONFIG_HOME/rusting` or `~/.config/rusting` elsewhere. `RUSTING_CONFIG_DIR` overrides the folder. Read or write errors are logged and never stop the app.

## Consequences
- A panel can be tabbed behind another. `PanelLayout::is_on_screen` tells whether it was drawn. The status bar uses it to show the locked-layer warning when Tool Settings isn't visible.
- Panels are drawn into egui_tiles' own `Ui`s instead of `egui::Panel`s. Each pane paints its own background and margin, and is clipped to its rect. Tool Settings scrolls when its pane is short. The Library lost its heading, which is now the tab title.
- Pane rects can sit on half points. The canvas already snaps its rect to whole physical pixels, so this changes nothing there.
- The layout is saved only on a normal exit. After a crash, the previous launch's layout comes back.
- egui_tiles has no per-pane minimum or fixed size. A panel can be dragged narrower than its content (the tool strip clips) or wider than it needs.
- A grid container cannot be made by dragging. If one ever appears, it is saved as tabs.
