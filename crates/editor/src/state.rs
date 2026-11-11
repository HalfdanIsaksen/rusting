//! Editor state that is not part of the saved document.

use animation_core::{FrameIndex, LayerId};
use document::{Document, drawing::BrushStyle};

/// The active editing tool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    /// Draw new strokes.
    #[default]
    Brush,
    /// Remove strokes.
    Eraser,
    /// Fill the area enclosed by strokes (fill bucket).
    Fill,
    /// Select strokes and fills, then move or delete them.
    Select,
    /// Select by drawing a loop around strokes and fills.
    Lasso,
    /// Scale and rotate the selection with box handles.
    Transform,
    /// Drag out a rectangle.
    Rectangle,
    /// Drag out an ellipse.
    Ellipse,
    /// Pick the colour of the artwork under the cursor.
    Eyedropper,
}

impl Tool {
    /// Whether the tool works on the selection (and keeps it alive).
    pub fn uses_selection(self) -> bool {
        matches!(self, Self::Select | Self::Lasso | Self::Transform)
    }
}

/// Session state of the editor: active tool, layer and frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorState {
    /// The active tool.
    pub tool: Tool,
    /// Brush settings used for new strokes.
    pub brush: BrushStyle,
    /// The layer drawing tools act on.
    pub active_layer: Option<LayerId>,
    current_frame: FrameIndex,
}

impl EditorState {
    /// Creates editor state for `doc`, selecting its top layer.
    pub fn for_document(doc: &Document) -> Self {
        Self {
            active_layer: doc.layers().iter().next_back().map(|l| l.id),
            ..Self::default()
        }
    }

    /// The frame shown on the canvas and the timeline playhead.
    pub fn current_frame(&self) -> FrameIndex {
        self.current_frame
    }

    /// Moves the playhead, clamped to the document length.
    pub fn set_current_frame(&mut self, doc: &Document, frame: FrameIndex) {
        self.current_frame = frame.min(doc.last_frame().unwrap_or(FrameIndex::ZERO));
    }

    /// Whether the active tool may draw on the active layer right now.
    pub fn can_draw(&self, doc: &Document) -> bool {
        self.active_layer
            .and_then(|id| doc.layers().get(id))
            .is_some_and(|layer| layer.is_editable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use document::CanvasSize;

    #[test]
    fn selects_top_layer_and_respects_lock() {
        let mut doc = Document::new(
            "T",
            CanvasSize {
                width: 10,
                height: 10,
            },
        );
        doc.add_layer("bottom");
        let top = doc.add_layer("top");
        let state = EditorState::for_document(&doc);
        assert_eq!(state.active_layer, Some(top));
        assert!(state.can_draw(&doc));
        doc.layer_mut(top).unwrap().locked = true;
        assert!(!state.can_draw(&doc));
    }

    #[test]
    fn playhead_is_clamped() {
        let doc = Document::new(
            "T",
            CanvasSize {
                width: 10,
                height: 10,
            },
        );
        let mut state = EditorState::for_document(&doc);
        state.set_current_frame(&doc, FrameIndex(500));
        assert_eq!(state.current_frame(), FrameIndex(23));
    }
}
