//! Layers and the layer stack.

use animation_core::{GroupId, LayerId, Vec2};
use serde::{Deserialize, Serialize};

/// A 2D transform from layer-local to document coordinates.
///
/// Identity in the MVP; animated transforms come with property keyframes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform2D {
    /// Translation in document units.
    pub translation: Vec2,
    /// Rotation in radians.
    pub rotation: f32,
    /// Scale factor per axis.
    pub scale: Vec2,
}

impl Transform2D {
    /// The identity transform.
    pub const IDENTITY: Self = Self {
        translation: Vec2::ZERO,
        rotation: 0.0,
        scale: Vec2::ONE,
    };

    /// Maps a layer-local point to document coordinates (scale, rotate, translate).
    pub fn apply(&self, local: Vec2) -> Vec2 {
        let scaled = local * self.scale;
        Vec2::from_angle(self.rotation).rotate(scaled) + self.translation
    }
}

impl Default for Transform2D {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// How a layer's colours combine with the layers below it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlendMode {
    /// Paints over what is below.
    #[default]
    Normal,
    /// Multiplies the colours: always darker, white leaves the backdrop.
    Multiply,
    /// Inverse of Multiply: always lighter, black leaves the backdrop.
    Screen,
    /// Multiply in the backdrop's darks, Screen in its lights: more contrast.
    Overlay,
    /// Overlay with the layers swapped: the layer's darks multiply, its lights screen.
    HardLight,
    /// A gentler Hard Light: darkens or lightens the backdrop a little.
    SoftLight,
}

impl BlendMode {
    /// Every mode, in menu order.
    pub const ALL: [Self; 6] = [
        Self::Normal,
        Self::Multiply,
        Self::Screen,
        Self::Overlay,
        Self::HardLight,
        Self::SoftLight,
    ];

    /// Name shown in the UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Multiply => "Multiply",
            Self::Screen => "Screen",
            Self::Overlay => "Overlay",
            Self::HardLight => "Hard Light",
            Self::SoftLight => "Soft Light",
        }
    }
}

/// A layer in the scene.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    /// Stable identifier.
    pub id: LayerId,
    /// User-visible name.
    pub name: String,
    /// Hidden layers are not rendered.
    pub visible: bool,
    /// Locked layers cannot be drawn on.
    pub locked: bool,
    /// Layer opacity in `0.0..=1.0`.
    pub opacity: f32,
    /// How the layer combines with the layers below it.
    #[serde(default)]
    pub blend: BlendMode,
    /// Layer-local to document transform.
    pub transform: Transform2D,
    /// The group (folder) the layer is in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupId>,
}

impl Layer {
    /// Creates a visible, unlocked layer with a fresh ID.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: LayerId::new(),
            name: name.into(),
            visible: true,
            locked: false,
            opacity: 1.0,
            blend: BlendMode::Normal,
            transform: Transform2D::IDENTITY,
            group: None,
        }
    }

    /// Whether drawing tools may modify this layer.
    pub fn is_editable(&self) -> bool {
        !self.locked
    }
}

/// A group (folder) of layers. Its members are the layers whose
/// [`Layer::group`] is its ID; they are always adjacent in the stack.
///
/// Hiding or locking a group hides or locks every member, and the group's
/// opacity multiplies theirs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerGroup {
    /// Stable identifier.
    pub id: GroupId,
    /// User-visible name.
    pub name: String,
    /// A hidden group hides all its layers.
    pub visible: bool,
    /// A locked group locks all its layers.
    pub locked: bool,
    /// Group opacity in `0.0..=1.0`, multiplied into each member's.
    pub opacity: f32,
}

impl LayerGroup {
    /// Creates a visible, unlocked, opaque group with a fresh ID.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: GroupId::new(),
            name: name.into(),
            visible: true,
            locked: false,
            opacity: 1.0,
        }
    }
}

/// The order of every layer and the group each is in, bottom to top. Taken
/// with [`LayerStack::arrangement`] and put back with
/// [`LayerStack::set_arrangement`] to undo reordering and regrouping.
pub type Arrangement = Vec<(LayerId, Option<GroupId>)>;

/// Why the layers' groups are inconsistent (see [`LayerStack::check_groups`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupError {
    /// A layer refers to a group that does not exist.
    MissingGroup(LayerId, GroupId),
    /// A group's layers are not adjacent in the stack.
    Split(GroupId),
    /// Two groups share an ID.
    Duplicate(GroupId),
}

/// An ordered collection of layers, bottom-to-top, and the groups they are in.
///
/// Group invariant: every layer's group exists, and each group's members
/// are adjacent. Every method that moves or inserts layers keeps it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LayerStack {
    layers: Vec<Layer>,
    /// Groups in creation order. A group with no members is kept (undo of a
    /// layer removal may need it) but not shown; saving drops it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    groups: Vec<LayerGroup>,
}

impl LayerStack {
    /// Creates an empty stack.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a layer at `index` (clamped to the stack size). Its group is
    /// adjusted if needed to keep groups together (see [`Self::move_to`]).
    pub fn insert(&mut self, index: usize, layer: Layer) {
        let index = index.min(self.layers.len());
        self.layers.insert(index, layer);
        self.fit_group(index);
    }

    /// Adds a layer on top.
    pub fn push(&mut self, layer: Layer) {
        self.insert(self.layers.len(), layer);
    }

    /// Removes a layer, returning its former index and the layer.
    pub fn remove(&mut self, id: LayerId) -> Option<(usize, Layer)> {
        let index = self.index_of(id)?;
        Some((index, self.layers.remove(index)))
    }

    /// Moves a layer to `new_index` (clamped). Returns the old index.
    ///
    /// Groups stay together: a layer dropped between two layers of one group
    /// joins it, and a group layer moved away from the rest of its group
    /// leaves it.
    pub fn move_to(&mut self, id: LayerId, new_index: usize) -> Option<usize> {
        let group = self.get(id)?.group;
        self.move_to_group(id, new_index, group).map(|(old, _)| old)
    }

    /// Moves a layer to `new_index` (clamped) and puts it in `group` (`None`:
    /// in no group). If that would split a group, the layer gets the group
    /// [`Self::move_to`] would give it instead. Returns the old index and
    /// group, for undo.
    pub fn move_to_group(
        &mut self,
        id: LayerId,
        new_index: usize,
        group: Option<GroupId>,
    ) -> Option<(usize, Option<GroupId>)> {
        let old = self.index_of(id)?;
        let mut layer = self.layers.remove(old);
        let old_group = layer.group;
        layer.group = group.filter(|g| self.group(*g).is_some());
        let new_index = new_index.min(self.layers.len());
        self.layers.insert(new_index, layer);
        self.fit_group(new_index);
        Some((old, old_group))
    }

    /// Gives the layer at `index` a group that keeps every group together:
    /// between two layers of one group it must join that group; otherwise it
    /// keeps its group only next to another member (or as the only member).
    fn fit_group(&mut self, index: usize) {
        let group_at = |i: Option<usize>| i.and_then(|i| self.layers.get(i)).and_then(|l| l.group);
        let below = group_at(index.checked_sub(1));
        let above = group_at(Some(index + 1));
        let own = self.layers[index].group;
        let fitted = if below.is_some() && below == above {
            below
        } else {
            own.filter(|&g| {
                below == Some(g)
                    || above == Some(g)
                    || !self
                        .layers
                        .iter()
                        .enumerate()
                        .any(|(i, l)| i != index && l.group == Some(g))
            })
        };
        self.layers[index].group = fitted;
    }

    /// Position of a layer in the stack (0 = bottom).
    pub fn index_of(&self, id: LayerId) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    /// Looks up a layer.
    pub fn get(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    /// Mutable lookup of a layer.
    pub fn get_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    /// Layers from bottom to top (render order).
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Layer> {
        self.layers.iter()
    }

    /// Number of layers.
    pub fn len(&self) -> usize {
        self.layers.len()
    }

    /// Whether the stack is empty.
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    // ----- Groups --------------------------------------------------------

    /// Every group, including empty ones, in creation order.
    pub fn groups(&self) -> impl Iterator<Item = &LayerGroup> {
        self.groups.iter()
    }

    /// Looks up a group.
    pub fn group(&self, id: GroupId) -> Option<&LayerGroup> {
        self.groups.iter().find(|g| g.id == id)
    }

    /// Mutable lookup of a group.
    pub fn group_mut(&mut self, id: GroupId) -> Option<&mut LayerGroup> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    /// Adds a group record (with no members yet). Does nothing if a group
    /// with its ID exists.
    pub fn add_group(&mut self, group: LayerGroup) {
        if self.group(group.id).is_none() {
            self.groups.push(group);
        }
    }

    /// Puts a group record back at `index` in the group list (clamped), as
    /// returned by [`Self::remove_group`]. Does nothing if its ID exists.
    pub fn insert_group(&mut self, index: usize, group: LayerGroup) {
        if self.group(group.id).is_none() {
            let index = index.min(self.groups.len());
            self.groups.insert(index, group);
        }
    }

    /// Removes a group record, taking its layers out of it. Returns its
    /// index in the group list and the group.
    pub fn remove_group(&mut self, id: GroupId) -> Option<(usize, LayerGroup)> {
        let index = self.groups.iter().position(|g| g.id == id)?;
        for layer in &mut self.layers {
            if layer.group == Some(id) {
                layer.group = None;
            }
        }
        Some((index, self.groups.remove(index)))
    }

    /// Drops groups that have no layers. Returns how many were dropped.
    pub fn remove_empty_groups(&mut self) -> usize {
        let before = self.groups.len();
        let layers = &self.layers;
        self.groups
            .retain(|g| layers.iter().any(|l| l.group == Some(g.id)));
        before - self.groups.len()
    }

    /// Stack indices (0 = bottom) of a group's layers, or `None` if it has none.
    pub fn group_range(&self, id: GroupId) -> Option<std::ops::RangeInclusive<usize>> {
        let first = self.layers.iter().position(|l| l.group == Some(id))?;
        let last = self.layers.iter().rposition(|l| l.group == Some(id))?;
        Some(first..=last)
    }

    /// The group of a layer, if it is in one.
    pub fn group_of(&self, layer: LayerId) -> Option<&LayerGroup> {
        self.group(self.get(layer)?.group?)
    }

    /// Whether a layer is shown: it and its group are visible.
    pub fn is_visible(&self, layer: LayerId) -> bool {
        self.get(layer).is_some_and(|l| l.visible) && self.group_of(layer).is_none_or(|g| g.visible)
    }

    /// Whether drawing tools may modify a layer: neither it nor its group
    /// is locked.
    pub fn is_editable(&self, layer: LayerId) -> bool {
        self.get(layer).is_some_and(Layer::is_editable)
            && self.group_of(layer).is_none_or(|g| !g.locked)
    }

    /// A layer's opacity times its group's.
    pub fn effective_opacity(&self, layer: LayerId) -> f32 {
        let own = self.get(layer).map_or(0.0, |l| l.opacity);
        own * self.group_of(layer).map_or(1.0, |g| g.opacity)
    }

    /// Moves a whole group so its bottom layer lands at `new_bottom`
    /// (counted in the stack without the group, clamped). Fails without
    /// changes if the group has no layers or would land inside another
    /// group. Returns the old bottom index.
    pub fn move_group(&mut self, id: GroupId, new_bottom: usize) -> Option<usize> {
        let range = self.group_range(id)?;
        let old = *range.start();
        let mut rest = self.layers.clone();
        let block: Vec<Layer> = rest.drain(range).collect();
        let at = new_bottom.min(rest.len());
        let below = at.checked_sub(1).and_then(|i| rest[i].group);
        let above = rest.get(at).and_then(|l| l.group);
        if below.is_some() && below == above {
            return None;
        }
        rest.splice(at..at, block);
        self.layers = rest;
        Some(old)
    }

    /// The order of every layer and its group, bottom to top.
    pub fn arrangement(&self) -> Arrangement {
        self.layers.iter().map(|l| (l.id, l.group)).collect()
    }

    /// Reorders and regroups the layers to `arrangement`, which must name
    /// exactly the layers in the stack. Groups it names that do not exist
    /// are dropped. Returns whether it was applied.
    pub fn set_arrangement(&mut self, arrangement: &[(LayerId, Option<GroupId>)]) -> bool {
        if arrangement.len() != self.layers.len()
            || arrangement.iter().any(|(id, _)| self.get(*id).is_none())
        {
            return false;
        }
        let mut old = std::mem::take(&mut self.layers);
        for (id, group) in arrangement {
            if let Some(i) = old.iter().position(|l| l.id == *id) {
                let mut layer = old.swap_remove(i);
                layer.group = group.filter(|g| self.groups.iter().any(|x| x.id == *g));
                self.layers.push(layer);
            }
        }
        true
    }

    /// Checks the group invariant.
    pub fn check_groups(&self) -> Result<(), GroupError> {
        let mut seen = std::collections::BTreeSet::new();
        for group in &self.groups {
            if !seen.insert(group.id) {
                return Err(GroupError::Duplicate(group.id));
            }
        }
        for layer in &self.layers {
            if let Some(g) = layer.group
                && !seen.contains(&g)
            {
                return Err(GroupError::MissingGroup(layer.id, g));
            }
        }
        for group in &self.groups {
            if let Some(range) = self.group_range(group.id)
                && self.layers[range].iter().any(|l| l.group != Some(group.id))
            {
                return Err(GroupError::Split(group.id));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(s: &LayerStack) -> Vec<&str> {
        s.iter().map(|l| l.name.as_str()).collect()
    }

    #[test]
    fn ordering_is_bottom_to_top() {
        let mut s = LayerStack::new();
        let a = Layer::new("A");
        let a_id = a.id;
        s.push(a);
        s.push(Layer::new("B"));
        s.push(Layer::new("C"));
        assert_eq!(names(&s), ["A", "B", "C"]);
        assert_eq!(s.move_to(a_id, 2), Some(0));
        assert_eq!(names(&s), ["B", "C", "A"]);
        assert_eq!(s.move_to(a_id, 99), Some(2));
        assert_eq!(names(&s), ["B", "C", "A"]);
    }

    #[test]
    fn remove_and_reinsert_restores_order() {
        let mut s = LayerStack::new();
        let b = Layer::new("B");
        let b_id = b.id;
        s.push(Layer::new("A"));
        s.push(b);
        s.push(Layer::new("C"));
        let (idx, layer) = s.remove(b_id).unwrap();
        assert_eq!(names(&s), ["A", "C"]);
        s.insert(idx, layer);
        assert_eq!(names(&s), ["A", "B", "C"]);
    }

    #[test]
    fn locked_layer_is_not_editable() {
        let mut l = Layer::new("L");
        assert!(l.is_editable());
        l.locked = true;
        assert!(!l.is_editable());
    }

    #[test]
    fn transform_applies_scale_rotate_translate() {
        let t = Transform2D {
            translation: Vec2::new(10.0, 0.0),
            rotation: std::f32::consts::FRAC_PI_2,
            scale: Vec2::splat(2.0),
        };
        let p = t.apply(Vec2::new(1.0, 0.0));
        assert!((p - Vec2::new(10.0, 2.0)).length() < 1e-5);
        assert_eq!(
            Transform2D::IDENTITY.apply(Vec2::new(3.0, 4.0)),
            Vec2::new(3.0, 4.0)
        );
    }

    /// Stack A, B, C, D, E (bottom to top) with B and C in group G.
    fn grouped() -> (LayerStack, Vec<LayerId>, GroupId) {
        let mut s = LayerStack::new();
        let group = LayerGroup::new("G");
        let g = group.id;
        s.add_group(group);
        let mut ids = Vec::new();
        for (name, in_group) in [
            ("A", false),
            ("B", true),
            ("C", true),
            ("D", false),
            ("E", false),
        ] {
            let mut l = Layer::new(name);
            l.group = in_group.then_some(g);
            ids.push(l.id);
            s.push(l);
        }
        (s, ids, g)
    }

    fn groups(s: &LayerStack) -> Vec<bool> {
        s.iter().map(|l| l.group.is_some()).collect()
    }

    #[test]
    fn moving_between_group_members_joins_and_moving_away_leaves() {
        let (mut s, ids, g) = grouped();
        // E between B and C joins G.
        s.move_to(ids[4], 2);
        assert_eq!(names(&s), ["A", "B", "E", "C", "D"]);
        assert_eq!(s.get(ids[4]).unwrap().group, Some(g));
        // C to the top leaves G.
        s.move_to(ids[2], 4);
        assert_eq!(s.get(ids[2]).unwrap().group, None);
        // A next to the group, but not between members, stays out.
        s.move_to(ids[0], 2);
        assert_eq!(names(&s), ["B", "E", "A", "D", "C"]);
        assert_eq!(s.get(ids[0]).unwrap().group, None);
        assert_eq!(s.check_groups(), Ok(()));
    }

    #[test]
    fn explicit_group_move_and_last_member_keeps_group() {
        let (mut s, ids, g) = grouped();
        // D just above C, asked into G: joins as the top member.
        assert_eq!(s.move_to_group(ids[3], 3, Some(g)), Some((3, None)));
        assert_eq!(groups(&s), [false, true, true, true, false]);
        // Asking A into G far from it is corrected to no group.
        s.move_to_group(ids[0], 4, Some(g));
        assert_eq!(s.get(ids[0]).unwrap().group, None);
        assert_eq!(s.check_groups(), Ok(()));

        let mut solo = LayerStack::new();
        let group = LayerGroup::new("Solo");
        let sg = group.id;
        solo.add_group(group);
        let mut a = Layer::new("A");
        a.group = Some(sg);
        let a_id = a.id;
        solo.push(a);
        solo.push(Layer::new("B"));
        solo.move_to(a_id, 1);
        assert_eq!(
            solo.get(a_id).unwrap().group,
            Some(sg),
            "sole member keeps its group"
        );
    }

    #[test]
    fn group_moves_as_a_block_and_never_inside_another_group() {
        let (mut s, ids, g) = grouped();
        assert_eq!(s.move_group(g, 3), Some(1));
        assert_eq!(names(&s), ["A", "D", "E", "B", "C"]);
        assert_eq!(s.move_group(g, 0), Some(3));
        assert_eq!(names(&s), ["B", "C", "A", "D", "E"]);

        let other = LayerGroup::new("H");
        let h = other.id;
        s.add_group(other);
        s.move_to_group(ids[3], 3, Some(h));
        s.move_to_group(ids[4], 4, Some(h));
        // Between D and E (both in H) is refused.
        let before = s.clone();
        assert_eq!(s.move_group(g, 2), None);
        assert_eq!(s, before);
        assert_eq!(s.check_groups(), Ok(()));
    }

    #[test]
    fn arrangement_round_trips_and_effective_state_uses_the_group() {
        let (mut s, ids, g) = grouped();
        let before = s.arrangement();
        s.move_group(g, 3);
        s.remove_group(g);
        s.add_group(LayerGroup {
            id: g,
            ..LayerGroup::new("G")
        });
        assert!(s.set_arrangement(&before));
        assert_eq!(s.arrangement(), before);
        assert!(!s.set_arrangement(&before[1..]));

        let group = s.group_mut(g).unwrap();
        group.visible = false;
        group.locked = true;
        group.opacity = 0.5;
        s.get_mut(ids[1]).unwrap().opacity = 0.5;
        assert!(!s.is_visible(ids[1]) && !s.is_editable(ids[1]));
        assert!(s.is_visible(ids[0]) && s.is_editable(ids[0]));
        assert_eq!(s.effective_opacity(ids[1]), 0.25);
        assert_eq!(s.effective_opacity(ids[0]), 1.0);
    }

    #[test]
    fn check_groups_finds_split_and_missing_groups_and_empty_ones_are_dropped() {
        let (mut s, ids, g) = grouped();
        // Bypass the fitting rules to build a split group.
        s.layers[4].group = Some(g);
        assert_eq!(s.check_groups(), Err(GroupError::Split(g)));
        s.layers[4].group = Some(GroupId::new());
        assert!(matches!(
            s.check_groups(),
            Err(GroupError::MissingGroup(..))
        ));
        s.layers[4].group = None;
        s.add_group(LayerGroup::new("Empty"));
        assert_eq!(s.remove_empty_groups(), 1);
        assert_eq!(s.groups().count(), 1);
        s.remove_group(g);
        assert!(s.iter().all(|l| l.group.is_none()));
        assert_eq!(s.len(), ids.len());
    }
}
