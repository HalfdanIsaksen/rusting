//! Native `.rusting` project files (ADR-006).
//!
//! A file is a JSON envelope `{ "format_version": N, "document": { ... } }`.
//! Loading checks the version, migrates older versions, and validates that
//! every reference in the document points at something that exists.

use animation_core::{DrawingId, FrameIndex, LayerId};
use document::Document;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The format version written by this build.
pub const FORMAT_VERSION: u32 = 1;

/// File extension for project files (without the dot).
pub const FILE_EXTENSION: &str = "rusting";

/// Errors from saving or loading a project.
#[derive(Debug, thiserror::Error)]
pub enum FileFormatError {
    /// Reading or writing the file failed.
    #[error("could not access project file: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not valid JSON or does not match the expected structure.
    #[error("project file is malformed: {0}")]
    Malformed(#[from] serde_json::Error),
    /// The file was written by a newer version of the application.
    #[error("project file version {found} is newer than supported version {FORMAT_VERSION}")]
    UnsupportedVersion {
        /// Version found in the file.
        found: u32,
    },
    /// The document parsed but is internally inconsistent.
    #[error("project file is invalid: {0}")]
    Invalid(#[from] ValidationError),
}

/// A consistency problem in a loaded document.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ValidationError {
    /// A keyframe refers to a drawing that does not exist.
    #[error("layer {layer} frame {frame} refers to missing drawing {drawing}")]
    MissingDrawing {
        /// Layer holding the keyframe.
        layer: LayerId,
        /// Frame of the keyframe.
        frame: FrameIndex,
        /// The missing drawing.
        drawing: DrawingId,
    },
    /// A track exists for a layer that does not exist.
    #[error("track belongs to missing layer {0}")]
    OrphanTrack(LayerId),
    /// A layer has no track.
    #[error("layer {0} has no track")]
    MissingTrack(LayerId),
}

#[derive(Serialize)]
struct EnvelopeOut<'a> {
    format_version: u32,
    document: &'a Document,
}

#[derive(Deserialize)]
struct EnvelopeIn {
    format_version: u32,
    document: serde_json::Value,
}

/// Serializes a document to a JSON string.
pub fn to_string(doc: &Document) -> Result<String, FileFormatError> {
    Ok(serde_json::to_string_pretty(&EnvelopeOut {
        format_version: FORMAT_VERSION,
        document: doc,
    })?)
}

/// Parses, migrates and validates a document from a JSON string.
pub fn from_str(text: &str) -> Result<Document, FileFormatError> {
    let envelope: EnvelopeIn = serde_json::from_str(text)?;
    if envelope.format_version > FORMAT_VERSION {
        return Err(FileFormatError::UnsupportedVersion {
            found: envelope.format_version,
        });
    }
    let migrated = migrate(envelope.format_version, envelope.document);
    let doc: Document = serde_json::from_value(migrated)?;
    validate(&doc)?;
    Ok(doc)
}

/// Saves a document to `path`.
pub fn save(doc: &Document, path: &Path) -> Result<(), FileFormatError> {
    std::fs::write(path, to_string(doc)?)?;
    Ok(())
}

/// Loads a document from `path`.
pub fn load(path: &Path) -> Result<Document, FileFormatError> {
    from_str(&std::fs::read_to_string(path)?)
}

/// Upgrades raw document JSON from `version` to [`FORMAT_VERSION`].
/// Version 1 is the first format, so there is nothing to migrate yet.
fn migrate(_version: u32, document: serde_json::Value) -> serde_json::Value {
    document
}

/// Checks that every reference in `doc` resolves.
pub fn validate(doc: &Document) -> Result<(), ValidationError> {
    for layer in doc.layers().iter() {
        if doc.track(layer.id).is_none() {
            return Err(ValidationError::MissingTrack(layer.id));
        }
    }
    for (&layer, track) in doc.tracks() {
        if doc.layers().get(layer).is_none() {
            return Err(ValidationError::OrphanTrack(layer));
        }
        for (frame, key) in track.keys() {
            if let Some(drawing) = key.drawing()
                && doc.drawing(drawing).is_none()
            {
                return Err(ValidationError::MissingDrawing {
                    layer,
                    frame,
                    drawing,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use animation_core::{Rgba, Vec2};
    use document::CanvasSize;
    use drawing::{BrushStyle, DrawingContent, Stroke, StrokePoint, VectorDrawing};
    use timeline::Keyframe;

    fn sample() -> Document {
        let mut d = Document::new(
            "Roundtrip",
            CanvasSize {
                width: 640,
                height: 480,
            },
        );
        d.background = None;
        let l1 = d.add_layer("Ink");
        let l2 = d.add_layer("Color");
        let mut v = VectorDrawing::new();
        v.push_stroke(Stroke::new(
            vec![
                StrokePoint::new(Vec2::new(1.0, 2.0)),
                StrokePoint {
                    pos: Vec2::new(3.5, 4.0),
                    pressure: 0.5,
                },
            ],
            BrushStyle {
                color: Rgba::new(0.2, 0.4, 0.6, 1.0),
                width: 3.0,
                opacity: 0.8,
            },
        ));
        let drawing = d.add_drawing(DrawingContent::Vector(v));
        d.track_mut(l1)
            .unwrap()
            .set_key(FrameIndex(0), Keyframe::Drawing(drawing));
        d.track_mut(l1)
            .unwrap()
            .set_key(FrameIndex(6), Keyframe::Empty);
        d.layer_mut(l2).unwrap().locked = true;
        d
    }

    #[test]
    fn roundtrip_preserves_document() {
        let doc = sample();
        let loaded = from_str(&to_string(&doc).unwrap()).unwrap();
        assert_eq!(loaded, doc);
    }

    #[test]
    fn roundtrip_through_file() {
        let doc = sample();
        let path = std::env::temp_dir().join(format!("rusting-test-{}.{FILE_EXTENSION}", doc.id));
        save(&doc, &path).unwrap();
        let loaded = load(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded.unwrap(), doc);
    }

    #[test]
    fn newer_version_is_rejected() {
        let text = to_string(&sample()).unwrap().replacen(
            "\"format_version\": 1",
            "\"format_version\": 99",
            1,
        );
        assert!(matches!(
            from_str(&text),
            Err(FileFormatError::UnsupportedVersion { found: 99 })
        ));
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(matches!(
            from_str("not json"),
            Err(FileFormatError::Malformed(_))
        ));
        assert!(matches!(
            from_str("{\"format_version\":1,\"document\":{}}"),
            Err(FileFormatError::Malformed(_))
        ));
    }

    #[test]
    fn missing_drawing_is_detected() {
        let mut doc = sample();
        let layer = doc.layers().iter().next().unwrap().id;
        doc.track_mut(layer)
            .unwrap()
            .set_key(FrameIndex(3), Keyframe::Drawing(DrawingId::new()));
        assert!(matches!(
            validate(&doc),
            Err(ValidationError::MissingDrawing { .. })
        ));
        assert!(matches!(
            from_str(&to_string(&doc).unwrap()),
            Err(FileFormatError::Invalid(_))
        ));
    }
}
