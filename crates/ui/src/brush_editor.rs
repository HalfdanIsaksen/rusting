//! Making and editing brushes (ADR-024).
//!
//! **Create Brush from Selection** turns the strokes, fills and pictures
//! selected on the canvas into a new library brush's tip. The **Brush
//! Editor** ([`BrushEditor`]) changes a brush's tip, grain and settings on a
//! draft, and Apply writes the draft back as one undo step. A built-in brush
//! is never changed: Apply saves the draft as a new library brush.
//!
//! The session can't render or decode pictures, so a new tip or grain is a
//! [`BrushCapture`] for the host: artwork to render, a picture file to pick,
//! or a picture to generate. The host makes the picture and calls
//! [`Session::finish_brush_capture`].

use crate::session::{Request, Session};
use document::animation_core::{AssetId, FrameIndex, LayerId};
use document::drawing::{BrushData, ImageData};
use document::{Asset, AssetKind, Document};
use editor::Command;
use editor::commands::{AddAsset, Batch, CopiedItems, RenameAsset, SetBrush, paste_items};

/// Size of the Brush Editor's scratchpad, in points. The host renders it
/// as a document of this many units.
pub const SCRATCHPAD_SIZE: [f32; 2] = [300.0, 110.0];

/// What a new picture becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushPictureTarget {
    /// The tip of a new library brush (Create Brush from Selection).
    NewBrush,
    /// The tip of the Brush Editor's draft.
    Tip,
    /// The paper grain of the Brush Editor's draft.
    Grain,
}

impl BrushPictureTarget {
    /// Whether the picture is a paper grain rather than a tip.
    pub fn is_grain(self) -> bool {
        self == Self::Grain
    }
}

/// Where a new brush picture comes from.
#[derive(Clone, Debug)]
pub enum BrushPictureSource {
    /// Render frame `frame` of `document`: the project's canvas size, no
    /// background, only the artwork to use.
    Artwork {
        /// The artwork.
        document: Box<Document>,
        /// The frame to render.
        frame: FrameIndex,
    },
    /// Ask the user for a PNG or JPEG picture.
    File,
    /// A generated round or flat tip ([`BrushEditor::hardness`],
    /// [`BrushEditor::aspect`]).
    Ellipse {
        /// Solid part of the radius, `0.0..=1.0`.
        hardness: f32,
        /// Height over width, `0.05..=1.0`.
        aspect: f32,
    },
    /// A generated, tiling paper grain.
    Noise {
        /// Size of the paper's tooth, `0.0..=1.0` (fine to coarse).
        coarseness: f32,
        /// Picks the pattern.
        seed: u64,
    },
}

/// A brush picture for the host to make.
#[derive(Clone, Debug)]
pub struct BrushCapture {
    /// Where it comes from.
    pub source: BrushPictureSource,
    /// What it becomes.
    pub target: BrushPictureTarget,
}

/// The Brush Editor window's state: the brush being edited and a draft of
/// it. Panels change the draft and call [`Session::brush_editor_edited`].
#[derive(Clone, Debug)]
pub struct BrushEditor {
    /// The brush being edited: a library brush or a built-in.
    pub asset: AssetId,
    /// The name in the name field.
    pub name: String,
    /// Tip, grain and settings as edited.
    pub draft: BrushData,
    /// The brush is built in: Apply saves a copy.
    pub built_in: bool,
    /// Generator setting: solid part of a generated tip's radius.
    pub hardness: f32,
    /// Generator setting: height over width of a generated tip.
    pub aspect: f32,
    /// Generator setting: tooth size of a generated grain.
    pub coarseness: f32,
    /// The host's pictures for the window, set by the host.
    pub textures: BrushEditorTextures,
    seed: u64,
    revision: u64,
}

/// Textures the host renders for the Brush Editor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BrushEditorTextures {
    /// The draft's tip.
    pub tip: Option<egui::TextureId>,
    /// The draft's grain.
    pub grain: Option<egui::TextureId>,
    /// A test stroke drawn with the draft.
    pub scratchpad: Option<egui::TextureId>,
}

impl BrushEditor {
    /// Bumped whenever the draft changes, so the host knows to redraw its
    /// pictures.
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

impl Session {
    /// Whether Create Brush from Selection (or a tip or grain from the
    /// selection) has something to work with.
    pub fn can_create_brush(&self) -> bool {
        self.tools.selection_at().is_some() && self.tools.selected_instance().is_none()
    }

    /// Starts Create Brush from Selection: the host renders the selected
    /// items and makes them the tip of a new brush, then the Brush Editor
    /// opens on it.
    pub fn create_brush_from_selection(&mut self) {
        self.brush_picture_from_selection(BrushPictureTarget::NewBrush);
    }

    /// Asks the host to render the selected items into `target`.
    pub fn brush_picture_from_selection(&mut self, target: BrushPictureTarget) {
        match self.selection_artwork() {
            Ok(document) => self.capture(
                BrushPictureSource::Artwork {
                    document: Box::new(document),
                    frame: FrameIndex(0),
                },
                target,
            ),
            Err(err) => self.set_error(format!("Brush: {err}")),
        }
    }

    /// Asks the host to render drawing layer `layer` at the current frame,
    /// on its own and at full opacity, into `target`.
    pub fn brush_picture_from_layer(&mut self, layer: LayerId, target: BrushPictureTarget) {
        let mut document = self.document.clone();
        document.background = None;
        let ids: Vec<_> = document.layers().iter().map(|l| (l.id, l.group)).collect();
        for (id, group) in ids {
            if let Some(l) = document.layer_mut(id) {
                l.visible = id == layer;
                if id == layer {
                    l.opacity = 1.0;
                }
            }
            if id == layer
                && let Some(g) = group.and_then(|g| document.group_mut(g))
            {
                g.visible = true;
                g.opacity = 1.0;
            }
        }
        let frame = self.current_frame();
        let document = Box::new(document);
        self.capture(BrushPictureSource::Artwork { document, frame }, target);
    }

    /// Asks the host for a PNG or JPEG picture to use as `target`.
    pub fn brush_picture_from_file(&mut self, target: BrushPictureTarget) {
        self.capture(BrushPictureSource::File, target);
    }

    /// Generates a new tip for the Brush Editor's draft from its hardness
    /// and aspect.
    pub fn generate_brush_tip(&mut self) {
        let Some(editor) = &self.brush_editor else {
            return;
        };
        let source = BrushPictureSource::Ellipse {
            hardness: editor.hardness,
            aspect: editor.aspect,
        };
        self.capture(source, BrushPictureTarget::Tip);
    }

    /// Generates a new paper grain for the Brush Editor's draft; each call
    /// gives a different pattern.
    pub fn generate_brush_grain(&mut self) {
        let Some(editor) = &mut self.brush_editor else {
            return;
        };
        editor.seed += 1;
        let source = BrushPictureSource::Noise {
            coarseness: editor.coarseness,
            seed: editor.seed,
        };
        self.capture(source, BrushPictureTarget::Grain);
    }

    fn capture(&mut self, source: BrushPictureSource, target: BrushPictureTarget) {
        self.brush_capture = Some(BrushCapture { source, target });
        self.requests.push(Request::CaptureBrushPicture);
    }

    /// The brush picture waiting to be made, for the host.
    pub fn take_brush_capture(&mut self) -> Option<BrushCapture> {
        self.brush_capture.take()
    }

    /// The host's picture for `target`, or why it could not make one.
    pub fn finish_brush_capture(
        &mut self,
        target: BrushPictureTarget,
        picture: Result<ImageData, String>,
    ) {
        let picture = match picture {
            Ok(picture) => picture,
            Err(err) => {
                self.show_error("Could not make the brush picture", err);
                return;
            }
        };
        match target {
            BrushPictureTarget::NewBrush => {
                let name = self.new_brush_name();
                let data = BrushData {
                    tip: picture,
                    grain: None,
                    settings: Default::default(),
                };
                if let Some(id) = self.add_brush(&name, data) {
                    self.use_brush(id);
                    self.open_brush_editor(id);
                    self.set_status(format!("Created brush {name}"));
                }
            }
            BrushPictureTarget::Tip | BrushPictureTarget::Grain => {
                let Some(editor) = &mut self.brush_editor else {
                    return;
                };
                if target.is_grain() {
                    editor.draft.grain = Some(picture);
                } else {
                    editor.draft.tip = picture;
                }
                editor.revision += 1;
            }
        }
    }

    /// Opens the Brush Editor on library or built-in brush `id`. Returns
    /// false if there is no such brush.
    pub fn open_brush_editor(&mut self, id: AssetId) -> bool {
        let Some((name, draft)) = self.brush_source(id) else {
            return false;
        };
        let (textures, seed, revision) = self
            .brush_editor
            .as_ref()
            .map_or((BrushEditorTextures::default(), 0, 0), |e| {
                (e.textures, e.seed, e.revision + 1)
            });
        self.brush_editor = Some(BrushEditor {
            asset: id,
            name,
            draft,
            built_in: self.is_preset(id),
            hardness: 0.6,
            aspect: 1.0,
            coarseness: 0.3,
            textures,
            seed,
            revision,
        });
        true
    }

    /// Adds a copy of library or built-in brush `id`, named "<name> copy",
    /// picks it for the Brush tool and opens it in the Brush Editor.
    pub fn duplicate_brush(&mut self, id: AssetId) {
        let Some((name, data)) = self.brush_source(id) else {
            return;
        };
        let name = format!("{name} copy");
        if let Some(copy) = self.add_brush(&name, data) {
            self.use_brush(copy);
            self.open_brush_editor(copy);
            self.set_status(format!("Added {name} to the Library"));
        }
    }

    /// Closes the Brush Editor, dropping unapplied changes.
    pub fn close_brush_editor(&mut self) {
        self.brush_editor = None;
    }

    /// Tells the session the Brush Editor's draft was changed in place.
    pub fn brush_editor_edited(&mut self) {
        if let Some(editor) = &mut self.brush_editor {
            editor.revision += 1;
        }
    }

    /// The name and data of library or built-in brush `id`; a built-in
    /// already in the library is read from there.
    fn brush_source(&self, id: AssetId) -> Option<(String, BrushData)> {
        if let Some(asset) = self.document.asset(id) {
            return Some((asset.name.clone(), asset.brush()?.clone()));
        }
        let preset = self.brush_presets().iter().find(|p| p.id == id)?;
        Some((preset.name.clone(), preset.data.clone()))
    }

    /// Whether the Brush Editor's draft or name differ from its brush.
    pub fn brush_editor_dirty(&self) -> bool {
        self.brush_editor
            .as_ref()
            .is_some_and(|editor| self.brush_editor_differs(editor))
    }

    /// Whether `editor`'s draft or name differ from its brush.
    pub(crate) fn brush_editor_differs(&self, editor: &BrushEditor) -> bool {
        self.brush_source(editor.asset)
            .is_none_or(|(name, data)| name != editor.name.trim() || data != editor.draft)
    }

    /// Puts the Brush Editor's draft back to the brush as it is.
    pub fn revert_brush_editor(&mut self) {
        if let Some(editor) = &self.brush_editor
            && let Some((name, draft)) = self.brush_source(editor.asset)
            && let Some(editor) = &mut self.brush_editor
        {
            editor.name = name;
            editor.draft = draft;
            editor.revision += 1;
        }
    }

    /// Applies the Brush Editor's draft as one undo step. A library brush
    /// takes the draft (every stroke drawn with it restyles); a built-in
    /// brush is left alone and the draft becomes a new library brush, named
    /// "<name> copy" unless it was renamed, which the editor then edits.
    pub fn apply_brush_editor(&mut self) {
        let Some(editor) = self.brush_editor.clone() else {
            return;
        };
        let name = editor.name.trim().to_owned();
        if name.is_empty() {
            self.set_error("Edit Brush: give the brush a name");
            return;
        }
        if editor.built_in {
            let original = self.brush_source(editor.asset).map(|(n, _)| n);
            let name = if original.as_deref() == Some(name.as_str()) {
                format!("{name} copy")
            } else {
                name
            };
            if let Some(id) = self.add_brush(&name, editor.draft) {
                self.use_brush(id);
                if let Some(e) = &mut self.brush_editor {
                    e.asset = id;
                    e.built_in = false;
                    e.name = name.clone();
                }
                self.set_status(format!("Saved the brush as {name} in the Library"));
            }
            return;
        }
        let mut commands: Vec<Box<dyn Command>> =
            vec![Box::new(SetBrush::new(editor.asset, editor.draft))];
        if self
            .document
            .asset(editor.asset)
            .is_some_and(|a| a.name != name)
        {
            commands.push(Box::new(RenameAsset::new(editor.asset, name.clone())));
        }
        if self
            .execute(Box::new(Batch::new("Edit Brush", commands)))
            .is_ok()
        {
            // Show the clamped settings the brush now has.
            self.revert_brush_editor();
            self.set_status(format!("Updated brush {name}"));
        }
    }

    /// Closes the Brush Editor if its brush is gone (deleted, its import
    /// undone, or another document opened).
    pub(crate) fn check_brush_editor(&mut self) {
        if let Some(editor) = &self.brush_editor
            && self.brush_source(editor.asset).is_none()
        {
            self.brush_editor = None;
        }
    }

    /// Adds brush `data` named `name` to the library as one undo step.
    fn add_brush(&mut self, name: &str, data: BrushData) -> Option<AssetId> {
        let asset = Asset::new(name, AssetKind::Brush(data));
        let id = asset.id;
        let command: Box<dyn Command> = Box::new(AddAsset::new(asset));
        self.execute(command).ok()?;
        Some(id)
    }

    /// "Brush 1", "Brush 2", …: the first name no library brush has.
    fn new_brush_name(&self) -> String {
        (1..)
            .map(|n| format!("Brush {n}"))
            .find(|name| {
                !self
                    .document
                    .assets()
                    .values()
                    .any(|a| a.brush().is_some() && &a.name == name)
            })
            .unwrap_or_else(|| "Brush".to_owned())
    }

    /// A document with only the selected canvas items, or why there is none.
    fn selection_artwork(&mut self) -> Result<Document, String> {
        self.sync_selection();
        if self.tools.is_active() {
            return Err("finish the current gesture first".into());
        }
        if self.tools.selected_instance().is_some() {
            return Err("objects can't become brushes; select strokes, fills or pictures".into());
        }
        let (layer, frame) = self
            .tools
            .selection_at()
            .ok_or("select strokes, fills or pictures on the canvas first")?;
        let drawing = crate::tools::vector_at(&self.document, layer, frame)
            .ok_or("the selection's drawing is gone")?;
        let copied = CopiedItems::from_drawing(drawing, self.tools.selection());
        if copied.is_empty() {
            return Err("select strokes, fills or pictures on the canvas first".into());
        }
        artwork_document(&self.document, &copied)
    }
}

/// A document the size of `source`'s canvas whose first frame shows only
/// `items` (with the library, so pictures and brushes resolve).
fn artwork_document(source: &Document, items: &CopiedItems) -> Result<Document, String> {
    let mut document = Document::new("Brush", source.canvas);
    document.background = None;
    for asset in source.assets().values() {
        document.insert_asset(asset.clone());
    }
    let layer = document.add_layer("Artwork");
    let (mut command, _) = paste_items(layer, FrameIndex(0), items);
    command
        .apply(&mut document)
        .map_err(|e| crate::session::describe_error(&e))?;
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brushes::BrushPreset;
    use document::animation_core::Vec2;
    use editor::Tool;

    fn draw_and_select(s: &mut Session) {
        s.state.tool = Tool::Brush;
        s.pointer_down(Vec2::new(10.0, 10.0), 1.0);
        s.pointer_move(Vec2::new(60.0, 40.0));
        s.pointer_up();
        s.state.tool = Tool::Select;
        s.select_all();
    }

    fn tip(n: u8) -> ImageData {
        ImageData::new(4, 4, vec![n; 3])
    }

    fn preset() -> BrushPreset {
        BrushPreset {
            id: AssetId::new(),
            name: "Chalk".into(),
            data: BrushData {
                tip: tip(1),
                ..BrushData::default()
            },
        }
    }

    #[test]
    fn selection_becomes_a_capture_then_a_brush_in_the_editor() {
        let mut s = Session::new();
        assert!(!s.can_create_brush());
        s.create_brush_from_selection();
        assert!(s.take_brush_capture().is_none());

        draw_and_select(&mut s);
        assert!(s.can_create_brush());
        s.create_brush_from_selection();
        assert_eq!(s.take_requests(), [Request::CaptureBrushPicture]);
        let capture = s.take_brush_capture().unwrap();
        assert_eq!(capture.target, BrushPictureTarget::NewBrush);
        let BrushPictureSource::Artwork { document, frame } = &capture.source else {
            panic!("artwork expected");
        };
        assert_eq!(document.canvas, s.document.canvas);
        assert_eq!(document.background, None);
        assert_eq!(document.evaluate(*frame).layers.len(), 1);

        s.finish_brush_capture(capture.target, Ok(tip(7)));
        let id = s.brush().unwrap();
        let asset = s.document.asset(id).unwrap();
        assert_eq!(asset.name, "Brush 1");
        assert_eq!(asset.brush().unwrap().tip, tip(7));
        assert_eq!(s.state.tool, Tool::Brush);
        let editor = s.brush_editor.as_ref().unwrap();
        assert_eq!(editor.asset, id);
        assert!(!editor.built_in);
        assert!(!s.brush_editor_dirty());

        // One undo step removes it again, and the editor closes.
        s.undo();
        assert!(s.document.asset(id).is_none());
        assert!(s.brush_editor.is_none());
    }

    #[test]
    fn failed_capture_shows_an_error() {
        let mut s = Session::new();
        s.finish_brush_capture(BrushPictureTarget::NewBrush, Err("empty".into()));
        assert!(s.dialog().is_some());
        assert!(s.document.assets().is_empty());
    }

    #[test]
    fn applying_a_library_brush_is_one_undo_step() {
        let mut s = Session::new();
        let id = s.import_brush("Mine", preset().data).unwrap();
        assert!(s.open_brush_editor(id));
        assert!(!s.open_brush_editor(AssetId::new()));
        let before = s.document.clone();
        let editor = s.brush_editor.as_mut().unwrap();
        editor.name = "Scratchy".into();
        editor.draft.settings.scatter = 0.5;
        let r = editor.revision();
        s.brush_editor_edited();
        assert!(s.brush_editor.as_ref().unwrap().revision() > r);
        assert!(s.brush_editor_dirty());
        s.apply_brush_editor();
        assert!(!s.brush_editor_dirty());
        let asset = s.document.asset(id).unwrap();
        assert_eq!(asset.name, "Scratchy");
        assert_eq!(asset.brush().unwrap().settings.scatter, 0.5);
        s.undo();
        assert_eq!(s.document, before);
        // The draft keeps the edits until reverted.
        assert!(s.brush_editor_dirty());
        s.revert_brush_editor();
        assert!(!s.brush_editor_dirty());
        assert_eq!(s.brush_editor.as_ref().unwrap().name, "Mine");
    }

    #[test]
    fn applying_a_built_in_saves_a_copy() {
        let mut s = Session::new();
        let p = preset();
        s.set_brush_presets(vec![p.clone()]);
        assert!(s.open_brush_editor(p.id));
        assert!(s.brush_editor.as_ref().unwrap().built_in);
        s.brush_editor.as_mut().unwrap().draft.settings.flow = 0.4;
        s.apply_brush_editor();
        assert!(
            s.document.asset(p.id).is_none(),
            "the built-in is untouched"
        );
        let editor = s.brush_editor.as_ref().unwrap();
        assert_ne!(editor.asset, p.id);
        assert!(!editor.built_in);
        let copy = s.document.asset(editor.asset).unwrap();
        assert_eq!(copy.name, "Chalk copy");
        assert_eq!(copy.brush().unwrap().settings.flow, 0.4);
        assert_eq!(s.brush(), Some(copy.id));
    }

    #[test]
    fn duplicating_adds_a_copy_and_edits_it() {
        let mut s = Session::new();
        let id = s.import_brush("Mine", preset().data).unwrap();
        s.duplicate_brush(id);
        let copy = s.brush().unwrap();
        assert_ne!(copy, id);
        assert_eq!(s.document.asset(copy).unwrap().name, "Mine copy");
        assert_eq!(s.brush_editor.as_ref().unwrap().asset, copy);
        s.undo();
        assert!(s.document.asset(copy).is_none());
        assert!(s.document.asset(id).is_some());
    }

    #[test]
    fn an_empty_name_is_refused() {
        let mut s = Session::new();
        let id = s.import_brush("Mine", preset().data).unwrap();
        s.open_brush_editor(id);
        s.brush_editor.as_mut().unwrap().name = "  ".into();
        s.apply_brush_editor();
        assert_eq!(s.document.asset(id).unwrap().name, "Mine");
    }

    #[test]
    fn pictures_go_to_the_draft() {
        let mut s = Session::new();
        let id = s.import_brush("Mine", preset().data).unwrap();
        s.open_brush_editor(id);
        s.generate_brush_tip();
        let capture = s.take_brush_capture().unwrap();
        assert!(matches!(capture.source, BrushPictureSource::Ellipse { .. }));
        s.finish_brush_capture(capture.target, Ok(tip(5)));
        s.generate_brush_grain();
        let first = s.take_brush_capture().unwrap();
        s.generate_brush_grain();
        let second = s.take_brush_capture().unwrap();
        let seed = |c: &BrushCapture| match c.source {
            BrushPictureSource::Noise { seed, .. } => seed,
            _ => panic!("noise expected"),
        };
        assert_ne!(seed(&first), seed(&second), "each grain is new");
        s.finish_brush_capture(second.target, Ok(tip(6)));
        let draft = &s.brush_editor.as_ref().unwrap().draft;
        assert_eq!(draft.tip, tip(5));
        assert_eq!(draft.grain, Some(tip(6)));
        // Nothing is applied yet.
        assert_eq!(s.document.asset(id).unwrap().brush().unwrap().tip, tip(1));
    }

    #[test]
    fn a_layer_renders_alone() {
        let mut s = Session::new();
        draw_and_select(&mut s);
        let drawn = s.state.active_layer.unwrap();
        s.execute(Box::new(editor::commands::AddLayer::new("Other")))
            .unwrap();
        s.brush_picture_from_layer(drawn, BrushPictureTarget::Grain);
        let capture = s.take_brush_capture().unwrap();
        assert_eq!(capture.target, BrushPictureTarget::Grain);
        let BrushPictureSource::Artwork { document, frame } = capture.source else {
            panic!("artwork expected");
        };
        let shown: Vec<_> = document
            .layers()
            .iter()
            .filter(|l| l.visible)
            .map(|l| l.id)
            .collect();
        assert_eq!(shown, [drawn]);
        assert_eq!(frame, s.current_frame());
        assert_eq!(document.background, None);
    }
}
