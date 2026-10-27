//! Concrete document commands.

use crate::{Command, CommandError};
use animation_core::LayerId;
use document::{Document, RemovedLayer, scene::Layer};

/// Adds a new layer on top of the stack.
#[derive(Debug)]
pub struct AddLayer {
    layer: Layer,
}

impl AddLayer {
    /// Creates the command. The layer ID is fixed now so redo reuses it.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            layer: Layer::new(name),
        }
    }

    /// ID of the layer this command adds.
    pub fn layer_id(&self) -> LayerId {
        self.layer.id
    }
}

impl Command for AddLayer {
    fn label(&self) -> &str {
        "Add Layer"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let top = doc.layers().len();
        doc.insert_layer(top, self.layer.clone());
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        doc.remove_layer(self.layer.id)
            .map(|_| ())
            .ok_or(CommandError::LayerNotFound)
    }
}

/// Removes a layer (and its track).
#[derive(Debug)]
pub struct RemoveLayer {
    id: LayerId,
    removed: Option<RemovedLayer>,
}

impl RemoveLayer {
    /// Creates the command.
    pub fn new(id: LayerId) -> Self {
        Self { id, removed: None }
    }
}

impl Command for RemoveLayer {
    fn label(&self) -> &str {
        "Remove Layer"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        self.removed = Some(
            doc.remove_layer(self.id)
                .ok_or(CommandError::LayerNotFound)?,
        );
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let removed = self.removed.take().ok_or(CommandError::LayerNotFound)?;
        doc.restore_layer(removed);
        Ok(())
    }
}

/// Renames a layer.
#[derive(Debug)]
pub struct RenameLayer {
    id: LayerId,
    name: String,
}

impl RenameLayer {
    /// Creates the command.
    pub fn new(id: LayerId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }

    fn swap(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let layer = doc.layer_mut(self.id).ok_or(CommandError::LayerNotFound)?;
        std::mem::swap(&mut layer.name, &mut self.name);
        Ok(())
    }
}

impl Command for RenameLayer {
    fn label(&self) -> &str {
        "Rename Layer"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        self.swap(doc)
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        self.swap(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::History;
    use document::CanvasSize;

    fn doc() -> Document {
        Document::new(
            "T",
            CanvasSize {
                width: 10,
                height: 10,
            },
        )
    }

    #[test]
    fn add_layer_undo_redo_keeps_id() {
        let mut d = doc();
        let mut h = History::new();
        let cmd = AddLayer::new("L1");
        let id = cmd.layer_id();
        h.execute(&mut d, Box::new(cmd)).unwrap();
        assert!(d.layers().get(id).is_some());
        assert!(h.undo(&mut d).unwrap());
        assert!(d.layers().is_empty());
        assert!(h.redo(&mut d).unwrap());
        assert_eq!(d.layers().get(id).map(|l| l.name.as_str()), Some("L1"));
    }

    #[test]
    fn remove_layer_undo_restores_exact_document() {
        let mut d = doc();
        let a = d.add_layer("A");
        d.add_layer("B");
        let before = d.clone();
        let mut h = History::new();
        h.execute(&mut d, Box::new(RemoveLayer::new(a))).unwrap();
        assert_ne!(d, before);
        h.undo(&mut d).unwrap();
        assert_eq!(d, before);
    }

    #[test]
    fn rename_undo_redo() {
        let mut d = doc();
        let id = d.add_layer("Old");
        let mut h = History::new();
        h.execute(&mut d, Box::new(RenameLayer::new(id, "New")))
            .unwrap();
        assert_eq!(d.layers().get(id).unwrap().name, "New");
        h.undo(&mut d).unwrap();
        assert_eq!(d.layers().get(id).unwrap().name, "Old");
        h.redo(&mut d).unwrap();
        assert_eq!(d.layers().get(id).unwrap().name, "New");
    }

    #[test]
    fn failed_command_is_not_recorded_and_new_edit_clears_redo() {
        let mut d = doc();
        let mut h = History::new();
        let missing = RemoveLayer::new(LayerId::new());
        assert_eq!(
            h.execute(&mut d, Box::new(missing)),
            Err(CommandError::LayerNotFound)
        );
        assert!(!h.can_undo());

        h.execute(&mut d, Box::new(AddLayer::new("A"))).unwrap();
        h.undo(&mut d).unwrap();
        assert!(h.can_redo());
        h.execute(&mut d, Box::new(AddLayer::new("B"))).unwrap();
        assert!(!h.can_redo());
        assert_eq!(h.undo_label(), Some("Add Layer"));
    }
}
