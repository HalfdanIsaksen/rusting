//! The editor's colours: the palette of the Rusting website
//! (`rusting-animatiom/src/styles/global.css`), dark and light.

use egui::{Color32, CornerRadius, Stroke, Visuals};

/// One mode's colour tokens, named as on the website.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// Page background: panels.
    pub bg: Color32,
    /// Cards: windows, rulers, headers.
    pub surface: Color32,
    /// Buttons.
    pub surface_2: Color32,
    /// Borders and separators.
    pub border: Color32,
    /// Body text.
    pub text: Color32,
    /// Secondary text; also the hover border.
    pub text_muted: Color32,
    /// Links, primary buttons, focus, the playhead.
    pub accent: Color32,
    /// Secondary accent.
    pub accent_2: Color32,
    /// Text on the accent colour.
    pub accent_text: Color32,
    /// Code blocks: text fields, the frame grid, the canvas workspace.
    pub code_bg: Color32,
}

/// The dark palette (the website's default).
pub const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x14, 0x11, 0x0f),
    surface: Color32::from_rgb(0x1d, 0x19, 0x16),
    surface_2: Color32::from_rgb(0x29, 0x23, 0x1e),
    border: Color32::from_rgb(0x3a, 0x31, 0x2a),
    text: Color32::from_rgb(0xf3, 0xec, 0xe6),
    text_muted: Color32::from_rgb(0xb3, 0xa7, 0x9c),
    accent: Color32::from_rgb(0xe0, 0x71, 0x3a),
    accent_2: Color32::from_rgb(0xe0, 0xa1, 0x3a),
    accent_text: Color32::from_rgb(0x1b, 0x0d, 0x05),
    code_bg: Color32::from_rgb(0x0f, 0x0d, 0x0b),
};

/// The light palette.
pub const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xfb, 0xf8, 0xf5),
    surface: Color32::from_rgb(0xf3, 0xed, 0xe7),
    surface_2: Color32::from_rgb(0xe9, 0xe0, 0xd7),
    border: Color32::from_rgb(0xdd, 0xd1, 0xc5),
    text: Color32::from_rgb(0x23, 0x1b, 0x15),
    text_muted: Color32::from_rgb(0x6b, 0x5d, 0x51),
    accent: Color32::from_rgb(0xb8, 0x50, 0x1f),
    accent_2: Color32::from_rgb(0xb9, 0x7d, 0x12),
    accent_text: Color32::from_rgb(0xff, 0xff, 0xff),
    code_bg: Color32::from_rgb(0xf6, 0xf1, 0xec),
};

/// The palette for dark or light mode.
pub fn palette(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}

/// egui visuals built from the palette.
pub fn visuals(dark: bool) -> Visuals {
    let p = palette(dark);
    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    let radius = CornerRadius::same(4);

    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.faint_bg_color = p.surface;
    v.extreme_bg_color = p.code_bg;
    v.code_bg_color = p.code_bg;
    v.hyperlink_color = p.accent;
    // Selected chips and rows: an accent tint; outlines and focus: the accent.
    v.selection.bg_fill = p.bg.lerp_to_gamma(p.accent, if dark { 0.55 } else { 0.3 });
    v.selection.stroke = Stroke::new(1.0, if dark { p.accent_2 } else { p.accent });
    v.text_cursor.stroke.color = p.accent;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.bg;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text_muted);
    for (state, fill) in [
        (&mut w.inactive, p.surface_2),
        (&mut w.hovered, p.surface_2),
        (&mut w.active, p.border),
        (&mut w.open, p.surface_2),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.fg_stroke.color = p.text;
        state.corner_radius = radius;
    }
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    w.hovered.bg_stroke = Stroke::new(1.0, p.text_muted);
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.open.bg_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = radius;
    v
}

/// Uses the website colours for both egui themes. egui picks dark or light
/// to follow the operating system.
pub fn apply(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Dark, visuals(true));
    ctx.set_visuals_of(egui::Theme::Light, visuals(false));
}
