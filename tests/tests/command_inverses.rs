//! Every editor command: apply → undo restores the original document exactly,
//! and undo → redo restores the applied document exactly (same IDs).

use document::animation_core::{Fps, FrameIndex, LayerId, StrokeId, Vec2};
use document::drawing::{BrushStyle, DrawingContent, Stroke, StrokePoint, VectorDrawing};
use document::scene::BlendMode;
use document::timeline::Keyframe;
use document::{CanvasSize, Document};
use editor::commands::{
    AddKeyframe, AddLayer, AddStroke, KeyframeKind, MoveLayer, RemoveKeyframe, RemoveLayer,
    RemoveStroke, RenameLayer, SetFps, SetFrameCount, SetLayerBlend, SetLayerLocked,
    SetLayerVisible,
};
use editor::{Command, History};

fn stroke(x: f32) -> Stroke {
    Stroke::new(
        vec![
            StrokePoint::new(Vec2::new(x, 0.0)),
            StrokePoint {
                pos: Vec2::new(x, 10.0),
                pressure: 0.5,
            },
        ],
        BrushStyle::default(),
    )
}

struct Fixture {
    doc: Document,
    bottom: LayerId,
    top: LayerId,
    /// A stroke in the drawing `bottom` shows at frames 0..4.
    stroke: StrokeId,
}

/// Two layers. `bottom`: drawing at 0, empty key at 4, drawing at 8.
/// `top`: no keys.
fn fixture() -> Fixture {
    let mut doc = Document::new(
        "Fixture",
        CanvasSize {
            width: 200,
            height: 100,
        },
    );
    let bottom = doc.add_layer("Bottom");
    let top = doc.add_layer("Top");
    let first = stroke(1.0);
    let stroke_id = first.id;
    let mut a = VectorDrawing::new();
    a.push_stroke(first);
    a.push_stroke(stroke(2.0));
    let a = doc.add_drawing(DrawingContent::Vector(a));
    let b = doc.add_drawing(DrawingContent::default());
    let track = doc.track_mut(bottom).expect("track");
    track.set_key(FrameIndex(0), Keyframe::Drawing(a));
    track.set_key(FrameIndex(4), Keyframe::Empty);
    track.set_key(FrameIndex(8), Keyframe::Drawing(b));
    Fixture {
        doc,
        bottom,
        top,
        stroke: stroke_id,
    }
}

/// Checks the inverse laws directly on the command and again through `History`.
fn check(name: &str, doc: &Document, make: impl Fn() -> Box<dyn Command>) {
    let original = doc.clone();

    let mut d = original.clone();
    let mut cmd = make();
    cmd.apply(&mut d)
        .unwrap_or_else(|e| panic!("{name}: apply failed: {e}"));
    assert_ne!(d, original, "{name}: apply changed nothing");
    let applied = d.clone();
    for round in 0..2 {
        cmd.undo(&mut d)
            .unwrap_or_else(|e| panic!("{name}: undo failed: {e}"));
        assert_eq!(d, original, "{name}: undo #{round} did not restore");
        cmd.apply(&mut d)
            .unwrap_or_else(|e| panic!("{name}: redo failed: {e}"));
        assert_eq!(d, applied, "{name}: redo #{round} did not reproduce");
    }

    let mut d = original.clone();
    let mut history = History::new();
    history
        .execute(&mut d, make())
        .unwrap_or_else(|e| panic!("{name}: execute failed: {e}"));
    let via_history = d.clone();
    assert!(history.undo(&mut d).expect("undo"), "{name}");
    assert_eq!(d, original, "{name}: history undo");
    assert!(history.redo(&mut d).expect("redo"), "{name}");
    assert_eq!(d, via_history, "{name}: history redo");
    assert!(history.is_dirty());
}

#[test]
fn every_command_is_exactly_invertible() {
    let f = fixture();
    let (b, t) = (f.bottom, f.top);
    let doc = &f.doc;

    check("AddLayer", doc, || Box::new(AddLayer::new("New")));
    check("RemoveLayer bottom", doc, || Box::new(RemoveLayer::new(b)));
    check("RemoveLayer top", doc, || Box::new(RemoveLayer::new(t)));
    check("RenameLayer", doc, || {
        Box::new(RenameLayer::new(b, "Renamed"))
    });
    check("MoveLayer up", doc, || Box::new(MoveLayer::new(b, 1)));
    check("MoveLayer clamp", doc, || Box::new(MoveLayer::new(b, 99)));
    check("MoveLayer down", doc, || Box::new(MoveLayer::new(t, 0)));
    check("SetLayerVisible", doc, || {
        Box::new(SetLayerVisible::new(b, false))
    });
    check("SetLayerLocked", doc, || {
        Box::new(SetLayerLocked::new(t, true))
    });
    check("SetLayerBlend", doc, || {
        Box::new(SetLayerBlend::new(t, BlendMode::Multiply))
    });

    let s = stroke(5.0);
    check("AddStroke existing drawing", doc, || {
        Box::new(AddStroke::new(b, FrameIndex(2), s.clone()))
    });
    check("AddStroke over empty key", doc, || {
        Box::new(AddStroke::new(b, FrameIndex(5), s.clone()))
    });
    check("AddStroke on empty key frame", doc, || {
        Box::new(AddStroke::new(b, FrameIndex(4), s.clone()))
    });
    check("AddStroke new track", doc, || {
        Box::new(AddStroke::new(t, FrameIndex(3), s.clone()))
    });
    let sid = f.stroke;
    check("RemoveStroke", doc, || {
        Box::new(RemoveStroke::new(b, FrameIndex(3), sid))
    });

    check("AddKeyframe drawing", doc, || {
        Box::new(AddKeyframe::new(b, FrameIndex(2), KeyframeKind::Drawing))
    });
    check("AddKeyframe empty", doc, || {
        Box::new(AddKeyframe::new(t, FrameIndex(0), KeyframeKind::Empty))
    });
    check("RemoveKeyframe drawing", doc, || {
        Box::new(RemoveKeyframe::new(b, FrameIndex(0)))
    });
    check("RemoveKeyframe empty", doc, || {
        Box::new(RemoveKeyframe::new(b, FrameIndex(4)))
    });

    let fps = Fps::new(24000, 1001).expect("fps");
    check("SetFps", doc, || Box::new(SetFps::new(fps)));
    check("SetFrameCount shorter", doc, || {
        Box::new(SetFrameCount::new(6))
    });
    check("SetFrameCount longer", doc, || {
        Box::new(SetFrameCount::new(100))
    });
}

#[test]
fn redo_reuses_ids_after_a_full_rewind() {
    let f = fixture();
    let mut doc = f.doc.clone();
    let mut history = History::new();
    history
        .execute(&mut doc, Box::new(AddLayer::new("A")))
        .expect("add layer");
    history
        .execute(
            &mut doc,
            Box::new(AddKeyframe::new(
                f.top,
                FrameIndex(1),
                KeyframeKind::Drawing,
            )),
        )
        .expect("add key");
    history
        .execute(
            &mut doc,
            Box::new(AddStroke::new(f.top, FrameIndex(1), stroke(3.0))),
        )
        .expect("add stroke");
    let end = doc.clone();
    while history.undo(&mut doc).expect("undo") {}
    assert_eq!(doc, f.doc);
    while history.redo(&mut doc).expect("redo") {}
    assert_eq!(doc, end);
}
