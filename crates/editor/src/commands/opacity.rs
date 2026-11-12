//! Changing layer opacity, one undo step per slider drag.

use crate::{Command, CommandError};
use animation_core::LayerId;
use document::Document;
use std::any::Any;

/// Sets a layer's opacity (clamped to `0.0..=1.0`).
///
/// Commands made with [`SetLayerOpacity::in_gesture`] for the same layer and
/// gesture merge in the history, so every value of one slider drag undoes as
/// one step back to the value before the drag.
#[derive(Debug)]
pub struct SetLayerOpacity {
    id: LayerId,
    opacity: f32,
    /// Opacity before the first apply; set by `apply`.
    before: Option<f32>,
    gesture: Option<u64>,
}

impl SetLayerOpacity {
    /// Creates the command.
    pub fn new(id: LayerId, opacity: f32) -> Self {
        Self {
            id,
            opacity: opacity.clamp(0.0, 1.0),
            before: None,
            gesture: None,
        }
    }

    /// Marks the command as one value of the drag or edit `gesture`; the
    /// history merges consecutive commands of the same gesture and layer.
    pub fn in_gesture(mut self, gesture: u64) -> Self {
        self.gesture = Some(gesture);
        self
    }
}

impl Command for SetLayerOpacity {
    fn label(&self) -> &str {
        "Set Layer Opacity"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let layer = doc.layer_mut(self.id).ok_or(CommandError::LayerNotFound)?;
        self.before = Some(layer.opacity);
        layer.opacity = self.opacity;
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let before = self.before.ok_or(CommandError::LayerNotFound)?;
        let layer = doc.layer_mut(self.id).ok_or(CommandError::LayerNotFound)?;
        layer.opacity = before;
        Ok(())
    }

    fn merge(&mut self, next: &dyn Command) -> bool {
        let Some(next) = (next as &dyn Any).downcast_ref::<Self>() else {
            return false;
        };
        if self.gesture.is_none() || next.gesture != self.gesture || next.id != self.id {
            return false;
        }
        self.opacity = next.opacity;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::History;
    use document::CanvasSize;

    fn doc() -> (Document, LayerId) {
        let mut d = Document::new(
            "T",
            CanvasSize {
                width: 10,
                height: 10,
            },
        );
        let id = d.add_layer("A");
        (d, id)
    }

    fn opacity(d: &Document, id: LayerId) -> f32 {
        d.layers().get(id).unwrap().opacity
    }

    #[test]
    fn sets_clamps_and_undoes() {
        let (mut d, id) = doc();
        let mut h = History::new();
        h.execute(&mut d, Box::new(SetLayerOpacity::new(id, 1.7)))
            .unwrap();
        assert_eq!(opacity(&d, id), 1.0);
        h.execute(&mut d, Box::new(SetLayerOpacity::new(id, 0.25)))
            .unwrap();
        assert_eq!(opacity(&d, id), 0.25);
        assert_eq!(h.undo_len(), 2, "without a gesture, steps stay separate");
        h.undo(&mut d).unwrap();
        assert_eq!(opacity(&d, id), 1.0);
    }

    #[test]
    fn one_gesture_is_one_undo_step() {
        let (mut d, id) = doc();
        let mut h = History::new();
        for v in [0.9, 0.6, 0.4] {
            h.execute(&mut d, Box::new(SetLayerOpacity::new(id, v).in_gesture(7)))
                .unwrap();
        }
        h.execute(
            &mut d,
            Box::new(SetLayerOpacity::new(id, 0.2).in_gesture(8)),
        )
        .unwrap();
        assert_eq!(h.undo_len(), 2);
        assert!(h.is_dirty());
        h.undo(&mut d).unwrap();
        assert_eq!(opacity(&d, id), 0.4);
        h.undo(&mut d).unwrap();
        assert_eq!(opacity(&d, id), 1.0);
        h.redo(&mut d).unwrap();
        assert_eq!(opacity(&d, id), 0.4);
    }

    #[test]
    fn merged_change_marks_document_dirty_again() {
        let (mut d, id) = doc();
        let mut h = History::new();
        h.execute(
            &mut d,
            Box::new(SetLayerOpacity::new(id, 0.5).in_gesture(1)),
        )
        .unwrap();
        h.mark_saved();
        h.execute(
            &mut d,
            Box::new(SetLayerOpacity::new(id, 0.3).in_gesture(1)),
        )
        .unwrap();
        assert!(h.is_dirty());
    }

    #[test]
    fn missing_layer_fails() {
        let (mut d, _) = doc();
        assert_eq!(
            SetLayerOpacity::new(LayerId::new(), 0.5).apply(&mut d),
            Err(CommandError::LayerNotFound)
        );
    }
}
