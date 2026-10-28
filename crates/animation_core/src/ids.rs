//! Stable entity identifiers.
//!
//! Every ID is a newtype over a random (v4) UUID. IDs are created once and
//! never change, including across save and load.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Creates a new, globally unique ID.
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wraps an existing UUID (used when loading files or in tests).
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// The underlying UUID.
            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

define_id!(
    /// Identifies a document.
    DocumentId
);
define_id!(
    /// Identifies a layer.
    LayerId
);
define_id!(
    /// Identifies a drawing (the content shown by a drawing keyframe).
    DrawingId
);
define_id!(
    /// Identifies a single stroke inside a drawing.
    StrokeId
);
define_id!(
    /// Identifies a single fill (a filled region) inside a drawing.
    FillId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        assert_ne!(LayerId::new(), LayerId::new());
    }

    #[test]
    fn id_roundtrips_through_uuid() {
        let id = DrawingId::new();
        assert_eq!(DrawingId::from_uuid(id.as_uuid()), id);
    }
}
