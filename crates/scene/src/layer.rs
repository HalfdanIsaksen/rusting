//! Layers and the layer stack.

use animation_core::{LayerId, Vec2};
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
}

impl BlendMode {
    /// Every mode, in menu order.
    pub const ALL: [Self; 2] = [Self::Normal, Self::Multiply];

    /// Name shown in the UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Multiply => "Multiply",
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
        }
    }

    /// Whether drawing tools may modify this layer.
    pub fn is_editable(&self) -> bool {
        !self.locked
    }
}

/// An ordered collection of layers, bottom-to-top.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LayerStack {
    layers: Vec<Layer>,
}

impl LayerStack {
    /// Creates an empty stack.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a layer at `index` (clamped to the stack size).
    pub fn insert(&mut self, index: usize, layer: Layer) {
        let index = index.min(self.layers.len());
        self.layers.insert(index, layer);
    }

    /// Adds a layer on top.
    pub fn push(&mut self, layer: Layer) {
        self.layers.push(layer);
    }

    /// Removes a layer, returning its former index and the layer.
    pub fn remove(&mut self, id: LayerId) -> Option<(usize, Layer)> {
        let index = self.index_of(id)?;
        Some((index, self.layers.remove(index)))
    }

    /// Moves a layer to `new_index` (clamped). Returns the old index.
    pub fn move_to(&mut self, id: LayerId, new_index: usize) -> Option<usize> {
        let old = self.index_of(id)?;
        let layer = self.layers.remove(old);
        let new_index = new_index.min(self.layers.len());
        self.layers.insert(new_index, layer);
        Some(old)
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
}
