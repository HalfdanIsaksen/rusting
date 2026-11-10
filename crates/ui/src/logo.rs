//! The Rusting logo, as on the website (`rusting-animatiom/public/favicon.svg`
//! and `assets/logo.svg`): an orange brush stroke ending in an amber dot on a
//! dark rounded square.
//!
//! The geometry lives here once. [`rgba`] rasterizes it for the window and
//! dock icon, and [`paint`] draws it with egui (the start menu).

use egui::epaint::CubicBezierShape;
use egui::{Color32, CornerRadius, Pos2, Rect, Shape, Stroke, pos2};

/// Side of the logo's design space (the SVG `viewBox`).
const SIDE: f32 = 64.0;
/// Corner radius of the background square.
const CORNER: f32 = 14.0;
/// Width of the brush stroke.
const STROKE_WIDTH: f32 = 6.0;
/// The brush stroke: two cubic Bézier segments.
const CURVES: [[[f32; 2]; 4]; 2] = [
    [[14.0, 46.0], [20.0, 32.0], [28.0, 20.0], [50.0, 16.0]],
    [[50.0, 16.0], [41.0, 22.0], [35.0, 31.0], [32.0, 42.0]],
];
/// The dot: centre and radius.
const DOT: ([f32; 2], f32) = ([47.0, 44.0], 5.0);

/// The logo's three colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogoColors {
    /// The rounded square.
    pub background: Color32,
    /// The brush stroke.
    pub stroke: Color32,
    /// The dot.
    pub dot: Color32,
}

/// The colours of the website's favicon, used for the app icon.
pub const ICON_COLORS: LogoColors = LogoColors {
    background: Color32::from_rgb(0x14, 0x11, 0x0f),
    stroke: Color32::from_rgb(0xe0, 0x71, 0x3a),
    dot: Color32::from_rgb(0xe0, 0xa1, 0x3a),
};

/// The logo in a theme's colours, as the website header draws it.
pub fn colors(palette: &crate::theme::Palette) -> LogoColors {
    LogoColors {
        background: palette.surface_2,
        stroke: palette.accent,
        dot: palette.accent_2,
    }
}

/// Draws the logo filling `rect` (which should be square).
pub fn paint(painter: &egui::Painter, rect: Rect, colors: LogoColors) {
    let scale = rect.width().min(rect.height()) / SIDE;
    let at = |[x, y]: [f32; 2]| rect.min + egui::vec2(x, y) * scale;
    painter.rect_filled(
        rect,
        CornerRadius::same((CORNER * scale) as u8),
        colors.background,
    );
    let stroke = Stroke::new(STROKE_WIDTH * scale, colors.stroke);
    for curve in CURVES {
        painter.add(CubicBezierShape::from_points_stroke(
            curve.map(at),
            false,
            Color32::TRANSPARENT,
            stroke,
        ));
    }
    // egui paths have butt ends; round them off like the SVG's round caps.
    for end in [CURVES[0][0], CURVES[0][3], CURVES[1][3]] {
        painter.add(Shape::circle_filled(
            at(end),
            stroke.width * 0.5,
            colors.stroke,
        ));
    }
    painter.circle_filled(at(DOT.0), DOT.1 * scale, colors.dot);
}

/// The logo as straight-alpha RGBA pixels, `size` × `size`, row by row.
///
/// `inset` is the transparent margin on each side as a fraction of `size`
/// (macOS dock icons leave one; window icons don't).
pub fn rgba(size: u32, inset: f32, colors: LogoColors) -> Vec<u8> {
    let polyline = flatten();
    let inner = size as f32 * (1.0 - 2.0 * inset);
    let units_per_px = SIDE / inner.max(1.0);
    let offset = size as f32 * inset;
    let coverage = |distance: f32| (0.5 - distance / units_per_px).clamp(0.0, 1.0);

    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let p = pos2(
                (x as f32 + 0.5 - offset) * units_per_px,
                (y as f32 + 0.5 - offset) * units_per_px,
            );
            let square = coverage(rounded_square_distance(p));
            let mut color = colors.background;
            if square > 0.0 {
                let stroke = coverage(polyline_distance(&polyline, p) - STROKE_WIDTH * 0.5);
                color = blend(color, colors.stroke, stroke);
                let dot = coverage(p.distance(pos2(DOT.0[0], DOT.0[1])) - DOT.1);
                color = blend(color, colors.dot, dot);
            }
            pixels.extend_from_slice(&[color.r(), color.g(), color.b(), unit_to_u8(square)]);
        }
    }
    pixels
}

/// The brush stroke as a polyline in design units.
fn flatten() -> Vec<Pos2> {
    const STEPS: usize = 24;
    let mut points = vec![pos2(CURVES[0][0][0], CURVES[0][0][1])];
    for [p0, p1, p2, p3] in CURVES {
        for step in 1..=STEPS {
            let t = step as f32 / STEPS as f32;
            let u = 1.0 - t;
            let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            let x = w[0] * p0[0] + w[1] * p1[0] + w[2] * p2[0] + w[3] * p3[0];
            let y = w[0] * p0[1] + w[1] * p1[1] + w[2] * p2[1] + w[3] * p3[1];
            points.push(pos2(x, y));
        }
    }
    points
}

/// Distance from `p` to the nearest point of `polyline`.
fn polyline_distance(polyline: &[Pos2], p: Pos2) -> f32 {
    polyline
        .windows(2)
        .map(|segment| {
            let (a, b) = (segment[0], segment[1]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_sq().max(f32::EPSILON)).clamp(0.0, 1.0);
            p.distance(a + ab * t)
        })
        .fold(f32::INFINITY, f32::min)
}

/// Signed distance from `p` to the edge of the background square (negative
/// inside).
fn rounded_square_distance(p: Pos2) -> f32 {
    let half = SIDE * 0.5;
    let qx = (p.x - half).abs() - (half - CORNER);
    let qy = (p.y - half).abs() - (half - CORNER);
    let outside = egui::vec2(qx.max(0.0), qy.max(0.0)).length();
    outside + qx.max(qy).min(0.0) - CORNER
}

/// `top` over `base`, with `top` covering the fraction `amount`.
fn blend(base: Color32, top: Color32, amount: f32) -> Color32 {
    let mix = |a: u8, b: u8| unit_to_u8((a as f32 + (b as f32 - a as f32) * amount) / 255.0);
    Color32::from_rgb(
        mix(base.r(), top.r()),
        mix(base.g(), top.g()),
        mix(base.b(), top.b()),
    )
}

fn unit_to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(pixels: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    }

    #[test]
    fn rasterizes_the_favicon() {
        let size = 64;
        let pixels = rgba(size, 0.0, ICON_COLORS);
        assert_eq!(pixels.len(), (size * size * 4) as usize);
        // Rounded corners are transparent; the middle of the square is opaque.
        assert_eq!(pixel(&pixels, size, 0, 0)[3], 0);
        assert_eq!(pixel(&pixels, size, 6, 60), [0x14, 0x11, 0x0f, 255]);
        // On the stroke, and on the dot.
        assert_eq!(pixel(&pixels, size, 50, 16), [0xe0, 0x71, 0x3a, 255]);
        assert_eq!(pixel(&pixels, size, 47, 44), [0xe0, 0xa1, 0x3a, 255]);
    }

    #[test]
    fn inset_leaves_a_transparent_margin() {
        let size = 100;
        let pixels = rgba(size, 0.1, ICON_COLORS);
        assert_eq!(pixel(&pixels, size, 5, 50)[3], 0);
        assert_eq!(pixel(&pixels, size, 15, 50)[3], 255);
    }
}
