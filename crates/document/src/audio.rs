//! Audio layers: which sounds play at a frame (ADR-020).
//!
//! An audio layer ([`scene::LayerKind::Audio`]) holds one [`AudioClip`]: a
//! library sound that starts at a frame and plays to its end. Nothing here
//! decodes audio; the app plays the clips [`Document::audio_at`] lists.

use crate::Document;
use crate::assets::AudioData;
use animation_core::{AssetId, Fps, FrameIndex, FrameRange, LayerId};
use scene::{AudioClip, Layer};

/// A clip that should be sounding at some frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundingClip {
    /// The audio layer.
    pub layer: LayerId,
    /// The library sound it plays.
    pub asset: AssetId,
    /// How far into the sound the frame is, in seconds.
    pub offset: f64,
    /// Loudness multiplier in `0.0..=1.0`.
    pub volume: f32,
}

/// Seconds into a clip that starts at `start` when the playhead is at
/// `frame`; `None` before the clip starts.
pub fn clip_offset(fps: Fps, start: FrameIndex, frame: FrameIndex) -> Option<f64> {
    let frames = frame.0.checked_sub(start.0)?;
    Some(fps.frame_to_secs(FrameIndex(frames)))
}

/// How many frames a sound of `duration` seconds covers at `fps`: every
/// frame during which some of it plays (at least 1).
pub fn duration_frames(fps: Fps, duration: f64) -> u32 {
    let frames = (duration * fps.as_f64()).ceil();
    if frames.is_finite() && frames >= 1.0 {
        frames.min(f64::from(u32::MAX)) as u32
    } else {
        1
    }
}

impl Document {
    /// Every audio layer and its clip, bottom to top.
    pub fn audio_layers(&self) -> impl Iterator<Item = (&Layer, &AudioClip)> {
        self.layers()
            .iter()
            .filter_map(|l| Some((l, l.audio_clip()?)))
    }

    /// The sound a clip plays, if its asset is a library sound.
    pub fn clip_sound(&self, clip: &AudioClip) -> Option<&AudioData> {
        self.asset(clip.asset)?.audio()
    }

    /// The frames during which `clip` sounds: from its start for the length
    /// of its sound. `None` if its asset is not a library sound.
    pub fn clip_range(&self, clip: &AudioClip) -> Option<FrameRange> {
        let len = duration_frames(self.fps, self.clip_sound(clip)?.duration);
        Some(FrameRange::new(
            clip.start,
            FrameIndex(clip.start.0.saturating_add(len)),
        ))
    }

    /// The clips that should be sounding at `frame`, bottom to top. Muted
    /// audio layers (hidden, or in a hidden group) are left out.
    pub fn audio_at(&self, frame: FrameIndex) -> Vec<SoundingClip> {
        self.audio_layers()
            .filter(|(layer, _)| self.layers().is_visible(layer.id))
            .filter_map(|(layer, clip)| {
                if !self.clip_range(clip)?.contains(frame) {
                    return None;
                }
                Some(SoundingClip {
                    layer: layer.id,
                    asset: clip.asset,
                    offset: clip_offset(self.fps, clip.start, frame)?,
                    volume: clip.volume,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{Asset, AssetKind, AudioFormat};
    use crate::{CanvasSize, Document};
    use std::sync::Arc;

    fn sound(duration: f64) -> Asset {
        Asset::new(
            "song.wav",
            AssetKind::Audio(AudioData {
                format: AudioFormat::Wav,
                sample_rate: 8000,
                channels: 1,
                duration,
                peaks: Vec::new(),
                bytes: Arc::from(vec![1u8]),
            }),
        )
    }

    fn fps(n: u32) -> Fps {
        Fps::whole(n).unwrap()
    }

    #[test]
    fn offset_counts_seconds_from_the_clip_start() {
        assert_eq!(clip_offset(fps(24), FrameIndex(10), FrameIndex(9)), None);
        assert_eq!(
            clip_offset(fps(24), FrameIndex(10), FrameIndex(10)),
            Some(0.0)
        );
        assert_eq!(
            clip_offset(fps(24), FrameIndex(10), FrameIndex(22)),
            Some(0.5)
        );
        assert_eq!(
            clip_offset(fps(12), FrameIndex(0), FrameIndex(24)),
            Some(2.0)
        );
    }

    #[test]
    fn duration_rounds_up_to_whole_frames() {
        assert_eq!(duration_frames(fps(24), 1.0), 24);
        assert_eq!(duration_frames(fps(24), 1.01), 25);
        assert_eq!(duration_frames(fps(24), 0.0), 1);
        assert_eq!(duration_frames(fps(24), f64::NAN), 1);
    }

    #[test]
    fn audio_at_lists_audible_clips_in_range() {
        let mut d = Document::new(
            "T",
            CanvasSize {
                width: 10,
                height: 10,
            },
        );
        d.add_layer("Ink");
        let short = sound(0.5);
        let long = sound(2.0);
        let (s, l) = (short.id, long.id);
        d.insert_asset(short);
        d.insert_asset(long);
        let a = d.insert_layer(1, Layer::audio("A", AudioClip::new(s, FrameIndex(0))));
        let b = d.insert_layer(2, Layer::audio("B", AudioClip::new(l, FrameIndex(6))));
        // 24 fps: A sounds on frames 0..12, B on 6..54.
        let at = |d: &Document, f| {
            d.audio_at(FrameIndex(f))
                .iter()
                .map(|c| (c.layer, c.offset))
                .collect::<Vec<_>>()
        };
        assert_eq!(at(&d, 0), [(a, 0.0)]);
        assert_eq!(at(&d, 6), [(a, 0.25), (b, 0.0)]);
        assert_eq!(at(&d, 12), [(b, 0.25)]);
        assert_eq!(at(&d, 54), []);
        assert_eq!(
            d.clip_range(d.layers().get(b).unwrap().audio_clip().unwrap()),
            Some(FrameRange::new(FrameIndex(6), FrameIndex(54)))
        );
        // Hiding a layer mutes it.
        d.layer_mut(a).unwrap().visible = false;
        assert_eq!(at(&d, 6), [(b, 0.0)]);
        // A clip whose sound is gone is silent.
        d.remove_asset(l);
        assert_eq!(at(&d, 6), []);
        assert_eq!(d.audio_layers().count(), 2);
    }
}
