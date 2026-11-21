//! Library commands: adding, removing, renaming assets (ADR-017) and
//! editing brushes (ADR-024).

use crate::{Command, CommandError};
use animation_core::AssetId;
use document::drawing::BrushData;
use document::{Asset, AssetKind, Document};

/// Adds an asset to the library. Undo removes it again.
#[derive(Debug)]
pub struct AddAsset {
    asset: Asset,
}

impl AddAsset {
    /// Creates the command. The asset keeps its own ID, so redo reuses it.
    pub fn new(asset: Asset) -> Self {
        Self { asset }
    }

    /// ID of the asset this command adds.
    pub fn asset_id(&self) -> AssetId {
        self.asset.id
    }
}

impl Command for AddAsset {
    fn label(&self) -> &str {
        "Add to Library"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        if doc.asset(self.asset.id).is_some() {
            return Err(CommandError::AssetExists);
        }
        doc.insert_asset(self.asset.clone());
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        doc.remove_asset(self.asset.id)
            .map(|_| ())
            .ok_or(CommandError::AssetNotFound)
    }
}

/// Removes an asset from the library. Refused with
/// [`CommandError::AssetInUse`] while the animation shows something placed
/// from it ([`Document::asset_in_use`]).
#[derive(Debug)]
pub struct RemoveAsset {
    id: AssetId,
    removed: Option<Asset>,
}

impl RemoveAsset {
    /// Creates the command.
    pub fn new(id: AssetId) -> Self {
        Self { id, removed: None }
    }
}

impl Command for RemoveAsset {
    fn label(&self) -> &str {
        "Delete from Library"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let asset = doc.asset(self.id).ok_or(CommandError::AssetNotFound)?;
        if doc.asset_in_use(self.id) {
            return Err(CommandError::AssetInUse(asset.name.clone()));
        }
        self.removed = doc.remove_asset(self.id);
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let asset = self.removed.take().ok_or(CommandError::AssetNotFound)?;
        doc.insert_asset(asset);
        Ok(())
    }
}

/// Renames a library asset.
#[derive(Debug)]
pub struct RenameAsset {
    id: AssetId,
    name: String,
    old: Option<String>,
}

impl RenameAsset {
    /// Creates the command.
    pub fn new(id: AssetId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            old: None,
        }
    }
}

impl Command for RenameAsset {
    fn label(&self) -> &str {
        "Rename Asset"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let asset = doc.asset_mut(self.id).ok_or(CommandError::AssetNotFound)?;
        self.old = Some(std::mem::replace(&mut asset.name, self.name.clone()));
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let old = self.old.take().ok_or(CommandError::AssetNotFound)?;
        doc.asset_mut(self.id)
            .ok_or(CommandError::AssetNotFound)?
            .name = old;
        Ok(())
    }
}

/// Replaces a library brush's tip, grain and settings (the Brush Editor's
/// Apply). Every stroke drawn with the brush takes the new look. Settings are
/// forced into range. Refused with [`CommandError::NotABrush`] for another
/// kind of asset.
#[derive(Debug)]
pub struct SetBrush {
    id: AssetId,
    data: BrushData,
    old: Option<BrushData>,
}

impl SetBrush {
    /// Creates the command.
    pub fn new(id: AssetId, mut data: BrushData) -> Self {
        data.settings = data.settings.clamped();
        Self {
            id,
            data,
            old: None,
        }
    }
}

impl Command for SetBrush {
    fn label(&self) -> &str {
        "Edit Brush"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let asset = doc.asset_mut(self.id).ok_or(CommandError::AssetNotFound)?;
        let AssetKind::Brush(brush) = &mut asset.kind else {
            return Err(CommandError::NotABrush);
        };
        self.old = Some(std::mem::replace(brush, self.data.clone()));
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let old = self.old.take().ok_or(CommandError::AssetNotFound)?;
        let asset = doc.asset_mut(self.id).ok_or(CommandError::AssetNotFound)?;
        let AssetKind::Brush(brush) = &mut asset.kind else {
            return Err(CommandError::NotABrush);
        };
        *brush = old;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::History;
    use crate::commands::{AddImage, Batch, RemoveImage};
    use animation_core::{FrameIndex, LayerId, Vec2};
    use document::drawing::{Image, ImageData};
    use document::{AssetKind, CanvasSize};

    fn doc() -> (Document, LayerId) {
        let mut d = Document::new(
            "T",
            CanvasSize {
                width: 100,
                height: 100,
            },
        );
        let layer = d.add_layer("L");
        (d, layer)
    }

    fn asset() -> Asset {
        Asset::new(
            "pic.png",
            AssetKind::Image(ImageData::new(2, 2, vec![1, 2, 3])),
        )
    }

    #[test]
    fn add_asset_undoes_to_the_original() {
        let (mut d, _) = doc();
        let original = d.clone();
        let mut h = History::new();
        let a = asset();
        let id = a.id;
        h.execute(&mut d, Box::new(AddAsset::new(a.clone())))
            .unwrap();
        assert_eq!(d.asset(id), Some(&a));
        h.undo(&mut d).unwrap();
        assert_eq!(d, original);
        h.redo(&mut d).unwrap();
        assert_eq!(d.asset(id), Some(&a));
        // The same asset can't be added twice.
        let err = h.execute(&mut d, Box::new(AddAsset::new(a))).unwrap_err();
        assert_eq!(err, CommandError::AssetExists);
    }

    #[test]
    fn remove_asset_undoes_to_the_original() {
        let (mut d, _) = doc();
        let a = asset();
        let id = a.id;
        d.insert_asset(a);
        let original = d.clone();
        let mut h = History::new();
        h.execute(&mut d, Box::new(RemoveAsset::new(id))).unwrap();
        assert!(d.asset(id).is_none());
        h.undo(&mut d).unwrap();
        assert_eq!(d, original);
        let err = h
            .execute(&mut d, Box::new(RemoveAsset::new(AssetId::new())))
            .unwrap_err();
        assert_eq!(err, CommandError::AssetNotFound);
    }

    #[test]
    fn rename_asset_undoes_to_the_original() {
        let (mut d, _) = doc();
        let a = asset();
        let id = a.id;
        d.insert_asset(a);
        let original = d.clone();
        let mut h = History::new();
        h.execute(&mut d, Box::new(RenameAsset::new(id, "Sky")))
            .unwrap();
        assert_eq!(d.asset(id).unwrap().name, "Sky");
        h.undo(&mut d).unwrap();
        assert_eq!(d, original);
    }

    fn brush() -> Asset {
        Asset::new(
            "Chalk",
            AssetKind::Brush(BrushData {
                tip: ImageData::new(2, 2, vec![1, 2, 3]),
                grain: None,
                settings: Default::default(),
            }),
        )
    }

    #[test]
    fn set_brush_undoes_to_the_original() {
        let (mut d, _) = doc();
        let a = brush();
        let id = a.id;
        d.insert_asset(a);
        let original = d.clone();
        let mut h = History::new();
        let mut data = d.asset(id).unwrap().brush().unwrap().clone();
        data.settings.spacing = 0.5;
        data.settings.scatter = 7.0; // out of range: clamped
        data.grain = Some(ImageData::new(1, 1, vec![9]));
        h.execute(&mut d, Box::new(SetBrush::new(id, data)))
            .unwrap();
        let set = d.asset(id).unwrap().brush().unwrap();
        assert_eq!(set.settings.spacing, 0.5);
        assert_eq!(set.settings.scatter, 1.0);
        assert!(set.grain.is_some());
        h.undo(&mut d).unwrap();
        assert_eq!(d, original);
        h.redo(&mut d).unwrap();
        assert_eq!(d.asset(id).unwrap().brush().unwrap().settings.spacing, 0.5);
    }

    #[test]
    fn set_brush_refuses_other_assets() {
        let (mut d, _) = doc();
        let a = asset();
        let id = a.id;
        d.insert_asset(a);
        let original = d.clone();
        let mut h = History::new();
        let data = brush().brush().unwrap().clone();
        let err = h
            .execute(&mut d, Box::new(SetBrush::new(id, data.clone())))
            .unwrap_err();
        assert_eq!(err, CommandError::NotABrush);
        let err = h
            .execute(&mut d, Box::new(SetBrush::new(AssetId::new(), data)))
            .unwrap_err();
        assert_eq!(err, CommandError::AssetNotFound);
        assert_eq!(d, original);
    }

    #[test]
    fn asset_in_use_cannot_be_removed() {
        let (mut d, layer) = doc();
        let mut h = History::new();
        let a = asset();
        let id = a.id;
        let data = a.image().unwrap().clone();
        let image = Image::new(data, Vec2::ZERO, Vec2::splat(10.0)).with_asset(id);
        let image_id = image.id;
        h.execute(
            &mut d,
            Box::new(Batch::new(
                "Import",
                vec![
                    Box::new(AddAsset::new(a)),
                    Box::new(AddImage::new(layer, FrameIndex(0), image)),
                ],
            )),
        )
        .unwrap();
        assert!(d.asset_in_use(id));
        let err = h
            .execute(&mut d, Box::new(RemoveAsset::new(id)))
            .unwrap_err();
        assert_eq!(err, CommandError::AssetInUse("pic.png".into()));
        assert!(d.asset(id).is_some());

        // Once the placed copy is gone the asset can go.
        h.execute(
            &mut d,
            Box::new(RemoveImage::new(layer, FrameIndex(0), image_id)),
        )
        .unwrap();
        h.execute(&mut d, Box::new(RemoveAsset::new(id))).unwrap();
        assert!(d.asset(id).is_none());

        // Undo: the asset comes back, then the placed copy, then the import goes.
        h.undo(&mut d).unwrap();
        h.undo(&mut d).unwrap();
        h.undo(&mut d).unwrap();
        assert!(d.assets().is_empty());
        assert!(d.drawing_at(layer, FrameIndex(0)).is_none());
    }
}
