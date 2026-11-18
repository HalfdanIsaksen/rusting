//! Stamp brushes (ADR-022).
//!
//! A brushed stroke is still vector data: the same points and pressure as a
//! solid stroke. Only its look differs. The renderer stamps the brush's tip
//! picture along the path, at the positions, sizes, angles and opacities
//! [`stamps`] computes here, and modulates the result with an optional paper
//! grain fixed in canvas space.
//!
//! [`stamps`] is pure and deterministic: its jitter is seeded by the
//! [`StrokeId`](animation_core::StrokeId), so a stroke looks the same in the
//! editor, the preview window and every export. Every stamp stays inside the
//! circle of radius `width * pressure / 2` around its point on the path, the
//! same footprint hit testing, erasing and the fill bucket use for the stroke.
//! Tips are expected to fade to nothing at the edge of their inscribed circle,
//! so rotating a stamp never pushes paint outside it.

use crate::image::ImageData;
use crate::stroke::Stroke;
use animation_core::Vec2;
use serde::{Deserialize, Serialize};

/// The most stamps one stroke produces. Longer strokes space theirs out.
pub const MAX_STAMPS: usize = 200_000;

/// The smallest distance between two stamps, in document units.
const MIN_STEP: f32 = 0.05;

/// How a brush lays its stamps. Every field has a default, so a
/// `brush.json` only needs the ones it changes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrushSettings {
    /// Distance between stamps as a fraction of the stamp size, in
    /// `0.01..=4.0`.
    pub spacing: f32,
    /// Opacity of one stamp in `0.0..=1.0`. Overlapping stamps of a stroke
    /// build up, so a low flow gives a soft, layered line.
    pub flow: f32,
    /// How much light pressure lowers a stamp's opacity, in `0.0..=1.0`
    /// (0: none, 1: opacity equals pressure). Size always follows pressure,
    /// as for solid strokes.
    pub opacity_pressure: f32,
    /// Tip rotation in degrees.
    pub angle: f32,
    /// Random rotation per stamp, in `0.0..=1.0` (1: any angle).
    pub angle_jitter: f32,
    /// Turn the tip with the direction of the stroke (for flat tips).
    pub follow_direction: bool,
    /// Random shrink per stamp, in `0.0..=1.0`.
    pub size_jitter: f32,
    /// Random opacity drop per stamp, in `0.0..=1.0`.
    pub opacity_jitter: f32,
    /// Random sideways offset, in `0.0..=1.0` of the stroke's half width.
    /// Stamps shrink by the same fraction so they stay inside the stroke.
    pub scatter: f32,
    /// Size of one grain pixel in document units, in `0.05..=64.0`.
    pub grain_scale: f32,
    /// How much of the paper grain stays empty, in `0.0..=1.0`. The paper
    /// takes paint only where the grain is brighter than a threshold that
    /// rises with this (0: everywhere; 1: only the brighter half), so the
    /// valleys stay empty however many stamps build up over them.
    pub grain_strength: f32,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            spacing: 0.15,
            flow: 1.0,
            opacity_pressure: 0.0,
            angle: 0.0,
            angle_jitter: 0.0,
            follow_direction: false,
            size_jitter: 0.0,
            opacity_jitter: 0.0,
            scatter: 0.0,
            grain_scale: 1.0,
            grain_strength: 0.0,
        }
    }
}

/// A brush setting that is not finite or out of range.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("brush setting `{field}` is {value}, outside {min}..={max}")]
pub struct BrushSettingsError {
    /// The field name, as in `brush.json`.
    pub field: &'static str,
    /// The offending value.
    pub value: f32,
    /// Smallest allowed value.
    pub min: f32,
    /// Largest allowed value.
    pub max: f32,
}

impl BrushSettings {
    /// Each numeric field with its allowed range.
    fn ranges(&self) -> [(&'static str, f32, f32, f32); 10] {
        [
            ("spacing", self.spacing, 0.01, 4.0),
            ("flow", self.flow, 0.0, 1.0),
            ("opacity_pressure", self.opacity_pressure, 0.0, 1.0),
            ("angle", self.angle, -360.0, 360.0),
            ("angle_jitter", self.angle_jitter, 0.0, 1.0),
            ("size_jitter", self.size_jitter, 0.0, 1.0),
            ("opacity_jitter", self.opacity_jitter, 0.0, 1.0),
            ("scatter", self.scatter, 0.0, 1.0),
            ("grain_scale", self.grain_scale, 0.05, 64.0),
            ("grain_strength", self.grain_strength, 0.0, 1.0),
        ]
    }

    /// Checks every field is finite and in range.
    pub fn validate(&self) -> Result<(), BrushSettingsError> {
        for (field, value, min, max) in self.ranges() {
            if !value.is_finite() || value < min || value > max {
                return Err(BrushSettingsError {
                    field,
                    value,
                    min,
                    max,
                });
            }
        }
        Ok(())
    }

    /// The settings with every field forced into range (non-finite values
    /// become the default).
    pub fn clamped(mut self) -> Self {
        let d = Self::default();
        let fix = |v: f32, def: f32, min: f32, max: f32| {
            if v.is_finite() {
                v.clamp(min, max)
            } else {
                def
            }
        };
        self.spacing = fix(self.spacing, d.spacing, 0.01, 4.0);
        self.flow = fix(self.flow, d.flow, 0.0, 1.0);
        self.opacity_pressure = fix(self.opacity_pressure, d.opacity_pressure, 0.0, 1.0);
        self.angle = fix(self.angle, d.angle, -360.0, 360.0);
        self.angle_jitter = fix(self.angle_jitter, d.angle_jitter, 0.0, 1.0);
        self.size_jitter = fix(self.size_jitter, d.size_jitter, 0.0, 1.0);
        self.opacity_jitter = fix(self.opacity_jitter, d.opacity_jitter, 0.0, 1.0);
        self.scatter = fix(self.scatter, d.scatter, 0.0, 1.0);
        self.grain_scale = fix(self.grain_scale, d.grain_scale, 0.05, 64.0);
        self.grain_strength = fix(self.grain_strength, d.grain_strength, 0.0, 1.0);
        self
    }
}

/// A brush: its tip picture, optional paper grain and stamp settings. Stored
/// in the asset library (`AssetKind::Brush`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BrushData {
    /// The tip, a PNG whose brightness (red channel) is the stamp's
    /// coverage: white paints, black leaves the paper.
    pub tip: ImageData,
    /// A tiling PNG whose brightness is how much paint the paper takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grain: Option<ImageData>,
    /// How stamps are laid.
    #[serde(default)]
    pub settings: BrushSettings,
}

/// One stamp of the tip, in the stroke's (document or layer) coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stamp {
    /// Centre of the stamp.
    pub pos: Vec2,
    /// Side of the stamp's square (its tip's inscribed circle's diameter).
    pub size: f32,
    /// Rotation in radians.
    pub angle: f32,
    /// Opacity in `0.0..=1.0`, before the stroke's colour opacity.
    pub opacity: f32,
}

/// A small deterministic random source (SplitMix64).
struct Rng(u64);

impl Rng {
    fn new(seed: u128) -> Self {
        Self((seed as u64) ^ ((seed >> 64) as u64) ^ 0x9E37_79B9_7F4A_7C15)
    }

    /// A value in `0.0..1.0`.
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 24) as f32
    }

    /// A value in `-1.0..1.0`.
    fn signed(&mut self) -> f32 {
        self.next() * 2.0 - 1.0
    }
}

/// Clamped, finite pressure (non-finite counts as full pressure).
fn pressure(p: f32) -> f32 {
    if p.is_finite() {
        p.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// The stamps that draw `stroke` with `settings`, in drawing order.
///
/// Stamps are placed every `spacing × width × pressure` along the path
/// (never closer than a small minimum, and at most [`MAX_STAMPS`]). A stroke
/// with one point, or with all its points in one place, gets one stamp.
pub fn stamps(stroke: &Stroke, settings: &BrushSettings) -> Vec<Stamp> {
    let settings = settings.clamped();
    let width = stroke.style.width.max(0.0);
    let points: Vec<(Vec2, f32)> = stroke
        .points
        .iter()
        .filter(|p| p.pos.is_finite())
        .map(|p| (p.pos, pressure(p.pressure)))
        .collect();
    let Some(&(first, first_pressure)) = points.first() else {
        return Vec::new();
    };
    if width <= 0.0 {
        return Vec::new();
    }
    let length: f32 = points.windows(2).map(|w| w[0].0.distance(w[1].0)).sum();
    let mut rng = Rng::new(stroke.id.as_uuid().as_u128());
    let mut out = Vec::new();

    // Grow the step if the stroke would need more than MAX_STAMPS.
    let min_step = (length / MAX_STAMPS as f32).max(MIN_STEP);
    let step_at = |p: f32| (settings.spacing * width * p).max(min_step);

    let mut stamp = |pos: Vec2, p: f32, direction: Vec2, out: &mut Vec<Stamp>| {
        if p <= 0.0 {
            return;
        }
        let radius = width * p * 0.5;
        let offset = settings.scatter * radius * rng.signed();
        let normal = direction.perp();
        let size = width * p * (1.0 - settings.scatter) * (1.0 - settings.size_jitter * rng.next());
        let mut angle = settings.angle.to_radians();
        if settings.follow_direction {
            angle += direction.y.atan2(direction.x);
        }
        angle += settings.angle_jitter * std::f32::consts::PI * rng.signed();
        let opacity = settings.flow
            * (1.0 - settings.opacity_pressure * (1.0 - p))
            * (1.0 - settings.opacity_jitter * rng.next());
        if size > 0.0 && opacity > 0.0 {
            out.push(Stamp {
                pos: pos + normal * offset,
                size,
                angle,
                opacity,
            });
        }
    };

    if length <= f32::EPSILON {
        stamp(first, first_pressure, Vec2::X, &mut out);
        return out;
    }

    // Distance still to travel before the next stamp.
    let mut to_next = 0.0;
    for w in points.windows(2) {
        let ((a, pa), (b, pb)) = (w[0], w[1]);
        let seg = a.distance(b);
        if seg <= f32::EPSILON {
            continue;
        }
        let dir = (b - a) / seg;
        let mut along = to_next;
        while along <= seg && out.len() < MAX_STAMPS {
            let t = along / seg;
            let p = pa + (pb - pa) * t;
            stamp(a + (b - a) * t, p, dir, &mut out);
            along += step_at(p);
        }
        to_next = along - seg;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stroke::{BrushStyle, StrokePoint};

    fn line(width: f32, len: f32) -> Stroke {
        Stroke::new(
            vec![
                StrokePoint::new(Vec2::ZERO),
                StrokePoint::new(Vec2::new(len, 0.0)),
            ],
            BrushStyle {
                width,
                ..BrushStyle::default()
            },
        )
    }

    #[test]
    fn stamps_are_spaced_by_spacing_times_size() {
        let s = BrushSettings {
            spacing: 0.5,
            ..BrushSettings::default()
        };
        let st = stamps(&line(10.0, 100.0), &s);
        // Every 5 units from 0 to 100 inclusive.
        assert_eq!(st.len(), 21);
        for pair in st.windows(2) {
            assert!((pair[1].pos.x - pair[0].pos.x - 5.0).abs() < 1e-3);
        }
    }

    #[test]
    fn single_point_gets_one_stamp() {
        let mut s = line(8.0, 0.0);
        s.points.truncate(1);
        let st = stamps(&s, &BrushSettings::default());
        assert_eq!(st.len(), 1);
        assert_eq!(st[0].size, 8.0);
    }

    #[test]
    fn jitter_is_deterministic_per_stroke() {
        let s = BrushSettings {
            angle_jitter: 1.0,
            size_jitter: 0.5,
            scatter: 0.3,
            opacity_jitter: 0.4,
            ..BrushSettings::default()
        };
        let a = line(6.0, 50.0);
        assert_eq!(stamps(&a, &s), stamps(&a.clone(), &s));
        let b = line(6.0, 50.0); // fresh id
        assert_ne!(stamps(&a, &s), stamps(&b, &s));
    }

    #[test]
    fn stamps_stay_inside_the_stroke_footprint() {
        let s = BrushSettings {
            scatter: 0.8,
            size_jitter: 0.5,
            spacing: 0.1,
            ..BrushSettings::default()
        };
        let mut stroke = line(10.0, 80.0);
        stroke.points[1].pressure = 0.3;
        for st in stamps(&stroke, &s) {
            let t = st.pos.x / 80.0;
            let p = 1.0 + (0.3 - 1.0) * t;
            let radius = 10.0 * p * 0.5;
            let reach = st.pos.y.abs() + st.size * 0.5;
            assert!(reach <= radius + 1e-3, "{reach} > {radius}");
        }
    }

    #[test]
    fn opacity_follows_flow_and_pressure() {
        let s = BrushSettings {
            flow: 0.5,
            opacity_pressure: 1.0,
            ..BrushSettings::default()
        };
        let mut stroke = line(4.0, 0.0);
        stroke.points.truncate(1);
        stroke.points[0].pressure = 0.5;
        let st = stamps(&stroke, &s);
        assert!((st[0].opacity - 0.25).abs() < 1e-6);
    }

    #[test]
    fn stamp_count_is_capped() {
        let s = BrushSettings {
            spacing: 0.01,
            ..BrushSettings::default()
        };
        let st = stamps(&line(1.0, 1.0e7), &s);
        assert!(st.len() <= MAX_STAMPS);
    }

    #[test]
    fn validate_and_clamp() {
        assert!(BrushSettings::default().validate().is_ok());
        let bad = BrushSettings {
            flow: 2.0,
            spacing: f32::NAN,
            ..BrushSettings::default()
        };
        assert!(bad.validate().is_err());
        let fixed = bad.clamped();
        assert!(fixed.validate().is_ok());
        assert_eq!(fixed.flow, 1.0);
        assert_eq!(fixed.spacing, BrushSettings::default().spacing);
    }

    #[test]
    fn settings_json_fills_defaults() {
        let s: BrushSettings = serde_json::from_str(r#"{"flow":0.3}"#).unwrap();
        assert_eq!(s.flow, 0.3);
        assert_eq!(s.spacing, BrushSettings::default().spacing);
    }
}
