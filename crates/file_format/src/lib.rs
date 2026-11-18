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
//! # Library assets
//! Assets in the document's library are saved once (ADR-017). An image item
//! placed from an asset is saved without its bytes and gets them back from
//! the asset on load. Assets nothing uses are saved too: the library owns
//! them.
//!
//! # Objects
//! Symbols (objects, ADR-018) are saved in the document's `symbols` map,
//! with their own layers and tracks. Their drawings are saved in the shared
//! drawing pool and are never dropped as unreferenced while the symbol is in
//! the library.
//!
//! # Audio
//! A library sound (ADR-020) is saved once, with its file bytes as base64
//! and its peak table. An audio layer saves its `kind` (the clip) and an
//! empty track.
//!
//! # Unknown fields
//! Within a supported version, fields this build does not know are **ignored**
//! by [`from_str`] and [`load`]; the document still loads. A writer that adds
//! data older builds cannot keep must bump [`FORMAT_VERSION`], which older
//! builds reject with [`FileFormatError::UnsupportedVersion`]. Unknown fields
//! can therefore only come from hand edits or other tools. To avoid dropping
//! them silently, [`from_str_with_report`] and [`load_with_report`] list them
//! in a [`LoadReport`] so the app can warn before overwriting the file.

use animation_core::{
    AssetId, DrawingId, FillId, FrameIndex, GroupId, ImageId, LayerId, StrokeId, SymbolId,
};
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
/// - 9: vector drawings may have `images`: imported PNG/JPEG pictures with
///   their file bytes as base64 (absent means none, so v8 needs no
///   migration; ADR-015). Older builds must reject these files: they would
///   drop the pictures.
/// - 10: the document may have a library of `assets` (absent means none), and
///   images an `asset` reference. An image with an asset is saved without
///   its `data`, which is filled back in from the asset on load (ADR-017).
///   v9 files need no migration. Older builds must reject these files: they
///   would load the pictures as empty.
/// - 11: the document may have `symbols` (objects: layers, tracks and a
///   length; absent means none), and keys may be `Instance` keys placing a
///   symbol (ADR-018). v10 files need no migration. Older builds must reject
///   these files: they cannot read instance keys.
/// - 12: instance keys may have a `tween` (`"Linear"`, `"EaseIn"`,
///   `"EaseOut"` or `"EaseInOut"`; absent means none), a motion tween to the
///   next key (ADR-019). v11 files need no migration. Older builds must
///   reject these files: they would drop the tweens and change the motion.
/// - 13: the library may hold `Audio` assets (MP3/WAV bytes as base64, plus
///   format, sample rate, channels, duration and peaks), and layers may have
///   a `kind` of `{"Audio": clip}` (absent means a drawing layer; ADR-020).
///   v12 files need no migration. Older builds must reject these files: they
///   would fail on the unknown asset kind, or load audio layers as drawing
///   layers.
/// - 14: the library may hold `Brush` assets (a tip and optional grain
///   picture as base64, plus stamp settings), and stroke styles may have a
///   `brush` asset reference (absent means a solid stroke; ADR-022). v13
///   files need no migration. Older builds must reject these files: they
///   would fail on the unknown asset kind, or draw brushed strokes solid.
pub const FORMAT_VERSION: u32 = 14;

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
    /// Two images share an ID.
    #[error("image ID {image} is used more than once (found again in drawing {drawing})")]
    DuplicateImage {
        /// The repeated image ID.
        image: ImageId,
        /// Drawing holding the second copy.
        drawing: DrawingId,
    },
    /// An image has no pixels or no file bytes.
    #[error("image {0} is empty")]
    EmptyImage(ImageId),
    /// An image refers to a library asset that is not in the file.
    #[error("image {image} was placed from library asset {asset}, which is not in the file")]
    MissingAsset {
        /// The image.
        image: ImageId,
        /// The missing asset.
        asset: AssetId,
    },
    /// An image refers to a library asset that is not a picture.
    #[error("image {image} was placed from library asset {asset}, which is not a picture")]
    NotAnImageAsset {
        /// The image.
        image: ImageId,
        /// The asset.
        asset: AssetId,
    },
    /// Two library assets share an ID.
    #[error("asset ID {0} is used by more than one library asset")]
    DuplicateAsset(AssetId),
    /// A library asset is stored under another asset's ID.
    #[error("library asset {id} is stored under the ID {key}")]
    AssetKeyMismatch {
        /// The key in the library.
        key: AssetId,
        /// The asset's own ID.
        id: AssetId,
    },
    /// An audio layer plays a library asset that is not in the file.
    #[error(
        "audio layer \"{name}\" ({layer}) plays library asset {asset}, which is not in the file"
    )]
    MissingAudioAsset {
        /// The layer.
        layer: LayerId,
        /// Its name.
        name: String,
        /// The missing asset.
        asset: AssetId,
    },
    /// An audio layer plays a library asset that is not a sound.
    #[error("audio layer \"{name}\" ({layer}) plays library asset {asset}, which is not a sound")]
    NotAnAudioAsset {
        /// The layer.
        layer: LayerId,
        /// Its name.
        name: String,
        /// The asset.
        asset: AssetId,
    },
    /// An audio layer has keyframes; its track must be empty.
    #[error("audio layer \"{name}\" ({layer}) has keyframes")]
    AudioLayerHasKeys {
        /// The layer.
        layer: LayerId,
        /// Its name.
        name: String,
    },
    /// An object (symbol) holds an audio layer. Objects only hold drawing
    /// layers; sound stays on the document's own audio layers.
    #[error("object \"{symbol_name}\" ({symbol}) holds audio layer \"{name}\" ({layer})")]
    AudioLayerInObject {
        /// The object.
        symbol: SymbolId,
        /// Its name.
        symbol_name: String,
        /// The audio layer.
        layer: LayerId,
        /// Its name.
        name: String,
    },
    /// A stroke is drawn with a library brush that is not in the file.
    #[error("stroke {stroke} uses library brush {asset}, which is not in the file")]
    MissingBrushAsset {
        /// The stroke.
        stroke: StrokeId,
        /// The missing asset.
        asset: AssetId,
    },
    /// A stroke is drawn with a library asset that is not a brush.
    #[error("stroke {stroke} uses library asset {asset} as a brush, which is not a brush")]
    NotABrushAsset {
        /// The stroke.
        stroke: StrokeId,
        /// The asset.
        asset: AssetId,
    },
    /// A library brush has a setting out of range.
    #[error("library brush \"{name}\" ({id}): {source}")]
    BadBrushSettings {
        /// The asset.
        id: AssetId,
        /// Its name.
        name: String,
        /// What is wrong.
        source: document::drawing::BrushSettingsError,
    },
    /// A library picture, sound or brush has no content.
    #[error("library asset \"{name}\" ({id}) is empty")]
    EmptyAsset {
        /// The asset.
        id: AssetId,
        /// Its name.
        name: String,
    },
    /// A key places an object (symbol) that is not in the file.
    #[error(
        "layer \"{layer_name}\" ({layer}) places object {symbol} at frame {frame}, which is not \
         in the file"
    )]
    MissingSymbol {
        /// Layer holding the key.
        layer: LayerId,
        /// Name of that layer.
        layer_name: String,
        /// Frame of the key.
        frame: FrameIndex,
        /// The missing symbol.
        symbol: SymbolId,
    },
    /// An object key starts past the end of its object.
    #[error(
        "layer \"{layer_name}\" ({layer}) starts an object at its frame {first_frame}, but the \
         object has only {frame_count} frames"
    )]
    FirstFrameOutOfRange {
        /// Layer holding the key.
        layer: LayerId,
        /// Name of that layer.
        layer_name: String,
        /// The key's first frame.
        first_frame: FrameIndex,
        /// The object's length.
        frame_count: u32,
    },
    /// An object key has a motion tween that does not move (`Step`).
    #[error(
        "layer \"{layer_name}\" ({layer}) has an object at frame {frame} with a \"Step\" tween; \
         a tween must be Linear, EaseIn, EaseOut or EaseInOut"
    )]
    StepTween {
        /// Layer holding the key.
        layer: LayerId,
        /// Name of that layer.
        layer_name: String,
        /// The key's frame.
        frame: FrameIndex,
    },
    /// Objects place each other in a loop.
    #[error("object \"{name}\" ({id}) contains itself")]
    SymbolCycle {
        /// An object on the loop.
        id: SymbolId,
        /// Its name.
        name: String,
    },
    /// An object is stored under another object's ID.
    #[error("object {id} is stored under the ID {key}")]
    SymbolKeyMismatch {
        /// The key in the file.
        key: SymbolId,
        /// The object's own ID.
        id: SymbolId,
    },
    /// An object has no frames.
    #[error("object \"{name}\" ({id}) has no frames")]
    EmptySymbol {
        /// The object.
        id: SymbolId,
        /// Its name.
        name: String,
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
/// and groups with no layers, are left out, and images placed from a library
/// asset are written without their bytes; the document passed in is not
/// changed.
pub fn to_string(doc: &Document) -> Result<String, FileFormatError> {
    let doc: Cow<'_, Document> =
        if doc.has_unreferenced_drawings() || doc.has_empty_groups() || has_asset_images(doc) {
            let mut copy = doc.clone();
            copy.garbage_collect_drawings();
            copy.remove_empty_groups();
            strip_asset_bytes(&mut copy);
            Cow::Owned(copy)
        } else {
            Cow::Borrowed(doc)
        };
    Ok(serde_json::to_string_pretty(&EnvelopeOut {
        format_version: FORMAT_VERSION,
        document: &doc,
    })?)
}

/// Whether some image was placed from a library asset.
fn has_asset_images(doc: &Document) -> bool {
    doc.drawings()
        .values()
        .filter_map(|c| c.as_vector())
        .any(|v| v.images().iter().any(|i| i.asset.is_some()))
}

/// Calls `f` on every image of every drawing in `doc`.
fn for_each_image_mut(doc: &mut Document, mut f: impl FnMut(&mut document::drawing::Image)) {
    let ids: Vec<DrawingId> = doc.drawings().keys().copied().collect();
    for id in ids {
        let Some(vector) = doc.drawing_mut(id).and_then(|c| c.as_vector_mut()) else {
            continue;
        };
        let images: Vec<ImageId> = vector.images().iter().map(|i| i.id).collect();
        for image in images {
            if let Some(image) = vector.image_mut(image) {
                f(image);
            }
        }
    }
}

/// Drops the bytes of images whose library asset holds the same picture; an
/// image whose asset is gone (possible only for a drawing kept for undo) keeps
/// its bytes and loses the reference, so the file always loads.
fn strip_asset_bytes(doc: &mut Document) {
    let pictures: BTreeSet<AssetId> = doc
        .assets()
        .values()
        .filter(|a| a.image().is_some())
        .map(|a| a.id)
        .collect();
    for_each_image_mut(doc, |image| {
        if let Some(asset) = image.asset {
            if pictures.contains(&asset) {
                image.data = document::drawing::ImageData::default();
            } else {
                image.asset = None;
            }
        }
    });
}

/// Gives every image placed from a library picture that picture's bytes.
/// References to missing assets are left for [`validate`] to report.
fn fill_asset_bytes(doc: &mut Document) {
    let pictures: std::collections::BTreeMap<AssetId, document::drawing::ImageData> = doc
        .assets()
        .values()
        .filter_map(|a| Some((a.id, a.image()?.clone())))
        .collect();
    for_each_image_mut(doc, |image| {
        if let Some(data) = image.asset.and_then(|a| pictures.get(&a)) {
            image.data = data.clone();
        }
    });
}

/// Parses, migrates and validates a document from a JSON string. Unknown
/// fields are ignored; use [`from_str_with_report`] to find out about them.
pub fn from_str(text: &str) -> Result<Document, FileFormatError> {
    let version = read_version(text)?;
    let mut doc = if version == FORMAT_VERSION {
        // Parsing straight from text keeps line/column info in error messages.
        serde_json::from_str::<EnvelopeCurrent>(text)?.document
    } else {
        let envelope: EnvelopeOld = serde_json::from_str(text)?;
        serde_json::from_value(migrate(version, envelope.document))?
    };
    fill_asset_bytes(&mut doc);
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

    validate_symbols(doc, &mut seen_layers)?;
    for (&layer, track) in doc.tracks() {
        let Some(layer_info) = doc.layers().get(layer) else {
            return Err(ValidationError::OrphanTrack(layer));
        };
        check_keys(doc, layer_info, track)?;
    }

    for (layer, clip) in doc.audio_layers() {
        let name = || layer.name.clone();
        match doc.asset(clip.asset) {
            None => {
                return Err(ValidationError::MissingAudioAsset {
                    layer: layer.id,
                    name: name(),
                    asset: clip.asset,
                });
            }
            Some(a) if a.audio().is_none() => {
                return Err(ValidationError::NotAnAudioAsset {
                    layer: layer.id,
                    name: name(),
                    asset: clip.asset,
                });
            }
            Some(_) => {}
        }
        if doc.track(layer.id).is_some_and(|t| !t.is_empty()) {
            return Err(ValidationError::AudioLayerHasKeys {
                layer: layer.id,
                name: name(),
            });
        }
        check_range(
            || format!("audio layer \"{}\" volume", layer.name),
            clip.volume,
            unit(clip.volume),
            "0..=1",
        )?;
    }

    let mut seen_assets = BTreeSet::new();
    for asset in doc.assets().values() {
        if !seen_assets.insert(asset.id) {
            return Err(ValidationError::DuplicateAsset(asset.id));
        }
    }
    for (&key, asset) in doc.assets() {
        if key != asset.id {
            return Err(ValidationError::AssetKeyMismatch { key, id: asset.id });
        }
        if asset
            .image()
            .is_some_and(document::drawing::ImageData::is_empty)
            || asset.audio().is_some_and(document::AudioData::is_empty)
            || asset
                .brush()
                .is_some_and(|b| b.tip.is_empty() || b.grain.as_ref().is_some_and(|g| g.is_empty()))
        {
            return Err(ValidationError::EmptyAsset {
                id: asset.id,
                name: asset.name.clone(),
            });
        }
        if let Some(brush) = asset.brush() {
            brush
                .settings
                .validate()
                .map_err(|source| ValidationError::BadBrushSettings {
                    id: asset.id,
                    name: asset.name.clone(),
                    source,
                })?;
        }
    }

    let mut seen_strokes = BTreeSet::new();
    let mut seen_fills = BTreeSet::new();
    let mut seen_images = BTreeSet::new();
    for (&drawing, content) in doc.drawings() {
        let Some(vector) = content.as_vector() else {
            continue;
        };
        for image in vector.images() {
            if !seen_images.insert(image.id) {
                return Err(ValidationError::DuplicateImage {
                    image: image.id,
                    drawing,
                });
            }
            if let Some(asset) = image.asset {
                match doc.asset(asset) {
                    None => {
                        return Err(ValidationError::MissingAsset {
                            image: image.id,
                            asset,
                        });
                    }
                    Some(a) if a.image().is_none() => {
                        return Err(ValidationError::NotAnImageAsset {
                            image: image.id,
                            asset,
                        });
                    }
                    Some(_) => {}
                }
            }
            if image.data.is_empty() {
                return Err(ValidationError::EmptyImage(image.id));
            }
            let what = |field: &str| format!("image {} {field}", image.id);
            check_range(
                || what("opacity"),
                image.opacity,
                unit(image.opacity),
                "0..=1",
            )?;
            let t = image.transform;
            for (i, v) in t.to_cols_array().into_iter().enumerate() {
                check_range(
                    || what(&format!("transform value {i}")),
                    v,
                    v.is_finite(),
                    "finite numbers",
                )?;
            }
        }
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
            if let Some(asset) = stroke.style.brush {
                match doc.asset(asset) {
                    None => {
                        return Err(ValidationError::MissingBrushAsset {
                            stroke: stroke.id,
                            asset,
                        });
                    }
                    Some(a) if a.brush().is_none() => {
                        return Err(ValidationError::NotABrushAsset {
                            stroke: stroke.id,
                            asset,
                        });
                    }
                    Some(_) => {}
                }
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

/// Checks the keys of one track: drawings and objects exist, and object
/// placements are usable.
fn check_keys(
    doc: &Document,
    layer: &document::scene::Layer,
    track: &document::timeline::Track,
) -> Result<(), ValidationError> {
    for (frame, key) in track.keys() {
        if let Some(drawing) = key.drawing()
            && doc.drawing(drawing).is_none()
        {
            return Err(ValidationError::MissingDrawing {
                layer: layer.id,
                layer_name: layer.name.clone(),
                frame,
                drawing,
            });
        }
        let Some(instance) = key.instance() else {
            continue;
        };
        let Some(symbol) = doc.symbol(instance.symbol) else {
            return Err(ValidationError::MissingSymbol {
                layer: layer.id,
                layer_name: layer.name.clone(),
                frame,
                symbol: instance.symbol,
            });
        };
        if instance.first_frame.0 >= symbol.frame_count {
            return Err(ValidationError::FirstFrameOutOfRange {
                layer: layer.id,
                layer_name: layer.name.clone(),
                first_frame: instance.first_frame,
                frame_count: symbol.frame_count,
            });
        }
        if instance.tween == Some(animation_core::Interpolation::Step) {
            return Err(ValidationError::StepTween {
                layer: layer.id,
                layer_name: layer.name.clone(),
                frame,
            });
        }
        let what = |part: &str| {
            format!(
                "layer \"{}\" object at frame {} {part}",
                layer.name,
                frame.display_number()
            )
        };
        check_range(
            || what("opacity"),
            instance.opacity,
            unit(instance.opacity),
            "0..=1",
        )?;
        let t = &instance.transform;
        for (part, v) in [
            ("x position", t.translation.x),
            ("y position", t.translation.y),
            ("rotation", t.rotation),
            ("x scale", t.scale.x),
            ("y scale", t.scale.y),
        ] {
            check_range(|| what(part), v, v.is_finite(), "finite numbers")?;
        }
    }
    Ok(())
}

/// Checks every symbol: its key, length, layers and tracks, the keys on
/// those tracks, and that no symbol contains itself. Layer IDs must be
/// unique across the document and all symbols.
fn validate_symbols(
    doc: &Document,
    seen_layers: &mut BTreeSet<LayerId>,
) -> Result<(), ValidationError> {
    for (&key, symbol) in doc.symbols() {
        if key != symbol.id {
            return Err(ValidationError::SymbolKeyMismatch { key, id: symbol.id });
        }
        if symbol.frame_count == 0 {
            return Err(ValidationError::EmptySymbol {
                id: symbol.id,
                name: symbol.name.clone(),
            });
        }
    }
    for symbol in doc.symbols().values() {
        for layer in symbol.layers.iter() {
            if !seen_layers.insert(layer.id) {
                return Err(ValidationError::DuplicateLayer(layer.id));
            }
            if layer.is_audio() {
                return Err(ValidationError::AudioLayerInObject {
                    symbol: symbol.id,
                    symbol_name: symbol.name.clone(),
                    layer: layer.id,
                    name: layer.name.clone(),
                });
            }
            if !symbol.tracks.contains_key(&layer.id) {
                return Err(ValidationError::MissingTrack {
                    id: layer.id,
                    name: layer.name.clone(),
                });
            }
            check_range(
                || {
                    format!(
                        "object \"{}\" layer \"{}\" opacity",
                        symbol.name, layer.name
                    )
                },
                layer.opacity,
                unit(layer.opacity),
                "0..=1",
            )?;
        }
        for (&layer, track) in &symbol.tracks {
            let Some(info) = symbol.layers.get(layer) else {
                return Err(ValidationError::OrphanTrack(layer));
            };
            check_keys(doc, info, track)?;
        }
    }

    // Depth-first search for a symbol that reaches itself.
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Visiting,
        Done,
    }
    fn visit(
        doc: &Document,
        id: SymbolId,
        marks: &mut std::collections::BTreeMap<SymbolId, Mark>,
    ) -> Result<(), SymbolId> {
        match marks.get(&id) {
            Some(Mark::Done) => return Ok(()),
            Some(Mark::Visiting) => return Err(id),
            None => {}
        }
        marks.insert(id, Mark::Visiting);
        if let Some(symbol) = doc.symbol(id) {
            for nested in symbol.nested_symbols() {
                visit(doc, nested, marks)?;
            }
        }
        marks.insert(id, Mark::Done);
        Ok(())
    }
    let mut marks = std::collections::BTreeMap::new();
    for &id in doc.symbols().keys() {
        visit(doc, id, &mut marks).map_err(|id| ValidationError::SymbolCycle {
            id,
            name: doc.symbol(id).map(|s| s.name.clone()).unwrap_or_default(),
        })?;
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
                brush: None,
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

    #[test]
    fn images_roundtrip_and_bad_images_are_rejected() {
        use document::drawing::{Image, ImageData};
        let mut doc = sample();
        let drawing = *doc.drawings().keys().next().unwrap();
        let image = Image::new(
            ImageData::new(3, 2, vec![0x89, b'P', b'N', b'G', 0, 255]),
            animation_core::Vec2::new(5.0, 6.0),
            animation_core::Vec2::new(30.0, 20.0),
        );
        let id = image.id;
        doc.drawing_mut(drawing)
            .and_then(|c| c.as_vector_mut())
            .unwrap()
            .push_image(image);
        let text = to_string(&doc).unwrap();
        assert!(text.contains("\"images\""));
        let loaded = from_str(&text).unwrap();
        let images = |d: &Document| {
            d.drawings()
                .values()
                .filter_map(|c| c.as_vector())
                .flat_map(|v| v.images().to_vec())
                .collect::<Vec<_>>()
        };
        assert_eq!(images(&loaded), images(&doc));

        let path = |j: &mut serde_json::Value| -> serde_json::Value {
            j["document"]["drawings"][drawing.to_string()]["Vector"]["images"][0].take()
        };
        let empty = edit_json(&doc, |j| {
            let mut img = path(j);
            img["data"]["width"] = 0.into();
            j["document"]["drawings"][drawing.to_string()]["Vector"]["images"][0] = img;
        });
        assert_eq!(invalid(&empty), ValidationError::EmptyImage(id));
        let bad = edit_json(&doc, |j| {
            let mut img = path(j);
            img["data"]["bytes"] = "not base64!".into();
            j["document"]["drawings"][drawing.to_string()]["Vector"]["images"][0] = img;
        });
        assert!(from_str(&bad).is_err());
    }

    /// `sample()` plus a library picture placed twice (frames 0 and 6 of
    /// the "Color" layer) and an unused library picture.
    fn library_sample() -> (Document, document::Asset, document::Asset) {
        use document::drawing::{Image, ImageData};
        use document::{Asset, AssetKind};
        let mut doc = sample();
        let data = ImageData::new(3, 2, vec![0x89, b'P', b'N', b'G', 7, 7, 7]);
        let used = Asset::new("photo.png", AssetKind::Image(data.clone()));
        let unused = Asset::new(
            "spare.jpg",
            AssetKind::Image(ImageData::new(1, 1, vec![0xFF, 0xD8, 1])),
        );
        doc.insert_asset(used.clone());
        doc.insert_asset(unused.clone());
        let layer = doc.layers().iter().nth(1).unwrap().id;
        for frame in [0, 6] {
            let mut v = VectorDrawing::new();
            v.push_image(
                Image::new(data.clone(), Vec2::ZERO, Vec2::new(30.0, 20.0)).with_asset(used.id),
            );
            let drawing = doc.add_drawing(DrawingContent::Vector(v));
            doc.track_mut(layer)
                .unwrap()
                .set_key(FrameIndex(frame), Keyframe::Drawing(drawing));
        }
        (doc, used, unused)
    }

    #[test]
    fn library_roundtrips_and_placed_images_are_saved_without_bytes() {
        let (doc, used, unused) = library_sample();
        let text = to_string(&doc).unwrap();
        assert!(text.contains("\"assets\""));
        // The picture's bytes ("iVBORwcHBw==") are saved once, in the library.
        assert_eq!(text.matches("iVBORwcHBw==").count(), 1, "{text}");
        let loaded = from_str(&text).unwrap();
        assert_eq!(loaded, doc);
        // The library owns its assets: an unused one is kept.
        assert_eq!(loaded.asset(unused.id), Some(&unused));
        assert_eq!(loaded.asset(used.id), Some(&used));
        let (_, report) = from_str_with_report(&text).unwrap();
        assert!(report.is_clean(), "{report:?}");
    }

    #[test]
    fn version_9_files_with_images_load_unchanged() {
        use document::drawing::{Image, ImageData};
        let mut doc = sample();
        let drawing = *doc.drawings().keys().next().unwrap();
        doc.drawing_mut(drawing)
            .and_then(|c| c.as_vector_mut())
            .unwrap()
            .push_image(Image::new(
                ImageData::new(2, 2, vec![1, 2, 3]),
                Vec2::ZERO,
                Vec2::ONE,
            ));
        let v9 = edit_json(&doc, |j| j["format_version"] = 9.into());
        assert!(!v9.contains("assets"));
        assert_eq!(from_str(&v9).unwrap(), doc);
    }

    #[test]
    fn image_of_a_deleted_asset_keeps_its_bytes() {
        let (mut doc, used, _) = library_sample();
        // Only possible in memory (undo keeps such drawings), but saving
        // must still write a file that loads.
        doc.remove_asset(used.id);
        let loaded = from_str(&to_string(&doc).unwrap()).unwrap();
        let images: Vec<_> = loaded
            .drawings()
            .values()
            .filter_map(|c| c.as_vector())
            .flat_map(|v| v.images().to_vec())
            .collect();
        assert_eq!(images.len(), 2);
        assert!(
            images
                .iter()
                .all(|i| i.asset.is_none() && !i.data.is_empty())
        );
    }

    #[test]
    fn broken_library_references_are_rejected() {
        let (doc, used, unused) = library_sample();

        let missing = edit_json(&doc, |j| {
            j["document"]["assets"]
                .as_object_mut()
                .unwrap()
                .remove(&used.id.to_string());
        });
        assert!(matches!(
            invalid(&missing),
            ValidationError::MissingAsset { asset, .. } if asset == used.id
        ));

        // The unused asset stored a second time, under another key.
        let duplicate = edit_json(&doc, |j| {
            let copy = j["document"]["assets"][unused.id.to_string()].clone();
            j["document"]["assets"][AssetId::new().to_string()] = copy;
        });
        assert_eq!(
            invalid(&duplicate),
            ValidationError::DuplicateAsset(unused.id)
        );

        let other = AssetId::new();
        let moved = edit_json(&doc, |j| {
            let assets = j["document"]["assets"].as_object_mut().unwrap();
            let copy = assets.remove(&unused.id.to_string()).unwrap();
            assets.insert(other.to_string(), copy);
        });
        assert_eq!(
            invalid(&moved),
            ValidationError::AssetKeyMismatch {
                key: other,
                id: unused.id
            }
        );

        let empty = edit_json(&doc, |j| {
            j["document"]["assets"][unused.id.to_string()]["kind"]["Image"]["width"] = 0.into();
        });
        assert!(matches!(
            invalid(&empty),
            ValidationError::EmptyAsset { id, .. } if id == unused.id
        ));
    }

    /// `sample()` plus a library sound played by an audio layer from frame
    /// 3, and the sound's asset.
    fn audio_sample() -> (Document, document::Asset, LayerId) {
        use document::scene::{AudioClip, Layer};
        use document::{Asset, AssetKind, AudioData, AudioFormat};
        let mut doc = sample();
        let sound = Asset::new(
            "song.mp3",
            AssetKind::Audio(AudioData {
                format: AudioFormat::Mp3,
                sample_rate: 44_100,
                channels: 2,
                duration: 1.5,
                peaks: vec![0, 128, 255],
                bytes: std::sync::Arc::from(vec![0xFF, 0xFB, 9, 9]),
            }),
        );
        doc.insert_asset(sound.clone());
        let mut clip = AudioClip::new(sound.id, FrameIndex(3));
        clip.volume = 0.5;
        let layer = doc.insert_layer(0, Layer::audio("Song", clip));
        (doc, sound, layer)
    }

    #[test]
    fn audio_layers_and_sounds_roundtrip() {
        let (doc, sound, layer) = audio_sample();
        let text = to_string(&doc).unwrap();
        assert!(text.contains("\"Audio\""), "{text}");
        let loaded = from_str(&text).unwrap();
        assert_eq!(loaded, doc);
        assert_eq!(loaded.asset(sound.id), Some(&sound));
        let clip = loaded.layers().get(layer).unwrap().audio_clip().unwrap();
        assert_eq!((clip.start, clip.volume), (FrameIndex(3), 0.5));
        let (_, report) = from_str_with_report(&text).unwrap();
        assert!(report.is_clean(), "{report:?}");
        // Drawing layers still leave `kind` out.
        assert_eq!(text.matches("\"kind\": {").count(), 2, "{text}");
    }

    /// `sample()` with a library brush that its first stroke is drawn with,
    /// and the brush's asset.
    fn brush_sample() -> (Document, document::Asset) {
        use document::drawing::{BrushData, BrushSettings, ImageData};
        use document::{Asset, AssetKind};
        let mut doc = sample();
        let brush = Asset::new(
            "Pencil",
            AssetKind::Brush(BrushData {
                tip: ImageData::new(2, 2, vec![0x89, b'P', b'N', b'G']),
                grain: Some(ImageData::new(4, 4, vec![1, 2, 3])),
                settings: BrushSettings {
                    flow: 0.4,
                    grain_strength: 0.7,
                    ..BrushSettings::default()
                },
            }),
        );
        doc.insert_asset(brush.clone());
        let drawing = *doc.drawings().keys().next().unwrap();
        let v = doc.drawing_mut(drawing).unwrap().as_vector_mut().unwrap();
        let id = v.strokes()[0].id;
        let (index, mut stroke) = v.remove_stroke(id).unwrap();
        stroke.style.brush = Some(brush.id);
        v.insert_stroke(index, stroke);
        (doc, brush)
    }

    #[test]
    fn brushes_roundtrip() {
        let (doc, brush) = brush_sample();
        let text = to_string(&doc).unwrap();
        assert!(text.contains("\"Brush\""), "{text}");
        assert!(text.contains("\"brush\""), "{text}");
        let loaded = from_str(&text).unwrap();
        assert_eq!(loaded, doc);
        assert!(loaded.asset_in_use(brush.id));
        let (_, report) = from_str_with_report(&text).unwrap();
        assert!(report.is_clean(), "{report:?}");
    }

    #[test]
    fn version_13_files_load_unchanged() {
        let (doc, _, _) = audio_sample();
        let v13 = edit_json(&doc, |j| j["format_version"] = 13.into());
        assert_eq!(from_str(&v13).unwrap(), doc);
    }

    #[test]
    fn broken_brushes_are_rejected() {
        let (doc, brush) = brush_sample();
        let missing = edit_json(&doc, |j| {
            j["document"]["assets"]
                .as_object_mut()
                .unwrap()
                .remove(&brush.id.to_string());
        });
        assert!(matches!(
            invalid(&missing),
            ValidationError::MissingBrushAsset { asset, .. } if asset == brush.id
        ));

        let (_, picture, _) = library_sample();
        let mut not_brush = doc.clone();
        not_brush.insert_asset(picture.clone());
        let id = brush.id.to_string();
        let pic = picture.id.to_string();
        let not_brush = to_string(&not_brush).unwrap().replace(
            &format!("\"brush\": \"{id}\""),
            &format!("\"brush\": \"{pic}\""),
        );
        assert!(matches!(
            invalid(&not_brush),
            ValidationError::NotABrushAsset { asset, .. } if asset == picture.id
        ));

        let bad = edit_json(&doc, |j| {
            j["document"]["assets"][&id]["kind"]["Brush"]["settings"]["flow"] = 3.0.into();
        });
        assert!(matches!(
            invalid(&bad),
            ValidationError::BadBrushSettings { id: a, .. } if a == brush.id
        ));

        let empty = edit_json(&doc, |j| {
            j["document"]["assets"][&id]["kind"]["Brush"]["tip"]["width"] = 0.into();
        });
        assert!(matches!(
            invalid(&empty),
            ValidationError::EmptyAsset { id: a, .. } if a == brush.id
        ));
    }

    #[test]
    fn version_10_library_files_load_unchanged() {
        let (doc, _, _) = library_sample();
        let v10 = edit_json(&doc, |j| j["format_version"] = 10.into());
        assert_eq!(from_str(&v10).unwrap(), doc);
    }

    #[test]
    fn broken_audio_layers_are_rejected() {
        let (doc, sound, layer) = audio_sample();
        let layer_index = 0;

        let missing = edit_json(&doc, |j| {
            j["document"]["assets"]
                .as_object_mut()
                .unwrap()
                .remove(&sound.id.to_string());
        });
        assert!(matches!(
            invalid(&missing),
            ValidationError::MissingAudioAsset { asset, .. } if asset == sound.id
        ));

        let (_, picture, _) = library_sample();
        let mut not_audio = doc.clone();
        not_audio.insert_asset(picture.clone());
        let not_audio = edit_json(&not_audio, |j| {
            j["document"]["layers"]["layers"][layer_index]["kind"]["Audio"]["asset"] =
                picture.id.to_string().into();
        });
        assert!(matches!(
            invalid(&not_audio),
            ValidationError::NotAnAudioAsset { asset, .. } if asset == picture.id
        ));

        let keys = edit_json(&doc, |j| {
            let drawing = j["document"]["drawings"]
                .as_object()
                .unwrap()
                .keys()
                .next()
                .unwrap()
                .clone();
            j["document"]["tracks"][layer.to_string()]["keys"]["0"] =
                serde_json::json!({ "Drawing": drawing });
        });
        assert!(matches!(
            invalid(&keys),
            ValidationError::AudioLayerHasKeys { layer: l, .. } if l == layer
        ));

        let loud = edit_json(&doc, |j| {
            j["document"]["layers"]["layers"][layer_index]["kind"]["Audio"]["volume"] = 2.0.into();
        });
        assert!(matches!(invalid(&loud), ValidationError::OutOfRange { .. }));

        let silent = edit_json(&doc, |j| {
            j["document"]["assets"][sound.id.to_string()]["kind"]["Audio"]["bytes"] = "".into();
        });
        assert!(matches!(
            invalid(&silent),
            ValidationError::EmptyAsset { id, .. } if id == sound.id
        ));
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

    /// `sample()` plus a 12-frame object (with a nested one-frame object)
    /// placed on its first layer at frame 2.
    fn symbol_sample() -> (Document, SymbolId, SymbolId) {
        use document::Symbol;
        use document::scene::Layer;
        use timeline::{InstanceKey, InstanceMode, InstanceTransform, Track};
        let mut doc = sample();
        let mut v = VectorDrawing::new();
        v.push_stroke(Stroke::new(
            vec![StrokePoint::new(Vec2::ZERO), StrokePoint::new(Vec2::ONE)],
            BrushStyle::default(),
        ));
        let drawing = doc.add_drawing(DrawingContent::Vector(v));
        let mut inner = Symbol::new("Dot", 1);
        let mut track = Track::new();
        track.set_key(FrameIndex(0), Keyframe::Drawing(drawing));
        inner.push_layer(Layer::new("dot"), track);
        let inner_id = inner.id;
        let mut outer = Symbol::new("Ball", 12);
        let mut track = Track::new();
        track.set_key(
            FrameIndex(3),
            Keyframe::Instance(
                InstanceKey::new(inner_id).with_transform(InstanceTransform {
                    translation: Vec2::new(1.5, -2.0),
                    scale: Vec2::new(2.0, 0.5),
                    rotation: 0.25,
                }),
            ),
        );
        track.set_key(FrameIndex(9), Keyframe::Empty);
        outer.push_layer(Layer::new("ball"), track);
        let outer_id = outer.id;
        doc.insert_symbol(inner);
        doc.insert_symbol(outer);
        let layer = doc.layers().iter().next().unwrap().id;
        doc.track_mut(layer).unwrap().set_key(
            FrameIndex(2),
            Keyframe::Instance(InstanceKey {
                opacity: 0.5,
                mode: InstanceMode::Once,
                first_frame: FrameIndex(4),
                ..InstanceKey::new(outer_id)
            }),
        );
        (doc, outer_id, inner_id)
    }

    #[test]
    fn symbols_roundtrip_and_keep_their_drawings() {
        let (doc, _, _) = symbol_sample();
        let text = to_string(&doc).unwrap();
        assert!(text.contains("\"symbols\"") && text.contains("\"Instance\""));
        let loaded = from_str(&text).unwrap();
        assert_eq!(loaded, doc);
        for f in 0..doc.frame_count {
            assert_eq!(loaded.evaluate(FrameIndex(f)), doc.evaluate(FrameIndex(f)));
        }
        // An unplaced object keeps its drawings through a save too.
        let mut unplaced = doc.clone();
        let layer = unplaced.layers().iter().next().unwrap().id;
        unplaced.track_mut(layer).unwrap().remove_key(FrameIndex(2));
        assert_eq!(from_str(&to_string(&unplaced).unwrap()).unwrap(), unplaced);
    }

    /// `symbol_sample()` with a second key of the outer object on frame
    /// 10, tweened from the key on frame 2 with `ease`.
    fn tween_sample(ease: animation_core::Interpolation) -> Document {
        use timeline::InstanceTransform;
        let (mut doc, _, _) = symbol_sample();
        let layer = doc.layers().iter().next().unwrap().id;
        let track = doc.track_mut(layer).unwrap();
        let Some(Keyframe::Instance(start)) = track.key_at(FrameIndex(2)).copied() else {
            panic!("instance key expected");
        };
        track.set_key(
            FrameIndex(2),
            Keyframe::Instance(timeline::InstanceKey {
                tween: Some(ease),
                ..start
            }),
        );
        track.set_key(
            FrameIndex(10),
            Keyframe::Instance(
                start.with_transform(InstanceTransform::from_translation(Vec2::new(40.0, 20.0))),
            ),
        );
        doc
    }

    #[test]
    fn tweens_roundtrip_for_every_ease() {
        for ease in animation_core::Interpolation::TWEENS {
            let doc = tween_sample(ease);
            let text = to_string(&doc).unwrap();
            assert!(text.contains(&format!("\"tween\": \"{ease:?}\"")));
            let loaded = from_str(&text).unwrap();
            assert_eq!(loaded, doc);
            for f in 0..doc.frame_count {
                assert_eq!(loaded.evaluate(FrameIndex(f)), doc.evaluate(FrameIndex(f)));
            }
        }
        // Keys without a tween don't write the field.
        let (plain, _, _) = symbol_sample();
        assert!(!to_string(&plain).unwrap().contains("tween"));
    }

    #[test]
    fn version_11_files_load_unchanged() {
        let (doc, _, _) = symbol_sample();
        let v11 = edit_json(&doc, |j| j["format_version"] = 11.into());
        assert!(!v11.contains("tween"));
        assert_eq!(from_str(&v11).unwrap(), doc);
    }

    #[test]
    fn version_12_files_load_unchanged() {
        let doc = tween_sample(animation_core::Interpolation::EaseInOut);
        let v12 = edit_json(&doc, |j| j["format_version"] = 12.into());
        assert!(v12.contains("EaseInOut"));
        assert_eq!(from_str(&v12).unwrap(), doc);
    }

    #[test]
    fn bad_tweens_are_rejected() {
        let doc = tween_sample(animation_core::Interpolation::Linear);
        let set_tween = |value: serde_json::Value| {
            edit_json(&doc, |j| {
                for track in j["document"]["tracks"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                {
                    if let Some(key) = track["keys"].get_mut("2") {
                        key["Instance"]["tween"] = value.clone();
                    }
                }
            })
        };
        assert!(matches!(
            invalid(&set_tween("Step".into())),
            ValidationError::StepTween {
                frame: FrameIndex(2),
                ..
            }
        ));
        assert!(matches!(
            from_str(&set_tween("Bounce".into())),
            Err(FileFormatError::Malformed(_))
        ));
        // A tween can only sit on an object key: other keys have no fields.
        let on_empty = edit_json(&doc, |j| {
            let tracks = j["document"]["tracks"].as_object_mut().unwrap();
            let track = tracks.values_mut().next().unwrap();
            track["keys"]["15"] = serde_json::json!({ "Empty": { "tween": "Linear" } });
        });
        assert!(from_str(&on_empty).is_err());
        // A broken pair still loads; the tween is inactive until it is whole.
        let mut broken = doc.clone();
        let layer = broken.layers().iter().next().unwrap().id;
        broken.track_mut(layer).unwrap().remove_key(FrameIndex(10));
        assert_eq!(from_str(&to_string(&broken).unwrap()).unwrap(), broken);
    }

    #[test]
    fn version_10_files_load_unchanged() {
        let doc = sample();
        let v10 = edit_json(&doc, |j| j["format_version"] = 10.into());
        assert!(!v10.contains("symbols"));
        assert_eq!(from_str(&v10).unwrap(), doc);
    }

    #[test]
    fn broken_symbols_are_rejected() {
        let (doc, outer, inner) = symbol_sample();
        let (outer_s, inner_s) = (outer.to_string(), inner.to_string());

        let mut missing = doc.clone();
        missing.remove_symbol(inner);
        assert!(matches!(
            invalid(&to_string(&missing).unwrap()),
            ValidationError::MissingSymbol { .. }
        ));

        let past_end = edit_json(&doc, |j| {
            for track in j["document"]["tracks"]
                .as_object_mut()
                .unwrap()
                .values_mut()
            {
                if let Some(key) = track["keys"].get_mut("2") {
                    key["Instance"]["first_frame"] = 12.into();
                }
            }
        });
        assert!(matches!(
            invalid(&past_end),
            ValidationError::FirstFrameOutOfRange {
                frame_count: 12,
                ..
            }
        ));

        let cycle = edit_json(&doc, |j| {
            let tracks = j["document"]["symbols"][&inner_s]["tracks"]
                .as_object_mut()
                .unwrap();
            let track = tracks.values_mut().next().unwrap();
            track["keys"]["0"] = serde_json::json!({ "Instance": {
                "symbol": outer_s,
                "transform": { "translation": [0.0, 0.0], "scale": [1.0, 1.0], "rotation": 0.0 },
                "opacity": 1.0, "mode": "Loop", "first_frame": 0
            }});
        });
        assert!(matches!(
            invalid(&cycle),
            ValidationError::SymbolCycle { .. }
        ));

        let no_track = edit_json(&doc, |j| {
            j["document"]["symbols"][&outer_s]["tracks"] = serde_json::json!({});
        });
        assert!(matches!(
            invalid(&no_track),
            ValidationError::MissingTrack { .. }
        ));

        let empty = edit_json(&doc, |j| {
            j["document"]["symbols"][&outer_s]["frame_count"] = 0.into();
        });
        assert!(matches!(
            invalid(&empty),
            ValidationError::EmptySymbol { .. }
        ));

        let mismatch = edit_json(&doc, |j| {
            j["document"]["symbols"][&outer_s]["id"] = inner_s.clone().into();
        });
        assert!(matches!(
            invalid(&mismatch),
            ValidationError::SymbolKeyMismatch { .. }
        ));

        let bad_opacity = edit_json(&doc, |j| {
            for track in j["document"]["tracks"]
                .as_object_mut()
                .unwrap()
                .values_mut()
            {
                if let Some(key) = track["keys"].get_mut("2") {
                    key["Instance"]["opacity"] = 2.0.into();
                }
            }
        });
        assert!(matches!(
            invalid(&bad_opacity),
            ValidationError::OutOfRange { .. }
        ));

        let older = edit_json(&doc, |j| j["format_version"] = 10.into());
        // A v10 file can't hold objects, but if one does it is still checked.
        assert!(from_str(&older).is_ok());
    }

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

    /// Every feature of the combined format: a library picture placed inside
    /// an object, a tweened object on the timeline, and an audio layer.
    fn combined_sample() -> (Document, SymbolId, document::Asset) {
        use document::drawing::{Image, ImageData};
        use document::scene::{AudioClip, Layer};
        use document::{Asset, AssetKind, AudioData, AudioFormat};
        let mut doc = tween_sample(animation_core::Interpolation::EaseOut);
        let data = ImageData::new(3, 2, vec![0x89, b'P', b'N', b'G', 1, 2, 3]);
        let picture = Asset::new("photo.png", AssetKind::Image(data.clone()));
        doc.insert_asset(picture.clone());
        let mut v = VectorDrawing::new();
        v.push_image(Image::new(data, Vec2::ZERO, Vec2::new(3.0, 2.0)).with_asset(picture.id));
        let drawing = doc.add_drawing(DrawingContent::Vector(v));
        let inner = *doc
            .symbols()
            .iter()
            .find(|(_, s)| s.name == "Dot")
            .unwrap()
            .0;
        let mut track = timeline::Track::new();
        track.set_key(FrameIndex(0), Keyframe::Drawing(drawing));
        doc.symbol_mut(inner)
            .unwrap()
            .push_layer(Layer::new("photo"), track);
        let sound = Asset::new(
            "song.wav",
            AssetKind::Audio(AudioData {
                format: AudioFormat::Wav,
                sample_rate: 8000,
                channels: 1,
                duration: 0.5,
                peaks: vec![3, 200],
                bytes: std::sync::Arc::from(vec![b'R', b'I', b'F', b'F']),
            }),
        );
        doc.insert_asset(sound.clone());
        doc.insert_layer(
            0,
            Layer::audio("Song", AudioClip::new(sound.id, FrameIndex(1))),
        );
        (doc, inner, picture)
    }

    #[test]
    fn objects_tweens_pictures_and_sounds_roundtrip_together() {
        let (doc, _, picture) = combined_sample();
        let text = to_string(&doc).unwrap();
        assert!(text.contains(&format!("\"format_version\": {FORMAT_VERSION}")));
        // The picture inside the object is stored once, in its asset.
        assert_eq!(text.matches("iVBORwECAw==").count(), 1, "{text}");
        let loaded = from_str(&text).unwrap();
        assert_eq!(loaded, doc);
        assert!(loaded.asset_in_use(picture.id));
        for f in 0..doc.frame_count {
            assert_eq!(loaded.evaluate(FrameIndex(f)), doc.evaluate(FrameIndex(f)));
        }
    }

    #[test]
    fn audio_layers_inside_objects_are_rejected() {
        use document::scene::{AudioClip, Layer};
        let (mut doc, inner, _) = combined_sample();
        let sound = doc.audio_layers().next().unwrap().1.asset;
        doc.symbol_mut(inner).unwrap().push_layer(
            Layer::audio("Inside", AudioClip::new(sound, FrameIndex(0))),
            timeline::Track::new(),
        );
        assert!(matches!(
            validate(&doc),
            Err(ValidationError::AudioLayerInObject { symbol, .. }) if symbol == inner
        ));
        assert!(matches!(
            invalid(&to_string(&doc).unwrap()),
            ValidationError::AudioLayerInObject { .. }
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
