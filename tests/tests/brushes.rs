//! Brushed strokes (ADR-022) through editor commands: a brush joins the
//! library with its first stroke in one undo step, the project file keeps
//! it, and it can't be deleted while a stroke uses it.

use document::animation_core::{AssetId, FrameIndex, Vec2};
use document::drawing::{BrushData, BrushSettings, BrushStyle, ImageData, Stroke, StrokePoint};
use document::{Asset, AssetKind, CanvasSize, Document};
use editor::commands::{AddAsset, AddLayer, AddStroke, Batch, RemoveAsset};
use editor::{Command, CommandError, History};

fn brush() -> Asset {
    Asset {
        id: AssetId::new(),
        name: "Pencil (HB)".into(),
        kind: AssetKind::Brush(BrushData {
            tip: ImageData::new(1, 1, vec![0x89, b'P', b'N', b'G']),
            grain: None,
            settings: BrushSettings {
                flow: 0.2,
                ..BrushSettings::default()
            },
        }),
    }
}

fn stroke(brush: AssetId, x: f32) -> Stroke {
    Stroke::new(
        vec![
            StrokePoint::new(Vec2::new(x, 0.0)),
            StrokePoint::new(Vec2::new(x, 40.0)),
        ],
        BrushStyle {
            brush: Some(brush),
            ..BrushStyle::default()
        },
    )
}

#[test]
fn a_brush_joins_the_library_with_its_first_stroke_and_survives_saving() {
    let mut doc = Document::new(
        "Brushes",
        CanvasSize {
            width: 320,
            height: 240,
        },
    );
    let mut history = History::new();
    let add = AddLayer::new("Pencil");
    let layer = add.layer_id();
    history.execute(&mut doc, Box::new(add)).unwrap();

    let pencil = brush();
    let commands: Vec<Box<dyn Command>> = vec![
        Box::new(AddAsset::new(pencil.clone())),
        Box::new(AddStroke::new(
            layer,
            FrameIndex(0),
            stroke(pencil.id, 10.0),
        )),
    ];
    history
        .execute(&mut doc, Box::new(Batch::new("Add Stroke", commands)))
        .unwrap();
    assert!(doc.asset_in_use(pencil.id));

    // In use: deleting it is refused.
    let refused = history.execute(&mut doc, Box::new(RemoveAsset::new(pencil.id)));
    assert!(matches!(refused, Err(CommandError::AssetInUse(_))));

    let dir = std::env::temp_dir().join(format!("rusting-brushes-{}", doc.id));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("brushes.rusting");
    file_format::save(&doc, &path).unwrap();
    let loaded = file_format::load(&path).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(loaded, doc);
    assert_eq!(loaded.asset(pencil.id), Some(&pencil));

    // Undo takes stroke and brush out together.
    history.undo(&mut doc).unwrap();
    assert!(doc.asset(pencil.id).is_none());
}
