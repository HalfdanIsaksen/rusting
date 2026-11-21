//! Making brush pictures (ADR-024): a tip or a paper grain from rendered
//! artwork or an imported picture, and generated tips and grains.
//!
//! A tip's brightness is its coverage, and it must fade to nothing at its
//! inscribed circle (ADR-022), so stamps never paint outside the stroke.
//! Artwork follows the Photoshop convention: dark marks paint. A picture
//! drawn the other way round (light marks on a dark, opaque background, as
//! the built-in tips are) is detected by its border and read as brightness.
//!
//! A grain's brightness is how much paint the paper takes, so dark marks
//! drawn for a grain become pits that stay empty.

use document::drawing::ImageData;
use image::{GrayImage, Luma};
use renderer::OffscreenRenderer;
use std::io::Cursor;
use ui::BrushCapture;

/// The longest side artwork is rendered at for a brush picture, in pixels.
const CAPTURE_SIDE: f32 = 2048.0;

/// Renders a capture's artwork and turns it into the brush picture its
/// target asks for.
pub fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    offscreen: &mut OffscreenRenderer,
    capture: &BrushCapture,
) -> Result<ImageData, String> {
    let canvas = capture.document.canvas;
    let longest = canvas.width.max(canvas.height).max(1) as f32;
    let scale = (CAPTURE_SIDE / longest).clamp(0.25, 4.0);
    let image = crate::export::render_frame(
        device,
        queue,
        offscreen,
        &capture.document,
        capture.frame,
        scale,
        None,
    )
    .map_err(|e| e.to_string())?;
    if capture.target.is_grain() {
        grain_from_rgba(image.width, image.height, &image.pixels)
    } else {
        tip_from_rgba(image.width, image.height, &image.pixels)
    }
}

/// The largest side of a tip made from artwork, in pixels.
pub const TIP_SIDE: u32 = 256;

/// The side of a generated tip, in pixels.
#[allow(dead_code)] // TODO(brush-editor): used by the Brush Editor window.
const GENERATED_TIP: u32 = 128;

/// The side of a generated grain, in pixels.
#[allow(dead_code)] // TODO(brush-editor): used by the Brush Editor window.
const GENERATED_GRAIN: u32 = 256;

/// The largest side of a grain made from artwork (the `.rustbrush` limit).
const MAX_GRAIN_SIDE: u32 = crate::brushes::MAX_SIDE;

/// Where a tip starts fading, as a fraction of its inscribed radius. The
/// artwork is scaled to fit inside this circle.
const FADE_START: f32 = 0.92;

/// Coverage below this counts as empty when cropping.
const EMPTY: f32 = 1.0 / 255.0;

/// Brightness of a straight-alpha sRGB pixel, `0.0..=1.0` (Rec. 709 weights
/// on the encoded values).
fn luminance(p: &[u8; 4]) -> f32 {
    (0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])) / 255.0
}

fn alpha(p: &[u8; 4]) -> f32 {
    f32::from(p[3]) / 255.0
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Checks `rgba` holds `width * height` pixels.
fn check(width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("The picture has no pixels.".into());
    }
    if rgba.len() as u64 != u64::from(width) * u64::from(height) * 4 {
        return Err("The picture's pixels don't match its size.".into());
    }
    Ok(())
}

/// The smallest rectangle `(x0, y0, x1, y1)` (exclusive end) holding every
/// value above [`EMPTY`], or `None` if there is none.
fn bounds(values: &[f32], width: u32) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for (i, &v) in values.iter().enumerate() {
        if v <= EMPTY {
            continue;
        }
        let (x, y) = ((i as u32) % width, (i as u32) / width);
        b = Some(match b {
            None => (x, y, x + 1, y + 1),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)),
        });
    }
    b
}

/// Encodes a gray picture as a PNG brush picture.
fn encode(img: &GrayImage) -> ImageData {
    let mut png = Vec::new();
    // Writing a valid in-memory picture to a Vec can't fail.
    let _ = img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png);
    ImageData::new(img.width(), img.height(), png)
}

/// Fades `img` to zero at its inscribed circle.
fn fade_edge(img: &mut GrayImage) {
    let side = img.width() as f32;
    let centre = side / 2.0;
    for (x, y, p) in img.enumerate_pixels_mut() {
        let dx = (x as f32 + 0.5 - centre) / centre;
        let dy = (y as f32 + 0.5 - centre) / centre;
        let r = (dx * dx + dy * dy).sqrt();
        let keep = 1.0 - smooth(((r - FADE_START) / (1.0 - FADE_START)).clamp(0.0, 1.0));
        p.0[0] = (f32::from(p.0[0]) * keep).round() as u8;
    }
}

/// A tip from straight-alpha sRGB pixels (rendered artwork or a decoded
/// picture). Dark marks paint (light ones, if the picture is light on an
/// opaque dark background). The marks are cropped, centred and scaled so
/// they fit inside the tip's circle, at most [`TIP_SIDE`] pixels across.
pub fn tip_from_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<ImageData, String> {
    check(width, height, rgba)?;
    let px = || rgba.as_chunks::<4>().0.iter();
    let mut coverage: Vec<f32> = px().map(|p| alpha(p) * (1.0 - luminance(p))).collect();
    // A light-on-dark picture: its border is mostly "paint" when read the
    // usual way round, so read brightness instead.
    let border: Vec<f32> = coverage
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            let (x, y) = ((*i as u32) % width, (*i as u32) / width);
            x == 0 || y == 0 || x + 1 == width || y + 1 == height
        })
        .map(|(_, &c)| c)
        .collect();
    if border.iter().sum::<f32>() / border.len() as f32 > 0.5 {
        coverage = px().map(|p| alpha(p) * luminance(p)).collect();
    }
    let (x0, y0, x1, y1) =
        bounds(&coverage, width).ok_or("There is nothing to make a brush tip from.")?;
    let (w, h) = ((x1 - x0) as f32, (y1 - y0) as f32);
    // The square whose fade circle holds the marks' bounding box.
    let side = ((w * w + h * h).sqrt() / FADE_START).ceil() as u32 + 2;
    let mut square = GrayImage::new(side, side);
    let (ox, oy) = (
        (side as f32 - w) / 2.0 - x0 as f32,
        (side as f32 - h) / 2.0 - y0 as f32,
    );
    for y in y0..y1 {
        for x in x0..x1 {
            let c = coverage[(y * width + x) as usize];
            let (sx, sy) = (
                (x as f32 + ox).round() as u32,
                (y as f32 + oy).round() as u32,
            );
            if sx < side && sy < side {
                square.put_pixel(sx, sy, Luma([(c.clamp(0.0, 1.0) * 255.0).round() as u8]));
            }
        }
    }
    let out = side.clamp(16, TIP_SIDE);
    let mut tip = if out == side {
        square
    } else {
        image::imageops::resize(&square, out, out, image::imageops::FilterType::Triangle)
    };
    fade_edge(&mut tip);
    Ok(encode(&tip))
}

/// A paper grain from straight-alpha sRGB pixels: the artwork over white
/// paper, cropped to what is drawn and at most [`MAX_GRAIN_SIDE`] across.
/// Dark marks become pits that stay empty.
pub fn grain_from_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<ImageData, String> {
    check(width, height, rgba)?;
    let pixels = rgba.as_chunks::<4>().0;
    let drawn: Vec<f32> = pixels.iter().map(alpha).collect();
    let (x0, y0, x1, y1) =
        bounds(&drawn, width).ok_or("There is nothing to make a paper grain from.")?;
    let mut grain = GrayImage::from_fn(x1 - x0, y1 - y0, |x, y| {
        let p = &pixels[((y + y0) * width + x + x0) as usize];
        let a = alpha(p);
        Luma([((a * luminance(p) + (1.0 - a)) * 255.0).round() as u8])
    });
    let (w, h) = grain.dimensions();
    if w.max(h) > MAX_GRAIN_SIDE {
        let scale = MAX_GRAIN_SIDE as f32 / w.max(h) as f32;
        let (nw, nh) = (
            ((w as f32 * scale) as u32).clamp(1, MAX_GRAIN_SIDE),
            ((h as f32 * scale) as u32).clamp(1, MAX_GRAIN_SIDE),
        );
        grain = image::imageops::resize(&grain, nw, nh, image::imageops::FilterType::Triangle);
    }
    Ok(encode(&grain))
}

/// Decodes a PNG/JPEG brush picture to straight-alpha RGBA.
#[allow(dead_code)] // TODO(brush-editor): used by the Brush Editor window.
pub fn decode(data: &ImageData) -> Result<(u32, u32, Vec<u8>), String> {
    let img = image::load_from_memory(&data.bytes)
        .map_err(|e| e.to_string())?
        .into_rgba8();
    Ok((img.width(), img.height(), img.into_raw()))
}

/// A generated tip: an ellipse `aspect` times as tall as it is wide
/// (`0.05..=1.0`; 1 is round), solid inside `hardness` (`0.0..=1.0`) of its
/// radius and soft beyond.
#[allow(dead_code)] // TODO(brush-editor): used by the Brush Editor window.
pub fn ellipse_tip(hardness: f32, aspect: f32) -> ImageData {
    let hard = hardness.clamp(0.0, 0.98) * FADE_START;
    let aspect = aspect.clamp(0.05, 1.0);
    let side = GENERATED_TIP;
    let mut tip = GrayImage::from_fn(side, side, |x, y| {
        let u = (x as f32 + 0.5) / side as f32 * 2.0 - 1.0;
        let v = ((y as f32 + 0.5) / side as f32 * 2.0 - 1.0) / aspect;
        let r = (u * u + v * v).sqrt();
        let c = 1.0 - smooth(((r - hard) / (FADE_START - hard)).clamp(0.0, 1.0));
        Luma([(c * 255.0).round() as u8])
    });
    fade_edge(&mut tip);
    encode(&tip)
}

/// A hash of a lattice point to `0.0..1.0`.
#[allow(dead_code)] // TODO(brush-editor): used by the Brush Editor window.
fn hash(x: i64, y: i64, seed: u64) -> f32 {
    let mut z = (x as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
        .wrapping_add(seed.wrapping_mul(0x1656_67B1_9E37_79F9));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 24) as f32
}

/// Value noise with `cells` lattice cells across `size` pixels, wrapping at
/// the edges so it tiles.
#[allow(dead_code)] // TODO(brush-editor): used by the Brush Editor window.
fn value_noise(x: f32, y: f32, size: u32, cells: u32, seed: u64) -> f32 {
    let scale = cells as f32 / size as f32;
    let (fx, fy) = (x * scale, y * scale);
    let (x0, y0) = (fx.floor() as i64, fy.floor() as i64);
    let (tx, ty) = (smooth(fx - x0 as f32), smooth(fy - y0 as f32));
    let c = i64::from(cells);
    let at = |i: i64, j: i64| hash(i.rem_euclid(c), j.rem_euclid(c), seed);
    let top = at(x0, y0) + (at(x0 + 1, y0) - at(x0, y0)) * tx;
    let bottom = at(x0, y0 + 1) + (at(x0 + 1, y0 + 1) - at(x0, y0 + 1)) * tx;
    top + (bottom - top) * ty
}

/// A generated, tiling paper grain. `coarseness` (`0.0..=1.0`) sets the size
/// of the paper's tooth, from fine to coarse; `seed` picks the pattern.
#[allow(dead_code)] // TODO(brush-editor): used by the Brush Editor window.
pub fn noise_grain(coarseness: f32, seed: u64) -> ImageData {
    let size = GENERATED_GRAIN;
    // 128 cells across (fine) down to 8 (coarse), a power of two so it tiles.
    let cells = 1u32 << (7 - (coarseness.clamp(0.0, 1.0) * 4.0).round() as u32);
    let grain = GrayImage::from_fn(size, size, |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        let tooth = value_noise(fx, fy, size, cells, seed) * 0.65
            + value_noise(fx, fy, size, cells * 2, seed + 1) * 0.35;
        let fine = hash(i64::from(x), i64::from(y), seed + 99);
        let v = ((tooth * 0.8 + fine * 0.2 - 0.5) * 2.0 + 0.5).clamp(0.0, 1.0) * 0.8 + 0.2;
        Luma([(v * 255.0).round() as u8])
    });
    encode(&grain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray(data: &ImageData) -> GrayImage {
        image::load_from_memory(&data.bytes).unwrap().into_luma8()
    }

    /// A transparent picture with an opaque `colour` rectangle.
    fn art(w: u32, h: u32, rect: (u32, u32, u32, u32), colour: [u8; 3]) -> Vec<u8> {
        let mut rgba = vec![0; (w * h * 4) as usize];
        for y in rect.1..rect.3 {
            for x in rect.0..rect.2 {
                let i = ((y * w + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&[colour[0], colour[1], colour[2], 255]);
            }
        }
        rgba
    }

    fn assert_faded(tip: &GrayImage) {
        let side = tip.width();
        assert_eq!(side, tip.height(), "tips are square");
        let c = side as f32 / 2.0;
        for (x, y, p) in tip.enumerate_pixels() {
            let r = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt() / c;
            if r >= 1.0 {
                assert_eq!(p.0[0], 0, "pixel ({x}, {y}) outside the circle paints");
            }
        }
    }

    #[test]
    fn dark_marks_become_a_centred_faded_tip() {
        let rgba = art(200, 100, (150, 20, 190, 40), [0, 0, 0]);
        let data = tip_from_rgba(200, 100, &rgba).unwrap();
        let tip = gray(&data);
        assert_eq!((data.width, data.height), tip.dimensions());
        assert_faded(&tip);
        let c = tip.width() / 2;
        assert!(tip.get_pixel(c, c).0[0] > 200, "the middle paints");
    }

    #[test]
    fn light_on_dark_pictures_read_as_brightness() {
        // An opaque black picture with a white square: the square paints.
        let mut rgba = art(64, 64, (0, 0, 64, 64), [0, 0, 0]);
        for y in 24..40 {
            for x in 24..40 {
                let i = ((y * 64 + x) * 4) as usize;
                rgba[i..i + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let tip = gray(&tip_from_rgba(64, 64, &rgba).unwrap());
        let c = tip.width() / 2;
        assert!(tip.get_pixel(c, c).0[0] > 200);
        assert_faded(&tip);
    }

    #[test]
    fn large_art_is_scaled_down() {
        let rgba = art(800, 800, (10, 10, 790, 790), [0, 0, 0]);
        let tip = tip_from_rgba(800, 800, &rgba).unwrap();
        assert_eq!((tip.width, tip.height), (TIP_SIDE, TIP_SIDE));
    }

    #[test]
    fn nothing_drawn_is_refused() {
        let empty = vec![0; 10 * 10 * 4];
        assert!(tip_from_rgba(10, 10, &empty).is_err());
        assert!(grain_from_rgba(10, 10, &empty).is_err());
        assert!(tip_from_rgba(10, 10, &[0; 3]).is_err());
        assert!(tip_from_rgba(0, 0, &[]).is_err());
        // White on nothing has no coverage either way round.
        let white = art(10, 10, (2, 2, 8, 8), [255, 255, 255]);
        assert!(tip_from_rgba(10, 10, &white).is_err());
    }

    #[test]
    fn grain_is_the_art_over_white_cropped() {
        let rgba = art(50, 40, (10, 5, 30, 25), [0, 0, 0]);
        let grain = gray(&grain_from_rgba(50, 40, &rgba).unwrap());
        assert_eq!(grain.dimensions(), (20, 20));
        assert_eq!(grain.get_pixel(5, 5).0[0], 0, "dark marks are pits");
    }

    #[test]
    fn a_selected_stroke_renders_into_a_tip() {
        let Ok(gpu) = pollster::block_on(renderer::headless_device()) else {
            eprintln!("skipped: no GPU adapter");
            return;
        };
        let mut session = ui::Session::new();
        session.state.tool = editor::Tool::Brush;
        session.state.brush.width = 30.0;
        session.pointer_down(document::animation_core::Vec2::new(100.0, 100.0), 1.0);
        session.pointer_move(document::animation_core::Vec2::new(300.0, 200.0));
        session.pointer_up();
        session.state.tool = editor::Tool::Select;
        session.select_all();
        session.create_brush_from_selection();
        let capture = session.take_brush_capture().expect("a capture");
        let mut offscreen = OffscreenRenderer::new(&gpu.0);
        let data = render(&gpu.0, &gpu.1, &mut offscreen, &capture).expect("a tip");
        let tip = gray(&data);
        assert_faded(&tip);
        let c = tip.width() / 2;
        assert!(tip.get_pixel(c, c).0[0] > 200, "the stroke's middle paints");
        session.finish_brush_capture(capture.target, Ok(data));
        assert!(session.brush().is_some());
    }

    #[test]
    fn generated_pictures() {
        for (hardness, aspect) in [(0.0, 1.0), (1.0, 1.0), (0.5, 0.2)] {
            let tip = gray(&ellipse_tip(hardness, aspect));
            assert_faded(&tip);
            let c = tip.width() / 2;
            assert!(tip.get_pixel(c, c).0[0] > 200);
        }
        let a = noise_grain(0.5, 1);
        assert_eq!((a.width, a.height), (GENERATED_GRAIN, GENERATED_GRAIN));
        assert_eq!(a, noise_grain(0.5, 1), "deterministic");
        assert_ne!(a, noise_grain(0.5, 2));
        let (w, h, rgba) = decode(&a).unwrap();
        assert_eq!(rgba.len() as u32, w * h * 4);
    }
}
