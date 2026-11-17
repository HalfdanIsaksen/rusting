//! Symbols: reusable animated objects (ADR-018).
//!
//! A [`Symbol`] is a small animation of its own: layers, one track per
//! layer and a length in frames. It lives in [`Document::symbols`] and is
//! shown in the library's Objects section. Tracks place it with
//! [`Keyframe::Instance`] keys. Its drawings stay in the document's shared
//! drawing pool, in the symbol's own coordinate space.
//!
//! [`Document::symbols`]: crate::Document::symbols
//! [`Keyframe::Instance`]: timeline::Keyframe::Instance

use animation_core::{DrawingId, LayerId, SymbolId};
use scene::{Layer, LayerStack};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use timeline::Track;

/// A reusable animated object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Symbol {
    /// Stable identifier; equal to its key in the document.
    pub id: SymbolId,
    /// Display name in the library.
    pub name: String,
    /// The symbol's layers, bottom to top (no groups).
    pub layers: LayerStack,
    /// One track per layer.
    pub tracks: BTreeMap<LayerId, Track>,
    /// Length of the symbol's animation in frames (at least 1).
    pub frame_count: u32,
}

impl Symbol {
    /// An empty symbol with a fresh ID.
    pub fn new(name: impl Into<String>, frame_count: u32) -> Self {
        Self {
            id: SymbolId::new(),
            name: name.into(),
            layers: LayerStack::new(),
            tracks: BTreeMap::new(),
            frame_count,
        }
    }

    /// Adds `layer` on top with `track`.
    pub fn push_layer(&mut self, layer: Layer, track: Track) {
        self.tracks.insert(layer.id, track);
        self.layers.push(layer);
    }

    /// Every drawing a key of this symbol shows (not through nested
    /// symbols).
    pub fn drawings(&self) -> impl Iterator<Item = DrawingId> + '_ {
        self.tracks
            .values()
            .flat_map(|t| t.keys().filter_map(|(_, k)| k.drawing()))
    }

    /// Every symbol an instance key of this symbol places (one level deep).
    pub fn nested_symbols(&self) -> impl Iterator<Item = SymbolId> + '_ {
        self.tracks
            .values()
            .flat_map(|t| t.keys().filter_map(|(_, k)| k.symbol()))
    }
}
