//! Still images: PNG and JPEG files, compositing, sequence file names.

use crate::ExportError;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

/// An 8-bit RGBA image: straight (not premultiplied) alpha, sRGB-encoded
/// colour, rows top to bottom with no padding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub pixels: Vec<u8>,
}

impl RgbaImage {
    /// An image filled with one colour.
    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let count = width as usize * height as usize;
        Self {
            width,
            height,
            pixels: rgba.repeat(count),
        }
    }

    /// The pixel at `(x, y)`.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let p = self.pixels.get(i..i + 4)?;
        Some([p[0], p[1], p[2], p[3]])
    }

    /// Checks that the pixel buffer matches the size.
    pub fn validate(&self) -> Result<(), ExportError> {
        let expected = self.width as usize * self.height as usize * 4;
        if self.width == 0 || self.height == 0 || self.pixels.len() != expected {
            return Err(ExportError::InvalidFrame(format!(
                "image is {}x{} but has {} bytes",
                self.width,
                self.height,
                self.pixels.len()
            )));
        }
        Ok(())
    }
}

/// File format of an exported still image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    /// Lossless, keeps alpha.
    Png,
    /// Lossy, no alpha. `quality` is 1..=100.
    Jpeg {
        /// Encoder quality, 1 (smallest) to 100 (best).
        quality: u8,
    },
}

impl ImageFormat {
    /// The usual file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg { .. } => "jpg",
        }
    }
}

/// Composites `image` over an opaque `background` (sRGB bytes). The blend is
/// done on the encoded values, which is exact for fully opaque or fully
/// transparent pixels and close for anti-aliased edges.
pub fn flatten(image: &RgbaImage, background: [u8; 3]) -> RgbaImage {
    let mut pixels = image.pixels.clone();
    for p in pixels.as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        for c in 0..3 {
            let v = u32::from(p[c]) * a + u32::from(background[c]) * (255 - a);
            p[c] = ((v + 127) / 255) as u8;
        }
        p[3] = 255;
    }
    RgbaImage {
        width: image.width,
        height: image.height,
        pixels,
    }
}

/// Writes `image` to `path`. JPEG drops alpha, so give it an opaque image
/// (see [`flatten`]); any remaining alpha is ignored.
pub fn write_image(path: &Path, image: &RgbaImage, format: ImageFormat) -> Result<(), ExportError> {
    image.validate()?;
    let io_err = |source| ExportError::Io {
        path: path.to_owned(),
        source,
    };
    let file = std::fs::File::create(path).map_err(io_err)?;
    let mut out = BufWriter::new(file);
    match format {
        ImageFormat::Png => {
            let encoder = image::codecs::png::PngEncoder::new(&mut out);
            image::ImageEncoder::write_image(
                encoder,
                &image.pixels,
                image.width,
                image.height,
                image::ExtendedColorType::Rgba8,
            )?;
        }
        ImageFormat::Jpeg { quality } => {
            let rgb: Vec<u8> = image
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect();
            let encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
            image::ImageEncoder::write_image(
                encoder,
                &rgb,
                image.width,
                image.height,
                image::ExtendedColorType::Rgb8,
            )?;
        }
    }
    std::io::Write::flush(&mut out).map_err(io_err)?;
    Ok(())
}

/// The file for frame `number` of a sequence based on `base`:
/// `dir/name.png` becomes `dir/name_0001.png`. Numbers are zero-padded to
/// at least four digits. The extension comes from `format`.
pub fn sequence_path(base: &Path, number: u32, format: ImageFormat) -> PathBuf {
    let stem = base
        .file_stem()
        .map_or_else(|| "frame".to_owned(), |s| s.to_string_lossy().into_owned());
    let name = format!("{stem}_{number:04}.{}", format.extension());
    base.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rusting-export-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn gradient(width: u32, height: u32) -> RgbaImage {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[(x * 20) as u8, (y * 30) as u8, 128, (x * y) as u8]);
            }
        }
        RgbaImage {
            width,
            height,
            pixels,
        }
    }

    #[test]
    fn png_round_trips_exactly_with_alpha() {
        let dir = temp_dir("png");
        let path = dir.join("a.png");
        let img = gradient(7, 5);
        write_image(&path, &img, ImageFormat::Png).unwrap();
        let back = image::open(&path).unwrap().to_rgba8();
        assert_eq!(back.dimensions(), (7, 5));
        assert_eq!(back.into_raw(), img.pixels);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn jpeg_round_trips_approximately() {
        let dir = temp_dir("jpg");
        let path = dir.join("a.jpg");
        let img = RgbaImage::filled(16, 9, [200, 40, 90, 255]);
        write_image(&path, &img, ImageFormat::Jpeg { quality: 95 }).unwrap();
        let back = image::open(&path).unwrap().to_rgb8();
        assert_eq!(back.dimensions(), (16, 9));
        for p in back.pixels() {
            for (got, want) in p.0.iter().zip([200u8, 40, 90]) {
                assert!(got.abs_diff(want) <= 4, "{:?}", p.0);
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn write_rejects_bad_buffers_and_reports_io_errors() {
        let bad = RgbaImage {
            width: 2,
            height: 2,
            pixels: vec![0; 3],
        };
        let dir = temp_dir("bad");
        assert!(matches!(
            write_image(&dir.join("x.png"), &bad, ImageFormat::Png),
            Err(ExportError::InvalidFrame(_))
        ));
        let missing = dir.join("no/such/dir/x.png");
        let good = RgbaImage::filled(1, 1, [0; 4]);
        assert!(matches!(
            write_image(&missing, &good, ImageFormat::Png),
            Err(ExportError::Io { .. })
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn flatten_composites_over_background() {
        let img = RgbaImage {
            width: 3,
            height: 1,
            pixels: vec![10, 20, 30, 255, 99, 99, 99, 0, 255, 0, 0, 128],
        };
        let flat = flatten(&img, [0, 0, 255]);
        assert_eq!(flat.pixel(0, 0), Some([10, 20, 30, 255]));
        assert_eq!(flat.pixel(1, 0), Some([0, 0, 255, 255]));
        assert_eq!(flat.pixel(2, 0), Some([128, 0, 127, 255]));
    }

    #[test]
    fn sequence_names_are_numbered() {
        let base = Path::new("/tmp/out/walk.png");
        assert_eq!(
            sequence_path(base, 1, ImageFormat::Png),
            Path::new("/tmp/out/walk_0001.png")
        );
        assert_eq!(
            sequence_path(base, 12345, ImageFormat::Jpeg { quality: 90 }),
            Path::new("/tmp/out/walk_12345.jpg")
        );
    }
}
