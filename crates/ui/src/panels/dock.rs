//! Dockable panels (ADR-021): the canvas, tool strip, tool settings, library
//! and timeline are panes in an [`egui_tiles`] tree. Drag a pane's tab onto
//! another pane to put it beside, above, below or behind it as a tab, and drag
//! the gaps between panes to resize them.
//!
//! The layout is user interface state: it is not part of the document, has no
//! undo, and the desktop app saves it per user ([`PanelLayout::to_json`]).

use super::{library, timeline, tool_options, toolbar};
use crate::session::{Session, ViewSettings};
use crate::theme;
use egui_tiles::{Container, Linear, LinearDir, Tile, TileId, Tiles, Tree};
use serde::{Deserialize, Serialize};

/// One dockable panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Pane {
    /// The canvas. It can be moved but never hidden.
    Canvas,
    /// The tool strip.
    Toolbar,
    /// Tool settings.
    ToolSettings,
    /// The library (ADR-017).
    Library,
    /// The timeline with its layer column.
    Timeline,
}

impl Pane {
    /// Every pane, in the order of [`PanelLayout::hidden`].
    pub const ALL: [Self; 5] = [
        Self::Canvas,
        Self::Toolbar,
        Self::ToolSettings,
        Self::Library,
        Self::Timeline,
    ];

    /// The tab title.
    pub fn title(self) -> &'static str {
        match self {
            Self::Canvas => "Canvas",
            Self::Toolbar => "Tools",
            Self::ToolSettings => "Tool Settings",
            Self::Library => "Library",
            Self::Timeline => "Timeline",
        }
    }

    /// Whether View shows the pane. The canvas is always shown.
    pub fn shown(self, view: &ViewSettings) -> bool {
        match self {
            Self::Canvas => true,
            Self::Toolbar => view.toolbar,
            Self::ToolSettings => view.tool_settings,
            Self::Library => view.library,
            Self::Timeline => view.timeline,
        }
    }

    /// Sets whether View shows the pane. Ignored for the canvas.
    fn set_shown(self, view: &mut ViewSettings, shown: bool) {
        match self {
            Self::Canvas => {}
            Self::Toolbar => view.toolbar = shown,
            Self::ToolSettings => view.tool_settings = shown,
            Self::Library => view.library = shown,
            Self::Timeline => view.timeline = shown,
        }
    }

    /// Where a pane goes back when it is missing from a layout: the side it
    /// has in the default layout, and its default size along that side.
    fn home(self) -> (LinearDir, bool, f32) {
        match self {
            Self::Toolbar => (LinearDir::Horizontal, true, toolbar::WIDTH),
            Self::Timeline => (LinearDir::Vertical, false, TIMELINE_HEIGHT),
            Self::Library => (LinearDir::Horizontal, false, LIBRARY_WIDTH),
            Self::ToolSettings | Self::Canvas => {
                (LinearDir::Horizontal, false, TOOL_SETTINGS_WIDTH)
            }
        }
    }
}

/// egui id of the tree; also salts the ids of everything inside the panes.
const TREE_ID: &str = "rusting_panels";
/// Default tool settings width, in points.
const TOOL_SETTINGS_WIDTH: f32 = 220.0;
/// Default library width, in points.
const LIBRARY_WIDTH: f32 = 200.0;
/// Default timeline height, in points.
const TIMELINE_HEIGHT: f32 = 210.0;
/// Area the default layout is sized for until the real one is known: a
/// 1280×800 window without menu and status bar.
const NOMINAL: egui::Vec2 = egui::vec2(1280.0, 754.0);
/// When the window or a panel changes size, the canvas takes the difference
/// while it stays at least this big; below that, panes shrink together.
const MIN_CANVAS: f32 = 120.0;
/// Gap between panes, in points; also the resize handle's line.
const GAP: f32 = 1.0;
/// Height of a tab bar, in points.
const TAB_BAR_HEIGHT: f32 = 24.0;
/// Version of the saved layout format.
const FORMAT_VERSION: u32 = 1;

/// The panel layout: which pane is where, and how big.
#[derive(Debug)]
pub struct PanelLayout {
    tree: Tree<Pane>,
    /// The default layout still needs sizing to the real area.
    fit_default: bool,
    /// Area size and hidden panes when the tree was last shown.
    last: Option<(egui::Vec2, [bool; 5])>,
}

impl Default for PanelLayout {
    /// Today's fixed layout: tools on the left, the canvas, then library and
    /// tool settings on the right, and the timeline across the bottom.
    fn default() -> Self {
        Self {
            tree: default_tree(NOMINAL),
            fit_default: true,
            last: None,
        }
    }
}

impl PanelLayout {
    /// The tile showing `pane`, if the layout has it.
    fn tile(&self, pane: Pane) -> Option<TileId> {
        self.tree.tiles.find_pane(&pane)
    }

    /// Whether `pane` is in the layout and not hidden (it may still be a tab
    /// behind another one).
    pub fn is_visible(&self, pane: Pane) -> bool {
        self.tile(pane)
            .is_some_and(|tile| self.tree.is_visible_in_layout(tile))
    }

    /// Whether `pane` was drawn last frame: visible, and the open tab if it
    /// shares a place with others.
    pub fn is_on_screen(&self, pane: Pane) -> bool {
        self.tile(pane)
            .is_some_and(|tile| self.tree.tiles.rect(tile).is_some())
    }

    /// Hidden flags per pane, in [`Pane::ALL`] order.
    fn hidden(&self) -> [bool; 5] {
        Pane::ALL.map(|pane| !self.is_visible(pane))
    }

    /// Makes the layout show and hide panes as `view` says, putting back any
    /// pane the layout lost near its default place.
    pub fn sync(&mut self, view: &ViewSettings) {
        for pane in Pane::ALL {
            if self.tile(pane).is_none() {
                add_pane(&mut self.tree, pane);
            }
            if let Some(tile) = self.tile(pane) {
                let shown = pane.shown(view);
                if self.tree.tiles.is_visible(tile) != shown {
                    self.tree.set_visible(tile, shown);
                }
            }
        }
    }

    /// Sets `view` to the panes this layout shows.
    pub fn apply_to_view(&self, view: &mut ViewSettings) {
        for pane in Pane::ALL {
            pane.set_shown(
                view,
                self.tile(pane)
                    .is_none_or(|t| self.tree.tiles.is_visible(t)),
            );
        }
    }

    /// The layout as JSON, for saving between launches.
    pub fn to_json(&self) -> String {
        let saved = SavedLayout {
            version: FORMAT_VERSION,
            root: self
                .tree
                .root
                .and_then(|root| save_node(&self.tree.tiles, root)),
        };
        serde_json::to_string_pretty(&saved).unwrap_or_default()
    }

    /// A layout saved by [`Self::to_json`], or `None` if `json` is not a
    /// valid one (another version, a pane twice, no canvas, ...). Panes it
    /// lacks are added back near their default place.
    pub fn from_json(json: &str) -> Option<Self> {
        let saved: SavedLayout = serde_json::from_str(json).ok()?;
        if saved.version != FORMAT_VERSION {
            return None;
        }
        let mut tiles = Tiles::default();
        let mut seen = Vec::new();
        let root = load_node(&mut tiles, saved.root?, &mut seen, 0)?;
        if !seen.contains(&Pane::Canvas) {
            return None;
        }
        let mut tree = Tree::new(egui::Id::unique(TREE_ID), root, tiles);
        for pane in Pane::ALL {
            if !seen.contains(&pane) {
                add_pane(&mut tree, pane);
            }
        }
        Some(Self {
            tree,
            fit_default: false,
            last: None,
        })
    }

    /// Shows the panes in `ui`'s remaining space. `canvas` draws the canvas pane.
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut Session,
        canvas: impl FnOnce(&mut egui::Ui, &mut Session),
    ) {
        self.sync(&session.view);
        let rect = ui.available_rect_before_wrap();
        let hidden = self.hidden();
        if self.fit_default {
            self.tree = default_tree(rect.size());
            self.sync(&session.view);
            self.fit_default = false;
        } else if self.last.is_some_and(|last| last != (rect.size(), hidden))
            && let (Some(root), Some(canvas)) = (self.tree.root, self.tile(Pane::Canvas))
        {
            keep_sizes(&mut self.tree.tiles, root, rect, canvas);
        }
        self.last = Some((rect.size(), hidden));

        let mut behavior = Panels {
            session,
            canvas: Some(canvas),
            canvas_tile: self.tile(Pane::Canvas),
        };
        self.tree.ui(&mut behavior, ui);
    }
}

/// The default layout, sized for an area of `size` points.
fn default_tree(size: egui::Vec2) -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let mut tab = |pane| {
        let id = tiles.insert_pane(pane);
        tiles.insert_tab_tile(vec![id])
    };
    let toolbar = tab(Pane::Toolbar);
    let canvas = tab(Pane::Canvas);
    let library = tab(Pane::Library);
    let settings = tab(Pane::ToolSettings);
    let timeline = tab(Pane::Timeline);

    let side = toolbar::WIDTH + LIBRARY_WIDTH + TOOL_SETTINGS_WIDTH + 3.0 * GAP;
    let canvas_width = (size.x - side).max(MIN_CANVAS);
    let mut top = Linear::new(
        LinearDir::Horizontal,
        vec![toolbar, canvas, library, settings],
    );
    set_shares(
        &mut top,
        &[
            (toolbar, toolbar::WIDTH),
            (canvas, canvas_width),
            (library, LIBRARY_WIDTH),
            (settings, TOOL_SETTINGS_WIDTH),
        ],
    );
    let top = tiles.insert_container(top);

    let top_height = (size.y - TIMELINE_HEIGHT - GAP).max(MIN_CANVAS);
    let mut root = Linear::new(LinearDir::Vertical, vec![top, timeline]);
    set_shares(&mut root, &[(top, top_height), (timeline, TIMELINE_HEIGHT)]);
    let root = tiles.insert_container(root);
    Tree::new(egui::Id::unique(TREE_ID), root, tiles)
}

/// Sets shares so the children get these sizes, keeping egui_tiles'
/// convention that the shares add up to the number of children.
fn set_shares(linear: &mut Linear, sizes: &[(TileId, f32)]) {
    let total: f32 = sizes.iter().map(|(_, size)| size).sum();
    if total <= 0.0 {
        return;
    }
    let scale = sizes.len() as f32 / total;
    for &(child, size) in sizes {
        linear.shares.set_share(child, size * scale);
    }
}

/// Puts `pane` back on its default side of the whole layout.
fn add_pane(tree: &mut Tree<Pane>, pane: Pane) {
    let id = tree.tiles.insert_pane(pane);
    let tab = tree.tiles.insert_tab_tile(vec![id]);
    let Some(root) = tree.root else {
        tree.root = Some(tab);
        return;
    };
    let (dir, first, size) = pane.home();
    let total = match dir {
        LinearDir::Horizontal => NOMINAL.x,
        LinearDir::Vertical => NOMINAL.y,
    };
    let fraction = (size / total).clamp(0.05, 0.5);
    let linear = if first {
        Linear::new_binary(dir, [tab, root], fraction)
    } else {
        Linear::new_binary(dir, [root, tab], 1.0 - fraction)
    };
    tree.root = Some(tree.tiles.insert_container(linear));
}

/// Whether `target` is `id` or inside it.
fn contains(tiles: &Tiles<Pane>, id: TileId, target: TileId) -> bool {
    id == target
        || matches!(tiles.get(id), Some(Tile::Container(c)) if c.children().any(|&child| contains(tiles, child, target)))
}

/// When the area or the set of visible panes changes, keeps every pane's
/// size from last frame along the splits that hold the canvas, and gives the
/// difference to the canvas. A window resize then grows the canvas instead
/// of stretching the side panels, and hiding a panel gives its room to the
/// canvas. Leaves the shares alone where the canvas would get too small.
fn keep_sizes(tiles: &mut Tiles<Pane>, id: TileId, rect: egui::Rect, canvas: TileId) {
    let Some(Tile::Container(container)) = tiles.get(id) else {
        return;
    };
    match container {
        Container::Linear(linear) => {
            let visible: Vec<TileId> = linear
                .children
                .iter()
                .copied()
                .filter(|&child| tiles.is_visible_in_layout(child))
                .collect();
            let Some(flex) = visible
                .iter()
                .copied()
                .find(|&child| contains(tiles, child, canvas))
            else {
                return;
            };
            let along = |r: egui::Rect| match linear.dir {
                LinearDir::Horizontal => r.width(),
                LinearDir::Vertical => r.height(),
            };
            let available = along(rect) - GAP * visible.len().saturating_sub(1) as f32;
            // Points per share last frame, from the children that were drawn.
            let (drawn_shares, drawn_size) = visible
                .iter()
                .filter_map(|&child| Some((linear.shares[child], along(tiles.rect(child)?))))
                .fold((0.0, 0.0), |(s, p), (share, size)| (s + share, p + size));
            if drawn_shares <= 0.0 || drawn_size <= 0.0 {
                return;
            }
            let points_per_share = drawn_size / drawn_shares;
            // Last frame's size of every child. Hidden children, and those
            // shown again this frame, keep the size their share stood for.
            let mut sizes: Vec<(TileId, f32)> = linear
                .children
                .iter()
                .map(|&child| {
                    let size = tiles
                        .rect(child)
                        .filter(|_| visible.contains(&child))
                        .map_or(linear.shares[child] * points_per_share, along);
                    (child, size)
                })
                .collect();
            let others: f32 = sizes
                .iter()
                .filter(|(child, _)| *child != flex && visible.contains(child))
                .map(|(_, size)| size)
                .sum();
            let flex_size = available - others;
            if flex_size < MIN_CANVAS {
                return;
            }
            let mut offset = 0.0;
            for (child, size) in &mut sizes {
                if *child == flex {
                    *size = flex_size;
                    break;
                }
                if visible.contains(child) {
                    offset += *size + GAP;
                }
            }
            let flex_rect = match linear.dir {
                LinearDir::Horizontal => egui::Rect::from_min_size(
                    rect.min + egui::vec2(offset, 0.0),
                    egui::vec2(flex_size, rect.height()),
                ),
                LinearDir::Vertical => egui::Rect::from_min_size(
                    rect.min + egui::vec2(0.0, offset),
                    egui::vec2(rect.width(), flex_size),
                ),
            };
            // Shares in the same scale for all children, adding up to the
            // number of visible ones.
            let scale = visible.len() as f32 / available.max(1.0);
            if let Some(Tile::Container(Container::Linear(linear))) = tiles.get_mut(id) {
                for (child, size) in sizes {
                    linear.shares.set_share(child, size * scale);
                }
            }
            keep_sizes(tiles, flex, flex_rect, canvas);
        }
        Container::Tabs(tabs) => {
            let Some(child) = tabs
                .children
                .iter()
                .copied()
                .find(|&child| contains(tiles, child, canvas))
            else {
                return;
            };
            let bar = if tab_bar_visible(tiles, &tabs.children) {
                TAB_BAR_HEIGHT
            } else {
                0.0
            };
            let child_rect = egui::Rect::from_min_max(rect.min + egui::vec2(0.0, bar), rect.max);
            keep_sizes(tiles, child, child_rect, canvas);
        }
        Container::Grid(_) => {}
    }
}

/// A tab bar is shown unless the canvas is alone in its place: it then
/// fills the place without a title, and can't be dragged away.
fn tab_bar_visible(tiles: &Tiles<Pane>, children: &[TileId]) -> bool {
    !matches!(children, [only] if tiles.get_pane(only) == Some(&Pane::Canvas))
}

/// What egui_tiles calls back while showing the tree.
struct Panels<'a, F> {
    session: &'a mut Session,
    /// The host's canvas, until the canvas pane has been drawn.
    canvas: Option<F>,
    canvas_tile: Option<TileId>,
}

impl<F: FnOnce(&mut egui::Ui, &mut Session)> egui_tiles::Behavior<Pane> for Panels<'_, F> {
    fn pane_ui(
        &mut self,
        ui: &mut egui::Ui,
        _tile: TileId,
        pane: &mut Pane,
    ) -> egui_tiles::UiResponse {
        let rect = ui.max_rect();
        ui.set_clip_rect(rect.intersect(ui.clip_rect()));
        let session = &mut *self.session;
        let margin = match pane {
            Pane::Canvas => {
                if let Some(canvas) = self.canvas.take() {
                    canvas(ui, session);
                }
                return egui_tiles::UiResponse::None;
            }
            Pane::Toolbar => egui::Margin::symmetric(4, 0),
            Pane::Timeline => egui::Margin::symmetric(8, 2),
            Pane::ToolSettings | Pane::Library => egui::Margin::same(8),
        };
        ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
        let inner = rect - margin;
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| match pane {
            Pane::Toolbar => toolbar::panel(ui, session),
            Pane::Timeline => timeline::panel(ui, session),
            Pane::Library => library::panel(ui, session),
            Pane::ToolSettings => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| tool_options::panel(ui, session));
            }
            Pane::Canvas => {}
        });
        egui_tiles::UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        pane.title().into()
    }

    fn is_tab_bar_visible(&self, _tile: TileId, tabs: &egui_tiles::Tabs) -> bool {
        !matches!(tabs.children.as_slice(), [only] if Some(*only) == self.canvas_tile)
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        TAB_BAR_HEIGHT
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        GAP
    }

    fn min_size(&self) -> f32 {
        40.0
    }

    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions {
            // Every pane keeps a tab: it is the handle to drag the pane by.
            all_panes_must_have_tabs: true,
            ..Default::default()
        }
    }

    fn cycle_tabs_with_keyboard(&self) -> bool {
        // Ctrl+Tab is left to the editor.
        false
    }

    fn tab_bar_color(&self, visuals: &egui::Visuals) -> egui::Color32 {
        theme::palette(visuals.dark_mode).surface
    }

    fn tab_bg_color(
        &self,
        visuals: &egui::Visuals,
        _tiles: &Tiles<Pane>,
        _tile: TileId,
        state: &egui_tiles::TabState,
    ) -> egui::Color32 {
        if state.active {
            visuals.panel_fill
        } else {
            egui::Color32::TRANSPARENT
        }
    }

    fn tab_outline_stroke(
        &self,
        visuals: &egui::Visuals,
        _tiles: &Tiles<Pane>,
        _tile: TileId,
        state: &egui_tiles::TabState,
    ) -> egui::Stroke {
        if state.active {
            egui::Stroke::new(1.0, theme::palette(visuals.dark_mode).border)
        } else {
            egui::Stroke::NONE
        }
    }

    fn tab_bar_hline_stroke(&self, visuals: &egui::Visuals) -> egui::Stroke {
        egui::Stroke::new(1.0, theme::palette(visuals.dark_mode).border)
    }

    fn tab_text_color(
        &self,
        visuals: &egui::Visuals,
        _tiles: &Tiles<Pane>,
        _tile: TileId,
        state: &egui_tiles::TabState,
    ) -> egui::Color32 {
        let p = theme::palette(visuals.dark_mode);
        if state.active { p.text } else { p.text_muted }
    }

    fn resize_stroke(
        &self,
        style: &egui::Style,
        resize_state: egui_tiles::ResizeState,
    ) -> egui::Stroke {
        let p = theme::palette(style.visuals.dark_mode);
        match resize_state {
            egui_tiles::ResizeState::Idle => egui::Stroke::new(GAP, p.border),
            egui_tiles::ResizeState::Hovering => egui::Stroke::new(GAP, p.text_muted),
            egui_tiles::ResizeState::Dragging => egui::Stroke::new(GAP, p.accent),
        }
    }

    fn drag_preview_stroke(&self, visuals: &egui::Visuals) -> egui::Stroke {
        egui::Stroke::new(1.5, theme::palette(visuals.dark_mode).accent)
    }

    fn drag_preview_color(&self, visuals: &egui::Visuals) -> egui::Color32 {
        theme::palette(visuals.dark_mode)
            .accent
            .gamma_multiply(0.25)
    }
}

/// The saved layout file (JSON). It has its own shape rather than
/// egui_tiles' internal one, so it survives egui_tiles upgrades.
#[derive(Serialize, Deserialize)]
struct SavedLayout {
    version: u32,
    root: Option<Node>,
}

/// One tile of a saved layout.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Node {
    /// A pane; hidden through View when `visible` is false.
    Pane { pane: Pane, visible: bool },
    /// Side by side, left to right.
    Row { children: Vec<Sized> },
    /// Stacked, top to bottom.
    Column { children: Vec<Sized> },
    /// Tabs; `active` is the index of the open one.
    Tabs { children: Vec<Node>, active: usize },
}

/// A child of a row or column with its share of the space.
#[derive(Serialize, Deserialize)]
struct Sized {
    share: f32,
    node: Node,
}

/// Saves the tile `id` and everything inside it.
fn save_node(tiles: &Tiles<Pane>, id: TileId) -> Option<Node> {
    Some(match tiles.get(id)? {
        Tile::Pane(pane) => Node::Pane {
            pane: *pane,
            visible: tiles.is_visible(id),
        },
        Tile::Container(Container::Linear(linear)) => {
            let children = linear
                .children
                .iter()
                .filter_map(|&child| {
                    Some(Sized {
                        share: linear.shares[child],
                        node: save_node(tiles, child)?,
                    })
                })
                .collect();
            match linear.dir {
                LinearDir::Horizontal => Node::Row { children },
                LinearDir::Vertical => Node::Column { children },
            }
        }
        Tile::Container(container) => {
            // A grid (not made by dragging) is saved as tabs.
            let ids = container.children_vec();
            let active = match container {
                Container::Tabs(tabs) => tabs
                    .active
                    .and_then(|active| ids.iter().position(|&child| child == active))
                    .unwrap_or(0),
                Container::Linear(_) | Container::Grid(_) => 0,
            };
            Node::Tabs {
                children: ids
                    .iter()
                    .filter_map(|&child| save_node(tiles, child))
                    .collect(),
                active,
            }
        }
    })
}

/// Deepest nesting a saved layout may have.
const MAX_DEPTH: usize = 32;

/// Adds a saved tile to `tiles`. `None` if it repeats a pane in `seen`,
/// holds an empty container or nests too deep.
fn load_node(
    tiles: &mut Tiles<Pane>,
    node: Node,
    seen: &mut Vec<Pane>,
    depth: usize,
) -> Option<TileId> {
    if depth > MAX_DEPTH {
        return None;
    }
    match node {
        Node::Pane { pane, visible } => {
            if seen.contains(&pane) {
                return None;
            }
            seen.push(pane);
            let id = tiles.insert_pane(pane);
            // The canvas is never hidden.
            tiles.set_visible(id, visible || pane == Pane::Canvas);
            Some(id)
        }
        Node::Row { children } => load_linear(tiles, LinearDir::Horizontal, children, seen, depth),
        Node::Column { children } => load_linear(tiles, LinearDir::Vertical, children, seen, depth),
        Node::Tabs { children, active } => {
            let ids = children
                .into_iter()
                .map(|child| load_node(tiles, child, seen, depth + 1))
                .collect::<Option<Vec<_>>>()?;
            let open = *ids.get(active).or(ids.first())?;
            let mut tabs = egui_tiles::Tabs::new(ids);
            tabs.set_active(open);
            Some(tiles.insert_container(tabs))
        }
    }
}

/// Adds a saved row or column.
fn load_linear(
    tiles: &mut Tiles<Pane>,
    dir: LinearDir,
    children: Vec<Sized>,
    seen: &mut Vec<Pane>,
    depth: usize,
) -> Option<TileId> {
    if children.is_empty() {
        return None;
    }
    let mut sizes = Vec::with_capacity(children.len());
    for child in children {
        let id = load_node(tiles, child.node, seen, depth + 1)?;
        let share = if child.share.is_finite() && child.share > 0.0 {
            child.share
        } else {
            1.0
        };
        sizes.push((id, share));
    }
    let mut linear = Linear::new(dir, sizes.iter().map(|(id, _)| *id).collect());
    set_shares(&mut linear, &sizes);
    Some(tiles.insert_container(Container::Linear(linear)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draws the whole editor at `size` and returns the canvas pane's rect.
    fn frame(ctx: &egui::Context, session: &mut Session, size: egui::Vec2) -> Option<egui::Rect> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let mut canvas = None;
        ctx.run_ui(input, |ui| {
            crate::show(ui, session, |ui, _| canvas = Some(ui.max_rect()));
        })
        .drop_without_applying_deltas();
        canvas
    }

    /// Two frames: the first lays out, the second has last frame's sizes.
    fn settle(ctx: &egui::Context, session: &mut Session, size: egui::Vec2) -> egui::Rect {
        frame(ctx, session, size);
        frame(ctx, session, size).expect("the canvas is drawn")
    }

    fn rect(session: &Session, pane: Pane) -> egui::Rect {
        let tile = session.panels.tile(pane).expect("pane in layout");
        session.panels.tree.tiles.rect(tile).expect("pane drawn")
    }

    const WINDOW: egui::Vec2 = egui::vec2(1280.0, 800.0);

    #[test]
    fn default_layout_has_every_pane_once_and_shows_it() {
        let layout = PanelLayout::default();
        for pane in Pane::ALL {
            let count = layout
                .tree
                .tiles
                .tiles()
                .filter(|tile| matches!(tile, Tile::Pane(p) if *p == pane))
                .count();
            assert_eq!(count, 1, "{pane:?}");
            assert!(layout.is_visible(pane), "{pane:?}");
        }
    }

    #[test]
    fn default_layout_matches_the_old_fixed_panels() {
        let ctx = egui::Context::default();
        let mut s = Session::new();
        let canvas = settle(&ctx, &mut s, WINDOW);
        let tools = rect(&s, Pane::Toolbar);
        let library = rect(&s, Pane::Library);
        let settings = rect(&s, Pane::ToolSettings);
        let timeline = rect(&s, Pane::Timeline);
        // Left to right: tools, canvas, library, tool settings; timeline below.
        assert!(tools.right() <= canvas.left());
        assert!(canvas.right() <= library.left());
        assert!(library.right() <= settings.left());
        assert!(canvas.bottom() <= timeline.top());
        assert!((tools.width() - toolbar::WIDTH).abs() < 1.0, "{tools:?}");
        assert!((settings.width() - TOOL_SETTINGS_WIDTH).abs() < 1.0);
        assert!((library.width() - LIBRARY_WIDTH).abs() < 1.0);
        // The pane's rect includes its tab bar.
        assert!((timeline.height() - TIMELINE_HEIGHT + TAB_BAR_HEIGHT).abs() < 1.0);
        assert!(canvas.width() > 600.0 && canvas.height() > 400.0);
    }

    #[test]
    fn resizing_the_window_resizes_only_the_canvas() {
        let ctx = egui::Context::default();
        let mut s = Session::new();
        let small = settle(&ctx, &mut s, WINDOW);
        let tools = rect(&s, Pane::Toolbar).width();
        let timeline = rect(&s, Pane::Timeline).height();
        let big = settle(&ctx, &mut s, egui::vec2(1800.0, 1100.0));
        assert!((rect(&s, Pane::Toolbar).width() - tools).abs() < 1.0);
        assert!((rect(&s, Pane::Timeline).height() - timeline).abs() < 1.0);
        assert!((big.width() - small.width() - 520.0).abs() < 1.0);
        assert!((big.height() - small.height() - 300.0).abs() < 1.0);
    }

    #[test]
    fn hiding_and_showing_a_panel_round_trips() {
        let ctx = egui::Context::default();
        let mut s = Session::new();
        let before = settle(&ctx, &mut s, WINDOW);
        let library = rect(&s, Pane::Library);

        s.view.library = false;
        let hidden = settle(&ctx, &mut s, WINDOW);
        assert!(!s.panels.is_visible(Pane::Library));
        assert!(!s.panels.is_on_screen(Pane::Library));
        // The canvas takes the library's room; the tool strip keeps its width.
        assert!((hidden.width() - before.width() - library.width() - GAP).abs() < 1.0);
        assert!((rect(&s, Pane::Toolbar).width() - toolbar::WIDTH).abs() < 1.0);

        s.view.library = true;
        let shown = settle(&ctx, &mut s, WINDOW);
        assert!(s.panels.is_on_screen(Pane::Library));
        assert!(
            (shown.width() - before.width()).abs() < 2.0,
            "{shown:?} vs {before:?}"
        );
        assert!((rect(&s, Pane::Library).width() - library.width()).abs() < 2.0);
    }

    #[test]
    fn every_panel_can_be_hidden_but_the_canvas() {
        let ctx = egui::Context::default();
        let mut s = Session::new();
        s.view = ViewSettings {
            toolbar: false,
            timeline: false,
            tool_settings: false,
            library: false,
        };
        let canvas = settle(&ctx, &mut s, WINDOW);
        for pane in Pane::ALL {
            assert_eq!(
                s.panels.is_on_screen(pane),
                pane == Pane::Canvas,
                "{pane:?}"
            );
        }
        assert!(canvas.width() > 1200.0);
    }

    /// Moves the library pane into a column under tool settings, as a drag
    /// onto the bottom half of the tool settings pane does.
    fn library_under_tool_settings(layout: &mut PanelLayout) {
        let tiles = &mut layout.tree.tiles;
        let library = tiles.find_pane(&Pane::Library).expect("library");
        let library_tabs = tiles.parent_of(library).expect("library tabs");
        let settings = tiles.find_pane(&Pane::ToolSettings).expect("settings");
        let settings_tabs = tiles.parent_of(settings).expect("settings tabs");
        let row = tiles.parent_of(settings_tabs).expect("row");
        if let Some(Tile::Container(row)) = tiles.get_mut(row) {
            row.remove_child(library_tabs);
        }
        let column = tiles.insert_vertical_tile(vec![settings_tabs, library_tabs]);
        if let Some(Tile::Container(row)) = tiles.get_mut(row) {
            assert!(row.replace_child(settings_tabs, column).is_some());
        }
    }

    #[test]
    fn library_can_sit_under_tool_settings_and_is_saved_there() {
        let ctx = egui::Context::default();
        let mut s = Session::new();
        settle(&ctx, &mut s, WINDOW);
        library_under_tool_settings(&mut s.panels);
        settle(&ctx, &mut s, WINDOW);
        let (settings, library) = (rect(&s, Pane::ToolSettings), rect(&s, Pane::Library));
        assert!((settings.left() - library.left()).abs() < 1.0);
        assert!(settings.bottom() <= library.top());

        s.view.timeline = false;
        let json = s.panel_layout_json();
        let mut restored = Session::new();
        assert!(restored.restore_panel_layout(&json));
        assert!(!restored.view.timeline && restored.view.library);
        settle(&ctx, &mut restored, WINDOW);
        let (settings2, library2) = (
            rect(&restored, Pane::ToolSettings),
            rect(&restored, Pane::Library),
        );
        assert!((settings2.left() - library2.left()).abs() < 1.0);
        assert!(settings2.bottom() <= library2.top());
        assert!(!restored.panels.is_on_screen(Pane::Timeline));
    }

    #[test]
    fn a_panel_behind_another_tab_is_not_on_screen() {
        let ctx = egui::Context::default();
        let mut s = Session::new();
        settle(&ctx, &mut s, WINDOW);
        // Library as a second tab in the tool settings place, and open.
        let tiles = &mut s.panels.tree.tiles;
        let library = tiles.find_pane(&Pane::Library).expect("library");
        let library_tabs = tiles.parent_of(library).expect("library tabs");
        let settings = tiles.find_pane(&Pane::ToolSettings).expect("settings");
        let settings_tabs = tiles.parent_of(settings).expect("settings tabs");
        let row = tiles.parent_of(library_tabs).expect("row");
        if let Some(Tile::Container(row)) = tiles.get_mut(row) {
            row.remove_child(library_tabs);
        }
        tiles.remove(library_tabs);
        if let Some(Tile::Container(Container::Tabs(tabs))) = tiles.get_mut(settings_tabs) {
            tabs.add_child(library);
            tabs.set_active(library);
        }
        settle(&ctx, &mut s, WINDOW);
        assert!(s.panels.is_visible(Pane::ToolSettings));
        assert!(!s.panels.is_on_screen(Pane::ToolSettings));
        assert!(s.panels.is_on_screen(Pane::Library));
    }

    #[test]
    fn reset_restores_the_default_layout_and_shows_every_panel() {
        let ctx = egui::Context::default();
        let mut s = Session::new();
        settle(&ctx, &mut s, WINDOW);
        library_under_tool_settings(&mut s.panels);
        s.view.toolbar = false;
        settle(&ctx, &mut s, WINDOW);

        s.reset_panel_layout();
        assert_eq!(s.view, ViewSettings::default());
        let canvas = settle(&ctx, &mut s, WINDOW);
        let mut fresh = Session::new();
        let fresh_canvas = settle(&egui::Context::default(), &mut fresh, WINDOW);
        assert_eq!(canvas, fresh_canvas);
        assert_eq!(rect(&s, Pane::Library), rect(&fresh, Pane::Library));
    }

    #[test]
    fn bad_saved_layouts_are_rejected() {
        let mut s = Session::new();
        let good = s.panel_layout_json();
        for bad in [
            "",
            "not json",
            "{}",
            r#"{"version": 99, "root": null}"#,
            r#"{"version": 1, "root": null}"#,
            // No canvas.
            r#"{"version": 1, "root": {"kind": "pane", "pane": "Library", "visible": true}}"#,
            // The canvas twice.
            r#"{"version": 1, "root": {"kind": "tabs", "active": 0, "children": [
                {"kind": "pane", "pane": "Canvas", "visible": true},
                {"kind": "pane", "pane": "Canvas", "visible": true}]}}"#,
            // An empty row.
            r#"{"version": 1, "root": {"kind": "row", "children": []}}"#,
            // An unknown pane.
            r#"{"version": 1, "root": {"kind": "pane", "pane": "Audio", "visible": true}}"#,
        ] {
            assert!(!s.restore_panel_layout(bad), "{bad}");
            assert_eq!(s.panel_layout_json(), good, "kept the layout after {bad}");
        }
    }

    #[test]
    fn a_saved_layout_without_some_panes_gets_them_back() {
        let ctx = egui::Context::default();
        let mut s = Session::new();
        // Only the canvas, with a hidden flag that must be ignored, and a bad share.
        let json = r#"{"version": 1, "root": {"kind": "row", "children": [
            {"share": -3, "node": {"kind": "pane", "pane": "Canvas", "visible": false}}]}}"#;
        assert!(s.restore_panel_layout(json));
        assert_eq!(s.view, ViewSettings::default());
        settle(&ctx, &mut s, WINDOW);
        for pane in Pane::ALL {
            assert!(s.panels.is_on_screen(pane), "{pane:?}");
        }
        assert!(rect(&s, Pane::Toolbar).left() < rect(&s, Pane::Canvas).left());
        assert!(rect(&s, Pane::Timeline).top() > rect(&s, Pane::Canvas).top());
    }
}
