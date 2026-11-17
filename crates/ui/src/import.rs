//! File → Import and the library: placing PNG/JPEG pictures on the canvas
//! (ADR-015, ADR-017) and MP3/WAV sounds on audio layers (ADR-020).
//!
//! The host shows the file dialog and reads (and, for sounds, decodes) the
//! file ([`crate::Request::ImportDialog`]); this module adds it to the
//! library, decides where it goes, and places it as one undoable step.

use crate::session::{Request, Session};
use document::animation_core::{AssetId, FrameIndex, LayerId, Vec2};
use document::drawing::{Image, ImageData, ItemId};
use document::scene::AudioClip;
use document::{Asset, AssetKind, AudioData};
use editor::commands::{AddAsset, AddAudioLayer, AddImage, Batch, RemoveAsset, RenameAsset};
use editor::{Command, Tool};

impl Session {
    /// Asks the host to show the import dialog; it then calls
    /// [`Self::import_image`] or [`Self::import_audio`] for each picked file.
    pub fn import(&mut self) {
        self.requests.push(Request::ImportDialog);
    }

    /// Adds `data` to the library under `name` (the file name) and places
    /// it on the active layer at the current frame, centred on the canvas
    /// and shrunk (keeping its proportions) if it is larger than the
    /// canvas; one undo step. The picture is then selected with the
    /// Selection tool so it can be moved straight away. Returns false, with
    /// a status message, if the layer can't be drawn on.
    pub fn import_image(&mut self, name: &str, data: ImageData) -> bool {
        let asset = Asset::new(name, AssetKind::Image(data));
        let add = Box::new(AddAsset::new(asset.clone()));
        let placed = self
            .place_picture(&asset, None, "Import Image", Some(add))
            .is_some();
        if placed {
            self.set_status(format!("Imported {name}"));
        }
        placed
    }

    /// Adds sound `data` to the library under `name` (the file name) and
    /// puts it on a new audio layer on top, starting at the first frame;
    /// one undo step. The active layer stays as it was, so drawing carries
    /// on. Returns the new layer's ID.
    pub fn import_audio(&mut self, name: &str, data: AudioData) -> Option<LayerId> {
        let asset = Asset::new(name, AssetKind::Audio(data));
        let add = Box::new(AddAsset::new(asset.clone()));
        let layer = self.add_audio_layer(&asset, FrameIndex(0), "Import Audio", Some(add))?;
        self.set_status(format!("Imported {name} on a new audio layer"));
        Some(layer)
    }

    /// Puts library sound `asset` on a new audio layer on top, starting at
    /// `frame` (clamped to the animation); one undo step. Returns the new
    /// layer's ID, or `None` (with a status message) if it is not a sound.
    pub fn place_audio(&mut self, asset: AssetId, frame: FrameIndex) -> Option<LayerId> {
        let asset = self.document.asset(asset).cloned()?;
        let last = self.document.last_frame().unwrap_or(FrameIndex(0));
        let layer = self.add_audio_layer(&asset, frame.min(last), "Place Audio", None)?;
        self.set_status(format!(
            "Placed {} from frame {}",
            asset.name,
            frame.min(last).display_number()
        ));
        Some(layer)
    }

    /// Runs `first` (if any) and an [`AddAudioLayer`] for `asset` starting at
    /// `start` as one undo step labelled `label`.
    fn add_audio_layer(
        &mut self,
        asset: &Asset,
        start: FrameIndex,
        label: &str,
        first: Option<Box<dyn Command>>,
    ) -> Option<LayerId> {
        if asset.audio().is_none() {
            self.set_error(format!("{label}: “{}” is not a sound", asset.name));
            return None;
        }
        let add = AddAudioLayer::new(asset.name.clone(), AudioClip::new(asset.id, start));
        let layer = add.layer_id();
        let mut commands: Vec<Box<dyn Command>> = first.into_iter().collect();
        commands.push(Box::new(add));
        self.execute(Box::new(Batch::new(label, commands))).ok()?;
        Some(layer)
    }

    /// Places library picture `asset` on the active layer at the current
    /// frame, centred on `centre` (document coordinates; the canvas centre
    /// if `None`) and shrunk to fit the canvas like an import; one undo
    /// step. The picture is selected with the Selection tool. Returns false,
    /// with a status message, if the asset is not a picture or the layer
    /// can't be drawn on.
    pub fn place_asset(&mut self, asset: AssetId, centre: Option<Vec2>) -> bool {
        let Some(asset) = self.document.asset(asset).cloned() else {
            return false;
        };
        let placed = self.place_picture(&asset, centre, "Place Image", None);
        if placed.is_some() {
            self.set_status(format!("Placed {}", asset.name));
        }
        placed.is_some()
    }

    /// The size (document units) library picture `asset` gets when placed:
    /// its pixel size, shrunk to fit the canvas. `None` if it is not a
    /// picture. The canvas uses it to outline a drop before it happens.
    pub fn placed_size(&self, asset: AssetId) -> Option<Vec2> {
        let data = self.document.asset(asset)?.image()?;
        Some(fit_within(
            Vec2::new(data.width.max(1) as f32, data.height.max(1) as f32),
            self.canvas_size(),
        ))
    }

    fn canvas_size(&self) -> Vec2 {
        Vec2::new(
            self.document.canvas.width as f32,
            self.document.canvas.height as f32,
        )
    }

    /// Renames library asset `asset`; one undo step. A blank or unchanged
    /// name does nothing.
    pub fn rename_asset(&mut self, asset: AssetId, name: &str) {
        let name = name.trim();
        if name.is_empty() || self.document.asset(asset).is_none_or(|a| a.name == name) {
            return;
        }
        let _ = self.execute(Box::new(RenameAsset::new(asset, name)));
    }

    /// Deletes library asset `asset`; one undo step. Refused, with a status
    /// message, while the animation shows a copy of it.
    pub fn delete_asset(&mut self, asset: AssetId) -> bool {
        let Some(name) = self.document.asset(asset).map(|a| a.name.clone()) else {
            return false;
        };
        let deleted = self.execute(Box::new(RemoveAsset::new(asset))).is_ok();
        if deleted {
            self.set_status(format!("Deleted {name} from the library"));
        }
        deleted
    }

    /// Runs `first` (if any) and an [`AddImage`] of `asset`'s picture as one
    /// undo step labelled `label`, then selects the picture. Returns its
    /// item ID.
    fn place_picture(
        &mut self,
        asset: &Asset,
        centre: Option<Vec2>,
        label: &str,
        first: Option<Box<dyn Command>>,
    ) -> Option<ItemId> {
        if let Some(reason) = self.draw_block_reason() {
            self.set_error(format!("{label} {}: {reason}", asset.name));
            return None;
        }
        let Some(data) = asset.image().cloned() else {
            self.set_error(format!("{label}: “{}” is not a picture", asset.name));
            return None;
        };
        let layer = self.state.active_layer?;
        let canvas = self.canvas_size();
        let size = fit_within(
            Vec2::new(data.width.max(1) as f32, data.height.max(1) as f32),
            canvas,
        );
        let centre = centre.unwrap_or(canvas * 0.5);
        let image = Image::new(data, centre - size * 0.5, size).with_asset(asset.id);
        let id = ItemId::Image(image.id);
        let frame = self.current_frame();
        let mut commands: Vec<Box<dyn Command>> = first.into_iter().collect();
        commands.push(Box::new(AddImage::new(layer, frame, image)));
        self.execute(Box::new(Batch::new(label, commands))).ok()?;
        self.state.tool = Tool::Select;
        self.tools.set_selection((layer, frame), vec![id]);
        Some(id)
    }
}

/// `size` scaled down uniformly to fit inside `bounds`; unchanged if it
/// already fits.
fn fit_within(size: Vec2, bounds: Vec2) -> Vec2 {
    let scale = (bounds / size).min_element().min(1.0);
    size * scale
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::commands::SetLayerLocked;

    fn v(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    #[test]
    fn fit_keeps_proportions_and_never_enlarges() {
        assert_eq!(
            fit_within(v(100.0, 50.0), v(1920.0, 1080.0)),
            v(100.0, 50.0)
        );
        assert_eq!(
            fit_within(v(4000.0, 1000.0), v(2000.0, 2000.0)),
            v(2000.0, 500.0)
        );
        assert_eq!(
            fit_within(v(1000.0, 3000.0), v(2000.0, 1500.0)),
            v(500.0, 1500.0)
        );
    }

    #[test]
    fn import_menu_asks_the_host() {
        let mut s = Session::new();
        s.import();
        assert_eq!(s.take_requests(), [Request::ImportDialog]);
    }

    #[test]
    fn imported_image_is_centred_selected_and_undoable() {
        let mut s = Session::new();
        let canvas = v(
            s.document.canvas.width as f32,
            s.document.canvas.height as f32,
        );
        assert!(s.import_image("pic.png", ImageData::new(200, 100, vec![1])));
        let drawing = s.active_drawing().unwrap();
        let image = &drawing.images()[0];
        let (min, max) = image.bounds();
        assert!(((min + max) * 0.5 - canvas * 0.5).length() < 1e-3);
        assert_eq!(max - min, v(200.0, 100.0));
        assert_eq!(s.state.tool, Tool::Select);
        assert_eq!(s.tools.selection(), [ItemId::Image(image.id)]);
        // The picture went into the library, named after the file.
        let asset = image.asset.unwrap();
        assert_eq!(s.document.asset(asset).unwrap().name, "pic.png");
        assert!(s.document.asset_in_use(asset));

        // One undo step takes back both.
        s.undo();
        assert!(s.active_drawing().is_none_or(|d| d.images().is_empty()));
        assert!(s.document.assets().is_empty());
    }

    #[test]
    fn library_picture_is_placed_centred_on_a_point() {
        let mut s = Session::new();
        assert!(s.import_image("pic.png", ImageData::new(40, 20, vec![1])));
        let asset = *s.document.assets().keys().next().unwrap();
        assert!(s.place_asset(asset, Some(v(100.0, 50.0))));
        let drawing = s.active_drawing().unwrap();
        let placed = &drawing.images()[1];
        assert_eq!(placed.asset, Some(asset));
        assert_eq!(placed.bounds(), (v(80.0, 40.0), v(120.0, 60.0)));
        assert_eq!(s.placed_size(asset), Some(v(40.0, 20.0)));
        assert_eq!(s.tools.selection(), [ItemId::Image(placed.id)]);
        assert_eq!(s.document.assets().len(), 1);
        s.undo();
        assert_eq!(s.active_drawing().unwrap().images().len(), 1);
        assert!(!s.place_asset(AssetId::new(), None));
    }

    #[test]
    fn library_assets_rename_and_delete_only_when_unused() {
        let mut s = Session::new();
        assert!(s.import_image("pic.png", ImageData::new(4, 4, vec![1])));
        let asset = *s.document.assets().keys().next().unwrap();
        s.rename_asset(asset, "  Sky ");
        assert_eq!(s.document.asset(asset).unwrap().name, "Sky");
        let steps = s.history.undo_len();
        s.rename_asset(asset, "Sky");
        s.rename_asset(asset, " ");
        assert_eq!(s.history.undo_len(), steps, "no-op renames add no step");

        assert!(!s.delete_asset(asset), "in use");
        assert!(s.document.asset(asset).is_some());
        s.delete_selection();
        assert!(s.delete_asset(asset));
        assert!(s.document.assets().is_empty());
        s.undo();
        assert_eq!(s.document.asset(asset).unwrap().name, "Sky");
    }

    fn sound(duration: f64) -> AudioData {
        AudioData {
            format: document::AudioFormat::Wav,
            sample_rate: 8000,
            channels: 1,
            duration,
            peaks: vec![1, 2, 3],
            bytes: std::sync::Arc::from(vec![1u8, 2, 3]),
        }
    }

    #[test]
    fn imported_sound_gets_an_audio_layer_in_one_undo_step() {
        let mut s = Session::new();
        let active = s.state.active_layer;
        let layer = s.import_audio("song.wav", sound(2.0)).unwrap();
        let top = s.document.layers().iter().next_back().unwrap();
        assert_eq!(top.id, layer);
        assert_eq!(top.name, "song.wav");
        let clip = *top.audio_clip().unwrap();
        assert_eq!(clip.start, FrameIndex(0));
        assert_eq!(s.document.asset(clip.asset).unwrap().name, "song.wav");
        assert!(s.document.asset_in_use(clip.asset));
        assert_eq!(s.state.active_layer, active, "drawing carries on");
        s.undo();
        assert!(s.document.assets().is_empty());
        assert!(s.document.layers().get(layer).is_none());
    }

    #[test]
    fn library_sound_is_placed_at_a_frame() {
        let mut s = Session::new();
        let first = s.import_audio("song.wav", sound(2.0)).unwrap();
        let asset = s
            .document
            .layers()
            .get(first)
            .unwrap()
            .audio_clip()
            .unwrap()
            .asset;
        let layer = s.place_audio(asset, FrameIndex(5)).unwrap();
        let clip = s
            .document
            .layers()
            .get(layer)
            .unwrap()
            .audio_clip()
            .unwrap();
        assert_eq!(clip.start, FrameIndex(5));
        let layer = s.place_audio(asset, FrameIndex(999)).unwrap();
        let clip = s
            .document
            .layers()
            .get(layer)
            .unwrap()
            .audio_clip()
            .unwrap();
        assert_eq!(clip.start, s.document.last_frame().unwrap(), "clamped");
        assert!(s.place_audio(AssetId::new(), FrameIndex(0)).is_none());
        // A picture is not a sound, and dropping one on the timeline does
        // nothing.
        assert!(s.import_image("pic.png", ImageData::new(2, 2, vec![1])));
        let picture = s
            .document
            .assets()
            .values()
            .find(|a| a.image().is_some())
            .unwrap()
            .id;
        let steps = s.history.undo_len();
        assert!(s.place_audio(picture, FrameIndex(0)).is_none());
        assert_eq!(s.history.undo_len(), steps);
    }

    #[test]
    fn locked_layer_refuses_import() {
        let mut s = Session::new();
        let layer = s.state.active_layer.unwrap();
        s.execute(Box::new(SetLayerLocked::new(layer, true)))
            .unwrap();
        assert!(!s.import_image("pic.png", ImageData::new(2, 2, vec![1])));
        assert!(s.active_drawing().is_none_or(|d| d.images().is_empty()));
        assert!(s.document.assets().is_empty());
    }
}
