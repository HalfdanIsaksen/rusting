//! Scene composition: layers, their order, visibility and transforms.
//!
//! Layers are stored **bottom-to-top**: index 0 is drawn first (furthest back).

pub mod layer;

pub use layer::{Arrangement, BlendMode, GroupError, Layer, LayerGroup, LayerStack, Transform2D};
