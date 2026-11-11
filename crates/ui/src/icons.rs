//! Tool and layer icons drawn with the egui painter.
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

/// Lasso tool: a loop with a rope end hanging from it.
pub fn lasso(painter: &Painter, rect: Rect, color: Color32) {
    let w = line_width(rect);
    painter.add(PathShape::closed_line(
        arc(rect, (0.52, 0.38), (0.34, 0.22), 0.0, std::f32::consts::TAU),
        Stroke::new(w, color),
    ));
    curve(
        painter,
        rect,
        color,
        [(0.30, 0.56), (0.20, 0.74), (0.34, 0.90)],
    );
}

/// Transform tool: a box with corner handles and a curved rotate arrow.
pub fn transform(painter: &Painter, rect: Rect, color: Color32) {
    let w = line_width(rect);
    outline(
        painter,
        rect,
        color,
        &[(0.16, 0.30), (0.70, 0.30), (0.70, 0.84), (0.16, 0.84)],
    );
    let h = 0.07;
    for (x, y) in [(0.16, 0.30), (0.70, 0.30), (0.70, 0.84), (0.16, 0.84)] {
        filled(
            painter,
            rect,
            color,
            &[
                (x - h, y - h),
                (x + h, y - h),
                (x + h, y + h),
                (x - h, y + h),
            ],
        );
    }
    // Rotate arrow arcing over the top-right corner.
    painter.add(PathShape::line(
        arc(rect, (0.70, 0.30), (0.20, 0.20), -2.2, -0.35),
        Stroke::new(w, color),
    ));
    filled(
        painter,
        rect,
        color,
        &[(0.96, 0.18), (0.82, 0.26), (0.97, 0.34)],
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

/// Rectangle tool: a rectangle outline.
pub fn rectangle(painter: &Painter, rect: Rect, color: Color32) {
    outline(
        painter,
        rect,
        color,
        &[(0.14, 0.24), (0.86, 0.24), (0.86, 0.76), (0.14, 0.76)],
    );
}

/// Ellipse tool: an ellipse outline.
pub fn ellipse(painter: &Painter, rect: Rect, color: Color32) {
    let pts: Vec<(f32, f32)> = (0..32)
        .map(|i| {
            let a = i as f32 / 32.0 * std::f32::consts::TAU;
            (0.5 + 0.38 * a.cos(), 0.5 + 0.28 * a.sin())
        })
        .collect();
    outline(painter, rect, color, &pts);
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

/// Eyedropper: a tilted pipette with a bulb at the top right and a drop
/// at the tip.
pub fn eyedropper(painter: &Painter, rect: Rect, color: Color32) {
    let w = line_width(rect);
    // Glass tube from the tip (bottom left) to the collar.
    line(painter, rect, color, w * 2.2, (0.20, 0.80), (0.56, 0.44));
    // Collar across the tube.
    line(painter, rect, color, w * 1.2, (0.48, 0.38), (0.62, 0.52));
    // Rubber bulb.
    painter.circle_filled(at(rect, 0.70, 0.30), rect.width() * 0.14, color);
    // A drop under the tip.
    painter.circle_filled(at(rect, 0.14, 0.90), rect.width() * 0.06, color);
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

/// Samples an elliptical arc of the unit square (angles in radians, 0 =
/// right, positive = down) onto `rect`.
fn arc(rect: Rect, center: (f32, f32), radius: (f32, f32), from: f32, to: f32) -> Vec<Pos2> {
    const STEPS: usize = 12;
    (0..=STEPS)
        .map(|i| {
            let a = from + (to - from) * i as f32 / STEPS as f32;
            at(
                rect,
                center.0 + radius.0 * a.cos(),
                center.1 + radius.1 * a.sin(),
            )
        })
        .collect()
}

fn curve(painter: &Painter, rect: Rect, color: Color32, pts: [(f32, f32); 3]) {
    painter.add(QuadraticBezierShape::from_points_stroke(
        [
            at(rect, pts[0].0, pts[0].1),
            at(rect, pts[1].0, pts[1].1),
            at(rect, pts[2].0, pts[2].1),
        ],
        false,
        Color32::TRANSPARENT,
        PathStroke::new(line_width(rect), color),
    ));
}

/// Open eye: the layer is visible.
pub fn eye(painter: &Painter, rect: Rect, color: Color32) {
    curve(
        painter,
        rect,
        color,
        [(0.08, 0.5), (0.5, 0.08), (0.92, 0.5)],
    );
    curve(
        painter,
        rect,
        color,
        [(0.08, 0.5), (0.5, 0.92), (0.92, 0.5)],
    );
    painter.circle_filled(at(rect, 0.5, 0.5), rect.width() * 0.13, color);
}

/// Closed eye (a lowered lid): the layer is hidden.
pub fn eye_closed(painter: &Painter, rect: Rect, color: Color32) {
    curve(
        painter,
        rect,
        color,
        [(0.1, 0.45), (0.5, 0.85), (0.9, 0.45)],
    );
}

/// Padlock, closed or open.
fn padlock(painter: &Painter, rect: Rect, color: Color32, closed: bool) {
    let w = line_width(rect);
    let body = Rect::from_min_max(at(rect, 0.22, 0.48), at(rect, 0.78, 0.9));
    painter.rect_filled(body, w, color);
    // The open shackle is lifted and its right leg stays out of the body.
    let (lift, right_end) = if closed { (0.0, 0.48) } else { (0.1, 0.3) };
    let top = 0.34 - lift;
    let mut shackle = vec![at(rect, 0.34, 0.48)];
    shackle.extend(arc(
        rect,
        (0.5, top),
        (0.16, 0.2),
        std::f32::consts::PI,
        std::f32::consts::TAU,
    ));
    shackle.push(at(rect, 0.66, right_end));
    painter.add(PathShape::line(shackle, Stroke::new(w * 1.3, color)));
}

/// Closed padlock: the layer is locked.
pub fn lock_closed(painter: &Painter, rect: Rect, color: Color32) {
    padlock(painter, rect, color, true);
}

/// Open padlock: the layer can be edited.
pub fn lock_open(painter: &Painter, rect: Rect, color: Color32) {
    padlock(painter, rect, color, false);
}

/// Plus sign (add).
pub fn plus(painter: &Painter, rect: Rect, color: Color32) {
    let w = line_width(rect) * 1.4;
    line(painter, rect, color, w, (0.5, 0.2), (0.5, 0.8));
    line(painter, rect, color, w, (0.2, 0.5), (0.8, 0.5));
}

/// Trash can (delete).
pub fn trash(painter: &Painter, rect: Rect, color: Color32) {
    let w = line_width(rect);
    line(painter, rect, color, w * 1.3, (0.18, 0.26), (0.82, 0.26));
    outline(
        painter,
        rect,
        color,
        &[(0.4, 0.26), (0.42, 0.14), (0.58, 0.14), (0.6, 0.26)],
    );
    outline(
        painter,
        rect,
        color,
        &[(0.26, 0.26), (0.32, 0.88), (0.68, 0.88), (0.74, 0.26)],
    );
    line(painter, rect, color, w, (0.44, 0.4), (0.45, 0.76));
    line(painter, rect, color, w, (0.56, 0.4), (0.55, 0.76));
}

/// Upward chevron (move up).
pub fn chevron_up(painter: &Painter, rect: Rect, color: Color32) {
    painter.add(PathShape::line(
        points(rect, &[(0.22, 0.64), (0.5, 0.34), (0.78, 0.64)]),
        Stroke::new(line_width(rect) * 1.5, color),
    ));
}

/// Downward chevron (move down).
pub fn chevron_down(painter: &Painter, rect: Rect, color: Color32) {
    painter.add(PathShape::line(
        points(rect, &[(0.22, 0.36), (0.5, 0.66), (0.78, 0.36)]),
        Stroke::new(line_width(rect) * 1.5, color),
    ));
}

/// Two overlapping frames, the front one with a plus (duplicate frame).
pub fn duplicate(painter: &Painter, rect: Rect, color: Color32) {
    let w = line_width(rect);
    // Back frame: only the parts not hidden by the front one.
    painter.add(PathShape::line(
        points(rect, &[(0.14, 0.62), (0.14, 0.14), (0.62, 0.14)]),
        Stroke::new(w, color),
    ));
    outline(
        painter,
        rect,
        color,
        &[(0.32, 0.32), (0.86, 0.32), (0.86, 0.86), (0.32, 0.86)],
    );
    line(painter, rect, color, w * 1.2, (0.59, 0.45), (0.59, 0.73));
    line(painter, rect, color, w * 1.2, (0.45, 0.59), (0.73, 0.59));
}

/// A luggage tag (frame tags).
pub fn tag(painter: &Painter, rect: Rect, color: Color32) {
    outline(
        painter,
        rect,
        color,
        &[
            (0.12, 0.30),
            (0.62, 0.30),
            (0.90, 0.50),
            (0.62, 0.70),
            (0.12, 0.70),
        ],
    );
    painter.circle_filled(at(rect, 0.64, 0.50), rect.width() * 0.06, color);
}

/// Small plus in the top-right corner, for the "new key" icons.
fn corner_plus(painter: &Painter, rect: Rect, color: Color32) {
    let w = line_width(rect) * 1.2;
    line(painter, rect, color, w, (0.78, 0.08), (0.78, 0.40));
    line(painter, rect, color, w, (0.62, 0.24), (0.94, 0.24));
}

/// Filled key dot with a plus: new drawing key.
pub fn drawing_key(painter: &Painter, rect: Rect, color: Color32) {
    painter.circle_filled(at(rect, 0.42, 0.58), rect.width() * 0.26, color);
    corner_plus(painter, rect, color);
}

/// Hollow key dot with a plus: new empty key (ends the exposure).
pub fn empty_key(painter: &Painter, rect: Rect, color: Color32) {
    painter.circle_stroke(
        at(rect, 0.42, 0.58),
        rect.width() * 0.24,
        Stroke::new(line_width(rect) * 1.2, color),
    );
    corner_plus(painter, rect, color);
}

/// Key dot with a cross: remove key.
pub fn remove_key(painter: &Painter, rect: Rect, color: Color32) {
    painter.circle_filled(at(rect, 0.42, 0.58), rect.width() * 0.26, color);
    let w = line_width(rect) * 1.2;
    line(painter, rect, color, w, (0.66, 0.10), (0.92, 0.36));
    line(painter, rect, color, w, (0.66, 0.36), (0.92, 0.10));
}
