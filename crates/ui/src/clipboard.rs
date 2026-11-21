//! Copy, cut and paste of canvas items (Edit → Copy / Cut / Paste, the
//! canvas and timeline right-click menus, and Cmd/Ctrl+C / X / V).
//!
//! Copy keeps the selected strokes, fills and pictures in the session.
//! Paste adds fresh copies to the active layer at the current frame (or to
//! a timeline cell), at the same coordinates they were copied from, as one
//! undo step ([`editor::commands::paste_items`]), and selects them.
//!
//! The copied items stay in the session: other apps can't read them. Copy
//! also puts [`CLIPBOARD_MARKER`] on the system clipboard, so Cmd/Ctrl+V
//! reaches the app as a paste (the windowing layer only reports a paste
//! when the clipboard holds something), and a paste whose text is anything
//! else came from another app and is left alone.

use crate::session::Session;
use document::animation_core::{FrameIndex, LayerId};
use editor::Tool;
use editor::commands::{CopiedItems, paste_items, remove_items};

/// The text Copy puts on the system clipboard.
pub const CLIPBOARD_MARKER: &str = "Rusting canvas items";

impl Session {
    /// Whether there is a canvas selection to copy or cut.
    pub fn can_copy(&self) -> bool {
        self.tools.selection_at().is_some()
    }

    /// Whether Paste has something to paste.
    pub fn can_paste(&self) -> bool {
        self.clipboard.as_ref().is_some_and(|c| !c.is_empty())
    }

    /// Copies the selected items. Returns whether anything was copied; if
    /// not, the status bar says why.
    pub fn copy_selection(&mut self) -> bool {
        self.sync_selection();
        if self.tools.is_active() {
            return false;
        }
        if self.tools.selected_instance().is_some() {
            self.set_error("Copy: objects can't be copied yet; select strokes, fills or pictures");
            return false;
        }
        let Some((layer, frame)) = self.tools.selection_at() else {
            self.set_error("Copy: select strokes, fills or pictures on the canvas first");
            return false;
        };
        let Some(drawing) = crate::tools::vector_at(&self.document, layer, frame) else {
            return false;
        };
        let copied = CopiedItems::from_drawing(drawing, self.tools.selection());
        if copied.is_empty() {
            return false;
        }
        let count = copied.len();
        self.clipboard = Some(copied);
        self.set_status(format!("Copied {}", items_text(count)));
        true
    }

    /// Copies the selected items and deletes them, as one undo step for the
    /// delete. Returns whether anything was cut.
    pub fn cut_selection(&mut self) -> bool {
        if !self.copy_selection() {
            return false;
        }
        let Some((layer, frame)) = self.tools.selection_at() else {
            return false;
        };
        let command = remove_items(layer, frame, self.tools.selection()).with_label("Cut");
        if self.execute(Box::new(command)).is_err() {
            return false;
        }
        self.clear_selection();
        let count = self.clipboard.as_ref().map_or(0, CopiedItems::len);
        self.set_status(format!("Cut {}", items_text(count)));
        true
    }

    /// Pastes the copied items on the active layer at the current frame.
    pub fn paste(&mut self) {
        if let Some(layer) = self.state.active_layer {
            let frame = self.current_frame();
            self.paste_at(layer, frame);
        } else {
            self.set_error("Paste: no layer selected — add a layer first");
        }
    }

    /// Makes `layer` and `frame` current, then pastes the copied items
    /// there, in place, and selects them (switching to the Selection tool
    /// unless a tool that uses the selection is active).
    pub fn paste_at(&mut self, layer: LayerId, frame: FrameIndex) {
        if self.tools.is_active() {
            return;
        }
        let Some(copied) = self.clipboard.as_ref().filter(|c| !c.is_empty()) else {
            self.set_error("Paste: nothing copied yet; select items and Copy first");
            return;
        };
        let (command, items) = paste_items(layer, frame, copied);
        let count = items.len();
        self.commit_text();
        self.playback.pause();
        if self.state.active_layer != Some(layer) {
            self.select_layer(layer);
        }
        if self.current_frame() != frame {
            self.set_frame(frame);
        }
        if let Some(reason) = self.draw_block_reason() {
            self.set_error(format!("Paste: {reason}"));
            return;
        }
        if self.execute(Box::new(command)).is_err() {
            return;
        }
        if !self.state.tool.uses_selection() {
            self.state.tool = Tool::Select;
        }
        self.tools.set_selection((layer, frame), items);
        self.set_status(format!("Pasted {}", items_text(count)));
    }
}

/// "1 item", "3 items".
fn items_text(count: usize) -> String {
    if count == 1 {
        "1 item".to_owned()
    } else {
        format!("{count} items")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use document::animation_core::{Rgba, Vec2};
    use document::drawing::ItemId;
    use editor::commands::{AddKeyframe, AddLayer, KeyframeKind, SetLayerLocked};

    /// A session with one stroke drawn on frame 1 of the active layer, and
    /// it selected.
    fn drawn() -> (Session, LayerId) {
        let mut s = Session::new();
        let layer = s.state.active_layer.unwrap();
        s.state.brush.color = Rgba::BLACK;
        s.state.tool = Tool::Brush;
        s.pointer_down(Vec2::new(10.0, 10.0), 1.0);
        s.pointer_move(Vec2::new(60.0, 40.0));
        s.pointer_up();
        s.state.tool = Tool::Select;
        s.select_all();
        assert_eq!(s.tools.selection().len(), 1);
        (s, layer)
    }

    fn items_at(s: &Session, layer: LayerId, frame: u32) -> Vec<ItemId> {
        crate::tools::vector_at(&s.document, layer, FrameIndex(frame))
            .map(|d| d.all_items())
            .unwrap_or_default()
    }

    #[test]
    fn copy_then_paste_on_another_frame_keeps_coordinates() {
        let (mut s, layer) = drawn();
        let source = crate::tools::vector_at(&s.document, layer, FrameIndex(0))
            .unwrap()
            .strokes()[0]
            .clone();
        assert!(s.copy_selection());
        // An empty key on frame 5: pasting there starts a new drawing.
        s.execute(Box::new(AddKeyframe::new(
            layer,
            FrameIndex(4),
            KeyframeKind::Empty,
        )))
        .unwrap();
        s.set_frame(FrameIndex(4));
        s.paste();
        let pasted = crate::tools::vector_at(&s.document, layer, FrameIndex(4))
            .unwrap()
            .strokes()
            .to_vec();
        assert_eq!(s.document.track(layer).unwrap().keys().count(), 2);
        let pasted = pasted.last().unwrap();
        assert_ne!(pasted.id, source.id);
        assert_eq!(pasted.points, source.points);
        assert_eq!(pasted.style, source.style);
        // The pasted copy is selected, on the new frame.
        assert_eq!(s.tools.selection_at(), Some((layer, FrameIndex(4))));
        assert_eq!(s.tools.selection(), &[ItemId::Stroke(pasted.id)]);
        // One undo step takes it away again, leaving the empty key.
        s.undo();
        assert!(items_at(&s, layer, 4).is_empty());
        assert_eq!(items_at(&s, layer, 0).len(), 1);
    }

    #[test]
    fn cut_removes_and_paste_puts_back() {
        let (mut s, layer) = drawn();
        assert!(s.cut_selection());
        assert!(items_at(&s, layer, 0).is_empty());
        assert_eq!(s.history.undo_label(), Some("Cut"));
        s.paste();
        assert_eq!(items_at(&s, layer, 0).len(), 1);
        assert_eq!(s.history.undo_label(), Some("Paste"));
    }

    #[test]
    fn paste_without_copy_or_on_a_locked_layer_does_nothing() {
        let (mut s, layer) = drawn();
        s.paste();
        assert!(s.status().is_some_and(|st| st.is_error));
        assert!(s.copy_selection());
        s.execute(Box::new(SetLayerLocked::new(layer, true)))
            .unwrap();
        let undo = s.history.undo_len();
        s.paste_at(layer, FrameIndex(2));
        assert_eq!(s.history.undo_len(), undo);
    }

    #[test]
    fn paste_at_a_timeline_cell_moves_there() {
        let (mut s, _) = drawn();
        assert!(s.copy_selection());
        let add = AddLayer::new("Top");
        let top = add.layer_id();
        s.execute(Box::new(add)).unwrap();
        s.paste_at(top, FrameIndex(3));
        assert_eq!(s.state.active_layer, Some(top));
        assert_eq!(s.current_frame(), FrameIndex(3));
        assert_eq!(items_at(&s, top, 3).len(), 1);
    }
}
