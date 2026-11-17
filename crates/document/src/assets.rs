//! The asset library: files imported into the project (ADR-017).
//!
//! An [`Asset`] is one imported file, kept once in [`Document::assets`]
//! (keyed by [`AssetId`]). Items placed on the canvas from it, such as
//! [`drawing::Image`], refer to it by ID. The library owns its assets: they
//! stay in the project (and in the file) when nothing uses them, until the
//! user deletes them.
//!
//! [`Document::assets`]: crate::Document::assets

use animation_core::AssetId;
use drawing::ImageData;
use serde::{Deserialize, Serialize};

/// One entry of the library.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    /// Stable identifier; equal to its key in the library.
    pub id: AssetId,
    /// Display name, e.g. the imported file's name.
    pub name: String,
    /// What the asset holds.
    pub kind: AssetKind,
}

impl Asset {
    /// An asset with a fresh ID.
    pub fn new(name: impl Into<String>, kind: AssetKind) -> Self {
        Self {
            id: AssetId::new(),
            name: name.into(),
            kind,
        }
    }

    /// The picture, if this is an image asset.
    pub fn image(&self) -> Option<&ImageData> {
        match &self.kind {
            AssetKind::Image(data) => Some(data),
        }
    }
}

/// What an [`Asset`] holds. New kinds (audio, objects) are added as new
/// variants; serde tags each with its variant name, so files stay readable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AssetKind {
    /// An imported PNG/JPEG picture (ADR-015).
    Image(ImageData),
}

impl AssetKind {
    /// Short user-visible name of the kind, e.g. "Image".
    pub fn label(&self) -> &'static str {
        match self {
            Self::Image(_) => "Image",
        }
    }
}
