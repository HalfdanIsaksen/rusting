//! Native `.rusting` project files (ADR-006).
//!
//! A file is a JSON envelope `{ "format_version": N, "document": { ... } }`.
//! Loading checks the version, migrates older versions, and validates that
//! every reference in the document points at something that exists.
//!
//! # Saving
//! Drawings that no keyframe refers to are kept in memory (undo may need
//! them) but are left out of the file. [`save`] writes to a temporary file in
//! the same directory and renames it over the target, so a crash or full disk
//! never leaves a half-written project behind.
//!
//! # Unknown fields
//! Within a supported version, fields this build does not know are **ignored**
//! by [`from_str`] and [`load`]; the document still loads. A writer that adds
//! data older builds cannot keep must bump [`FORMAT_VERSION`], which older
//! builds reject with [`FileFormatError::UnsupportedVersion`]. Unknown fields
//! can therefore only come from hand edits or other tools. To avoid dropping
//! them silently, [`from_str_with_report`] and [`load_with_report`] list them
//! in a [`LoadReport`] so the app can warn before overwriting the file.

use animation_core::{DrawingId, FillId, FrameIndex, GroupId, LayerId, StrokeId};
use document::Document;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The format version written by this build.
///
/// - 1: first format.
/// - 2: vector drawings have a `fills` list (fill bucket, ADR-007).
/// - 3: fills may have a `gradient` (absent means solid, so v2 needs no
///   migration).
/// - 4: layers have a `blend` mode (absent means Normal, so v3 needs no
///   migration).
/// - 5: the document has frame `tags` (absent means none, so v4 needs no
///   migration; ADR-009).
/// - 6: strokes and fills may have a `placement` draw-order tier, `"Front"`
///   (absent means `Normal`, so v5 needs no migration; ADR-010). Older builds
///   must reject these files: they would drop the tier and change the
///   picture.
/// - 7: `placement` may also be a back level, `{"Back": n}`, for items drawn
///   "Behind" (ADR-011). v6 needs no migration; older builds must reject
///   these files.
/// - 8: the layer stack may have `groups`, and layers a `group` (absent
///   means none, so v7 needs no migration; ADR-013). Older builds must reject
///   these files: they would drop group visibility and opacity and change
///   the picture.
pub const FORMAT_VERSION: u32 = 8;

/// File extension for project files (without the dot).
pub const FILE_EXTENSION: &str = "rusting";

/// Errors from saving or loading a project.
#[derive(Debug, thiserror::Error)]
pub enum FileFormatError {
    /// Reading or writing the file failed.
    #[error("could not access project file: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not valid JSON or does not match the expected structure.
    /// The message includes the line and column where parsing stopped.
    #[error("project file is malformed: {0}")]
    Malformed(#[from] serde_json::Error),
    /// The file is JSON but has no `format_version`, so it is not a project file.
    #[error("not a Rusting project file: the \"format_version\" field is missing")]
    NotAProject,
    /// The file was written by a newer version of the application.
    #[error(
        "project file version {found} is newer than this app supports (version {FORMAT_VERSION}); \
         open it with a newer version of Rusting"
    )]
    UnsupportedVersion {
        /// Version found in the file.
        found: u32,
    },
    /// The document parsed but is internally inconsistent.
    #[error("project file is invalid: {0}")]
    Invalid(#[from] ValidationError),
}

/// A consistency problem in a loaded document.
///
/// Frames in messages are shown one-based, as in the timeline.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ValidationError {
    /// A frame tag covers no frames.
    #[error("frame tag \"{0}\" covers no frames")]
    EmptyTag(String),
    /// A keyframe refers to a drawing that does not exist.
    #[error(
        "layer \"{layer_name}\" ({layer}) has a keyframe at frame {frame} showing drawing \
         {drawing}, which is not in the file"
    )]
    MissingDrawing {
        /// Layer holding the keyframe.
        layer: LayerId,
        /// Name of that layer.
        layer_name: String,
        /// Frame of the keyframe.
        frame: FrameIndex,
        /// The missing drawing.
        drawing: DrawingId,
    },
    /// A track exists for a layer that does not exist.
    #[error("the timeline has a track for layer {0}, but there is no such layer")]
    OrphanTrack(LayerId),
    /// A layer has no track.
    #[error("layer \"{name}\" ({id}) has no timeline track")]
    MissingTrack {
        /// The layer.
        id: LayerId,
        /// Its name.
        name: String,
    },
    /// Two layers share an ID.
    #[error("layer ID {0} is used by more than one layer")]
    DuplicateLayer(LayerId),
    /// A layer is in a group that does not exist.
    #[error("layer \"{name}\" ({id}) is in group {group}, which is not in the file")]
    MissingGroup {
        /// The layer.
        id: LayerId,
        /// Its name.
        name: String,
        /// The missing group.
        group: GroupId,
    },
    /// A group's layers are not next to each other in the stack.
    #[error("the layers of group {0} are not next to each other")]
    SplitGroup(GroupId),
    /// Two groups share an ID.
    #[error("group ID {0} is used by more than one group")]
    DuplicateGroup(GroupId),
    /// Two strokes share an ID.
    #[error("stroke ID {stroke} is used more than once (found again in drawing {drawing})")]
    DuplicateStroke {
        /// The repeated stroke ID.
        stroke: StrokeId,
        /// Drawing holding the second copy.
        drawing: DrawingId,
    },
    /// Two fills share an ID.
    #[error("fill ID {fill} is used more than once (found again in drawing {drawing})")]
    DuplicateFill {
        /// The repeated fill ID.
        fill: FillId,
        /// Drawing holding the second copy.
        drawing: DrawingId,
    },
    /// The frame rate has a zero numerator or denominator.
    #[error("frame rate {numerator}/{denominator} is not valid; both parts must be above zero")]
    InvalidFps {
        /// Numerator in the file.
        numerator: u32,
        /// Denominator in the file.
        denominator: u32,
    },
    /// The animation has no frames.
    #[error("the animation must have at least one frame")]
    ZeroFrameCount,
    /// The canvas has no area.
    #[error("canvas size {width}x{height} is not valid; both sides must be above zero")]
    InvalidCanvas {
        /// Width in the file.
        width: u32,
        /// Height in the file.
        height: u32,
    },
    /// A number that must be finite and inside a range is not.
    #[error("{what} is {value}, expected a value in {expected}")]
    OutOfRange {
        /// Which value, e.g. `layer "Ink" opacity`.
        what: String,
        /// The value found (may be NaN or infinite).
        value: f32,
        /// The allowed range, for the message.
        expected: &'static str,
    },
}

/// Things a load accepted but could not keep. See the crate docs on unknown
/// fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// JSON paths (e.g. `document.layers.layers[0].blend_mode`) of fields this
    /// build ignored. Saving the document drops them.
    pub unknown_fields: Vec<String>,
}

impl LoadReport {
    /// Whether nothing was dropped.
    pub fn is_clean(&self) -> bool {
        self.unknown_fields.is_empty()
    }
}

#[derive(Serialize)]
struct EnvelopeOut<'a> {
    format_version: u32,
    document: &'a Document,
}

/// Only the version, read first so we know how to parse the rest.
#[derive(Deserialize)]
struct VersionProbe {
    format_version: Option<u32>,
}

#[derive(Deserialize)]
struct EnvelopeCurrent {
    document: Document,
}

#[derive(Deserialize)]
struct EnvelopeOld {
    document: serde_json::Value,
}

/// Serializes a document to a JSON string. Drawings no keyframe refers to,
/// and groups with no layers, are left out; the document passed in is not
/// changed.
pub fn to_string(doc: &Document) -> Result<String, FileFormatError> {
    let doc: Cow<'_, Document> = if doc.has_unreferenced_drawings() || doc.has_empty_groups() {
        let mut copy = doc.clone();
        copy.garbage_collect_drawings();
        copy.remove_empty_groups();
        Cow::Owned(copy)
    } else {
        Cow::Borrowed(doc)
    };
    Ok(serde_json::to_string_pretty(&EnvelopeOut {
        format_version: FORMAT_VERSION,
        document: &doc,
    })?)
}

/// Parses, migrates and validates a document from a JSON string. Unknown
/// fields are ignored; use [`from_str_with_report`] to find out about them.
pub fn from_str(text: &str) -> Result<Document, FileFormatError> {
    let version = read_version(text)?;
    let doc = if version == FORMAT_VERSION {
        // Parsing straight from text keeps line/column info in error messages.
        serde_json::from_str::<EnvelopeCurrent>(text)?.document
    } else {
        let envelope: EnvelopeOld = serde_json::from_str(text)?;
        serde_json::from_value(migrate(version, envelope.document))?
    };
    validate(&doc)?;
    Ok(doc)
}

/// Like [`from_str`], but also reports fields that were ignored.
pub fn from_str_with_report(text: &str) -> Result<(Document, LoadReport), FileFormatError> {
    let doc = from_str(text)?;
    let mut input: serde_json::Value = serde_json::from_str(text)?;
    if let Some(obj) = input.as_object_mut() {
        obj.remove("format_version");
    }
    let known = serde_json::to_value(EnvelopeOut {
        format_version: FORMAT_VERSION,
        document: &doc,
    })?;
    let mut report = LoadReport::default();
    unknown_fields(&input, &known, String::new(), &mut report.unknown_fields);
    Ok((doc, report))
}

fn read_version(text: &str) -> Result<u32, FileFormatError> {
    let probe: VersionProbe = serde_json::from_str(text)?;
    let version = probe.format_version.ok_or(FileFormatError::NotAProject)?;
    if version > FORMAT_VERSION {
        return Err(FileFormatError::UnsupportedVersion { found: version });
    }
    Ok(version)
}

/// Collects paths of object keys present in `input` but not in `known`.
fn unknown_fields(
    input: &serde_json::Value,
    known: &serde_json::Value,
    path: String,
    out: &mut Vec<String>,
) {
    use serde_json::Value;
    let join = |key: &str| {
        if path.is_empty() {
            key.to_owned()
        } else {
            format!("{path}.{key}")
        }
    };
    match (input, known) {
        (Value::Object(input), Value::Object(known)) => {
            for (key, value) in input {
                match known.get(key) {
                    Some(k) => unknown_fields(value, k, join(key), out),
                    None => out.push(join(key)),
                }
            }
        }
        (Value::Array(input), Value::Array(known)) => {
            for (i, (value, k)) in input.iter().zip(known).enumerate() {
                unknown_fields(value, k, format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

/// Saves a document to `path` atomically: the data goes to a temporary file
/// next to `path`, is flushed to disk, and then renamed over `path`. If any
/// step fails the old file is untouched and the temporary file is removed.
pub fn save(doc: &Document, path: &Path) -> Result<(), FileFormatError> {
    let text = to_string(doc)?;
    let tmp = temp_path(path);
    let result = write_and_sync(&tmp, text.as_bytes()).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(result?)
}

fn write_and_sync(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// `dir/.name.tmp-<pid>`: same directory (so rename stays on one file
/// system), hidden, and unique per process.
fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map_or_else(|| "project".into(), |n| n.to_string_lossy());
    path.with_file_name(format!(".{name}.tmp-{}", std::process::id()))
}

/// Loads a document from `path`.
pub fn load(path: &Path) -> Result<Document, FileFormatError> {
    from_str(&std::fs::read_to_string(path)?)
}

/// Loads a document from `path` and reports fields that were ignored.
pub fn load_with_report(path: &Path) -> Result<(Document, LoadReport), FileFormatError> {
    from_str_with_report(&std::fs::read_to_string(path)?)
}

/// Upgrades raw document JSON from `version` to [`FORMAT_VERSION`], one
/// version step at a time.
fn migrate(version: u32, mut document: serde_json::Value) -> serde_json::Value {
    if version < 2 {
        // v1 → v2: vector drawings gain an empty `fills` list.
        if let Some(drawings) = document
            .get_mut("drawings")
            .and_then(serde_json::Value::as_object_mut)
        {
            for content in drawings.values_mut() {
                if let Some(vector) = content
                    .get_mut("Vector")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    vector
                        .entry("fills")
                        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
                }
            }
        }
    }
    document
}

fn check_range(
    what: impl FnOnce() -> String,
    value: f32,
    ok: bool,
    expected: &'static str,
) -> Result<(), ValidationError> {
    if ok {
        Ok(())
    } else {
        Err(ValidationError::OutOfRange {
            what: what(),
            value,
            expected,
        })
    }
}

fn unit(v: f32) -> bool {
    (0.0..=1.0).contains(&v)
}

/// Checks that every reference in `doc` resolves and every value is usable.
/// Keyframes past `frame_count` are allowed: shortening the animation keeps
/// them so it can be lengthened again.
pub fn validate(doc: &Document) -> Result<(), ValidationError> {
    if doc.fps.numerator() == 0 || doc.fps.denominator() == 0 {
        return Err(ValidationError::InvalidFps {
            numerator: doc.fps.numerator(),
            denominator: doc.fps.denominator(),
        });
    }
    if doc.frame_count == 0 {
        return Err(ValidationError::ZeroFrameCount);
    }
    if doc.canvas.width == 0 || doc.canvas.height == 0 {
        return Err(ValidationError::InvalidCanvas {
            width: doc.canvas.width,
            height: doc.canvas.height,
        });
    }
    if let Some(bg) = doc.background {
        for (channel, v) in [
            ("red", bg.r),
            ("green", bg.g),
            ("blue", bg.b),
            ("alpha", bg.a),
        ] {
            check_range(|| format!("background {channel}"), v, unit(v), "0..=1")?;
        }
    }

    if let Some(tag) = doc.tags.iter().find(|t| t.range.is_empty()) {
        return Err(ValidationError::EmptyTag(tag.name.clone()));
    }

    let mut seen_layers = BTreeSet::new();
    for layer in doc.layers().iter() {
        if !seen_layers.insert(layer.id) {
            return Err(ValidationError::DuplicateLayer(layer.id));
        }
        if doc.track(layer.id).is_none() {
            return Err(ValidationError::MissingTrack {
                id: layer.id,
                name: layer.name.clone(),
            });
        }
        check_range(
            || format!("layer \"{}\" opacity", layer.name),
            layer.opacity,
            unit(layer.opacity),
            "0..=1",
        )?;
        let t = &layer.transform;
        for (part, v) in [
            ("x position", t.translation.x),
            ("y position", t.translation.y),
            ("rotation", t.rotation),
            ("x scale", t.scale.x),
            ("y scale", t.scale.y),
        ] {
            check_range(
                || format!("layer \"{}\" {part}", layer.name),
                v,
                v.is_finite(),
                "finite numbers",
            )?;
        }
    }

    for group in doc.layers().groups() {
        check_range(
            || format!("group \"{}\" opacity", group.name),
            group.opacity,
            unit(group.opacity),
            "0..=1",
        )?;
    }
    doc.layers().check_groups().map_err(|e| match e {
        document::scene::GroupError::MissingGroup(id, group) => ValidationError::MissingGroup {
            id,
            name: doc
                .layers()
                .get(id)
                .map(|l| l.name.clone())
                .unwrap_or_default(),
            group,
        },
        document::scene::GroupError::Split(g) => ValidationError::SplitGroup(g),
        document::scene::GroupError::Duplicate(g) => ValidationError::DuplicateGroup(g),
    })?;

    for (&layer, track) in doc.tracks() {
        let Some(layer_info) = doc.layers().get(layer) else {
            return Err(ValidationError::OrphanTrack(layer));
        };
        for (frame, key) in track.keys() {
            if let Some(drawing) = key.drawing()
                && doc.drawing(drawing).is_none()
            {
                return Err(ValidationError::MissingDrawing {
                    layer,
                    layer_name: layer_info.name.clone(),
                    frame,
                    drawing,
                });
            }
        }
    }

    let mut seen_strokes = BTreeSet::new();
    let mut seen_fills = BTreeSet::new();
    for (&drawing, content) in doc.drawings() {
        let Some(vector) = content.as_vector() else {
            continue;
        };
        for fill in vector.fills() {
            if !seen_fills.insert(fill.id) {
                return Err(ValidationError::DuplicateFill {
                    fill: fill.id,
                    drawing,
                });
            }
            let what = |field: &str| format!("fill {} {field}", fill.id);
            check_range(
                || what("opacity"),
                fill.opacity,
                unit(fill.opacity),
                "0..=1",
            )?;
            for (channel, v) in [
                ("red", fill.color.r),
                ("green", fill.color.g),
                ("blue", fill.color.b),
                ("alpha", fill.color.a),
            ] {
                check_range(|| what(channel), v, unit(v), "0..=1")?;
            }
            for (c, contour) in fill.contours.iter().enumerate() {
                for (i, p) in contour.iter().enumerate() {
                    check_range(
                        || what(&format!("contour {c} point {i}")),
                        if p.x.is_finite() { p.y } else { p.x },
                        p.is_finite(),
                        "finite numbers",
                    )?;
                }
            }
        }
        for stroke in vector.strokes() {
            if !seen_strokes.insert(stroke.id) {
                return Err(ValidationError::DuplicateStroke {
                    stroke: stroke.id,
                    drawing,
                });
            }
            let what = |field: &str| format!("stroke {} {field}", stroke.id);
            let st = &stroke.style;
            check_range(
                || what("width"),
                st.width,
                st.width.is_finite() && st.width >= 0.0,
                "0 or more",
            )?;
            check_range(|| what("opacity"), st.opacity, unit(st.opacity), "0..=1")?;
            for (channel, v) in [
                ("red", st.color.r),
                ("green", st.color.g),
                ("blue", st.color.b),
                ("alpha", st.color.a),
            ] {
                check_range(|| what(channel), v, unit(v), "0..=1")?;
            }
            for (i, p) in stroke.points.iter().enumerate() {
                check_range(
                    || what(&format!("point {i} position")),
                    if p.pos.x.is_finite() {
                        p.pos.y
                    } else {
                        p.pos.x
                    },
                    p.pos.is_finite(),
                    "finite numbers",
                )?;
                check_range(
                    || what(&format!("point {i} pressure")),
                    p.pressure,
                    unit(p.pressure),
                    "0..=1",
                )?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use animation_core::{DocumentId as DocumentIdForTest, FrameRange, Rgba, Vec2};
    use document::CanvasSize;
    use drawing::{
        BrushStyle, DrawingContent, Fill, FillShape, Placement, Stroke, StrokePoint, VectorDrawing,
    };
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
        d.tags.push(document::FrameTag {
            name: "walk".into(),
            range: FrameRange::new(FrameIndex(0), FrameIndex(4)),
        });
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
        v.push_fill(Fill::new(
            FillShape {
                contours: vec![
                    vec![Vec2::ZERO, Vec2::new(8.0, 0.0), Vec2::splat(8.0)],
                    vec![Vec2::ONE, Vec2::new(2.0, 1.5), Vec2::new(3.0, 1.0)],
                ],
            },
            Rgba::new(0.9, 0.5, 0.1, 1.0),
            0.75,
        ));
        // A front stroke over the normal items (ADR-010).
        v.push_stroke(
            Stroke::new(
                vec![StrokePoint::new(Vec2::new(0.0, 4.0))],
                BrushStyle::default(),
            )
            .with_placement(Placement::Front),
        );
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

    /// `sample` with "Color" in a half-transparent group.
    fn grouped_sample() -> (Document, GroupId) {
        let mut d = sample();
        let mut group = document::scene::LayerGroup::new("Paint");
        group.opacity = 0.5;
        let g = group.id;
        d.add_group(group);
        let color = d.layers().iter().nth(1).unwrap().id;
        d.move_layer_to_group(color, 1, Some(g));
        (d, g)
    }

    #[test]
    fn groups_roundtrip_and_empty_groups_are_not_saved() {
        let (mut doc, g) = grouped_sample();
        let loaded = from_str(&to_string(&doc).unwrap()).unwrap();
        assert_eq!(loaded, doc);
        assert_eq!(loaded.layers().group(g).unwrap().opacity, 0.5);

        doc.add_group(document::scene::LayerGroup::new("Empty"));
        let loaded = from_str(&to_string(&doc).unwrap()).unwrap();
        assert_eq!(loaded.layers().groups().count(), 1);
        assert_eq!(
            doc.layers().groups().count(),
            2,
            "the open document keeps it"
        );
    }

    #[test]
    fn corrupt_groups_are_rejected() {
        let (doc, g) = grouped_sample();
        let text = edit_json(&doc, |v| {
            v["document"]["layers"]["groups"] = serde_json::json!([]);
        });
        assert!(matches!(
            invalid(&text),
            ValidationError::MissingGroup { .. }
        ));

        let mut split = doc.clone();
        split.add_layer("Top");
        let text = edit_json(&split, |v| {
            let layers = &mut v["document"]["layers"]["layers"];
            layers[0]["group"] = serde_json::json!(g);
            layers[1]["group"] = serde_json::Value::Null;
            layers[2]["group"] = serde_json::json!(g);
        });
        assert_eq!(invalid(&text), ValidationError::SplitGroup(g));

        let text = edit_json(&doc, |v| {
            v["document"]["layers"]["groups"][0]["opacity"] = 2.0.into();
        });
        assert!(matches!(invalid(&text), ValidationError::OutOfRange { .. }));
    }

    #[test]
    fn roundtrip_preserves_document() {
        let doc = sample();
        let loaded = from_str(&to_string(&doc).unwrap()).unwrap();
        assert_eq!(loaded, doc);
    }

    #[test]
    fn fills_roundtrip() {
        let doc = sample();
        let text = to_string(&doc).unwrap();
        assert!(text.contains("\"fills\""));
        let loaded = from_str(&text).unwrap();
        let fills = |d: &Document| {
            d.drawings()
                .values()
                .filter_map(|c| c.as_vector())
                .flat_map(|v| v.fills().to_vec())
                .collect::<Vec<_>>()
        };
        assert_eq!(fills(&loaded).len(), 1);
        assert_eq!(fills(&loaded), fills(&doc));
    }

    /// A version 1 file as written by the first release (no `fills`).
    const V1_FILE: &str = r#"{
  "format_version": 1,
  "document": {
    "id": "5f0c2a8e-8d7e-4a51-9c0c-2b7f3d1e9a10",
    "name": "Old",
    "canvas": { "width": 64, "height": 32 },
    "background": { "r": 1.0, "g": 1.0, "b": 1.0, "a": 1.0 },
    "fps": { "numerator": 24, "denominator": 1 },
    "frame_count": 12,
    "layers": { "layers": [ {
      "id": "0d9d6f3c-3b1e-4a0e-8f43-7c1c5a5e2b11",
      "name": "Ink", "visible": true, "locked": false, "opacity": 1.0,
      "transform": { "translation": [0.0, 0.0], "rotation": 0.0, "scale": [1.0, 1.0] }
    } ] },
    "tracks": { "0d9d6f3c-3b1e-4a0e-8f43-7c1c5a5e2b11": { "keys": {
      "0": { "Drawing": "9a4b7f9e-1c2d-4e5f-8a9b-0c1d2e3f4a5b" }
    } } },
    "drawings": { "9a4b7f9e-1c2d-4e5f-8a9b-0c1d2e3f4a5b": { "Vector": { "strokes": [ {
      "id": "1b2c3d4e-5f60-4718-9a2b-3c4d5e6f7081",
      "points": [ { "pos": [1.0, 2.0], "pressure": 1.0 }, { "pos": [5.0, 6.0], "pressure": 0.5 } ],
      "style": { "color": { "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 }, "width": 4.0, "opacity": 1.0 }
    } ] } } }
  }
}"#;

    #[test]
    fn version_1_file_loads_without_fills() {
        let (doc, report) = from_str_with_report(V1_FILE).unwrap();
        assert!(report.is_clean(), "{report:?}");
        assert_eq!(doc.name, "Old");
        let vector = doc.drawings().values().next().unwrap().as_vector().unwrap();
        assert_eq!(vector.strokes().len(), 1);
        assert!(vector.fills().is_empty());
        assert!(doc.tags.is_empty());
        // Saving writes the current version, which loads back the same.
        let text = to_string(&doc).unwrap();
        assert!(text.contains(&format!("\"format_version\": {FORMAT_VERSION}")));
        assert_eq!(from_str(&text).unwrap(), doc);
    }

    #[test]
    fn back_levels_roundtrip_and_v6_files_load_unchanged() {
        let doc = sample();
        let mut with_back = doc.clone();
        {
            let id = *with_back.drawings().keys().next().unwrap();
            let v = with_back.drawing_mut(id).unwrap().as_vector_mut().unwrap();
            let fill = v.fills()[0].id;
            v.fill_mut(fill).unwrap().placement = Placement::Back(2);
        }
        let text = to_string(&with_back).unwrap();
        assert!(text.contains("\"Back\": 2"), "{text}");
        let reloaded = from_str(&text).unwrap();
        assert_eq!(reloaded, with_back);
        let v = reloaded.drawings().values().next().unwrap();
        assert_eq!(
            v.as_vector().unwrap().fills()[0].placement,
            Placement::Back(2)
        );

        // A v6 file (front tier, no back levels) loads as it was saved.
        let v6 = edit_json(&doc, |json| json["format_version"] = 6.into());
        let (old, report) = from_str_with_report(&v6).unwrap();
        assert!(report.is_clean(), "{report:?}");
        assert_eq!(old, doc);

        // A malformed back level is rejected, not a panic.
        let bad = text.replace("\"Back\": 2", "\"Back\": -1");
        assert_ne!(bad, text);
        assert!(matches!(from_str(&bad), Err(FileFormatError::Malformed(_))));
    }

    #[test]
    fn placement_roundtrips_and_older_files_load_as_normal() {
        let doc = sample();
        let text = to_string(&doc).unwrap();
        // Only front items write the field.
        assert_eq!(text.matches("\"placement\"").count(), 1);
        let front_fill = |d: &mut Document| {
            let id = *d.drawings().keys().next().unwrap();
            let v = d.drawing_mut(id).unwrap().as_vector_mut().unwrap();
            let fill = v.fills()[0].id;
            v.fill_mut(fill).unwrap().placement = Placement::Front;
        };
        let mut with_front_fill = doc.clone();
        front_fill(&mut with_front_fill);
        let reloaded = from_str(&to_string(&with_front_fill).unwrap()).unwrap();
        assert_eq!(reloaded, with_front_fill);
        let v = reloaded.drawings().values().next().unwrap();
        let v = v.as_vector().unwrap();
        assert_eq!(v.fills()[0].placement, Placement::Front);
        assert_eq!(
            v.strokes().iter().map(|s| s.placement).collect::<Vec<_>>(),
            [Placement::Normal, Placement::Front]
        );

        // A v5 file has no placement: everything loads in the normal tier.
        let v5 = edit_json(&doc, |json| {
            json["format_version"] = 5.into();
            let drawings = json["document"]["drawings"].as_object_mut().unwrap();
            for content in drawings.values_mut() {
                for stroke in content["Vector"]["strokes"].as_array_mut().unwrap() {
                    stroke.as_object_mut().unwrap().remove("placement");
                }
            }
        });
        let (old, report) = from_str_with_report(&v5).unwrap();
        assert!(report.is_clean(), "{report:?}");
        let v = old.drawings().values().next().unwrap().as_vector().unwrap();
        assert!(v.strokes().iter().all(|s| s.placement == Placement::Normal));
        assert!(v.fills().iter().all(|f| f.placement == Placement::Normal));

        // An unknown tier is a malformed file, not a panic.
        let bad = text.replace("\"placement\": \"Front\"", "\"placement\": \"Sideways\"");
        assert_ne!(bad, text);
        assert!(matches!(from_str(&bad), Err(FileFormatError::Malformed(_))));
    }

    #[test]
    fn duplicate_or_corrupt_fills_are_rejected() {
        let doc = sample();
        let dup = edit_json(&doc, |v| {
            let drawing = v["document"]["drawings"]
                .as_object_mut()
                .unwrap()
                .values_mut()
                .next()
                .unwrap();
            let fills = drawing["Vector"]["fills"].as_array_mut().unwrap();
            let copy = fills[0].clone();
            fills.push(copy);
        });
        assert!(matches!(
            invalid(&dup),
            ValidationError::DuplicateFill { .. }
        ));
        let bad = edit_json(&doc, |v| {
            let drawing = v["document"]["drawings"]
                .as_object_mut()
                .unwrap()
                .values_mut()
                .next()
                .unwrap();
            drawing["Vector"]["fills"][0]["opacity"] = 3.0.into();
        });
        assert!(matches!(invalid(&bad), ValidationError::OutOfRange { .. }));
    }

    #[test]
    fn empty_tags_are_rejected() {
        let text = edit_json(&sample(), |v| {
            v["document"]["tags"][0]["range"]["end"] = 0.into();
        });
        assert!(matches!(invalid(&text), ValidationError::EmptyTag(name) if name == "walk"));
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
            &format!("\"format_version\": {FORMAT_VERSION}"),
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

    fn edit_json(doc: &Document, f: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut v: serde_json::Value = serde_json::from_str(&to_string(doc).unwrap()).unwrap();
        f(&mut v);
        v.to_string()
    }

    fn invalid(text: &str) -> ValidationError {
        match from_str(text) {
            Err(FileFormatError::Invalid(e)) => e,
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    #[test]
    fn newer_version_message_says_what_to_do() {
        let newer = FORMAT_VERSION + 1;
        let text = edit_json(&sample(), |v| v["format_version"] = newer.into());
        let err = from_str(&text).unwrap_err();
        assert!(matches!(
            err,
            FileFormatError::UnsupportedVersion { found } if found == newer
        ));
        let msg = err.to_string();
        assert!(
            msg.contains(&format!("version {newer}")) && msg.contains("newer"),
            "{msg}"
        );
    }

    #[test]
    fn json_without_version_is_not_a_project() {
        assert!(matches!(
            from_str("{\"document\":{}}"),
            Err(FileFormatError::NotAProject)
        ));
        assert!(matches!(from_str("{}"), Err(FileFormatError::NotAProject)));
        assert!(matches!(from_str(""), Err(FileFormatError::Malformed(_))));
        assert!(matches!(
            from_str("[1]"),
            Err(FileFormatError::Malformed(_))
        ));
        assert!(matches!(
            from_str("{\"format_version\":\"one\"}"),
            Err(FileFormatError::Malformed(_))
        ));
    }

    #[test]
    fn malformed_messages_point_at_the_line() {
        let text = to_string(&sample()).unwrap().replacen(
            "\"pressure\": 0.5",
            "\"pressure\": \"high\"",
            1,
        );
        let msg = from_str(&text).unwrap_err().to_string();
        assert!(msg.contains("line"), "{msg}");
    }

    #[test]
    fn every_truncation_is_an_error_not_a_panic() {
        let text = to_string(&sample()).unwrap();
        for end in (0..text.len()).filter(|&i| text.is_char_boundary(i)) {
            assert!(
                from_str(&text[..end]).is_err(),
                "prefix of {end} bytes loaded"
            );
        }
    }

    #[test]
    fn invalid_utf8_file_is_an_io_error() {
        let path =
            std::env::temp_dir().join(format!("rusting-bad-{}.rusting", DocumentIdForTest::new()));
        std::fs::write(&path, [0xff, 0xfe, 0x00, 0x7b]).unwrap();
        let result = load(&path);
        let _ = std::fs::remove_file(&path);
        assert!(matches!(result, Err(FileFormatError::Io(_))));
    }

    #[test]
    fn missing_drawing_message_names_layer_and_display_frame() {
        let mut doc = sample();
        let layer = doc.layers().iter().next().unwrap().id;
        doc.track_mut(layer)
            .unwrap()
            .set_key(FrameIndex(3), Keyframe::Drawing(DrawingId::new()));
        let msg = validate(&doc).unwrap_err().to_string();
        assert!(msg.contains("\"Ink\"") && msg.contains("frame 4"), "{msg}");
    }

    type JsonEdit = Box<dyn FnOnce(&mut serde_json::Value)>;

    #[test]
    fn corrupt_values_are_reported() {
        let doc = sample();
        let cases: Vec<(&str, JsonEdit)> = vec![
            (
                "fps",
                Box::new(|v| v["document"]["fps"]["denominator"] = 0.into()),
            ),
            (
                "frames",
                Box::new(|v| v["document"]["frame_count"] = 0.into()),
            ),
            (
                "canvas",
                Box::new(|v| v["document"]["canvas"]["width"] = 0.into()),
            ),
            (
                "opacity",
                Box::new(|v| v["document"]["layers"]["layers"][0]["opacity"] = 5.0.into()),
            ),
            (
                "orphan track",
                Box::new(|v| {
                    let layers = v["document"]["layers"]["layers"].as_array_mut().unwrap();
                    layers.remove(1);
                }),
            ),
            (
                "missing track",
                Box::new(|v| {
                    let id = v["document"]["layers"]["layers"][1]["id"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                    v["document"]["tracks"].as_object_mut().unwrap().remove(&id);
                }),
            ),
            (
                "duplicate layer",
                Box::new(|v| {
                    let layers = v["document"]["layers"]["layers"].as_array_mut().unwrap();
                    let copy = layers[0].clone();
                    layers.push(copy);
                }),
            ),
            (
                "pressure",
                Box::new(|v| {
                    let drawings = v["document"]["drawings"].as_object_mut().unwrap();
                    let d = drawings.values_mut().next().unwrap();
                    d["Vector"]["strokes"][0]["points"][0]["pressure"] = 2.0.into();
                }),
            ),
            (
                "duplicate stroke",
                Box::new(|v| {
                    let drawings = v["document"]["drawings"].as_object_mut().unwrap();
                    let d = drawings.values_mut().next().unwrap();
                    let strokes = d["Vector"]["strokes"].as_array_mut().unwrap();
                    let copy = strokes[0].clone();
                    strokes.push(copy);
                }),
            ),
        ];
        for (name, edit) in cases {
            let err = invalid(&edit_json(&doc, edit));
            let expected = match name {
                "fps" => matches!(err, ValidationError::InvalidFps { .. }),
                "frames" => err == ValidationError::ZeroFrameCount,
                "canvas" => matches!(err, ValidationError::InvalidCanvas { .. }),
                "opacity" | "pressure" => matches!(err, ValidationError::OutOfRange { .. }),
                "orphan track" => matches!(err, ValidationError::OrphanTrack(_)),
                "missing track" => matches!(err, ValidationError::MissingTrack { .. }),
                "duplicate layer" => matches!(err, ValidationError::DuplicateLayer(_)),
                "duplicate stroke" => matches!(err, ValidationError::DuplicateStroke { .. }),
                _ => false,
            };
            assert!(expected, "{name}: got {err:?} ({err})");
        }
    }

    #[test]
    fn unknown_fields_are_ignored_but_reported() {
        let doc = sample();
        let text = edit_json(&doc, |v| {
            v["extra"] = true.into();
            v["document"]["layers"]["layers"][0]["blend_mode"] = "multiply".into();
        });
        assert_eq!(from_str(&text).unwrap(), doc);
        let (loaded, report) = from_str_with_report(&text).unwrap();
        assert_eq!(loaded, doc);
        assert_eq!(
            report.unknown_fields,
            ["document.layers.layers[0].blend_mode", "extra"]
        );
        let (_, clean) = from_str_with_report(&to_string(&doc).unwrap()).unwrap();
        assert!(clean.is_clean(), "{clean:?}");
    }

    #[test]
    fn saving_drops_unreferenced_drawings_without_touching_the_document() {
        let mut doc = sample();
        let orphan = doc.add_drawing(DrawingContent::default());
        let loaded = from_str(&to_string(&doc).unwrap()).unwrap();
        assert!(doc.drawing(orphan).is_some());
        assert!(loaded.drawing(orphan).is_none());
        doc.garbage_collect_drawings();
        assert_eq!(loaded, doc);
    }

    #[test]
    fn atomic_save_replaces_file_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join(format!("rusting-save-{}", DocumentIdForTest::new()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("project.rusting");
        std::fs::write(&path, "old contents").unwrap();
        let doc = sample();
        save(&doc, &path).unwrap();
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        let loaded = load(&path);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(entries, ["project.rusting"]);
        assert_eq!(loaded.unwrap(), doc);
    }

    #[test]
    fn failed_save_leaves_old_file_alone() {
        let dir = std::env::temp_dir().join(format!("rusting-fail-{}", DocumentIdForTest::new()));
        std::fs::create_dir(&dir).unwrap();
        // The target is a non-empty directory, so the final rename fails.
        let path = dir.join("busy.rusting");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("keep"), "x").unwrap();
        let result = save(&sample(), &path);
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        entries.sort();
        let kept = std::fs::read_to_string(path.join("keep"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(result, Err(FileFormatError::Io(_))));
        assert_eq!(entries, ["busy.rusting"]);
        assert_eq!(kept.unwrap(), "x");
        assert!(matches!(
            save(&sample(), Path::new("/definitely/not/a/dir/x.rusting")),
            Err(FileFormatError::Io(_))
        ));
    }
}
