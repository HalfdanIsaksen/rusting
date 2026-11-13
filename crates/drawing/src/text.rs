//! Text as vector outlines (ADR-016).
//!
//! [`Font::layout`] turns a string into a [`FillShape`]: every glyph's
//! outline, flattened into polygons, in document coordinates. The text tool
//! adds that shape as one ordinary [`Fill`](crate::Fill), so text renders,
//! selects, moves, transforms, erases and exports like any other fill; it is
//! not editable as text afterwards.
//!
//! Font files are parsed with `ttf-parser` (TrueType and CFF outlines). The
//! caller passes the font bytes; this crate embeds no font.
//!
//! Layout is deliberately simple: left-aligned lines separated by `\n`, each
//! glyph advanced by its horizontal advance. There is no kerning, shaping
//! (ligatures, complex scripts) or wrapping.

use std::fmt;

use animation_core::Vec2;
use ttf_parser::{Face, GlyphId, OutlineBuilder};

use crate::fill::{FillShape, signed_area};

/// Most line segments a single curve is flattened into.
const MAX_CURVE_SEGMENTS: u32 = 64;

/// Spaces a tab advances by.
const TAB_WIDTH: f32 = 4.0;

/// Why a font could not be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextError {
    /// The bytes are not a font `ttf-parser` can read.
    InvalidFont,
}

impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFont => f.write_str("not a readable TrueType/OpenType font"),
        }
    }
}

impl std::error::Error for TextError {}

/// A parsed font. Cheap to create: parsing only reads the table directory.
pub struct Font<'a> {
    face: Face<'a>,
}

impl fmt::Debug for Font<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Font")
            .field("units_per_em", &self.face.units_per_em())
            .finish_non_exhaustive()
    }
}

/// The result of [`Font::layout`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextLayout {
    /// Glyph outlines in document coordinates: outer contours have positive
    /// signed area and holes (the counters of "o", "A", "B") negative, as
    /// [`FillShape`] expects. Empty when the text has no visible glyphs.
    pub shape: FillShape,
    /// Top of the caret after the last character, in document coordinates.
    pub caret: Vec2,
    /// Height of one line (and of the caret), in document units.
    pub line_height: f32,
}

impl<'a> Font<'a> {
    /// Parses the first face of a TrueType/OpenType font file.
    pub fn parse(data: &'a [u8]) -> Result<Self, TextError> {
        let face = Face::parse(data, 0).map_err(|_| TextError::InvalidFont)?;
        if face.units_per_em() == 0 {
            return Err(TextError::InvalidFont);
        }
        Ok(Self { face })
    }

    /// Document units per font unit at font size `size` (the em height).
    fn scale(&self, size: f32) -> f32 {
        size / f32::from(self.face.units_per_em())
    }

    /// Distance between baselines at font size `size`.
    pub fn line_height(&self, size: f32) -> f32 {
        let f = &self.face;
        let units = f32::from(f.ascender()) - f32::from(f.descender()) + f32::from(f.line_gap());
        units * self.scale(size)
    }

    /// Lays out `text` at font size `size` (document units per em) with the
    /// top-left of the first line at `origin`. Lines break at `\n`.
    /// Characters the font lacks draw as its "missing glyph" box.
    ///
    /// Curves are flattened so no point of the polygon is further than
    /// about `size / 1000` (at least 0.005 units) from the true outline.
    pub fn layout(&self, text: &str, size: f32, origin: Vec2) -> TextLayout {
        let size = if size.is_finite() { size.max(0.0) } else { 0.0 };
        let scale = self.scale(size);
        let line_height = self.line_height(size);
        let ascender = f32::from(self.face.ascender()) * scale;
        let tolerance = (size * 0.001).max(0.005);
        let space = self
            .face
            .glyph_index(' ')
            .and_then(|g| self.face.glyph_hor_advance(g))
            .map_or(size * 0.25, |a| f32::from(a) * scale);

        let mut contours = Vec::new();
        let mut pen = Vec2::ZERO;
        for ch in text.chars() {
            match ch {
                '\n' => {
                    pen = Vec2::new(0.0, pen.y + line_height);
                    continue;
                }
                '\r' => continue,
                '\t' => {
                    pen.x += space * TAB_WIDTH;
                    continue;
                }
                c if c.is_control() => continue,
                _ => {}
            }
            let glyph = self.face.glyph_index(ch).unwrap_or(GlyphId(0));
            let baseline = origin + pen + Vec2::new(0.0, ascender);
            let mut builder = Flattener::new(baseline, scale, tolerance);
            // `None` for glyphs without an outline, such as the space.
            if self.face.outline_glyph(glyph, &mut builder).is_some() {
                contours.extend(orient(builder.finish()));
            }
            pen.x += self
                .face
                .glyph_hor_advance(glyph)
                .map_or(0.0, |a| f32::from(a) * scale);
        }
        TextLayout {
            shape: FillShape { contours },
            caret: origin + pen,
            line_height,
        }
    }
}

/// Collects a glyph outline as polygons in document coordinates, flattening
/// curves. Font units are y-up; documents are y-down.
struct Flattener {
    baseline: Vec2,
    scale: f32,
    tolerance: f32,
    contours: Vec<Vec<Vec2>>,
    current: Vec<Vec2>,
}

impl Flattener {
    fn new(baseline: Vec2, scale: f32, tolerance: f32) -> Self {
        Self {
            baseline,
            scale,
            tolerance,
            contours: Vec::new(),
            current: Vec::new(),
        }
    }

    fn map(&self, x: f32, y: f32) -> Vec2 {
        self.baseline + Vec2::new(x, -y) * self.scale
    }

    fn last(&self) -> Vec2 {
        self.current.last().copied().unwrap_or(self.baseline)
    }

    /// Segments needed so a curve whose second derivative is at most
    /// `curvature` (document units) stays within the tolerance: a chord of
    /// parameter step `h` deviates by at most `curvature * h² / 8`.
    fn segments(&self, curvature: f32) -> u32 {
        let n = (curvature / (8.0 * self.tolerance)).sqrt().ceil();
        if n.is_finite() {
            (n as u32).clamp(1, MAX_CURVE_SEGMENTS)
        } else {
            1
        }
    }

    fn close_current(&mut self) {
        let mut contour = std::mem::take(&mut self.current);
        if contour.len() > 1 && contour.first() == contour.last() {
            contour.pop();
        }
        if contour.len() >= 3 {
            self.contours.push(contour);
        }
    }

    fn finish(mut self) -> Vec<Vec<Vec2>> {
        self.close_current();
        self.contours
    }
}

impl OutlineBuilder for Flattener {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close_current();
        let p = self.map(x, y);
        self.current.push(p);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        self.current.push(p);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (p0, p1, p2) = (self.last(), self.map(x1, y1), self.map(x, y));
        // B'' = 2 (p0 - 2 p1 + p2).
        let n = self.segments(2.0 * (p0 - 2.0 * p1 + p2).length());
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            self.current
                .push(p0 * (u * u) + p1 * (2.0 * u * t) + p2 * (t * t));
        }
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let p0 = self.last();
        let (p1, p2, p3) = (self.map(x1, y1), self.map(x2, y2), self.map(x, y));
        // |B''| <= 6 max(|p0 - 2 p1 + p2|, |p1 - 2 p2 + p3|).
        let d = (p0 - 2.0 * p1 + p2)
            .length()
            .max((p1 - 2.0 * p2 + p3).length());
        let n = self.segments(6.0 * d);
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            self.current.push(
                p0 * (u * u * u)
                    + p1 * (3.0 * u * u * t)
                    + p2 * (3.0 * u * t * t)
                    + p3 * (t * t * t),
            );
        }
    }

    fn close(&mut self) {
        self.close_current();
    }
}

/// Gives a glyph's contours the orientation [`FillShape`] expects, whatever
/// the font's convention (TrueType and CFF wind in opposite directions):
/// a contour inside an even number of the glyph's other contours is an outer
/// boundary (positive signed area), inside an odd number a hole (negative).
fn orient(mut contours: Vec<Vec<Vec2>>) -> Vec<Vec<Vec2>> {
    let depths: Vec<usize> = contours
        .iter()
        .enumerate()
        .map(|(i, contour)| {
            let probe = interior_probe(contour);
            contours
                .iter()
                .enumerate()
                .filter(|&(j, other)| j != i && point_in_polygon(other, probe))
                .count()
        })
        .collect();
    for (contour, depth) in contours.iter_mut().zip(depths) {
        let area = signed_area(contour);
        let want_positive = depth % 2 == 0;
        if (area > 0.0) != want_positive {
            contour.reverse();
        }
    }
    contours
}

/// A point to test a contour's nesting with: its first vertex. Glyph
/// contours of one glyph don't touch, so any vertex lies strictly inside or
/// outside each other contour.
fn interior_probe(contour: &[Vec2]) -> Vec2 {
    contour.first().copied().unwrap_or(Vec2::ZERO)
}

fn point_in_polygon(polygon: &[Vec2], p: Vec2) -> bool {
    let n = polygon.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (polygon[i], polygon[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if p.x < x {
                inside = !inside;
            }
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Fill, tessellate_fill};
    use animation_core::Rgba;

    const FONT: &[u8] = epaint_default_fonts::UBUNTU_LIGHT;

    fn font() -> Font<'static> {
        Font::parse(FONT).unwrap()
    }

    fn layout(text: &str, size: f32) -> TextLayout {
        font().layout(text, size, Vec2::new(10.0, 20.0))
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(
            Font::parse(b"not a font").unwrap_err(),
            TextError::InvalidFont
        );
    }

    #[test]
    fn empty_and_blank_text_produce_nothing() {
        for text in ["", " ", "\n\n", "\t "] {
            assert!(layout(text, 48.0).shape.contours.is_empty(), "{text:?}");
        }
    }

    #[test]
    fn o_has_an_outer_contour_and_a_hole() {
        let shape = layout("o", 100.0).shape;
        assert_eq!(shape.contours.len(), 2);
        let areas: Vec<f32> = shape.contours.iter().map(|c| signed_area(c)).collect();
        assert_eq!(areas.iter().filter(|a| **a > 0.0).count(), 1);
        assert_eq!(areas.iter().filter(|a| **a < 0.0).count(), 1);
        // The middle of the "o" is not filled; its ring is.
        let (min, max) = shape.bounds().unwrap();
        let centre = (min + max) * 0.5;
        assert!(!shape.contains(centre));
        assert!(shape.contains(Vec2::new(min.x + 2.0, centre.y)));
        // The tessellator sees the hole: less area than the outer contour.
        let fill = Fill::new(shape.clone(), Rgba::BLACK, 1.0);
        let mesh = tessellate_fill(&fill);
        let mesh_area: f32 = mesh
            .indices
            .chunks(3)
            .map(|t| {
                let [a, b, c] = [0, 1, 2].map(|k| Vec2::from(mesh.vertices[t[k] as usize]));
                ((b - a).perp_dot(c - a) * 0.5).abs()
            })
            .sum();
        assert!((mesh_area - shape.area()).abs() < shape.area() * 0.01);
        assert!(mesh_area < areas.iter().copied().fold(0.0, f32::max) * 0.9);
    }

    #[test]
    fn counters_of_a_and_b_are_holes() {
        for (text, holes) in [("A", 1), ("B", 2), ("8", 2), ("l", 0)] {
            let shape = layout(text, 100.0).shape;
            let negative = shape
                .contours
                .iter()
                .filter(|c| signed_area(c) < 0.0)
                .count();
            assert_eq!(negative, holes, "{text}");
            assert!(shape.area() > 0.0, "{text}");
        }
    }

    #[test]
    fn bounds_scale_with_font_size() {
        let (a_min, a_max) = layout("Hello", 50.0).shape.bounds().unwrap();
        let (b_min, b_max) = layout("Hello", 100.0).shape.bounds().unwrap();
        let (a, b) = (a_max - a_min, b_max - b_min);
        assert!((b.x / a.x - 2.0).abs() < 0.02, "{a} {b}");
        assert!((b.y / a.y - 2.0).abs() < 0.02, "{a} {b}");
        // Cap height of a 100-unit font is somewhere around 70 units.
        assert!(b.y > 50.0 && b.y < 100.0, "{b}");
    }

    #[test]
    fn text_starts_at_the_origin_and_sits_below_it() {
        let origin = Vec2::new(10.0, 20.0);
        let l = font().layout("H", 100.0, origin);
        let (min, max) = l.shape.bounds().unwrap();
        assert!(min.x >= origin.x && min.x < origin.x + 15.0, "{min}");
        assert!(
            min.y > origin.y && max.y < origin.y + l.line_height,
            "{min} {max}"
        );
    }

    #[test]
    fn lines_stack_and_the_caret_follows() {
        let f = font();
        let origin = Vec2::new(0.0, 0.0);
        let one = f.layout("Hi", 40.0, origin);
        let two = f.layout("Hi\nHi", 40.0, origin);
        assert_eq!(two.shape.contours.len(), 2 * one.shape.contours.len());
        let (_, max1) = one.shape.bounds().unwrap();
        let (_, max2) = two.shape.bounds().unwrap();
        assert!((max2.y - max1.y - one.line_height).abs() < 0.01);
        // Caret: end of the line, then the start of the next line.
        assert!(one.caret.x > 0.0 && one.caret.y == 0.0);
        let newline = f.layout("Hi\n", 40.0, origin);
        assert_eq!(newline.caret, Vec2::new(0.0, one.line_height));
        // A space moves the caret but adds nothing.
        let spaced = f.layout("Hi ", 40.0, origin);
        assert!(spaced.caret.x > one.caret.x);
        assert_eq!(spaced.shape, one.shape);
    }

    #[test]
    fn curves_are_flattened_finely() {
        // A big "o" is smooth: many points, short edges.
        let shape = layout("o", 200.0).shape;
        let outer = shape
            .contours
            .iter()
            .max_by(|a, b| signed_area(a).total_cmp(&signed_area(b)))
            .unwrap();
        assert!(outer.len() >= 32, "{}", outer.len());
        let longest = (0..outer.len())
            .map(|i| outer[i].distance(outer[(i + 1) % outer.len()]))
            .fold(0.0, f32::max);
        assert!(longest < 15.0, "{longest}");
    }

    #[test]
    fn missing_glyphs_still_draw_something() {
        // U+E000 is a private-use code point the font does not map.
        let l = layout("\u{E000}", 50.0);
        assert!(l.caret.x > 10.0);
    }
}
