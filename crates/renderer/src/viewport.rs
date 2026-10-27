//! Canvas viewport math: zoom, pan and coordinate conversion. No GPU code.
//!
//! Coordinate spaces:
//! - **Canvas** (document) space: document units, origin at the canvas's
//!   top-left corner, y pointing down. Strokes are stored in this space.
//! - **Screen** space: logical points relative to the top-left of the canvas
//!   panel, y pointing down. UI input arrives in this space.
//! - **Physical** space: screen space multiplied by `scale_factor`; this is the
//!   pixel grid of the canvas render target.
//!
//! The mapping is `screen = canvas * zoom + pan`. Zoom and pan are stored in
//! f64 so repeated zooming about a cursor does not drift at extreme zooms.

use animation_core::glam::{DMat4, DVec2, DVec4, Mat4, Vec2};

/// Smallest allowed zoom (logical points per document unit).
pub const MIN_ZOOM: f64 = 0.02;
/// Largest allowed zoom (logical points per document unit).
pub const MAX_ZOOM: f64 = 64.0;
/// Space kept around the canvas by [`Viewport::fit_canvas`], in logical points.
pub const FIT_MARGIN: f32 = 32.0;

/// How the canvas is placed inside the canvas panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Canvas size in document units.
    pub canvas_size: Vec2,
    /// Logical points per document unit.
    pub zoom: f64,
    /// Screen position (logical points) of the canvas origin.
    pub pan: DVec2,
    /// Size of the canvas panel in logical points.
    pub screen_size: Vec2,
    /// Physical pixels per logical point (the window's DPI scale).
    pub scale_factor: f32,
}

impl Viewport {
    /// A viewport with the canvas fitted and centred in the panel.
    pub fn new(canvas_size: Vec2, screen_size: Vec2, scale_factor: f32) -> Self {
        let mut viewport = Self {
            canvas_size,
            zoom: 1.0,
            pan: DVec2::ZERO,
            screen_size,
            scale_factor: sanitize_scale(scale_factor),
        };
        viewport.fit_canvas();
        viewport
    }

    /// Converts a screen point (logical points) to canvas coordinates.
    pub fn screen_to_canvas(&self, screen: Vec2) -> Vec2 {
        self.screen_to_canvas_f64(screen.as_dvec2()).as_vec2()
    }

    /// Converts a canvas point to screen coordinates (logical points).
    pub fn canvas_to_screen(&self, canvas: Vec2) -> Vec2 {
        (canvas.as_dvec2() * self.zoom + self.pan).as_vec2()
    }

    fn screen_to_canvas_f64(&self, screen: DVec2) -> DVec2 {
        (screen - self.pan) / self.zoom
    }

    /// Multiplies the zoom by `factor`, keeping the canvas point under
    /// `cursor` (screen space) fixed. The result is clamped to
    /// [`MIN_ZOOM`]`..=`[`MAX_ZOOM`]. Non-finite or non-positive factors are ignored.
    pub fn zoom_about(&mut self, cursor: Vec2, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        self.set_zoom_about(cursor, self.zoom * f64::from(factor));
    }

    /// Sets the zoom to `zoom` (clamped), keeping the canvas point under
    /// `cursor` (screen space) fixed.
    pub fn set_zoom_about(&mut self, cursor: Vec2, zoom: f64) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        if !zoom.is_finite() || zoom == self.zoom {
            return;
        }
        let cursor = cursor.as_dvec2();
        let anchor = self.screen_to_canvas_f64(cursor);
        self.zoom = zoom;
        // Solve `cursor = anchor * zoom + pan` for the new pan.
        self.pan = cursor - anchor * self.zoom;
    }

    /// Moves the canvas by `delta` logical points.
    pub fn pan_by(&mut self, delta: Vec2) {
        if delta.is_finite() {
            self.pan += delta.as_dvec2();
        }
    }

    /// Zooms so the whole canvas fits the panel with a margin, and centres it.
    pub fn fit_canvas(&mut self) {
        let available = (self.screen_size - Vec2::splat(2.0 * FIT_MARGIN)).max(Vec2::ONE);
        let canvas = self.canvas_size.max(Vec2::ONE);
        let zoom = (available.as_dvec2() / canvas.as_dvec2()).min_element();
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.center_canvas();
    }

    /// Centres the canvas in the panel without changing the zoom.
    pub fn center_canvas(&mut self) {
        self.pan = (self.screen_size.as_dvec2() - self.canvas_size.as_dvec2() * self.zoom) * 0.5;
    }

    /// Updates the panel size and DPI scale. The canvas point at the centre of
    /// the panel stays at the centre.
    pub fn set_screen_size(&mut self, screen_size: Vec2, scale_factor: f32) {
        if !screen_size.is_finite() {
            return;
        }
        self.pan += (screen_size - self.screen_size).as_dvec2() * 0.5;
        self.screen_size = screen_size;
        self.scale_factor = sanitize_scale(scale_factor);
    }

    /// Size of the canvas render target in physical pixels (at least 1x1).
    pub fn physical_size(&self) -> (u32, u32) {
        let size = (self.screen_size * self.scale_factor)
            .round()
            .max(Vec2::ONE);
        // `as` saturates, so absurd sizes clamp instead of wrapping.
        (size.x as u32, size.y as u32)
    }

    /// The canvas rectangle in screen space: `(min, max)` in logical points.
    pub fn canvas_screen_rect(&self) -> (Vec2, Vec2) {
        (
            self.canvas_to_screen(Vec2::ZERO),
            self.canvas_to_screen(self.canvas_size),
        )
    }

    /// Matrix mapping canvas coordinates to wgpu clip space
    /// (x right, y up, `-1..=1` across the panel).
    pub fn view_projection(&self) -> Mat4 {
        let size = self.screen_size.max(Vec2::ONE).as_dvec2();
        let sx = 2.0 * self.zoom / size.x;
        let sy = -2.0 * self.zoom / size.y;
        let tx = 2.0 * self.pan.x / size.x - 1.0;
        let ty = 1.0 - 2.0 * self.pan.y / size.y;
        DMat4::from_cols(
            DVec4::new(sx, 0.0, 0.0, 0.0),
            DVec4::new(0.0, sy, 0.0, 0.0),
            DVec4::new(0.0, 0.0, 1.0, 0.0),
            DVec4::new(tx, ty, 0.0, 1.0),
        )
        .as_mat4()
    }
}

fn sanitize_scale(scale_factor: f32) -> f32 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport() -> Viewport {
        Viewport::new(Vec2::new(1920.0, 1080.0), Vec2::new(1000.0, 700.0), 2.0)
    }

    fn assert_close(a: Vec2, b: Vec2, eps: f32) {
        assert!((a - b).abs().max_element() <= eps, "{a} != {b} (eps {eps})");
    }

    #[test]
    fn round_trip_screen_canvas() {
        let mut v = viewport();
        v.zoom_about(Vec2::new(123.0, 456.0), 3.7);
        v.pan_by(Vec2::new(-41.5, 17.25));
        for p in [
            Vec2::ZERO,
            Vec2::new(10.0, 20.0),
            Vec2::new(-300.0, 900.0),
            Vec2::new(1920.0, 1080.0),
        ] {
            assert_close(v.screen_to_canvas(v.canvas_to_screen(p)), p, 1e-3);
            assert_close(v.canvas_to_screen(v.screen_to_canvas(p)), p, 1e-3);
        }
    }

    #[test]
    fn zoom_keeps_point_under_cursor() {
        let mut v = viewport();
        let cursor = Vec2::new(321.0, 210.0);
        let anchor = v.screen_to_canvas(cursor);
        v.zoom_about(cursor, 2.5);
        assert_close(v.screen_to_canvas(cursor), anchor, 1e-3);
        v.zoom_about(cursor, 0.1);
        assert_close(v.screen_to_canvas(cursor), anchor, 1e-3);
    }

    #[test]
    fn repeated_zooms_do_not_drift() {
        let mut v = viewport();
        let cursor = Vec2::new(777.0, 33.0);
        let anchor = v.screen_to_canvas_f64(cursor.as_dvec2());
        // Wheel-like steps in and out, including hitting both clamps.
        for _ in 0..200 {
            v.zoom_about(cursor, 1.1);
        }
        assert_eq!(v.zoom, MAX_ZOOM);
        for _ in 0..400 {
            v.zoom_about(cursor, 1.0 / 1.1);
        }
        assert_eq!(v.zoom, MIN_ZOOM);
        for i in 0..1000 {
            v.zoom_about(cursor, if i % 3 == 0 { 0.93 } else { 1.07 });
        }
        let error = (v.screen_to_canvas_f64(cursor.as_dvec2()) - anchor)
            .abs()
            .max_element();
        assert!(error < 1e-9, "anchor drifted by {error} canvas units");
        let screen = anchor * v.zoom + v.pan;
        assert!((screen - cursor.as_dvec2()).abs().max_element() < 1e-6);
    }

    #[test]
    fn zoom_is_clamped_and_bad_factors_ignored() {
        let mut v = viewport();
        v.zoom_about(Vec2::ZERO, 1e9);
        assert_eq!(v.zoom, MAX_ZOOM);
        v.zoom_about(Vec2::ZERO, 1e-9);
        assert_eq!(v.zoom, MIN_ZOOM);
        let before = v;
        v.zoom_about(Vec2::ZERO, f32::NAN);
        v.zoom_about(Vec2::ZERO, 0.0);
        v.zoom_about(Vec2::ZERO, -2.0);
        assert_eq!(v, before);
    }

    #[test]
    fn pan_moves_canvas_by_delta() {
        let mut v = viewport();
        let p = Vec2::new(50.0, 60.0);
        let before = v.canvas_to_screen(p);
        v.pan_by(Vec2::new(12.0, -7.0));
        assert_close(v.canvas_to_screen(p), before + Vec2::new(12.0, -7.0), 1e-4);
    }

    #[test]
    fn fit_centres_canvas_inside_margins() {
        let v = viewport();
        let (min, max) = v.canvas_screen_rect();
        assert!(min.x >= FIT_MARGIN - 1e-3 && min.y >= FIT_MARGIN - 1e-3);
        assert!(max.x <= 1000.0 - FIT_MARGIN + 1e-3 && max.y <= 700.0 - FIT_MARGIN + 1e-3);
        assert_close((min + max) * 0.5, Vec2::new(500.0, 350.0), 1e-3);
        // Width-limited: 1920 wide canvas into 1000 - 64 points.
        assert!((v.zoom - 936.0 / 1920.0).abs() < 1e-6);
    }

    #[test]
    fn resize_keeps_centre_and_reports_physical_size() {
        let mut v = viewport();
        let centre = v.screen_to_canvas(v.screen_size * 0.5);
        v.set_screen_size(Vec2::new(1200.5, 640.0), 1.5);
        assert_close(v.screen_to_canvas(v.screen_size * 0.5), centre, 1e-3);
        assert_eq!(v.physical_size(), (1801, 960));
        v.set_screen_size(Vec2::ZERO, 0.0);
        assert_eq!(v.scale_factor, 1.0);
        assert_eq!(v.physical_size(), (1, 1));
    }

    #[test]
    fn view_projection_maps_to_clip_space() {
        let mut v = viewport();
        v.zoom_about(Vec2::new(10.0, 10.0), 1.8);
        for p in [Vec2::ZERO, Vec2::new(400.0, 300.0), v.canvas_size] {
            let screen = v.canvas_to_screen(p);
            let expected = Vec2::new(
                screen.x / v.screen_size.x * 2.0 - 1.0,
                1.0 - screen.y / v.screen_size.y * 2.0,
            );
            let clip = v.view_projection() * p.extend(0.0).extend(1.0);
            assert_close(Vec2::new(clip.x, clip.y), expected, 1e-5);
            assert_eq!(clip.w, 1.0);
        }
    }
}
