//! Animated GIF: a looping file with one palette per frame.
//!
//! GIF stores frame delays in hundredths of a second and has no audio or
//! inter-frame compression, so files grow quickly. Exports longer than
//! [`MAX_GIF_SECONDS`] are refused (see [`check_gif_length`]); a video or an
//! image sequence suits those better.

use crate::{ExportError, RgbaImage};
use image::codecs::gif::{GifEncoder as ImageGifEncoder, Repeat};
use image::{Delay, Frame};
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

/// The longest animation, in seconds, that can be exported as a GIF.
pub const MAX_GIF_SECONDS: f64 = 15.0;

/// Colour quantisation speed, 1 (best, very slow) to 30 (fastest).
const QUANTIZE_SPEED: i32 = 10;

/// Timing of an animated GIF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GifSettings {
    /// Frame rate numerator (e.g. 24000).
    pub fps_numerator: u32,
    /// Frame rate denominator (e.g. 1001).
    pub fps_denominator: u32,
}

impl GifSettings {
    /// How long `frames` frames play, in seconds.
    pub fn duration_seconds(&self, frames: u32) -> f64 {
        f64::from(frames) * f64::from(self.fps_denominator.max(1))
            / f64::from(self.fps_numerator.max(1))
    }

    /// The end time of frame `index` (zero-based), in hundredths of a
    /// second, rounded. Delays are differences of these, so rounding errors
    /// don't add up over a long animation.
    fn end_centis(&self, index: u64) -> u64 {
        let num = u64::from(self.fps_numerator.max(1));
        let den = u64::from(self.fps_denominator.max(1));
        ((index + 1) * 100 * den + num / 2) / num
    }

    /// The delay of frame `index`, in hundredths of a second. At least 1:
    /// many viewers treat a zero delay as "slow".
    fn delay_centis(&self, index: u64) -> u64 {
        let start = if index == 0 {
            0
        } else {
            self.end_centis(index - 1)
        };
        (self.end_centis(index) - start).max(1)
    }
}

/// Checks that `frames` frames at `settings`' rate fit in a GIF. Fails with
/// [`ExportError::TooLong`] if they play longer than [`MAX_GIF_SECONDS`].
pub fn check_gif_length(frames: u32, settings: GifSettings) -> Result<(), ExportError> {
    let seconds = settings.duration_seconds(frames);
    if seconds > MAX_GIF_SECONDS + 1e-9 {
        return Err(ExportError::TooLong {
            seconds,
            max_seconds: MAX_GIF_SECONDS,
        });
    }
    Ok(())
}

/// Writes frames to a looping GIF as they arrive. The file is deleted if the
/// encoder is dropped before [`Self::finish`].
pub struct GifEncoder {
    path: PathBuf,
    settings: GifSettings,
    encoder: Option<ImageGifEncoder<BufWriter<File>>>,
    size: Option<(u32, u32)>,
    frames: u64,
}

impl std::fmt::Debug for GifEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GifEncoder")
            .field("path", &self.path)
            .field("settings", &self.settings)
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

impl GifEncoder {
    /// Creates the file at `path`.
    pub fn create(path: &Path, settings: GifSettings) -> Result<Self, ExportError> {
        let file = File::create(path).map_err(|source| ExportError::Io {
            path: path.to_owned(),
            source,
        })?;
        let mut encoder = ImageGifEncoder::new_with_speed(BufWriter::new(file), QUANTIZE_SPEED);
        let mut this = Self {
            path: path.to_owned(),
            settings,
            encoder: None,
            size: None,
            frames: 0,
        };
        // On failure `this` is dropped, which deletes the file.
        encoder.set_repeat(Repeat::Infinite)?;
        this.encoder = Some(encoder);
        Ok(this)
    }

    /// Appends `image`. Every frame must have the size of the first. Any
    /// alpha is dropped: give it an opaque image (see [`crate::flatten`]).
    pub fn push_frame(&mut self, image: &RgbaImage) -> Result<(), ExportError> {
        image.validate()?;
        let size = *self.size.get_or_insert((image.width, image.height));
        if size != (image.width, image.height) || size.0 > 0xFFFF || size.1 > 0xFFFF {
            return Err(ExportError::InvalidFrame(format!(
                "GIF frame is {}x{}, expected {}x{} (at most 65535 on a side)",
                image.width, image.height, size.0, size.1
            )));
        }
        let Some(encoder) = self.encoder.as_mut() else {
            return Err(ExportError::Cancelled);
        };
        let mut pixels = image.pixels.clone();
        for p in pixels.as_chunks_mut::<4>().0 {
            p[3] = 255;
        }
        let buffer = image::RgbaImage::from_raw(image.width, image.height, pixels)
            .ok_or_else(|| ExportError::InvalidFrame("GIF frame buffer size".into()))?;
        let centis = self.settings.delay_centis(self.frames);
        let ms = u32::try_from(centis * 10).unwrap_or(u32::MAX);
        let delay = Delay::from_numer_denom_ms(ms, 1);
        encoder.encode_frame(Frame::from_parts(buffer, 0, 0, delay))?;
        self.frames += 1;
        Ok(())
    }

    /// Writes the trailer and closes the file. Returns its path.
    pub fn finish(mut self) -> Result<PathBuf, ExportError> {
        if self.frames == 0 {
            return Err(ExportError::InvalidFrame("no frames were exported".into()));
        }
        // Dropping the image encoder writes the GIF trailer and flushes.
        drop(self.encoder.take());
        Ok(std::mem::take(&mut self.path))
    }
}

impl Drop for GifEncoder {
    fn drop(&mut self) {
        // Not finished: remove the partial file. `finish` takes the path.
        if !self.path.as_os_str().is_empty() {
            drop(self.encoder.take());
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(num: u32, den: u32) -> GifSettings {
        GifSettings {
            fps_numerator: num,
            fps_denominator: den,
        }
    }

    #[test]
    fn delays_add_up_to_the_duration() {
        let s = settings(24, 1);
        let total: u64 = (0..24).map(|i| s.delay_centis(i)).sum();
        assert_eq!(total, 100);
        assert!((0..24).all(|i| (4..=5).contains(&s.delay_centis(i))));
        let ntsc = settings(30000, 1001);
        let total: u64 = (0..300).map(|i| ntsc.delay_centis(i)).sum();
        assert_eq!(total, 1001);
        // Faster than 100 fps still advances.
        assert_eq!(settings(240, 1).delay_centis(0), 1);
    }

    #[test]
    fn length_limit() {
        let s = settings(24, 1);
        assert!(check_gif_length(24 * 15, s).is_ok());
        assert!(matches!(
            check_gif_length(24 * 15 + 1, s),
            Err(ExportError::TooLong { .. })
        ));
        assert!((s.duration_seconds(48) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn writes_a_looping_gif() {
        let dir = std::env::temp_dir().join(format!("rusting-gif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.gif");
        let mut gif = GifEncoder::create(&path, settings(12, 1)).unwrap();
        gif.push_frame(&RgbaImage::filled(8, 6, [255, 0, 0, 255]))
            .unwrap();
        gif.push_frame(&RgbaImage::filled(8, 6, [0, 0, 255, 255]))
            .unwrap();
        assert!(matches!(
            gif.push_frame(&RgbaImage::filled(4, 4, [0; 4])),
            Err(ExportError::InvalidFrame(_))
        ));
        assert_eq!(gif.finish().unwrap(), path);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..6], b"GIF89a");
        assert!(bytes.windows(11).any(|w| w == b"NETSCAPE2.0"));
        let img = image::open(&path).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (8, 6));
        let p = img.get_pixel(3, 3).0;
        assert!(p[0] > 240 && p[2] < 16, "{p:?}");

        // Dropped before finishing: no file left.
        let partial = dir.join("b.gif");
        let mut gif = GifEncoder::create(&partial, settings(12, 1)).unwrap();
        gif.push_frame(&RgbaImage::filled(2, 2, [0, 0, 0, 255]))
            .unwrap();
        drop(gif);
        assert!(!partial.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
