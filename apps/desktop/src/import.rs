//! Reading files for File → Import: PNG/JPEG pictures (ADR-015),
//! MP3/WAV sounds (ADR-020) and `.rustbrush` brushes (ADR-022).

use document::drawing::ImageData;
use document::{AudioData, AudioFormat};
use rodio::Source;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

/// Picture file extensions the import dialog offers.
pub const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];

/// Sound file extensions the import dialog offers.
pub const AUDIO_EXTENSIONS: &[&str] = &["mp3", "wav"];

/// Every extension the import dialog offers.
pub const ALL_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "mp3", "wav", "rustbrush"];

/// What an imported file is, by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// A PNG or JPEG picture.
    Picture,
    /// An MP3 or WAV sound.
    Audio(AudioFormat),
    /// A `.rustbrush` brush package.
    Brush,
}

/// The kind of file at `path`, by its extension (any case); `None` for
/// files Import doesn't take.
pub fn file_kind(path: &Path) -> Option<FileKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" => Some(FileKind::Picture),
        "mp3" => Some(FileKind::Audio(AudioFormat::Mp3)),
        "wav" => Some(FileKind::Audio(AudioFormat::Wav)),
        crate::brushes::EXTENSION => Some(FileKind::Brush),
        _ => None,
    }
}

/// Reads `path` as a `format` sound: decodes all of it to check it, measure
/// its length and build its waveform peaks. The file's bytes are kept as
/// they are; playback decodes them again.
pub fn read_audio(path: &Path, format: AudioFormat) -> Result<AudioData, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    audio_data(bytes, format)
}

/// Decodes an MP3/WAV file and wraps it with its length and peaks.
fn audio_data(bytes: Vec<u8>, format: AudioFormat) -> Result<AudioData, String> {
    let bytes: Arc<[u8]> = bytes.into();
    let decoder = rodio::Decoder::builder()
        .with_data(Cursor::new(bytes.clone()))
        .with_byte_len(bytes.len() as u64)
        .with_hint(format.extension())
        .build()
        .map_err(|e| format!("Not a readable {} file: {e}", format.label()))?;
    let sample_rate = decoder.sample_rate().get();
    let channels = decoder.channels().get();
    let (frames, peaks) = measure(decoder, sample_rate, channels);
    if frames == 0 {
        return Err(format!("The {} file has no sound in it.", format.label()));
    }
    Ok(AudioData {
        format,
        sample_rate,
        channels,
        duration: frames as f64 / f64::from(sample_rate),
        peaks,
        bytes,
    })
}

/// Counts the sample frames of `samples` (interleaved, `channels` per
/// frame) and finds the loudest sample of each 1/100 s slice, scaled to
/// `0..=255`.
fn measure(
    samples: impl Iterator<Item = rodio::Sample>,
    sample_rate: u32,
    channels: u16,
) -> (u64, Vec<u8>) {
    let channels = u64::from(channels.max(1));
    let per_peak = u64::from((sample_rate / AudioData::PEAKS_PER_SECOND).max(1)) * channels;
    let mut peaks = Vec::new();
    let (mut count, mut loudest) = (0u64, 0.0f32);
    for sample in samples {
        loudest = loudest.max(sample.abs());
        count += 1;
        if count % per_peak == 0 {
            peaks.push(scale_peak(loudest));
            loudest = 0.0;
        }
    }
    if count % per_peak != 0 {
        peaks.push(scale_peak(loudest));
    }
    (count / channels, peaks)
}

fn scale_peak(peak: f32) -> u8 {
    (peak.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Reads `path` and checks that it is a PNG or JPEG whose size can be read.
/// The file's bytes are kept as they are; the renderer decodes them.
pub fn read_image(path: &Path) -> Result<ImageData, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    image_data(bytes)
}

/// Wraps an encoded PNG/JPEG, reading its size from the header and checking
/// that the whole picture decodes.
pub(crate) fn image_data(bytes: Vec<u8>) -> Result<ImageData, String> {
    let reader = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    match reader.format() {
        Some(image::ImageFormat::Png | image::ImageFormat::Jpeg) => {}
        _ => return Err("Only PNG and JPEG pictures can be imported.".into()),
    }
    let picture = reader.decode().map_err(|e| e.to_string())?;
    let (width, height) = (picture.width(), picture.height());
    if width == 0 || height == 0 {
        return Err("The picture has no pixels.".into());
    }
    Ok(ImageData::new(width, height, bytes))
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// A 16-bit PCM WAV file of `samples` (interleaved over `channels`).
    pub(crate) fn wav(sample_rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&sample_rate.to_le_bytes());
        let block = u32::from(channels) * 2;
        out.extend_from_slice(&(sample_rate * block).to_le_bytes());
        out.extend_from_slice(&(block as u16).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    #[test]
    fn file_kind_follows_the_extension() {
        assert_eq!(file_kind(Path::new("a/b.PNG")), Some(FileKind::Picture));
        assert_eq!(file_kind(Path::new("x.jpeg")), Some(FileKind::Picture));
        assert_eq!(
            file_kind(Path::new("song.Mp3")),
            Some(FileKind::Audio(AudioFormat::Mp3))
        );
        assert_eq!(
            file_kind(Path::new("take.wav")),
            Some(FileKind::Audio(AudioFormat::Wav))
        );
        assert_eq!(file_kind(Path::new("notes.txt")), None);
        assert_eq!(file_kind(Path::new("noext")), None);
    }

    #[test]
    fn reads_a_wav_measures_it_and_builds_peaks() {
        // Half a second of stereo at 8 kHz: quiet, then loud on one channel.
        let mut samples = Vec::new();
        for i in 0..4000 {
            let loud = if i >= 2000 { 16384 } else { 0 };
            samples.extend_from_slice(&[loud, -100]);
        }
        let bytes = wav(8000, 2, &samples);
        let data = audio_data(bytes.clone(), AudioFormat::Wav).unwrap();
        assert_eq!((data.sample_rate, data.channels), (8000, 2));
        assert!((data.duration - 0.5).abs() < 1e-9, "{}", data.duration);
        assert_eq!(&*data.bytes, bytes.as_slice());
        assert_eq!(data.peaks.len(), 50);
        assert!(data.peaks[..25].iter().all(|&p| p < 5), "{:?}", data.peaks);
        assert!(data.peaks[25..].iter().all(|&p| (126..=129).contains(&p)));
    }

    #[test]
    fn rejects_broken_and_silent_sounds() {
        assert!(audio_data(b"hello".to_vec(), AudioFormat::Wav).is_err());
        assert!(audio_data(b"hello".to_vec(), AudioFormat::Mp3).is_err());
        assert!(audio_data(wav(8000, 1, &[]), AudioFormat::Wav).is_err());
        assert!(read_audio(Path::new("/no/such/song.mp3"), AudioFormat::Mp3).is_err());
    }

    #[test]
    fn peaks_cover_a_partial_last_slice() {
        let (frames, peaks) = measure([0.5, -1.0, 0.25].into_iter(), 200, 1);
        assert_eq!(frames, 3);
        assert_eq!(peaks, [255, 64]);
    }

    fn encoded(format: image::ImageFormat) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbImage::from_pixel(3, 2, image::Rgb([10, 20, 30]))
            .write_to(&mut Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    #[test]
    fn reads_png_and_jpeg_sizes_and_keeps_the_bytes() {
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
            let bytes = encoded(format);
            let data = image_data(bytes.clone()).unwrap();
            assert_eq!((data.width, data.height), (3, 2));
            assert_eq!(&*data.bytes, bytes.as_slice());
        }
    }

    #[test]
    fn rejects_other_and_broken_files() {
        assert!(image_data(b"hello".to_vec()).is_err());
        let mut png = encoded(image::ImageFormat::Png);
        png.truncate(png.len() / 2);
        assert!(image_data(png).is_err());
    }

    #[test]
    fn missing_file_is_an_error() {
        assert!(read_image(Path::new("/no/such/picture.png")).is_err());
    }
}
