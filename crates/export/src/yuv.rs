//! RGBA to YUV 4:2:0 (I420) conversion for the H.264 encoder.
//!
//! Uses BT.709 coefficients with limited ("TV") range, which is what the
//! encoder signals in the stream's VUI and what players assume for HD video.

use crate::RgbaImage;

/// A planar I420 frame: full-size Y, then quarter-size U and V.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Yuv420 {
    /// Width in pixels (even).
    pub width: u32,
    /// Height in pixels (even).
    pub height: u32,
    /// `width * height` luma samples.
    pub y: Vec<u8>,
    /// `width/2 * height/2` blue-difference samples.
    pub u: Vec<u8>,
    /// `width/2 * height/2` red-difference samples.
    pub v: Vec<u8>,
}

impl Yuv420 {
    /// The planes concatenated (Y, U, V), as the encoder takes them.
    pub fn into_i420(self) -> Vec<u8> {
        let mut out = self.y;
        out.extend_from_slice(&self.u);
        out.extend_from_slice(&self.v);
        out
    }
}

/// `n` rounded up to the next even number (at least 2).
pub fn even(n: u32) -> u32 {
    (n + (n & 1)).max(2)
}

/// Copies `image` into an even-sized one, repeating the last column and row
/// where a pixel is added. Images that are already even are returned as is.
pub fn pad_to_even(image: &RgbaImage) -> std::borrow::Cow<'_, RgbaImage> {
    let (w, h) = (even(image.width), even(image.height));
    if (w, h) == (image.width, image.height) {
        return std::borrow::Cow::Borrowed(image);
    }
    let mut pixels = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        let sy = y.min(image.height.saturating_sub(1));
        for x in 0..w {
            let sx = x.min(image.width.saturating_sub(1));
            pixels.extend_from_slice(&image.pixel(sx, sy).unwrap_or([0, 0, 0, 255]));
        }
    }
    std::borrow::Cow::Owned(RgbaImage {
        width: w,
        height: h,
        pixels,
    })
}

/// BT.709 luma weights.
const KR: f32 = 0.2126;
const KB: f32 = 0.0722;
const KG: f32 = 1.0 - KR - KB;

fn luma(r: f32, g: f32, b: f32) -> f32 {
    KR * r + KG * g + KB * b
}

fn to_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// Converts an opaque, even-sized image to limited-range BT.709 I420. Alpha
/// is ignored. Each chroma sample averages a 2x2 block. Returns `None` if a
/// dimension is odd or the buffer is the wrong size.
pub fn rgba_to_yuv420(image: &RgbaImage) -> Option<Yuv420> {
    let (w, h) = (image.width as usize, image.height as usize);
    if w % 2 != 0 || h % 2 != 0 || image.pixels.len() != w * h * 4 {
        return None;
    }
    let px = |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        let p = &image.pixels[i..i + 3];
        (
            f32::from(p[0]) / 255.0,
            f32::from(p[1]) / 255.0,
            f32::from(p[2]) / 255.0,
        )
    };
    let mut y_plane = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = px(x, y);
            y_plane.push(to_u8(16.0 + 219.0 * luma(r, g, b)));
        }
    }
    let (cw, ch) = (w / 2, h / 2);
    let mut u_plane = Vec::with_capacity(cw * ch);
    let mut v_plane = Vec::with_capacity(cw * ch);
    for cy in 0..ch {
        for cx in 0..cw {
            let (mut r, mut g, mut b) = (0.0, 0.0, 0.0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = px(cx * 2 + dx, cy * 2 + dy);
                r += p.0;
                g += p.1;
                b += p.2;
            }
            let (r, g, b) = (r / 4.0, g / 4.0, b / 4.0);
            let l = luma(r, g, b);
            u_plane.push(to_u8(128.0 + 224.0 * (b - l) / (2.0 * (1.0 - KB))));
            v_plane.push(to_u8(128.0 + 224.0 * (r - l) / (2.0 * (1.0 - KR))));
        }
    }
    Some(Yuv420 {
        width: image.width,
        height: image.height,
        y: y_plane,
        u: u_plane,
        v: v_plane,
    })
}

/// Converts one limited-range BT.709 sample back to sRGB bytes (for tests
/// and checks of decoded video).
pub fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let l = (f32::from(y) - 16.0) / 219.0;
    let cb = (f32::from(u) - 128.0) / 224.0;
    let cr = (f32::from(v) - 128.0) / 224.0;
    let r = l + 2.0 * (1.0 - KR) * cr;
    let b = l + 2.0 * (1.0 - KB) * cb;
    let g = (l - KR * r - KB * b) / KG;
    [to_u8(r * 255.0), to_u8(g * 255.0), to_u8(b * 255.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_colours_convert_to_reference_values() {
        // Reference limited-range BT.709 values.
        for (rgb, yuv) in [
            ([0, 0, 0], (16, 128, 128)),
            ([255, 255, 255], (235, 128, 128)),
            ([255, 0, 0], (63, 102, 240)),
            ([0, 255, 0], (173, 42, 26)),
            ([0, 0, 255], (32, 240, 118)),
        ] {
            let img = RgbaImage::filled(2, 2, [rgb[0], rgb[1], rgb[2], 255]);
            let out = rgba_to_yuv420(&img).unwrap();
            assert_eq!(out.y, vec![yuv.0; 4], "{rgb:?}");
            assert_eq!((out.u[0], out.v[0]), (yuv.1, yuv.2), "{rgb:?}");
        }
    }

    #[test]
    fn round_trip_is_close() {
        for rgb in [[12, 200, 77], [250, 128, 3], [90, 90, 90]] {
            let img = RgbaImage::filled(2, 2, [rgb[0], rgb[1], rgb[2], 255]);
            let out = rgba_to_yuv420(&img).unwrap();
            let back = yuv_to_rgb(out.y[0], out.u[0], out.v[0]);
            for (a, b) in back.iter().zip(rgb) {
                assert!(a.abs_diff(b) <= 2, "{back:?} vs {rgb:?}");
            }
        }
    }

    #[test]
    fn chroma_averages_2x2_blocks_and_planes_have_i420_sizes() {
        let mut img = RgbaImage::filled(4, 2, [255, 255, 255, 255]);
        // Left block: two red, two blue pixels.
        img.pixels[0..4].copy_from_slice(&[255, 0, 0, 255]);
        img.pixels[16..20].copy_from_slice(&[0, 0, 255, 255]);
        let out = rgba_to_yuv420(&img).unwrap();
        assert_eq!((out.y.len(), out.u.len(), out.v.len()), (8, 2, 2));
        assert_eq!((out.u[1], out.v[1]), (128, 128));
        assert_ne!(out.u[0], 128);
        assert_eq!(out.into_i420().len(), 12);
    }

    #[test]
    fn odd_sizes_are_padded_by_repeating_the_edge() {
        assert!(rgba_to_yuv420(&RgbaImage::filled(3, 2, [0; 4])).is_none());
        let mut img = RgbaImage::filled(3, 3, [1, 2, 3, 255]);
        img.pixels[32..36].copy_from_slice(&[9, 9, 9, 255]); // (2, 2)
        let padded = pad_to_even(&img);
        assert_eq!((padded.width, padded.height), (4, 4));
        assert_eq!(padded.pixel(3, 3), Some([9, 9, 9, 255]));
        assert_eq!(padded.pixel(3, 0), Some([1, 2, 3, 255]));
        assert!(rgba_to_yuv420(&padded).is_some());
        let even_img = RgbaImage::filled(2, 4, [0; 4]);
        assert!(matches!(
            pad_to_even(&even_img),
            std::borrow::Cow::Borrowed(_)
        ));
        assert_eq!((even(1), even(2), even(7)), (2, 2, 8));
    }
}
