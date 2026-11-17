//! Sound for audio layers (ADR-020): plays the clips in sync with playback
//! and a short snippet when the playhead moves while paused.
//!
//! [`AudioPlayer::update`] runs once per drawn frame. It takes the session's
//! [`ui::PlaybackEvent`] and, while playing, keeps one rodio [`Player`] per
//! sounding clip: clips that start sounding are started at their offset,
//! clips that end, are muted or removed stop, and volume changes apply at
//! once. The output device opens on first use; if it can't, the problem is
//! logged once and the editor carries on silently.
//!
//! What to start and stop is decided by [`plan`], which needs no device, so
//! it is unit-tested.

use document::animation_core::{AssetId, Fps, FrameIndex, LayerId};
use document::{AudioData, Document, SoundingClip};
use rodio::{Player, Source};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

/// The shortest scrub snippet, so a single step is still audible.
const MIN_SNIPPET: Duration = Duration::from_millis(60);

/// How long the snippet played for one scrubbed frame is: one frame, but at
/// least [`MIN_SNIPPET`].
pub fn snippet_length(fps: Fps) -> Duration {
    Duration::from_secs_f64(fps.frame_duration_secs().max(0.0)).max(MIN_SNIPPET)
}

/// Changes to the playing clips.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    /// Clips to start now, at their offset.
    pub start: Vec<SoundingClip>,
    /// Layers whose players stop.
    pub stop: Vec<LayerId>,
    /// Layers whose players keep going at a new volume.
    pub volume: Vec<(LayerId, f32)>,
}

/// What to do so that exactly the `wanted` clips (from
/// [`Document::audio_at`] at frame `frame`) are playing, given what plays
/// now: `playing` maps each layer to the clip it plays (asset, the frame
/// the clip's sound starts on) and its volume.
fn plan(
    playing: &BTreeMap<LayerId, (AssetId, FrameIndex, f32)>,
    wanted: &[(SoundingClip, FrameIndex)],
) -> Plan {
    let mut plan = Plan::default();
    for &(clip, start) in wanted {
        match playing.get(&clip.layer) {
            Some(&(asset, was_start, volume)) if asset == clip.asset && was_start == start => {
                if volume != clip.volume {
                    plan.volume.push((clip.layer, clip.volume));
                }
            }
            Some(_) => {
                plan.stop.push(clip.layer);
                plan.start.push(clip);
            }
            None => plan.start.push(clip),
        }
    }
    for &layer in playing.keys() {
        if !wanted.iter().any(|(c, _)| c.layer == layer) {
            plan.stop.push(layer);
        }
    }
    plan
}

/// The clips sounding at `frame`, each with the frame its clip starts on.
fn wanted(doc: &Document, frame: FrameIndex) -> Vec<(SoundingClip, FrameIndex)> {
    doc.audio_at(frame)
        .into_iter()
        .filter_map(|c| {
            let start = doc.layers().get(c.layer)?.audio_clip()?.start;
            Some((c, start))
        })
        .collect()
}

/// The output device: not opened yet, open, or failed to open (then we stay
/// silent and don't try again).
enum Output {
    Closed,
    Open(rodio::MixerDeviceSink),
    Failed,
}

/// One clip being played.
struct PlayingClip {
    player: Player,
    asset: AssetId,
    start: FrameIndex,
    volume: f32,
}

/// Plays the document's audio layers.
pub struct AudioPlayer {
    output: Output,
    clips: BTreeMap<LayerId, PlayingClip>,
    /// The snippet of the last scrub; replaced by the next one.
    snippets: Vec<Player>,
}

impl Default for AudioPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioPlayer {
    /// A silent player; the device opens when a sound first plays.
    pub fn new() -> Self {
        Self {
            output: Output::Closed,
            clips: BTreeMap::new(),
            snippets: Vec::new(),
        }
    }

    /// Follows the session's playback; call once per drawn frame, after the
    /// UI ran.
    pub fn update(&mut self, session: &mut ui::Session) {
        let event = session.playback_event();
        let doc = &session.document;
        let frame = session.current_frame();
        match event {
            Some(ui::PlaybackEvent::Started(f) | ui::PlaybackEvent::Seeked(f)) => {
                self.stop();
                self.follow(doc, f);
            }
            Some(ui::PlaybackEvent::Stopped) => self.stop(),
            Some(ui::PlaybackEvent::Scrubbed(f)) => self.scrub(doc, f),
            None if session.playback.is_playing() => self.follow(doc, frame),
            None => {}
        }
    }

    /// Stops every sound.
    fn stop(&mut self) {
        self.clips.clear();
        self.snippets.clear();
    }

    /// Starts and stops players so the clips sounding at `frame` play.
    fn follow(&mut self, doc: &Document, frame: FrameIndex) {
        let wanted = wanted(doc, frame);
        if wanted.is_empty() && self.clips.is_empty() {
            return;
        }
        let playing = self
            .clips
            .iter()
            .map(|(&layer, c)| (layer, (c.asset, c.start, c.volume)))
            .collect();
        let plan = plan(&playing, &wanted);
        for layer in &plan.stop {
            self.clips.remove(layer);
        }
        for (layer, volume) in plan.volume {
            if let Some(clip) = self.clips.get_mut(&layer) {
                clip.player.set_volume(volume);
                clip.volume = volume;
            }
        }
        for clip in plan.start {
            let Some(start) = wanted
                .iter()
                .find(|(c, _)| c.layer == clip.layer)
                .map(|&(_, s)| s)
            else {
                continue;
            };
            if let Some(player) = self.play(doc, &clip, None) {
                self.clips.insert(
                    clip.layer,
                    PlayingClip {
                        player,
                        asset: clip.asset,
                        start,
                        volume: clip.volume,
                    },
                );
            }
        }
    }

    /// Plays a short snippet of every clip sounding at `frame`.
    fn scrub(&mut self, doc: &Document, frame: FrameIndex) {
        self.snippets.clear();
        let length = snippet_length(doc.fps);
        for clip in doc.audio_at(frame) {
            if let Some(player) = self.play(doc, &clip, Some(length)) {
                self.snippets.push(player);
            }
        }
    }

    /// Starts `clip`'s sound at its offset on a new player, for `length` or
    /// to its end. `None` if there is no output or the sound won't decode.
    fn play(
        &mut self,
        doc: &Document,
        clip: &SoundingClip,
        length: Option<Duration>,
    ) -> Option<Player> {
        let data = doc.asset(clip.asset)?.audio()?;
        let mixer = self.mixer()?;
        let source = match decode_at(data, clip.offset) {
            Ok(source) => source,
            Err(err) => {
                tracing::warn!(asset = %clip.asset, %err, "could not decode sound");
                return None;
            }
        };
        let player = Player::connect_new(mixer);
        player.set_volume(clip.volume);
        match length {
            Some(length) => player.append(source.take_duration(length).fade_out(length / 4)),
            None => player.append(source),
        }
        Some(player)
    }

    /// The output mixer, opening the default device on first use.
    fn mixer(&mut self) -> Option<&rodio::mixer::Mixer> {
        if matches!(self.output, Output::Closed) {
            self.output = match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(mut sink) => {
                    sink.log_on_drop(false);
                    tracing::info!("audio output opened");
                    Output::Open(sink)
                }
                Err(err) => {
                    tracing::warn!(%err, "no audio output; audio layers stay silent");
                    Output::Failed
                }
            };
        }
        match &self.output {
            Output::Open(sink) => Some(sink.mixer()),
            Output::Closed | Output::Failed => None,
        }
    }
}

/// A decoder for `data`'s file, moved `offset` seconds in.
fn decode_at(data: &AudioData, offset: f64) -> Result<Box<dyn Source + Send>, String> {
    let bytes: Arc<[u8]> = data.bytes.clone();
    let len = bytes.len() as u64;
    let mut decoder = rodio::Decoder::builder()
        .with_data(Cursor::new(bytes))
        .with_byte_len(len)
        .with_seekable(true)
        .with_hint(data.format.extension())
        .build()
        .map_err(|e| e.to_string())?;
    let offset = Duration::from_secs_f64(offset.max(0.0));
    if offset.is_zero() {
        return Ok(Box::new(decoder));
    }
    match decoder.try_seek(offset) {
        Ok(()) => Ok(Box::new(decoder)),
        // Some files can't seek; decode up to the offset instead.
        Err(_) => Ok(Box::new(decoder.skip_duration(offset))),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn clip(layer: LayerId, asset: AssetId, offset: f64, volume: f32) -> SoundingClip {
        SoundingClip {
            layer,
            asset,
            offset,
            volume,
        }
    }

    #[test]
    fn snippets_last_a_frame_but_at_least_60_ms() {
        assert_eq!(snippet_length(Fps::whole(24).unwrap()), MIN_SNIPPET);
        assert_eq!(
            snippet_length(Fps::whole(10).unwrap()),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn plan_starts_new_clips_and_stops_gone_ones() {
        let (a, b, c) = (LayerId::new(), LayerId::new(), LayerId::new());
        let (song, other) = (AssetId::new(), AssetId::new());
        let playing = BTreeMap::from([
            (a, (song, FrameIndex(0), 1.0)),
            (b, (song, FrameIndex(4), 1.0)),
        ]);
        let wanted = [
            (clip(a, song, 0.5, 0.5), FrameIndex(0)),
            (clip(c, other, 0.0, 1.0), FrameIndex(12)),
        ];
        let plan = plan(&playing, &wanted);
        assert_eq!(plan.start, [wanted[1].0]);
        assert_eq!(plan.stop, [b]);
        assert_eq!(plan.volume, [(a, 0.5)]);

        // Nothing to do when the same clips keep playing.
        let same = [(clip(a, song, 0.9, 1.0), FrameIndex(0))];
        let one = BTreeMap::from([(a, (song, FrameIndex(0), 1.0))]);
        assert_eq!(super::plan(&one, &same), Plan::default());
    }

    #[test]
    fn plan_restarts_a_clip_that_moved_or_changed_sound() {
        let a = LayerId::new();
        let (song, other) = (AssetId::new(), AssetId::new());
        let playing = BTreeMap::from([(a, (song, FrameIndex(0), 1.0))]);
        for wanted in [
            (clip(a, song, 0.0, 1.0), FrameIndex(3)),
            (clip(a, other, 0.0, 1.0), FrameIndex(0)),
        ] {
            let plan = plan(&playing, &[wanted]);
            assert_eq!(plan.stop, [a]);
            assert_eq!(plan.start, [wanted.0]);
        }
    }

    #[test]
    fn decodes_a_wav_from_an_offset() {
        let bytes = crate::import::tests::wav(1000, 1, &[100; 1000]);
        let data = AudioData {
            format: document::AudioFormat::Wav,
            sample_rate: 1000,
            channels: 1,
            duration: 1.0,
            peaks: Vec::new(),
            bytes: bytes.into(),
        };
        assert_eq!(decode_at(&data, 0.0).unwrap().count(), 1000);
        let rest = decode_at(&data, 0.25).unwrap().count();
        assert!((740..=760).contains(&rest), "{rest}");
        let broken = AudioData {
            bytes: Arc::from(vec![1u8, 2, 3]),
            ..data
        };
        assert!(decode_at(&broken, 0.0).is_err());
    }

    #[test]
    fn wanted_pairs_each_clip_with_its_start() {
        use document::scene::{AudioClip, Layer};
        use document::{Asset, AssetKind, CanvasSize};
        let mut doc = Document::new(
            "T",
            CanvasSize {
                width: 4,
                height: 4,
            },
        );
        let sound = Asset::new(
            "s.wav",
            AssetKind::Audio(AudioData {
                format: document::AudioFormat::Wav,
                sample_rate: 1000,
                channels: 1,
                duration: 1.0,
                peaks: Vec::new(),
                bytes: Arc::from(vec![1u8]),
            }),
        );
        let id = sound.id;
        doc.insert_asset(sound);
        let layer = doc.insert_layer(0, Layer::audio("S", AudioClip::new(id, FrameIndex(2))));
        let w = wanted(&doc, FrameIndex(5));
        assert_eq!(w.len(), 1);
        assert_eq!((w[0].0.layer, w[0].1), (layer, FrameIndex(2)));
        assert!(wanted(&doc, FrameIndex(1)).is_empty());
    }
}
