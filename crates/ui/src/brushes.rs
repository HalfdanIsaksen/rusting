//! Stamp brushes (ADR-022): the built-in presets, picking the brush the
//! Brush tool draws with, and importing and exporting library brushes.
//!
//! The brush the Brush tool uses is `state.brush.brush`: `None` for the
//! solid vector brush, or the asset ID of a library brush. A built-in preset
//! has a fixed ID; it is added to the document's library, in the same undo
//! step, the first time a stroke is drawn with it ([`Session::brush_asset`]),
//! so a project file always holds the brushes its strokes use.
//!
//! Decoding and reading `.rustbrush` files is the host's job: it hands the
//! presets over with [`Session::set_brush_presets`] and imported brushes to
//! [`Session::import_brush`].

use crate::session::{Request, Session};
use document::animation_core::AssetId;
use document::drawing::BrushData;
use document::{Asset, AssetKind};
use editor::Command;
use editor::commands::AddAsset;

/// A brush that ships with the app.
#[derive(Clone, Debug, PartialEq)]
pub struct BrushPreset {
    /// Fixed ID, the same in every project the brush is used in.
    pub id: AssetId,
    /// Display name, e.g. "Pencil (HB)".
    pub name: String,
    /// Tip, grain and settings.
    pub data: BrushData,
}

impl BrushPreset {
    /// The preset as a library asset.
    pub fn asset(&self) -> Asset {
        Asset {
            id: self.id,
            name: self.name.clone(),
            kind: AssetKind::Brush(self.data.clone()),
        }
    }
}

/// One brush the Brush tool can draw with, for the picker.
#[derive(Clone, Debug, PartialEq)]
pub struct BrushOption {
    /// `None` for the solid vector brush.
    pub id: Option<AssetId>,
    /// Display name.
    pub name: String,
    /// The brush, `None` for the solid vector brush.
    pub data: Option<BrushData>,
}

/// The name of the solid (unbrushed) vector brush in the picker.
pub const SOLID_BRUSH_NAME: &str = "Vector (solid)";

impl Session {
    /// Gives the session the built-in brushes (the host decodes them at
    /// startup).
    pub fn set_brush_presets(&mut self, presets: Vec<BrushPreset>) {
        self.tools.brush_presets = presets;
    }

    /// The built-in brushes.
    pub fn brush_presets(&self) -> &[BrushPreset] {
        &self.tools.brush_presets
    }

    /// Every brush the picker offers: the solid brush, the presets, then the
    /// document's own library brushes that are not presets, by name.
    pub fn brush_options(&self) -> Vec<BrushOption> {
        let mut out = vec![BrushOption {
            id: None,
            name: SOLID_BRUSH_NAME.to_owned(),
            data: None,
        }];
        for p in &self.tools.brush_presets {
            // A preset in the library may have been renamed there.
            let name = self
                .document
                .asset(p.id)
                .map_or_else(|| p.name.clone(), |a| a.name.clone());
            out.push(BrushOption {
                id: Some(p.id),
                name,
                data: Some(p.data.clone()),
            });
        }
        let mut own: Vec<&Asset> = self
            .document
            .assets()
            .values()
            .filter(|a| a.brush().is_some() && !self.is_preset(a.id))
            .collect();
        own.sort_by(|a, b| a.name.cmp(&b.name));
        out.extend(own.into_iter().map(|a| BrushOption {
            id: Some(a.id),
            name: a.name.clone(),
            data: a.brush().cloned(),
        }));
        out
    }

    pub(crate) fn is_preset(&self, id: AssetId) -> bool {
        self.tools.brush_presets.iter().any(|p| p.id == id)
    }

    /// The brush the Brush tool draws with (`None`: solid).
    pub fn brush(&self) -> Option<AssetId> {
        self.state.brush.brush
    }

    /// Picks the brush the Brush tool draws with. An ID that is neither a
    /// library brush nor a preset picks the solid brush.
    pub fn set_brush(&mut self, brush: Option<AssetId>) {
        self.state.brush.brush = brush.filter(|&id| self.brush_exists(id));
    }

    /// Whether `id` is a library brush or a preset.
    fn brush_exists(&self, id: AssetId) -> bool {
        self.document.asset(id).is_some_and(|a| a.brush().is_some()) || self.is_preset(id)
    }

    /// Forgets the picked brush if it no longer exists (after a document is
    /// opened, or the brush deleted or its import undone).
    pub(crate) fn check_brush(&mut self) {
        if let Some(id) = self.state.brush.brush
            && !self.brush_exists(id)
        {
            self.state.brush.brush = None;
        }
        self.check_brush_editor();
    }

    /// The command that adds preset `brush` to the library, if a stroke
    /// drawn with it needs one (it is a preset not yet in the document).
    pub(crate) fn brush_asset(&self, brush: Option<AssetId>) -> Option<Box<dyn Command>> {
        let id = brush?;
        if self.document.asset(id).is_some() {
            return None;
        }
        let preset = self.tools.brush_presets.iter().find(|p| p.id == id)?;
        Some(Box::new(AddAsset::new(preset.asset())))
    }

    /// Adds brush `data` to the library under `name` and picks it for the
    /// Brush tool; one undo step. Returns its asset ID.
    pub fn import_brush(&mut self, name: &str, data: BrushData) -> Option<AssetId> {
        let asset = Asset::new(name, AssetKind::Brush(data));
        let id = asset.id;
        self.execute(Box::new(AddAsset::new(asset))).ok()?;
        self.state.brush.brush = Some(id);
        self.set_status(format!("Imported brush {name}"));
        Some(id)
    }

    /// Picks library or preset brush `id` and switches to the Brush tool.
    pub fn use_brush(&mut self, id: AssetId) {
        self.set_brush(Some(id));
        self.tools.cancel();
        self.state.tool = editor::Tool::Brush;
    }

    /// Asks the host for a save dialog to write library brush `id` as a
    /// `.rustbrush` file.
    pub fn export_brush(&mut self, id: AssetId) {
        if self.document.asset(id).is_some_and(|a| a.brush().is_some()) {
            self.requests.push(Request::ExportBrushDialog(id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use document::animation_core::Vec2;
    use document::drawing::ImageData;

    fn preset() -> BrushPreset {
        BrushPreset {
            id: AssetId::new(),
            name: "Pencil".into(),
            data: BrushData {
                tip: ImageData::new(1, 1, vec![1]),
                ..BrushData::default()
            },
        }
    }

    #[test]
    fn options_list_solid_presets_and_library_brushes() {
        let mut s = Session::new();
        let p = preset();
        s.set_brush_presets(vec![p.clone()]);
        let own = s.import_brush("Mine", p.data.clone()).unwrap();
        let names: Vec<String> = s.brush_options().into_iter().map(|o| o.name).collect();
        assert_eq!(names, [SOLID_BRUSH_NAME, "Pencil", "Mine"]);
        assert_eq!(s.brush(), Some(own));
        s.set_brush(Some(AssetId::new()));
        assert_eq!(s.brush(), None);

        // Undoing the import forgets the brush it picked.
        s.set_brush(Some(own));
        s.undo();
        assert_eq!(s.brush(), None);
        assert_eq!(s.brush_options().len(), 2);
    }

    #[test]
    fn library_brushes_can_be_exported() {
        let mut s = Session::new();
        let id = s.import_brush("Mine", preset().data).unwrap();
        s.export_brush(id);
        assert_eq!(s.requests.last(), Some(&Request::ExportBrushDialog(id)));
    }

    #[test]
    fn first_stroke_with_a_preset_adds_it_to_the_library_once() {
        let mut s = Session::new();
        let p = preset();
        s.set_brush_presets(vec![p.clone()]);
        s.use_brush(p.id);
        for x in [0.0, 50.0] {
            s.pointer_down(Vec2::new(x, 10.0), 1.0);
            s.pointer_move(Vec2::new(x + 30.0, 40.0));
            s.pointer_up();
        }
        assert_eq!(s.document.assets().len(), 1);
        assert!(s.document.asset_in_use(p.id));
        // Undoing both strokes takes the brush out again.
        s.undo();
        s.undo();
        assert!(s.document.assets().is_empty());
        // The preset stays picked.
        assert_eq!(s.brush(), Some(p.id));
    }
}
