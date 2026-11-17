//! Audio layer commands (ADR-020): adding an audio layer and changing its
//! clip. Removing, renaming, reordering, muting (hiding) and locking audio
//! layers use the ordinary layer commands.

use crate::{Command, CommandError};
use animation_core::LayerId;
use document::Document;
use document::scene::{AudioClip, Layer};

/// Fails unless `clip`'s asset is a sound in the library.
fn check_sound(doc: &Document, clip: &AudioClip) -> Result<(), CommandError> {
    let asset = doc.asset(clip.asset).ok_or(CommandError::AssetNotFound)?;
    if asset.audio().is_none() {
        return Err(CommandError::NotAudio(asset.name.clone()));
    }
    Ok(())
}

/// A finite volume in `0.0..=1.0`.
fn clamp_volume(volume: f32) -> f32 {
    if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// Adds an audio layer playing `clip`, on top of the stack or at an index.
/// Fails with [`CommandError::AssetNotFound`] or [`CommandError::NotAudio`]
/// unless the clip's asset is a library sound.
#[derive(Debug)]
pub struct AddAudioLayer {
    layer: Layer,
    index: Option<usize>,
}

impl AddAudioLayer {
    /// Creates the command for a layer named `name` on top of the stack.
    /// The layer ID is fixed now so redo reuses it.
    pub fn new(name: impl Into<String>, mut clip: AudioClip) -> Self {
        clip.volume = clamp_volume(clip.volume);
        Self {
            layer: Layer::audio(name, clip),
            index: None,
        }
    }

    /// Puts the layer at `index` in the stack (0 = bottom) instead of on top.
    pub fn at(mut self, index: usize) -> Self {
        self.index = Some(index);
        self
    }

    /// ID of the layer this command adds.
    pub fn layer_id(&self) -> LayerId {
        self.layer.id
    }
}

impl Command for AddAudioLayer {
    fn label(&self) -> &str {
        "Add Audio Layer"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let clip = self
            .layer
            .audio_clip()
            .ok_or(CommandError::NotAnAudioLayer)?;
        check_sound(doc, clip)?;
        let index = self.index.unwrap_or(doc.layers().len());
        doc.insert_layer(index, self.layer.clone());
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        doc.remove_layer(self.layer.id)
            .map(|_| ())
            .ok_or(CommandError::LayerNotFound)
    }
}

/// Replaces an audio layer's clip: where it starts, its volume or its
/// sound. Refused on locked layers.
///
/// Commands made with [`SetAudioClip::in_gesture`] for the same layer and
/// gesture merge in the history, so one drag of the clip or of the volume
/// slider undoes as one step.
#[derive(Debug)]
pub struct SetAudioClip {
    layer: LayerId,
    clip: AudioClip,
    old: Option<AudioClip>,
    gesture: Option<u64>,
}

impl SetAudioClip {
    /// Creates the command. The volume is clamped to `0.0..=1.0`.
    pub fn new(layer: LayerId, mut clip: AudioClip) -> Self {
        clip.volume = clamp_volume(clip.volume);
        Self {
            layer,
            clip,
            old: None,
            gesture: None,
        }
    }

    /// Marks the command as one value of the drag or edit `gesture`.
    pub fn in_gesture(mut self, gesture: u64) -> Self {
        self.gesture = Some(gesture);
        self
    }
}

impl Command for SetAudioClip {
    fn label(&self) -> &str {
        "Change Audio Clip"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let layer = doc
            .layers()
            .get(self.layer)
            .ok_or(CommandError::LayerNotFound)?;
        if !layer.is_audio() {
            return Err(CommandError::NotAnAudioLayer);
        }
        if !doc.layers().is_editable(self.layer) {
            return Err(CommandError::LayerLocked);
        }
        check_sound(doc, &self.clip)?;
        let clip = doc
            .layer_mut(self.layer)
            .and_then(Layer::audio_clip_mut)
            .ok_or(CommandError::NotAnAudioLayer)?;
        self.old = Some(std::mem::replace(clip, self.clip));
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let old = self.old.ok_or(CommandError::NotAnAudioLayer)?;
        *doc.layer_mut(self.layer)
            .and_then(Layer::audio_clip_mut)
            .ok_or(CommandError::NotAnAudioLayer)? = old;
        Ok(())
    }

    fn merge(&mut self, next: &dyn Command) -> bool {
        let Some(next) = (next as &dyn std::any::Any).downcast_ref::<Self>() else {
            return false;
        };
        if self.gesture.is_none() || next.gesture != self.gesture || next.layer != self.layer {
            return false;
        }
        self.clip = next.clip;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::History;
    use crate::commands::{
        AddKeyframe, AddStroke, DuplicateFrame, InsertFrames, KeyframeKind, MoveFrames,
        RemoveAsset, RemoveFrames, SetLayerLocked,
    };
    use animation_core::{AssetId, FrameIndex, FrameRange, Vec2};
    use document::drawing::{BrushStyle, Stroke, StrokePoint};
    use document::{Asset, AssetKind, AudioData, AudioFormat, CanvasSize};
    use std::sync::Arc;

    fn sound() -> Asset {
        Asset::new(
            "song.wav",
            AssetKind::Audio(AudioData {
                format: AudioFormat::Wav,
                sample_rate: 8000,
                channels: 1,
                duration: 1.0,
                peaks: vec![10, 20],
                bytes: Arc::from(vec![1u8, 2]),
            }),
        )
    }

    /// A document with a drawing layer, a sound in the library and an
    /// audio layer playing it from frame `start`.
    fn doc(start: u32) -> (Document, LayerId, AssetId) {
        let mut d = Document::new(
            "T",
            CanvasSize {
                width: 10,
                height: 10,
            },
        );
        d.add_layer("Ink");
        let asset = sound();
        let id = asset.id;
        d.insert_asset(asset);
        let mut add = AddAudioLayer::new("Song", AudioClip::new(id, FrameIndex(start)));
        add.apply(&mut d).unwrap();
        (d, add.layer_id(), id)
    }

    fn start_of(d: &Document, layer: LayerId) -> u32 {
        d.layers().get(layer).unwrap().audio_clip().unwrap().start.0
    }

    /// Applies `cmd`, undoes it and checks the document is back as it was.
    fn roundtrip(d: &mut Document, mut cmd: impl Command) {
        let before = d.clone();
        cmd.apply(d).unwrap();
        assert_ne!(*d, before, "{} changed nothing", cmd.label());
        cmd.undo(d).unwrap();
        assert_eq!(*d, before, "{} did not undo", cmd.label());
    }

    #[test]
    fn add_audio_layer_roundtrips_and_needs_a_sound() {
        let (mut d, _, sound_id) = doc(0);
        let mut cmd = AddAudioLayer::new("Again", AudioClip::new(sound_id, FrameIndex(4))).at(0);
        let before = d.clone();
        cmd.apply(&mut d).unwrap();
        let layer = d.layers().iter().next().unwrap();
        assert_eq!(layer.id, cmd.layer_id());
        assert_eq!(layer.audio_clip().unwrap().start, FrameIndex(4));
        assert!(d.track(cmd.layer_id()).unwrap().is_empty());
        cmd.undo(&mut d).unwrap();
        assert_eq!(d, before);

        let missing = AssetId::new();
        let mut cmd = AddAudioLayer::new("X", AudioClip::new(missing, FrameIndex(0)));
        assert_eq!(cmd.apply(&mut d), Err(CommandError::AssetNotFound));
        let picture = Asset::new(
            "pic.png",
            AssetKind::Image(document::drawing::ImageData::new(1, 1, vec![1])),
        );
        let picture_id = picture.id;
        d.insert_asset(picture);
        let mut cmd = AddAudioLayer::new("X", AudioClip::new(picture_id, FrameIndex(0)));
        assert_eq!(
            cmd.apply(&mut d),
            Err(CommandError::NotAudio("pic.png".into()))
        );
    }

    #[test]
    fn set_audio_clip_roundtrips_merges_and_respects_locks() {
        let (mut d, layer, sound_id) = doc(0);
        let mut clip = AudioClip::new(sound_id, FrameIndex(5));
        clip.volume = 3.0;
        roundtrip(&mut d, SetAudioClip::new(layer, clip));

        let mut history = History::new();
        let before = d.clone();
        for start in 1..4 {
            history
                .execute(
                    &mut d,
                    Box::new(
                        SetAudioClip::new(layer, AudioClip::new(sound_id, FrameIndex(start)))
                            .in_gesture(7),
                    ),
                )
                .unwrap();
        }
        assert_eq!(start_of(&d, layer), 3);
        assert_eq!(history.undo_len(), 1, "one drag is one step");
        history.undo(&mut d).unwrap();
        assert_eq!(d, before);
        for gesture in [None, None, Some(1), Some(2)] {
            let mut cmd = SetAudioClip::new(layer, AudioClip::new(sound_id, FrameIndex(2)));
            if let Some(g) = gesture {
                cmd = cmd.in_gesture(g);
            }
            history.execute(&mut d, Box::new(cmd)).unwrap();
        }
        assert_eq!(history.undo_len(), 4, "separate edits are separate steps");

        let mut cmd = SetAudioClip::new(layer, clip);
        cmd.apply(&mut d).unwrap();
        assert_eq!(
            d.layers().get(layer).unwrap().audio_clip().unwrap().volume,
            1.0,
            "volume is clamped"
        );
        cmd.undo(&mut d).unwrap();

        SetLayerLocked::new(layer, true).apply(&mut d).unwrap();
        assert_eq!(
            SetAudioClip::new(layer, clip).apply(&mut d),
            Err(CommandError::LayerLocked)
        );
        let ink = d.layers().iter().next().unwrap().id;
        assert_eq!(
            SetAudioClip::new(ink, clip).apply(&mut d),
            Err(CommandError::NotAnAudioLayer)
        );
    }

    #[test]
    fn drawing_commands_refuse_audio_layers() {
        let (mut d, layer, _) = doc(0);
        let stroke = Stroke::new(
            vec![StrokePoint::new(Vec2::ZERO), StrokePoint::new(Vec2::ONE)],
            BrushStyle::default(),
        );
        assert_eq!(
            AddStroke::new(layer, FrameIndex(0), stroke).apply(&mut d),
            Err(CommandError::AudioLayer)
        );
        assert_eq!(
            AddKeyframe::new(layer, FrameIndex(0), KeyframeKind::Drawing).apply(&mut d),
            Err(CommandError::AudioLayer)
        );
        assert!(d.track(layer).unwrap().is_empty());
    }

    #[test]
    fn sound_in_use_by_an_audio_layer_cannot_be_removed() {
        let (mut d, _, sound_id) = doc(0);
        assert_eq!(
            RemoveAsset::new(sound_id).apply(&mut d),
            Err(CommandError::AssetInUse("song.wav".into()))
        );
    }

    #[test]
    fn inserting_frames_moves_clips_that_start_at_or_after() {
        let (mut d, layer, _) = doc(4);
        roundtrip(&mut d, InsertFrames::new(FrameIndex(2), 3));
        let mut cmd = InsertFrames::new(FrameIndex(4), 2);
        cmd.apply(&mut d).unwrap();
        assert_eq!(start_of(&d, layer), 6);
        cmd.undo(&mut d).unwrap();
        let mut cmd = InsertFrames::new(FrameIndex(5), 2);
        cmd.apply(&mut d).unwrap();
        assert_eq!(start_of(&d, layer), 4, "a clip starting earlier stays");
    }

    #[test]
    fn duplicating_a_frame_moves_later_clips() {
        let (mut d, layer, _) = doc(4);
        roundtrip(&mut d, DuplicateFrame::new(FrameIndex(1)));
        let mut cmd = DuplicateFrame::new(FrameIndex(1));
        cmd.apply(&mut d).unwrap();
        assert_eq!(start_of(&d, layer), 5);
    }

    #[test]
    fn removing_frames_moves_later_clips_back() {
        let (mut d, layer, _) = doc(10);
        roundtrip(
            &mut d,
            RemoveFrames::new(FrameRange::new(FrameIndex(2), FrameIndex(5))),
        );
        let mut cmd = RemoveFrames::new(FrameRange::new(FrameIndex(2), FrameIndex(5)));
        cmd.apply(&mut d).unwrap();
        assert_eq!(start_of(&d, layer), 7);
        cmd.undo(&mut d).unwrap();
        // A clip starting inside the removed frames starts where they were.
        let mut cmd = RemoveFrames::new(FrameRange::new(FrameIndex(8), FrameIndex(12)));
        cmd.apply(&mut d).unwrap();
        assert_eq!(start_of(&d, layer), 8);
        cmd.undo(&mut d).unwrap();
        assert_eq!(start_of(&d, layer), 10);
    }

    #[test]
    fn moving_frames_moves_clips_starting_inside_them() {
        let (mut d, layer, _) = doc(3);
        roundtrip(
            &mut d,
            MoveFrames::new(
                FrameRange::new(FrameIndex(2), FrameIndex(4)),
                FrameIndex(10),
            ),
        );
        let mut cmd = MoveFrames::new(
            FrameRange::new(FrameIndex(2), FrameIndex(4)),
            FrameIndex(10),
        );
        cmd.apply(&mut d).unwrap();
        assert_eq!(start_of(&d, layer), 11);
        cmd.undo(&mut d).unwrap();
        assert_eq!(start_of(&d, layer), 3);

        // A clip already sitting where the frames land stays put, on undo too.
        let (mut d, layer, _) = doc(11);
        let before = d.clone();
        let mut cmd = MoveFrames::new(
            FrameRange::new(FrameIndex(2), FrameIndex(4)),
            FrameIndex(10),
        );
        cmd.apply(&mut d).unwrap();
        assert_eq!(start_of(&d, layer), 11);
        cmd.undo(&mut d).unwrap();
        assert_eq!(d, before);
    }
}
