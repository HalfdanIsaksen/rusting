//! Brush packages (ADR-022): reading and writing `.rustbrush` files, and the
//! built-in brushes in `assets/brushes/`.
//!
//! A `.rustbrush` file is a zip with:
//! - `brush.json`: `{ "format_version": 1, "id": "<uuid>"?, "name": "…",
//!   "settings": { … } }`. Every setting is optional (see
//!   [`BrushSettings`]); out-of-range values are clamped.
//! - `tip.png`: the tip. Coverage is red × alpha, so white (or opaque, for
//!   tips with transparency) paints. It should fade out at the edge of its
//!   inscribed circle.
//! - `grain.png` (optional): a tiling paper grain; brightness is how much
//!   paint the paper takes.
//!
//! A built-in brush is the same three files in a folder, compiled into the
//! app.

use crate::import::image_data;
use document::animation_core::AssetId;
use document::drawing::{BrushData, BrushSettings, ImageData};
use serde::{Deserialize, Serialize};
use std::io::{Cursor, Read, Write};
use std::path::Path;
use ui::BrushPreset;

/// File extension of brush packages (without the dot).
pub const EXTENSION: &str = "rustbrush";

/// The `brush.json` version this build writes and reads.
pub const FORMAT_VERSION: u32 = 1;

/// Largest tip or grain side, in pixels.
pub const MAX_SIDE: u32 = 1024;

/// Largest uncompressed file inside a package, against zip bombs.
const MAX_ENTRY_BYTES: u64 = 32 * 1024 * 1024;

/// `brush.json`.
#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    format_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<AssetId>,
    name: String,
    #[serde(default)]
    settings: BrushSettings,
}

/// A parsed brush package.
#[derive(Debug)]
pub struct Package {
    /// The brush's fixed ID, if the package has one (built-ins do).
    pub id: Option<AssetId>,
    /// Display name.
    pub name: String,
    /// Tip, grain and settings.
    pub data: BrushData,
}

/// Checks one picture of a package: a PNG no larger than [`MAX_SIDE`].
fn picture(bytes: Vec<u8>, what: &str) -> Result<ImageData, String> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(format!("{what} is not a PNG picture."));
    }
    let data = image_data(bytes).map_err(|e| format!("{what}: {e}"))?;
    if data.width > MAX_SIDE || data.height > MAX_SIDE {
        return Err(format!(
            "{what} is {}×{} pixels; brushes may be at most {MAX_SIDE}×{MAX_SIDE}.",
            data.width, data.height
        ));
    }
    Ok(data)
}

/// Builds a brush from the three parts of a package.
pub fn parse(json: &str, tip: Vec<u8>, grain: Option<Vec<u8>>) -> Result<Package, String> {
    let manifest: Manifest =
        serde_json::from_str(json).map_err(|e| format!("brush.json is not valid: {e}"))?;
    if manifest.format_version > FORMAT_VERSION {
        return Err(format!(
            "The brush is format version {}; this app reads up to version {FORMAT_VERSION}. \
             Update Rusting to use it.",
            manifest.format_version
        ));
    }
    let name = manifest.name.trim();
    if name.is_empty() {
        return Err("brush.json has no name.".into());
    }
    Ok(Package {
        id: manifest.id,
        name: name.to_owned(),
        data: BrushData {
            tip: picture(tip, "tip.png")?,
            grain: grain.map(|g| picture(g, "grain.png")).transpose()?,
            settings: manifest.settings.clamped(),
        },
    })
}

/// Reads one entry of a zip, or `None` if it is missing.
fn entry<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<Option<Vec<u8>>, String> {
    let file = match zip.by_name(name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_ENTRY_BYTES {
        return Err(format!("{name} is too large."));
    }
    Ok(Some(bytes))
}

/// Parses a `.rustbrush` file's bytes.
pub fn from_bytes(bytes: &[u8]) -> Result<Package, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| format!("Not a brush package: {e}"))?;
    let json = entry(&mut zip, "brush.json")?.ok_or("The package has no brush.json.")?;
    let json = String::from_utf8(json).map_err(|_| "brush.json is not UTF-8 text.")?;
    let tip = entry(&mut zip, "tip.png")?.ok_or("The package has no tip.png.")?;
    let grain = entry(&mut zip, "grain.png")?;
    parse(&json, tip, grain)
}

/// Reads a `.rustbrush` file.
pub fn read(path: &Path) -> Result<Package, String> {
    from_bytes(&std::fs::read(path).map_err(|e| e.to_string())?)
}

/// Encodes brush `data` named `name` as a `.rustbrush` file. `id` is kept
/// for built-ins only; other brushes get a new ID when imported.
pub fn to_bytes(id: Option<AssetId>, name: &str, data: &BrushData) -> Result<Vec<u8>, String> {
    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        id,
        name: name.to_owned(),
        settings: data.settings,
    };
    let json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: &str, bytes: &[u8]| -> Result<(), String> {
        zip.start_file(name, options).map_err(|e| e.to_string())?;
        zip.write_all(bytes).map_err(|e| e.to_string())
    };
    put("brush.json", json.as_bytes())?;
    put("tip.png", &data.tip.bytes)?;
    if let Some(grain) = &data.grain {
        put("grain.png", &grain.bytes)?;
    }
    Ok(zip.finish().map_err(|e| e.to_string())?.into_inner())
}

/// Writes brush `data` named `name` to `path` as a `.rustbrush` file.
pub fn write(path: &Path, name: &str, data: &BrushData) -> Result<(), String> {
    std::fs::write(path, to_bytes(None, name, data)?).map_err(|e| e.to_string())
}

/// One built-in brush folder, compiled in.
struct Builtin {
    json: &'static str,
    tip: &'static [u8],
    grain: Option<&'static [u8]>,
}

macro_rules! builtin {
    ($dir:literal) => {
        Builtin {
            json: include_str!(concat!("../../../assets/brushes/", $dir, "/brush.json")),
            tip: include_bytes!(concat!("../../../assets/brushes/", $dir, "/tip.png")),
            grain: None,
        }
    };
    ($dir:literal, grain) => {
        Builtin {
            grain: Some(include_bytes!(concat!(
                "../../../assets/brushes/",
                $dir,
                "/grain.png"
            ))),
            ..builtin!($dir)
        }
    };
}

/// The built-in brushes, in picker order.
const BUILTINS: &[Builtin] = &[
    builtin!("pencil-hb", grain),
    builtin!("soft-pencil-6b", grain),
    builtin!("charcoal", grain),
    builtin!("chalk", grain),
    builtin!("ink-pen"),
    builtin!("marker"),
];

/// Decodes the built-in brushes. One that fails (a build mistake) is
/// logged and left out.
pub fn presets() -> Vec<BrushPreset> {
    BUILTINS
        .iter()
        .filter_map(
            |b| match parse(b.json, b.tip.to_vec(), b.grain.map(<[u8]>::to_vec)) {
                Ok(Package {
                    id: Some(id),
                    name,
                    data,
                }) => Some(BrushPreset { id, name, data }),
                Ok(p) => {
                    tracing::error!("built-in brush {} has no id", p.name);
                    None
                }
                Err(e) => {
                    tracing::error!("built-in brush failed to load: {e}");
                    None
                }
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn every_builtin_loads_with_a_unique_id() {
        let presets = presets();
        assert_eq!(presets.len(), BUILTINS.len());
        let ids: std::collections::BTreeSet<_> = presets.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), presets.len());
        for p in &presets {
            assert!(p.data.settings.validate().is_ok(), "{}", p.name);
        }
        assert_eq!(presets[0].name, "Pencil (HB)");
        assert!(presets[0].data.grain.is_some());
    }

    #[test]
    fn packages_roundtrip() {
        let pencil = presets().remove(0);
        let bytes = to_bytes(None, "My Pencil", &pencil.data).unwrap();
        let back = from_bytes(&bytes).unwrap();
        assert_eq!(back.id, None);
        assert_eq!(back.name, "My Pencil");
        assert_eq!(back.data, pencil.data);
    }

    #[test]
    fn bad_packages_are_rejected() {
        let pencil = presets().remove(0);
        let tip = pencil.data.tip.bytes.to_vec();
        let json = r#"{"format_version": 1, "name": "X"}"#;
        assert!(parse(json, tip.clone(), None).is_ok());
        let newer = r#"{"format_version": 2, "name": "X"}"#;
        assert!(
            parse(newer, tip.clone(), None)
                .unwrap_err()
                .contains("version")
        );
        assert!(parse(r#"{"format_version": 1, "name": " "}"#, tip.clone(), None).is_err());
        assert!(parse(json, b"not a png".to_vec(), None).is_err());

        // A zip without a tip.
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("brush.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(json.as_bytes()).unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert!(from_bytes(&bytes).unwrap_err().contains("tip.png"));
        assert!(from_bytes(b"nope").is_err());
    }

    #[test]
    fn settings_out_of_range_are_clamped() {
        let tip = presets().remove(0).data.tip.bytes.to_vec();
        let p = parse(
            r#"{"format_version": 1, "name": "X", "settings": {"flow": 5}}"#,
            tip,
            None,
        )
        .unwrap();
        assert_eq!(p.data.settings.flow, 1.0);
    }
}
