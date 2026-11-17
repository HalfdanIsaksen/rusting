//! The asset library: files imported into the project (ADR-017).
//!
//! An [`Asset`] is one imported file, kept once in [`Document::assets`]
//! (keyed by [`AssetId`]). Items placed on the canvas from it, such as
//! [`drawing::Image`], refer to it by ID. The library owns its assets: they
//! stay in the project (and in the file) when nothing uses them, until the
//! user deletes them.
//!
//! [`Document::assets`]: crate::Document::assets

use animation_core::AssetId;
use drawing::ImageData;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// One entry of the library.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    /// Stable identifier; equal to its key in the library.
    pub id: AssetId,
    /// Display name, e.g. the imported file's name.
    pub name: String,
    /// What the asset holds.
    pub kind: AssetKind,
}

impl Asset {
    /// An asset with a fresh ID.
    pub fn new(name: impl Into<String>, kind: AssetKind) -> Self {
        Self {
            id: AssetId::new(),
            name: name.into(),
            kind,
        }
    }

    /// The picture, if this is an image asset.
    pub fn image(&self) -> Option<&ImageData> {
        match &self.kind {
            AssetKind::Image(data) => Some(data),
            AssetKind::Audio(_) => None,
        }
    }

    /// The sound, if this is an audio asset.
    pub fn audio(&self) -> Option<&AudioData> {
        match &self.kind {
            AssetKind::Audio(data) => Some(data),
            AssetKind::Image(_) => None,
        }
    }
}

/// What an [`Asset`] holds. New kinds (audio, objects) are added as new
/// variants; serde tags each with its variant name, so files stay readable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AssetKind {
    /// An imported PNG/JPEG picture (ADR-015).
    Image(ImageData),
    /// An imported MP3/WAV sound (ADR-020).
    Audio(AudioData),
}

impl AssetKind {
    /// Short user-visible name of the kind, e.g. "Image".
    pub fn label(&self) -> &'static str {
        match self {
            Self::Image(_) => "Image",
            Self::Audio(_) => "Audio",
        }
    }
}

/// The file type of an imported sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioFormat {
    /// MPEG-1/2 Layer III.
    Mp3,
    /// RIFF WAVE (PCM).
    Wav,
}

impl AudioFormat {
    /// Upper-case name shown in the UI, e.g. "MP3".
    pub fn label(self) -> &'static str {
        match self {
            Self::Mp3 => "MP3",
            Self::Wav => "WAV",
        }
    }

    /// The usual file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
        }
    }
}

/// An imported sound: the file as it was imported plus what the editor
/// needs without decoding it again (ADR-020).
///
/// Core crates never decode audio. The desktop app decodes the file once on
/// import to check it, measure it and build [`Self::peaks`], and decodes it
/// again only to play it. Cloning is cheap: the bytes are shared.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioData {
    /// The file type.
    pub format: AudioFormat,
    /// Samples per second per channel.
    pub sample_rate: u32,
    /// Number of channels (1 mono, 2 stereo, ...).
    pub channels: u16,
    /// Length in seconds.
    pub duration: f64,
    /// The waveform outline: the loudest sample of each
    /// 1/[`Self::PEAKS_PER_SECOND`] s slice, over all channels, scaled so
    /// 255 is full scale.
    #[serde(default)]
    pub peaks: Vec<u8>,
    /// The MP3 or WAV file, as imported. Saved as base64 text.
    #[serde(
        serialize_with = "drawing::image::to_base64",
        deserialize_with = "drawing::image::from_base64"
    )]
    pub bytes: Arc<[u8]>,
}

impl AudioData {
    /// How many [`Self::peaks`] there are per second of sound.
    pub const PEAKS_PER_SECOND: u32 = 100;

    /// Whether there is no sound: no bytes, no length or no sample rate.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty() || self.sample_rate == 0 || self.channels == 0 || self.duration <= 0.0
    }

    /// The loudest peak (`0.0..=1.0`) between `from` and `to` seconds, for
    /// drawing the waveform; 0 outside the sound.
    pub fn peak_between(&self, from: f64, to: f64) -> f32 {
        let per = f64::from(Self::PEAKS_PER_SECOND);
        let len = self.peaks.len();
        let index = |secs: f64| ((secs * per).max(0.0) as usize).min(len);
        let (a, b) = (index(from), index(to));
        let b = if b <= a { (a + 1).min(len) } else { b };
        self.peaks[a..b]
            .iter()
            .max()
            .map_or(0.0, |&p| f32::from(p) / 255.0)
    }
}

impl std::fmt::Debug for AudioData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioData")
            .field("format", &self.format)
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels)
            .field("duration", &self.duration)
            .field("peaks", &self.peaks.len())
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sound(duration: f64) -> AudioData {
        AudioData {
            format: AudioFormat::Wav,
            sample_rate: 8000,
            channels: 1,
            duration,
            peaks: (0..(duration * 100.0) as usize)
                .map(|i| (i % 256) as u8)
                .collect(),
            bytes: Arc::from(vec![1u8, 2, 3]),
        }
    }

    #[test]
    fn audio_assets_expose_their_sound() {
        let asset = Asset::new("song.wav", AssetKind::Audio(sound(1.0)));
        assert_eq!(asset.kind.label(), "Audio");
        assert!(asset.image().is_none());
        assert_eq!(asset.audio().map(|a| a.duration), Some(1.0));
        assert!(!sound(1.0).is_empty());
        assert!(sound(0.0).is_empty());
    }

    #[test]
    fn peaks_between_two_times() {
        let a = sound(1.0);
        // Peaks 0..=99 over one second.
        assert_eq!(a.peak_between(0.0, 0.1), 9.0 / 255.0);
        assert_eq!(a.peak_between(0.5, 0.5), 50.0 / 255.0);
        assert_eq!(a.peak_between(2.0, 3.0), 0.0);
        assert_eq!(a.peak_between(-1.0, 0.0), 0.0);
    }
}
