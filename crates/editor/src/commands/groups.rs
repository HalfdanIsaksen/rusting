//! Layer groups (folders): grouping, ungrouping, moving and group properties.

use crate::{Command, CommandError};
use animation_core::{GroupId, LayerId};
use document::Document;
use document::scene::{Arrangement, LayerGroup};

/// Puts layers into a new group. The layers move together to where the
/// topmost of them is, keeping their order; layers already in another group
/// leave it.
#[derive(Debug)]
pub struct GroupLayers {
    layers: Vec<LayerId>,
    group: LayerGroup,
    before: Option<Arrangement>,
}

impl GroupLayers {
    /// Creates the command. The group ID is fixed now so redo reuses it.
    pub fn new(layers: Vec<LayerId>, name: impl Into<String>) -> Self {
        Self {
            layers,
            group: LayerGroup::new(name),
            before: None,
        }
    }

    /// ID of the group this command makes.
    pub fn group_id(&self) -> GroupId {
        self.group.id
    }
}

/// The arrangement with `members` gathered at the topmost one, in `group`.
fn gathered(before: &Arrangement, members: &[LayerId], group: GroupId) -> Arrangement {
    let is_member = |id: &LayerId| members.contains(id);
    let top = before
        .iter()
        .rposition(|(id, _)| is_member(id))
        .unwrap_or(0);
    let mut below = 0;
    let mut rest = Vec::new();
    let mut block = Vec::new();
    for (i, &(id, g)) in before.iter().enumerate() {
        if is_member(&id) {
            block.push((id, Some(group)));
        } else {
            if i < top {
                below += 1;
            }
            rest.push((id, g));
        }
    }
    // Never land inside another group: go above it instead.
    let group_at = |i: usize| rest.get(i).and_then(|&(_, g): &(LayerId, _)| g);
    if below > 0
        && let Some(split) = group_at(below - 1).filter(|&g| group_at(below) == Some(g))
    {
        while group_at(below) == Some(split) {
            below += 1;
        }
    }
    rest.splice(below..below, block);
    rest
}

impl Command for GroupLayers {
    fn label(&self) -> &str {
        "Group Layers"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        if self.layers.is_empty() || self.layers.iter().any(|&id| doc.layers().get(id).is_none()) {
            return Err(CommandError::LayerNotFound);
        }
        let before = doc.layers().arrangement();
        let after = gathered(&before, &self.layers, self.group.id);
        doc.add_group(self.group.clone());
        doc.set_arrangement(&after);
        self.before = Some(before);
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let before = self.before.take().ok_or(CommandError::GroupNotFound)?;
        doc.set_arrangement(&before);
        doc.remove_group(self.group.id)
            .map(|_| ())
            .ok_or(CommandError::GroupNotFound)
    }
}

/// Removes a group and keeps its layers where they are.
#[derive(Debug)]
pub struct UngroupLayers {
    group: GroupId,
    removed: Option<(usize, LayerGroup, Arrangement)>,
}

impl UngroupLayers {
    /// Creates the command.
    pub fn new(group: GroupId) -> Self {
        Self {
            group,
            removed: None,
        }
    }
}

impl Command for UngroupLayers {
    fn label(&self) -> &str {
        "Ungroup Layers"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let arrangement = doc.layers().arrangement();
        let (index, group) = doc
            .remove_group(self.group)
            .ok_or(CommandError::GroupNotFound)?;
        self.removed = Some((index, group, arrangement));
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let (index, group, arrangement) = self.removed.take().ok_or(CommandError::GroupNotFound)?;
        doc.insert_group(index, group);
        doc.set_arrangement(&arrangement);
        Ok(())
    }
}

/// Moves a whole group so its bottom layer lands at `to`, counted in the
/// stack without the group's layers. Fails with
/// [`CommandError::GroupInGroup`] if it would land inside another group.
#[derive(Debug)]
pub struct MoveGroup {
    group: GroupId,
    to: usize,
    before: Option<Arrangement>,
}

impl MoveGroup {
    /// Creates the command.
    pub fn new(group: GroupId, to: usize) -> Self {
        Self {
            group,
            to,
            before: None,
        }
    }
}

impl Command for MoveGroup {
    fn label(&self) -> &str {
        "Move Group"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        if doc.layers().group_range(self.group).is_none() {
            return Err(CommandError::GroupNotFound);
        }
        let before = doc.layers().arrangement();
        doc.move_group(self.group, self.to)
            .ok_or(CommandError::GroupInGroup)?;
        self.before = Some(before);
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let before = self.before.take().ok_or(CommandError::GroupNotFound)?;
        doc.set_arrangement(&before);
        Ok(())
    }
}

/// A group property [`SetGroup`] changes.
#[derive(Debug, Clone)]
enum Property {
    Name(String),
    Visible(bool),
    Locked(bool),
}

/// Renames, shows/hides or locks/unlocks a group.
#[derive(Debug)]
pub struct SetGroup {
    group: GroupId,
    property: Property,
}

impl SetGroup {
    /// Renames the group.
    pub fn rename(group: GroupId, name: impl Into<String>) -> Self {
        Self {
            group,
            property: Property::Name(name.into()),
        }
    }

    /// Shows or hides the group (and so all its layers).
    pub fn visible(group: GroupId, visible: bool) -> Self {
        Self {
            group,
            property: Property::Visible(visible),
        }
    }

    /// Locks or unlocks the group (and so all its layers).
    pub fn locked(group: GroupId, locked: bool) -> Self {
        Self {
            group,
            property: Property::Locked(locked),
        }
    }

    fn swap(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let group = doc
            .group_mut(self.group)
            .ok_or(CommandError::GroupNotFound)?;
        match &mut self.property {
            Property::Name(name) => std::mem::swap(&mut group.name, name),
            Property::Visible(visible) => std::mem::swap(&mut group.visible, visible),
            Property::Locked(locked) => std::mem::swap(&mut group.locked, locked),
        }
        Ok(())
    }
}

impl Command for SetGroup {
    fn label(&self) -> &str {
        match self.property {
            Property::Name(_) => "Rename Group",
            Property::Visible(_) => "Set Group Visibility",
            Property::Locked(_) => "Set Group Lock",
        }
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
    use crate::commands::{AddLayer, MoveLayer, RemoveLayer};
    use document::CanvasSize;

    /// Layers A..E, bottom to top.
    fn doc() -> (Document, Vec<LayerId>) {
        let mut d = Document::new(
            "T",
            CanvasSize {
                width: 10,
                height: 10,
            },
        );
        let ids = ["A", "B", "C", "D", "E"]
            .into_iter()
            .map(|n| d.add_layer(n))
            .collect();
        (d, ids)
    }

    fn names(d: &Document) -> Vec<String> {
        d.layers()
            .iter()
            .map(|l| match l.group {
                Some(_) => format!("[{}]", l.name),
                None => l.name.clone(),
            })
            .collect()
    }

    /// Runs `command`, checks undo restores the document and redo the result.
    fn round_trip(d: &mut Document, command: Box<dyn Command>) {
        let before = d.clone();
        let mut h = History::new();
        h.execute(d, command).unwrap();
        let after = d.clone();
        h.undo(d).unwrap();
        assert_eq!(*d, before);
        h.redo(d).unwrap();
        assert_eq!(*d, after);
    }

    #[test]
    fn grouping_gathers_layers_at_the_topmost() {
        let (mut d, ids) = doc();
        let cmd = GroupLayers::new(vec![ids[0], ids[3]], "G");
        let g = cmd.group_id();
        round_trip(&mut d, Box::new(cmd));
        assert_eq!(names(&d), ["B", "C", "[A]", "[D]", "E"]);
        assert_eq!(d.layers().group(g).unwrap().name, "G");
        assert_eq!(d.layers().check_groups(), Ok(()));
    }

    #[test]
    fn grouping_never_lands_inside_another_group() {
        let (mut d, ids) = doc();
        let mut h = History::new();
        h.execute(
            &mut d,
            Box::new(GroupLayers::new(vec![ids[1], ids[2], ids[3]], "G")),
        )
        .unwrap();
        // A and C: C is in the middle of G, so the new group goes above G.
        round_trip(
            &mut d,
            Box::new(GroupLayers::new(vec![ids[0], ids[2]], "H")),
        );
        assert_eq!(names(&d), ["[B]", "[D]", "[A]", "[C]", "E"]);
        assert_eq!(d.layers().check_groups(), Ok(()));
    }

    #[test]
    fn ungroup_keeps_layers_in_place() {
        let (mut d, ids) = doc();
        let group = GroupLayers::new(vec![ids[1], ids[2]], "G");
        let g = group.group_id();
        let mut h = History::new();
        h.execute(&mut d, Box::new(group)).unwrap();
        round_trip(&mut d, Box::new(UngroupLayers::new(g)));
        assert_eq!(names(&d), ["A", "B", "C", "D", "E"]);
        assert!(d.layers().group(g).is_none());
    }

    #[test]
    fn move_group_and_its_properties() {
        let (mut d, ids) = doc();
        let group = GroupLayers::new(vec![ids[0], ids[1]], "G");
        let g = group.group_id();
        let mut h = History::new();
        h.execute(&mut d, Box::new(group)).unwrap();
        round_trip(&mut d, Box::new(MoveGroup::new(g, 3)));
        assert_eq!(names(&d), ["C", "D", "E", "[A]", "[B]"]);
        round_trip(&mut d, Box::new(SetGroup::rename(g, "Ink")));
        round_trip(&mut d, Box::new(SetGroup::visible(g, false)));
        round_trip(&mut d, Box::new(SetGroup::locked(g, true)));
        let group = d.layers().group(g).unwrap();
        assert_eq!(
            (group.name.as_str(), group.visible, group.locked),
            ("Ink", false, true)
        );
        assert!(!d.layers().is_editable(ids[0]));
        assert_eq!(
            MoveGroup::new(GroupId::new(), 0).apply(&mut d),
            Err(CommandError::GroupNotFound)
        );
    }

    #[test]
    fn move_group_into_another_group_is_refused() {
        let (mut d, ids) = doc();
        let g = GroupLayers::new(vec![ids[0]], "G");
        let g_id = g.group_id();
        let mut h = History::new();
        h.execute(&mut d, Box::new(g)).unwrap();
        h.execute(
            &mut d,
            Box::new(GroupLayers::new(vec![ids[2], ids[3]], "H")),
        )
        .unwrap();
        let before = d.clone();
        // Without G: B, [C], [D], E; index 2 is between C and D.
        assert_eq!(
            MoveGroup::new(g_id, 2).apply(&mut d),
            Err(CommandError::GroupInGroup)
        );
        assert_eq!(d, before);
    }

    #[test]
    fn moving_layers_in_and_out_of_groups_undoes_exactly() {
        let (mut d, ids) = doc();
        let group = GroupLayers::new(vec![ids[1], ids[2]], "G");
        let g = group.group_id();
        let mut h = History::new();
        h.execute(&mut d, Box::new(group)).unwrap();
        // E just above C, into G.
        round_trip(&mut d, Box::new(MoveLayer::into_group(ids[4], 3, Some(g))));
        assert_eq!(names(&d), ["A", "[B]", "[C]", "[E]", "D"]);
        // B out of G, at the bottom.
        round_trip(&mut d, Box::new(MoveLayer::into_group(ids[1], 0, None)));
        assert_eq!(names(&d), ["B", "A", "[C]", "[E]", "D"]);
        assert_eq!(d.layers().check_groups(), Ok(()));
    }

    #[test]
    fn removing_and_restoring_a_member_keeps_the_group() {
        let (mut d, ids) = doc();
        let group = GroupLayers::new(vec![ids[1]], "G");
        let g = group.group_id();
        let mut h = History::new();
        h.execute(&mut d, Box::new(group)).unwrap();
        h.execute(&mut d, Box::new(RemoveLayer::new(ids[1])))
            .unwrap();
        assert!(d.layers().group_range(g).is_none());
        assert!(d.has_empty_groups());
        h.undo(&mut d).unwrap();
        assert_eq!(d.layers().get(ids[1]).unwrap().group, Some(g));
        // A new layer on top stays out of groups.
        h.execute(&mut d, Box::new(AddLayer::new("F"))).unwrap();
        assert_eq!(names(&d), ["A", "[B]", "C", "D", "E", "F"]);
    }
}
