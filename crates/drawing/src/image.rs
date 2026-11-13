//! Imported pictures (PNG/JPEG) placed in a vector drawing (ADR-015).
//!
//! An [`Image`] keeps the file's original encoded bytes, never decoded
//! pixels: they are small, they save to the project file as they are, and the
//! renderer decodes them once into a GPU texture. Where the picture sits is an
//! affine [`Image::transform`] from the unit square (`0..1` on both axes, the
//! picture's top-left at the origin) to document coordinates, so moving,
//! scaling and rotating it with the selection tools is a matrix product and
//! never resamples the pixels.

use crate::stroke::Placement;
use animation_core::{ImageId, Vec2};
use glam::Affine2;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::sync::Arc;

/// The encoded file of an imported picture and its pixel size.
///
/// Cloning is cheap: the bytes are shared, so undo history and duplicated
/// frames don't copy them.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageData {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The PNG or JPEG file, as imported. Saved as base64 text.
    #[serde(serialize_with = "to_base64", deserialize_with = "from_base64")]
    pub bytes: Arc<[u8]>,
}

impl ImageData {
    /// Wraps an encoded PNG/JPEG file of `width` x `height` pixels.
    pub fn new(width: u32, height: u32, bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            width,
            height,
            bytes: bytes.into(),
        }
    }

    /// Width over height (1 for a degenerate size).
    pub fn aspect(&self) -> f32 {
        if self.width == 0 || self.height == 0 {
            1.0
        } else {
            self.width as f32 / self.height as f32
        }
    }
}

impl std::fmt::Debug for ImageData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageData")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// A picture placed in a drawing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Image {
    /// Stable identifier.
    pub id: ImageId,
    /// The picture.
    pub data: ImageData,
    /// Maps the unit square (picture top-left at the origin, y down) to
    /// document coordinates.
    pub transform: Affine2,
    /// Opacity multiplier in `0.0..=1.0`.
    pub opacity: f32,
    /// Draw-order tier; absent in files means [`Placement::Normal`]. Within a
    /// tier, images paint under fills and strokes.
    #[serde(default, skip_serializing_if = "Placement::is_normal")]
    pub placement: Placement,
}

impl Image {
    /// An image with a fresh ID covering the rectangle `min..min + size`
    /// (document coordinates), fully opaque, in the normal tier.
    pub fn new(data: ImageData, min: Vec2, size: Vec2) -> Self {
        Self {
            id: ImageId::new(),
            data,
            transform: Affine2::from_scale_angle_translation(size, 0.0, min),
            opacity: 1.0,
            placement: Placement::Normal,
        }
    }

    /// The four corners in document coordinates: top-left, top-right,
    /// bottom-right, bottom-left of the picture.
    pub fn corners(&self) -> [Vec2; 4] {
        [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ]
        .map(|p| self.transform.transform_point2(p))
    }

    /// Axis-aligned bounds of the corners.
    pub fn bounds(&self) -> (Vec2, Vec2) {
        let c = self.corners();
        c.iter()
            .skip(1)
            .fold((c[0], c[0]), |(lo, hi), &p| (lo.min(p), hi.max(p)))
    }

    /// Whether `point` (document coordinates) is on the picture. A collapsed
    /// (zero-area) image contains nothing.
    pub fn contains(&self, point: Vec2) -> bool {
        if self.transform.matrix2.determinant().abs() < f32::EPSILON {
            return false;
        }
        let local = self.transform.inverse().transform_point2(point);
        (0.0..=1.0).contains(&local.x) && (0.0..=1.0).contains(&local.y)
    }

    /// Applies `transform` on top of the current placement.
    pub fn apply(&mut self, transform: &Affine2) {
        self.transform = *transform * self.transform;
    }

    /// A transformed copy, keeping the same ID.
    pub fn transformed(&self, transform: &Affine2) -> Self {
        let mut out = self.clone();
        out.apply(transform);
        out
    }

    /// Moves the picture by `delta`.
    pub fn translate(&mut self, delta: Vec2) {
        self.transform.translation += delta;
    }

    /// A copy moved by `delta`, keeping the same ID.
    pub fn translated(&self, delta: Vec2) -> Self {
        let mut moved = self.clone();
        moved.translate(delta);
        moved
    }

    /// The same image in `placement`'s tier.
    pub fn with_placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(BASE64[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decodes standard base64; whitespace is skipped, padding is optional.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let digits: Vec<u8> = text
        .bytes()
        .filter(|c| !c.is_ascii_whitespace())
        .take_while(|&c| c != b'=')
        .collect();
    if digits.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(digits.len() / 4 * 3 + 2);
    for chunk in digits.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value(c)? << (18 - 6 * i);
        }
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Some(out)
}

fn to_base64<S: Serializer>(bytes: &Arc<[u8]>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&encode_base64(bytes))
}

fn from_base64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<[u8]>, D::Error> {
    let text = String::deserialize(deserializer)?;
    decode_base64(&text)
        .map(Arc::from)
        .ok_or_else(|| serde::de::Error::custom("image bytes are not valid base64"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1e-4
    }

    fn picture() -> Image {
        Image::new(
            ImageData::new(4, 2, vec![1, 2, 3]),
            Vec2::new(10.0, 20.0),
            Vec2::new(40.0, 20.0),
        )
    }

    #[test]
    fn base64_roundtrips_every_length() {
        for len in 0..10 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 250) as u8).collect();
            let text = encode_base64(&bytes);
            assert_eq!(text.len() % 4, 0);
            assert_eq!(decode_base64(&text).unwrap(), bytes, "length {len}");
        }
        assert_eq!(encode_base64(b"Man"), "TWFu");
        assert_eq!(encode_base64(b"Ma"), "TWE=");
        assert_eq!(decode_base64("TWE").unwrap(), b"Ma");
        assert!(decode_base64("T").is_none());
        assert!(decode_base64("T*==").is_none());
    }

    #[test]
    fn placement_and_hit_testing() {
        let img = picture();
        assert_eq!(img.bounds(), (Vec2::new(10.0, 20.0), Vec2::new(50.0, 40.0)));
        assert!(close(img.corners()[2], Vec2::new(50.0, 40.0)));
        assert!(img.contains(Vec2::new(30.0, 30.0)));
        assert!(!img.contains(Vec2::new(51.0, 30.0)));
    }

    #[test]
    fn transforms_compose_on_the_placement() {
        let mut img = picture();
        img.translate(Vec2::new(5.0, 5.0));
        assert_eq!(img.bounds().0, Vec2::new(15.0, 25.0));
        let quarter = std::f32::consts::FRAC_PI_2;
        let turned = img.transformed(&Affine2::from_angle(quarter));
        assert_eq!(turned.id, img.id);
        // The top-left corner (15, 25) turns to (-25, 15).
        assert!(close(turned.corners()[0], Vec2::new(-25.0, 15.0)));
        let squashed = img.transformed(&Affine2::from_scale(Vec2::new(1.0, 0.0)));
        assert!(!squashed.contains(squashed.corners()[0]));
    }

    #[test]
    fn serde_keeps_bytes_and_placement() {
        let img = picture().with_placement(Placement::Front);
        let json = serde_json_like(&img);
        assert!(json.contains("\"AQID\""), "{json}");
        let back: Image = serde_json::from_str(&json).unwrap();
        assert_eq!(back, img);
        assert_eq!(img.data.aspect(), 2.0);
    }

    fn serde_json_like(img: &Image) -> String {
        serde_json::to_string(img).unwrap()
    }
}
