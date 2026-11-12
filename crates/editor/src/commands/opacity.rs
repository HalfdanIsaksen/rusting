//! Changing layer and group opacity, one undo step per slider drag.

use crate::{Command, CommandError};
use animation_core::{GroupId, LayerId};
use document::Document;
use std::any::Any;

/// Whose opacity [`SetOpacity`] changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Layer(LayerId),
    Group(GroupId),
}

impl Target {
    fn opacity_mut(self, doc: &mut Document) -> Result<&mut f32, CommandError> {
        match self {
            Self::Layer(id) => doc
                .layer_mut(id)
                .map(|l| &mut l.opacity)
                .ok_or(CommandError::LayerNotFound),
            Self::Group(id) => doc
                .group_mut(id)
                .map(|g| &mut g.opacity)
                .ok_or(CommandError::GroupNotFound),
        }
    }
}

/// Sets a layer's or a group's opacity (clamped to `0.0..=1.0`).
///
/// Commands made with [`SetOpacity::in_gesture`] for the same target and
/// gesture merge in the history, so every value of one slider drag undoes as
/// one step back to the value before the drag.
#[derive(Debug)]
pub struct SetOpacity {
    target: Target,
    opacity: f32,
    /// Opacity before the first apply; set by `apply`.
    before: Option<f32>,
    gesture: Option<u64>,
}

impl SetOpacity {
    fn new(target: Target, opacity: f32) -> Self {
        Self {
            target,
            opacity: opacity.clamp(0.0, 1.0),
            before: None,
            gesture: None,
        }
    }

    /// Sets a layer's own opacity.
    pub fn layer(id: LayerId, opacity: f32) -> Self {
        Self::new(Target::Layer(id), opacity)
    }

    /// Sets a group's opacity, which multiplies its layers' opacity.
    pub fn group(id: GroupId, opacity: f32) -> Self {
        Self::new(Target::Group(id), opacity)
    }

    /// Marks the command as one value of the drag or edit `gesture`; the
    /// history merges consecutive commands of the same gesture and target.
    pub fn in_gesture(mut self, gesture: u64) -> Self {
        self.gesture = Some(gesture);
        self
    }
}

impl Command for SetOpacity {
    fn label(&self) -> &str {
        match self.target {
            Target::Layer(_) => "Set Layer Opacity",
            Target::Group(_) => "Set Group Opacity",
        }
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let value = self.target.opacity_mut(doc)?;
        self.before = Some(*value);
        *value = self.opacity;
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let value = self.target.opacity_mut(doc)?;
        *value = self.before.unwrap_or(*value);
        Ok(())
    }

    fn merge(&mut self, next: &dyn Command) -> bool {
        let Some(next) = (next as &dyn Any).downcast_ref::<Self>() else {
            return false;
        };
        if self.gesture.is_none() || next.gesture != self.gesture || next.target != self.target {
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
    use document::scene::LayerGroup;

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
        h.execute(&mut d, Box::new(SetOpacity::layer(id, 1.7)))
            .unwrap();
        assert_eq!(opacity(&d, id), 1.0);
        h.execute(&mut d, Box::new(SetOpacity::layer(id, 0.25)))
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
            h.execute(&mut d, Box::new(SetOpacity::layer(id, v).in_gesture(7)))
                .unwrap();
        }
        h.execute(&mut d, Box::new(SetOpacity::layer(id, 0.2).in_gesture(8)))
            .unwrap();
        assert_eq!(h.undo_len(), 2);
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
        h.execute(&mut d, Box::new(SetOpacity::layer(id, 0.5).in_gesture(1)))
            .unwrap();
        h.mark_saved();
        h.execute(&mut d, Box::new(SetOpacity::layer(id, 0.3).in_gesture(1)))
            .unwrap();
        assert!(h.is_dirty());
    }

    #[test]
    fn group_opacity_and_missing_targets() {
        let (mut d, _) = doc();
        let group = LayerGroup::new("G");
        let g = group.id;
        d.add_group(group);
        let mut h = History::new();
        h.execute(&mut d, Box::new(SetOpacity::group(g, 0.3)))
            .unwrap();
        assert_eq!(d.layers().group(g).unwrap().opacity, 0.3);
        h.undo(&mut d).unwrap();
        assert_eq!(d.layers().group(g).unwrap().opacity, 1.0);
        assert_eq!(
            SetOpacity::layer(LayerId::new(), 0.5).apply(&mut d),
            Err(CommandError::LayerNotFound)
        );
        assert_eq!(
            SetOpacity::group(GroupId::new(), 0.5).apply(&mut d),
            Err(CommandError::GroupNotFound)
        );
    }
}
