//! Builds a document through editor commands, saves it, reloads it and checks
//! that every frame evaluates the same way.

use document::animation_core::{FrameIndex, Vec2};
use document::drawing::{BrushStyle, DrawingContent, Stroke, StrokePoint, VectorDrawing};
use document::timeline::Keyframe;
use document::{CanvasSize, Document};
use editor::History;
use editor::commands::{AddLayer, RenameLayer};

fn drawing_with_stroke(x: f32) -> DrawingContent {
    let mut v = VectorDrawing::new();
    v.push_stroke(Stroke::new(
        vec![
            StrokePoint::new(Vec2::new(x, 0.0)),
            StrokePoint::new(Vec2::new(x, 10.0)),
        ],
        BrushStyle::default(),
    ));
    DrawingContent::Vector(v)
}

#[test]
fn edit_save_load_evaluate() {
    let mut doc = Document::new(
        "Integration",
        CanvasSize {
            width: 320,
            height: 240,
        },
    );
    let mut history = History::new();

    let add = AddLayer::new("Ink");
    let ink = add.layer_id();
    history.execute(&mut doc, Box::new(add)).unwrap();
    history
        .execute(&mut doc, Box::new(RenameLayer::new(ink, "Line art")))
        .unwrap();

    let a = doc.add_drawing(drawing_with_stroke(1.0));
    let b = doc.add_drawing(drawing_with_stroke(2.0));
    let track = doc.track_mut(ink).unwrap();
    track.set_key(FrameIndex(0), Keyframe::Drawing(a));
    track.set_key(FrameIndex(3), Keyframe::Drawing(b));

    let text = file_format::to_string(&doc).unwrap();
    let loaded = file_format::from_str(&text).unwrap();
    assert_eq!(loaded, doc);

    for f in 0..doc.frame_count {
        assert_eq!(
            loaded.evaluate(FrameIndex(f)),
            doc.evaluate(FrameIndex(f)),
            "frame {f}"
        );
    }
    let x_at = |f| {
        loaded.evaluate(FrameIndex(f)).layers[0].drawing.strokes()[0].points[0]
            .pos
            .x
    };
    assert_eq!(x_at(2), 1.0);
    assert_eq!(x_at(3), 2.0);
}
