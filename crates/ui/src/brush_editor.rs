//! Making and editing brushes (ADR-024).
//!
//! **Create Brush from Selection** turns the strokes, fills and pictures
//! selected on the canvas into a new library brush's tip. The session can't
//! render, so it hands the host a [`BrushCapture`]: a small document holding
//! just the artwork to render. The host renders it, turns the pixels into a
//! tip, and calls [`Session::finish_brush_capture`].

use crate::session::{Request, Session};
use document::animation_core::{AssetId, FrameIndex};
use document::drawing::{BrushData, ImageData};
use document::{Asset, AssetKind, Document};
use editor::Command;
use editor::commands::{AddAsset, CopiedItems, paste_items};

/// What a captured picture becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushPictureTarget {
    /// The tip of a new library brush (Create Brush from Selection).
    NewBrush,
}

impl BrushPictureTarget {
    /// Whether the picture is a paper grain rather than a tip.
    pub fn is_grain(self) -> bool {
        match self {
            Self::NewBrush => false,
        }
    }
}

/// Artwork for the host to render into a brush picture.
#[derive(Clone, Debug)]
pub struct BrushCapture {
    /// A document holding only the artwork, the size of the project's
    /// canvas, with no background.
    pub document: Document,
    /// The frame of `document` to render.
    pub frame: FrameIndex,
    /// What the picture becomes.
    pub target: BrushPictureTarget,
}

impl Session {
    /// Whether Create Brush from Selection has something to work with.
    pub fn can_create_brush(&self) -> bool {
        self.tools.selection_at().is_some() && self.tools.selected_instance().is_none()
    }

    /// Starts Create Brush from Selection: the host renders the selected
    /// items and makes them the tip of a new brush.
    pub fn create_brush_from_selection(&mut self) {
        self.sync_selection();
        if self.tools.is_active() {
            return;
        }
        if self.tools.selected_instance().is_some() {
            self.set_error(
                "Create Brush: objects can't become brushes; select strokes, fills or pictures",
            );
            return;
        }
        let Some((layer, frame)) = self.tools.selection_at() else {
            self.set_error("Create Brush: select strokes, fills or pictures on the canvas first");
            return;
        };
        let Some(drawing) = crate::tools::vector_at(&self.document, layer, frame) else {
            return;
        };
        let copied = CopiedItems::from_drawing(drawing, self.tools.selection());
        if copied.is_empty() {
            return;
        }
        match artwork_document(&self.document, &copied) {
            Ok(document) => {
                self.brush_capture = Some(BrushCapture {
                    document,
                    frame: FrameIndex(0),
                    target: BrushPictureTarget::NewBrush,
                });
                self.requests.push(Request::CaptureBrushPicture);
            }
            Err(err) => self.set_error(format!("Create Brush: {err}")),
        }
    }

    /// The artwork waiting to be rendered, for the host.
    pub fn take_brush_capture(&mut self) -> Option<BrushCapture> {
        self.brush_capture.take()
    }

    /// The host's rendered and converted picture for `target`, or why it
    /// could not make one.
    pub fn finish_brush_capture(
        &mut self,
        target: BrushPictureTarget,
        picture: Result<ImageData, String>,
    ) {
        let picture = match picture {
            Ok(picture) => picture,
            Err(err) => {
                self.show_error("Could not make the brush", err);
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
                    self.set_status(format!("Created brush {name}"));
                }
            }
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

    #[test]
    fn selection_becomes_a_capture_then_a_brush() {
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
        assert_eq!(capture.document.canvas, s.document.canvas);
        assert_eq!(capture.document.background, None);
        assert_eq!(capture.document.evaluate(capture.frame).layers.len(), 1);

        let tip = ImageData::new(4, 4, vec![1, 2, 3]);
        s.finish_brush_capture(capture.target, Ok(tip.clone()));
        let id = s.brush().unwrap();
        let asset = s.document.asset(id).unwrap();
        assert_eq!(asset.name, "Brush 1");
        assert_eq!(asset.brush().unwrap().tip, tip);
        assert_eq!(s.state.tool, Tool::Brush);

        // One undo step removes it again.
        s.undo();
        assert!(s.document.asset(id).is_none());
    }

    #[test]
    fn failed_capture_shows_an_error() {
        let mut s = Session::new();
        s.finish_brush_capture(BrushPictureTarget::NewBrush, Err("empty".into()));
        assert!(s.dialog().is_some());
        assert!(s.document.assets().is_empty());
    }
}
