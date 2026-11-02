//! The tool strip: a narrow column of icon buttons, two per row, in groups.
//!
//! The strip only picks the tool. Per-tool settings live in the tool settings
//! panel ([`super::tool_options`]); tools with extra options show a corner
//! triangle and open a small popup on right-click or press-and-hold.

use super::tool_options;
use crate::icons::{self, IconFn};
use crate::session::Session;
use editor::Tool;

/// Side of a tool button, in points.
const BUTTON: f32 = 30.0;
/// Space between buttons, in points.
const GAP: f32 = 2.0;
/// Margin between the strip edge and the buttons, in points.
const MARGIN: f32 = 4.0;
/// How long a press must be held to open a tool's options popup, in seconds.
const HOLD_SECS: f64 = 0.4;

/// Width of the strip including its margins: two buttons per row.
pub const WIDTH: f32 = 2.0 * BUTTON + GAP + 2.0 * MARGIN;

/// Tool groups, top to bottom. A divider separates neighbouring groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolGroup {
    /// Selection tools.
    Selection,
    /// Drawing tools.
    Drawing,
    /// Tools that change existing artwork: eraser, paint bucket.
    Paint,
}

/// Popup content with a tool's extra options.
pub type OptionsFn = fn(&mut egui::Ui, &mut Session);

/// One button of the strip.
#[derive(Clone, Copy)]
pub struct ToolEntry {
    /// The tool the button selects.
    pub tool: Tool,
    /// Draws the button's icon.
    pub icon: IconFn,
    /// Name shown in the tooltip.
    pub name: &'static str,
    /// Keyboard shortcut shown in the tooltip.
    pub shortcut: &'static str,
    /// Group the button belongs to; entries of one group must be adjacent.
    pub group: ToolGroup,
    /// Extra options: adds the corner triangle and the popup.
    pub options: Option<OptionsFn>,
}

impl ToolEntry {
    /// An entry without extra options.
    pub const fn new(
        tool: Tool,
        icon: IconFn,
        name: &'static str,
        shortcut: &'static str,
        group: ToolGroup,
    ) -> Self {
        Self {
            tool,
            icon,
            name,
            shortcut,
            group,
            options: None,
        }
    }

    /// Adds extra options, shown in a popup.
    pub const fn with_options(mut self, options: OptionsFn) -> Self {
        self.options = Some(options);
        self
    }
}

/// The strip's buttons, top to bottom.
pub const TOOLS: &[ToolEntry] = &[
    ToolEntry::new(
        Tool::Select,
        icons::arrow,
        "Selection",
        "V",
        ToolGroup::Selection,
    ),
    ToolEntry::new(
        Tool::Lasso,
        icons::lasso,
        "Lasso",
        "L",
        ToolGroup::Selection,
    ),
    ToolEntry::new(
        Tool::Transform,
        icons::transform,
        "Transform",
        "Q",
        ToolGroup::Selection,
    ),
    ToolEntry::new(Tool::Brush, icons::brush, "Brush", "B", ToolGroup::Drawing),
    ToolEntry::new(Tool::Eraser, icons::eraser, "Eraser", "E", ToolGroup::Paint)
        .with_options(eraser_popup),
    ToolEntry::new(
        Tool::Fill,
        icons::paint_bucket,
        "Paint Bucket",
        "K",
        ToolGroup::Paint,
    )
    .with_options(fill_popup),
];

fn eraser_popup(ui: &mut egui::Ui, session: &mut Session) {
    ui.strong("Eraser mode");
    if tool_options::eraser_mode_choices(ui, session) {
        session.state.tool = Tool::Eraser;
        ui.close();
    }
}

fn fill_popup(ui: &mut egui::Ui, session: &mut Session) {
    ui.strong("Fill type");
    if tool_options::fill_style_choices(ui, session) {
        session.state.tool = Tool::Fill;
        ui.close();
    }
    ui.separator();
    ui.strong("Close gaps");
    if tool_options::gap_choices(ui, session) {
        session.state.tool = Tool::Fill;
        ui.close();
    }
}

/// Draws the tool strip.
pub fn panel(ui: &mut egui::Ui, session: &mut Session) {
    let busy = session.tools.is_active();
    ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
    ui.add_space(MARGIN);
    // Tool switching is locked while a gesture is in progress.
    ui.add_enabled_ui(!busy, |ui| {
        for (index, group) in TOOLS.chunk_by(|a, b| a.group == b.group).enumerate() {
            if index > 0 {
                ui.add(egui::Separator::default().spacing(8.0));
            }
            for row in group.chunks(2) {
                ui.horizontal(|ui| {
                    for entry in row {
                        tool_button(ui, session, entry);
                    }
                });
            }
        }
    });
}

/// One square icon button, with its tooltip and options popup.
fn tool_button(ui: &mut egui::Ui, session: &mut Session, entry: &ToolEntry) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(BUTTON, BUTTON), egui::Sense::click());
    let selected = session.state.tool == entry.tool;
    let tooltip = format!("{} ({})", entry.name, entry.shortcut);

    if ui.is_rect_visible(rect) {
        let widgets = &ui.visuals().widgets;
        let (bg, mut fg) = if selected {
            (
                Some(widgets.hovered.weak_bg_fill),
                widgets.active.fg_stroke.color,
            )
        } else if response.hovered() {
            (
                Some(widgets.inactive.weak_bg_fill),
                widgets.hovered.fg_stroke.color,
            )
        } else {
            (None, widgets.inactive.fg_stroke.color)
        };
        if !ui.is_enabled() {
            fg = fg.gamma_multiply(0.45);
        }
        let painter = ui.painter();
        if let Some(bg) = bg {
            painter.rect_filled(rect, 4.0, bg);
        }
        let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(18.0, 18.0));
        (entry.icon)(painter, icon_rect, fg);
        if entry.options.is_some() {
            icons::options_marker(painter, rect, fg.gamma_multiply(0.8));
        }
    }

    if response.clicked() {
        session.state.tool = entry.tool;
    }

    if let Some(options) = entry.options {
        // Right-click, or press and hold, opens the options.
        let pressed_for = ui.input(|i| {
            i.pointer
                .press_start_time()
                .map_or(0.0, |start| i.time - start)
        });
        let holding = response.is_pointer_button_down_on();
        let held = holding && pressed_for >= HOLD_SECS;
        if holding && !held {
            ui.ctx()
                .request_repaint_after_secs((HOLD_SECS - pressed_for) as f32);
        }
        // A click on the corner triangle opens the options too.
        let marker =
            egui::Rect::from_min_max(rect.right_bottom() - egui::vec2(10.0, 10.0), rect.max);
        let marker_clicked = response.clicked()
            && response
                .interact_pointer_pos()
                .is_some_and(|p| marker.contains(p));
        let open = response.secondary_clicked() || held || marker_clicked;
        egui::Popup::from_response(&response)
            .id(response.id.with("options"))
            .open_memory(open.then_some(egui::SetOpenCommand::Bool(true)))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .align(egui::RectAlign::BOTTOM_START)
            .gap(4.0)
            .show(|ui| options(ui, session));
    }

    response
        .on_hover_text(tooltip.clone())
        .on_disabled_hover_text(tooltip);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_list_is_well_formed() {
        // Each tool appears once.
        for (i, a) in TOOLS.iter().enumerate() {
            assert!(TOOLS[i + 1..].iter().all(|b| b.tool != a.tool));
        }
        // Each group is one run, so it gets exactly one divider.
        let runs: Vec<ToolGroup> = TOOLS
            .chunk_by(|a, b| a.group == b.group)
            .map(|run| run[0].group)
            .collect();
        for (i, g) in runs.iter().enumerate() {
            assert!(!runs[i + 1..].contains(g), "{g:?} is split");
        }
    }
}
