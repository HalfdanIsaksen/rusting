//! Regenerates the tip and grain pictures of the built-in brushes in
//! `assets/brushes/` (ADR-022).
//!
//! ```bash
//! cargo run -p desktop --example generate_brushes
//! ```
//!
//! Everything is computed from fixed seeds, so running it again writes the
//! same files. Tips and grains are 8-bit grayscale PNGs: in a tip, white is
//! paint and black is none (fading to black at the edge of the inscribed
//! circle, so a rotated stamp stays inside it); in a grain, white is paper
//! that takes paint. Grains tile seamlessly. The `brush.json` files are
//! written by hand and are not touched.

#![allow(clippy::unwrap_used)]

use image::{GrayImage, Luma};
use std::path::{Path, PathBuf};

const TIP: u32 = 96;
const GRAIN: u32 = 256;

/// A hash of a lattice point to `0.0..1.0`.
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

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Value noise with `cells` lattice cells across `size` pixels, wrapping at
/// the edges so it tiles.
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

/// Fractal noise: `octaves` layers of tiling value noise starting at
/// `cells` cells, each twice as fine and half as strong.
fn fbm(x: f32, y: f32, size: u32, cells: u32, octaves: u32, seed: u64) -> f32 {
    let (mut sum, mut amp, mut total) = (0.0, 1.0, 0.0);
    for o in 0..octaves {
        sum += value_noise(x, y, size, cells << o, seed + u64::from(o)) * amp;
        total += amp;
        amp *= 0.5;
    }
    sum / total
}

/// Raises contrast around 0.5 by `k` and clamps to `0.0..=1.0`.
fn contrast(v: f32, k: f32) -> f32 {
    ((v - 0.5) * k + 0.5).clamp(0.0, 1.0)
}

/// A tip picture: `f` gets the position in the unit disc (`-1..1` on both
/// axes) and returns coverage.
fn tip(f: impl Fn(f32, f32, u32, u32) -> f32) -> GrayImage {
    GrayImage::from_fn(TIP, TIP, |x, y| {
        let u = (x as f32 + 0.5) / TIP as f32 * 2.0 - 1.0;
        let v = (y as f32 + 0.5) / TIP as f32 * 2.0 - 1.0;
        Luma([(f(u, v, x, y).clamp(0.0, 1.0) * 255.0).round() as u8])
    })
}

/// A round falloff: 1 inside radius `hard`, 0 at radius 1.
fn disc(u: f32, v: f32, hard: f32) -> f32 {
    let r = (u * u + v * v).sqrt();
    1.0 - smooth(((r - hard) / (1.0 - hard)).clamp(0.0, 1.0))
}

/// A grain picture from `f(x, y)`.
fn grain(f: impl Fn(f32, f32) -> f32) -> GrayImage {
    GrayImage::from_fn(GRAIN, GRAIN, |x, y| {
        Luma([(f(x as f32, y as f32).clamp(0.0, 1.0) * 255.0).round() as u8])
    })
}

/// Paper tooth: mostly paper that takes paint, with fine pits (a few
/// pixels across) and speckle that stay empty, and a faint broad
/// unevenness.
fn paper(seed: u64, k: f32) -> GrayImage {
    grain(|x, y| {
        let tooth = fbm(x, y, GRAIN, 64, 2, seed);
        let broad = fbm(x, y, GRAIN, 8, 2, seed + 50);
        let fine = hash(x as i64, y as i64, seed + 99);
        contrast(tooth * 0.6 + fine * 0.25 + broad * 0.15, k) * 0.8 + 0.2
    })
}

fn save(dir: &Path, name: &str, img: &GrayImage) {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    img.save(&path).unwrap();
    println!("wrote {}", path.display());
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/brushes");

    // Pencil (HB): small soft tip with fine graphite speckle.
    let dir = root.join("pencil-hb");
    save(
        &dir,
        "tip.png",
        &tip(|u, v, x, y| disc(u, v, 0.3) * (0.55 + 0.45 * hash(x.into(), y.into(), 1))),
    );
    save(&dir, "grain.png", &paper(10, 2.2));

    // Soft Pencil (6B): fuller, darker, smoother.
    let dir = root.join("soft-pencil-6b");
    save(
        &dir,
        "tip.png",
        &tip(|u, v, x, y| disc(u, v, 0.45) * (0.75 + 0.25 * hash(x.into(), y.into(), 2))),
    );
    save(&dir, "grain.png", &paper(20, 1.6));

    // Charcoal: a ragged blob with heavy texture.
    let dir = root.join("charcoal");
    save(
        &dir,
        "tip.png",
        &tip(|u, v, x, y| {
            let edge = fbm(x as f32, y as f32, TIP, 4, 3, 3);
            let shape = disc(u * (0.8 + 0.4 * edge), v * (0.8 + 0.4 * edge), 0.2);
            shape * (0.3 + 0.7 * contrast(fbm(x as f32, y as f32, TIP, 12, 3, 4), 2.5))
        }),
    );
    save(
        &dir,
        "grain.png",
        &grain(|x, y| contrast(fbm(x, y, GRAIN, 32, 4, 30), 2.4)),
    );

    // Chalk / Crayon: speckled dots inside the disc.
    let dir = root.join("chalk");
    save(
        &dir,
        "tip.png",
        &tip(|u, v, x, y| {
            let dot = if hash(x.into(), y.into(), 5) > 0.45 {
                1.0
            } else {
                0.0
            };
            disc(u, v, 0.5) * dot
        }),
    );
    save(
        &dir,
        "grain.png",
        &grain(|x, y| {
            let broad = fbm(x, y, GRAIN, 32, 3, 40);
            contrast(broad, 3.5)
        }),
    );

    // Ink Pen: a hard round tip, no grain.
    save(
        &root.join("ink-pen"),
        "tip.png",
        &tip(|u, v, _, _| disc(u, v, 0.9)),
    );

    // Marker: a flat chisel tip, long across the stroke (y) and narrow
    // along it (x); `follow_direction` turns x along the stroke.
    save(
        &root.join("marker"),
        "tip.png",
        &tip(|u, v, _, _| disc(u * 3.0, v, 0.85)),
    );
}
