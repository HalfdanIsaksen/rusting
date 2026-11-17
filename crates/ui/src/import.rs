//! File → Import and the library: placing PNG/JPEG pictures on the canvas
//! (ADR-015, ADR-017).
//!
//! The host shows the file dialog and reads the file
//! ([`crate::Request::ImportDialog`]); this module adds the picture to the
//! library, decides where it goes, and places it as one undoable step.

use crate::session::{Request, Session};
use document::animation_core::{AssetId, Vec2};
use document::drawing::{Image, ImageData, ItemId};
use document::{Asset, AssetKind};
use editor::commands::{AddAsset, AddImage, Batch, RemoveAsset, RenameAsset};
use editor::{Command, Tool};

impl Session {
    /// Asks the host to show the import dialog; it then calls
    /// [`Self::import_image`] for each picked file.
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
        let canvas = Vec2::new(
            self.document.canvas.width as f32,
            self.document.canvas.height as f32,
        );
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
