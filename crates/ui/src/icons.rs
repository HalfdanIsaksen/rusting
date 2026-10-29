//! Tool icons drawn with the egui painter.
//!
//! Every icon is a few vector shapes laid out in a unit square that is mapped
//! onto the target rect, so icons stay crisp at any size and DPI. Colours come
//! from the caller (normally the egui theme's widget foreground colour).

use egui::epaint::{PathShape, PathStroke, QuadraticBezierShape};
use egui::{Color32, Painter, Pos2, Rect, Stroke};

/// Draws an icon into `rect` in `color`.
pub type IconFn = fn(&Painter, Rect, Color32);

/// Maps a point of the unit square onto `rect`.
fn at(rect: Rect, x: f32, y: f32) -> Pos2 {
    rect.min + egui::vec2(x * rect.width(), y * rect.height())
}

/// Maps several unit-square points onto `rect`.
fn points(rect: Rect, pts: &[(f32, f32)]) -> Vec<Pos2> {
    pts.iter().map(|&(x, y)| at(rect, x, y)).collect()
}

/// Line width for an icon of this size: about 1.3 points at 18 points.
fn line_width(rect: Rect) -> f32 {
    (rect.width() / 14.0).max(1.0)
}

fn filled(painter: &Painter, rect: Rect, color: Color32, pts: &[(f32, f32)]) {
    painter.add(PathShape::convex_polygon(
        points(rect, pts),
        color,
        Stroke::NONE,
    ));
}

fn outline(painter: &Painter, rect: Rect, color: Color32, pts: &[(f32, f32)]) {
    painter.add(PathShape::closed_line(
        points(rect, pts),
        Stroke::new(line_width(rect), color),
    ));
}

fn line(painter: &Painter, rect: Rect, color: Color32, width: f32, a: (f32, f32), b: (f32, f32)) {
    painter.line_segment(
        [at(rect, a.0, a.1), at(rect, b.0, b.1)],
        Stroke::new(width, color),
    );
}

/// Selection arrow (a mouse pointer).
pub fn arrow(painter: &Painter, rect: Rect, color: Color32) {
    // Non-convex, so filled as a head triangle plus a tail quad.
    filled(
        painter,
        rect,
        color,
        &[(0.28, 0.10), (0.74, 0.58), (0.28, 0.78)],
    );
    filled(
        painter,
        rect,
        color,
        &[(0.43, 0.60), (0.56, 0.58), (0.68, 0.84), (0.56, 0.90)],
    );
}

/// Paint brush: handle, ferrule and a filled bristle tip.
pub fn brush(painter: &Painter, rect: Rect, color: Color32) {
    let w = line_width(rect);
    line(painter, rect, color, w * 1.2, (0.88, 0.12), (0.55, 0.45));
    line(painter, rect, color, w * 2.6, (0.56, 0.44), (0.47, 0.53));
    filled(
        painter,
        rect,
        color,
        &[
            (0.42, 0.50),
            (0.50, 0.58),
            (0.42, 0.74),
            (0.26, 0.86),
            (0.12, 0.90),
            (0.18, 0.74),
            (0.28, 0.58),
        ],
    );
}

/// Eraser: a tilted block with the rubber end filled.
pub fn eraser(painter: &Painter, rect: Rect, color: Color32) {
    // Block along the diagonal: u points to the top right, v across it.
    let (cx, cy) = (0.5, 0.5);
    let (half_len, half_wid) = (0.36, 0.17);
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let (ux, uy) = (s, -s);
    let (vx, vy) = (s, s);
    let corner = |a: f32, b: f32| {
        (
            cx + a * half_len * ux + b * half_wid * vx,
            cy + a * half_len * uy + b * half_wid * vy,
        )
    };
    outline(
        painter,
        rect,
        color,
        &[
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ],
    );
    filled(
        painter,
        rect,
        color,
        &[
            corner(-1.0, -1.0),
            corner(-0.1, -1.0),
            corner(-0.1, 1.0),
            corner(-1.0, 1.0),
        ],
    );
}

/// Paint bucket: a tipped bucket with a handle and a drop of paint.
pub fn paint_bucket(painter: &Painter, rect: Rect, color: Color32) {
    let w = line_width(rect);
    // Body: a tilted square, the lower half filled with paint.
    outline(
        painter,
        rect,
        color,
        &[(0.12, 0.50), (0.44, 0.20), (0.74, 0.50), (0.42, 0.82)],
    );
    filled(
        painter,
        rect,
        color,
        &[(0.12, 0.50), (0.74, 0.50), (0.42, 0.82)],
    );
    // Handle over the top.
    painter.add(QuadraticBezierShape::from_points_stroke(
        [
            at(rect, 0.26, 0.38),
            at(rect, 0.32, 0.02),
            at(rect, 0.60, 0.36),
        ],
        false,
        Color32::TRANSPARENT,
        PathStroke::new(w, color),
    ));
    // Paint dripping from the lip.
    line(painter, rect, color, w, (0.74, 0.50), (0.84, 0.62));
    painter.circle_filled(at(rect, 0.84, 0.72), rect.width() * 0.09, color);
}

/// Hand (pan) tool: palm with four fingers and a thumb.
pub fn hand(painter: &Painter, rect: Rect, color: Color32) {
    let finger = line_width(rect) * 2.0;
    for (x, top) in [(0.36, 0.22), (0.50, 0.12), (0.64, 0.16), (0.78, 0.30)] {
        line(painter, rect, color, finger, (x, top), (x, 0.58));
        painter.circle_filled(at(rect, x, top), finger * 0.5, color);
    }
    line(painter, rect, color, finger, (0.16, 0.50), (0.32, 0.70));
    painter.circle_filled(at(rect, 0.16, 0.50), finger * 0.5, color);
    filled(
        painter,
        rect,
        color,
        &[
            (0.30, 0.55),
            (0.84, 0.55),
            (0.80, 0.74),
            (0.66, 0.90),
            (0.42, 0.90),
            (0.30, 0.76),
        ],
    );
}

/// Zoom tool: a magnifying glass.
pub fn zoom(painter: &Painter, rect: Rect, color: Color32) {
    let w = line_width(rect);
    painter.circle_stroke(
        at(rect, 0.42, 0.42),
        rect.width() * 0.26,
        Stroke::new(w * 1.2, color),
    );
    line(painter, rect, color, w * 2.4, (0.62, 0.62), (0.86, 0.86));
}

/// The small triangle in a tool button's bottom-right corner that marks a
/// tool with extra options.
pub fn options_marker(painter: &Painter, button: Rect, color: Color32) {
    let size = (button.width() * 0.16).max(3.0);
    let corner = button.right_bottom() - egui::vec2(3.0, 3.0);
    painter.add(PathShape::convex_polygon(
        vec![
            corner,
            corner - egui::vec2(size, 0.0),
            corner - egui::vec2(0.0, size),
        ],
        color,
        Stroke::NONE,
    ));
}
