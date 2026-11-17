//! Scene composition: layers, their order, visibility and transforms.
//!
//! Layers are stored **bottom-to-top**: index 0 is drawn first (furthest back).

pub mod layer;

pub use layer::{
    Arrangement, AudioClip, BlendMode, GroupError, Layer, LayerGroup, LayerKind, LayerStack,
    Transform2D,
};
