//! Selection tool data flow across crates: pick items with the `drawing`
//! hit-tests, move them with `editor::commands::TranslateItems`, delete them
//! with `remove_items`, and check undo/redo restores exact positions.

use document::animation_core::{FrameIndex, LayerId, Rgba, Vec2};
use document::drawing::{BrushStyle, Fill, FillShape, ItemId, Stroke, StrokePoint, VectorDrawing};
use document::{CanvasSize, Document};
use editor::History;
use editor::commands::{AddFill, AddStroke, TranslateItems, remove_items};

fn line(a: Vec2, b: Vec2) -> Stroke {
    Stroke::new(
        vec![
            StrokePoint::new(a),
            StrokePoint {
                pos: a.lerp(b, 0.33),
                pressure: 0.6,
            },
            StrokePoint::new(b),
        ],
        BrushStyle::default(),
    )
}

fn drawing(doc: &Document, layer: LayerId, frame: u32) -> &VectorDrawing {
    let id = doc
        .drawing_at(layer, FrameIndex(frame))
        .expect("layer shows a drawing");
    doc.drawing(id)
        .and_then(|d| d.as_vector())
        .expect("vector drawing")
}

/// Two lines and a filled square on frame 0 of one layer.
fn setup() -> (Document, History, LayerId) {
    let mut doc = Document::new(
        "Selection",
        CanvasSize {
            width: 640,
            height: 480,
        },
    );
    let layer = doc.add_layer("Ink");
    let mut history = History::new();
    let f0 = FrameIndex(0);
    for stroke in [
        line(Vec2::new(10.3, 10.7), Vec2::new(90.1, 12.9)),
        line(Vec2::new(10.0, 300.0), Vec2::new(90.0, 310.0)),
    ] {
        history
            .execute(&mut doc, Box::new(AddStroke::new(layer, f0, stroke)))
            .expect("add stroke");
    }
    let square = Fill::new(
        FillShape {
            contours: vec![vec![
                Vec2::new(200.0, 200.0),
                Vec2::new(260.0, 200.0),
                Vec2::new(260.0, 260.0),
                Vec2::new(200.0, 260.0),
            ]],
        },
        Rgba::rgb(0.2, 0.4, 0.9),
        0.8,
    );
    history
        .execute(&mut doc, Box::new(AddFill::new(layer, f0, square)))
        .expect("add fill");
    (doc, history, layer)
}

#[test]
fn select_move_undo_redo() {
    let (mut doc, mut history, layer) = setup();
    let original = doc.clone();

    // Select: click the first line, then marquee over the fill.
    let d = drawing(&doc, layer, 0);
    let clicked = d.item_at(Vec2::new(50.0, 12.0), 2.0).unwrap();
    assert!(matches!(clicked, ItemId::Stroke(_)));
    let boxed = d.items_in_rect((Vec2::new(190.0, 190.0), Vec2::new(210.0, 210.0)));
    assert_eq!(boxed.len(), 1);
    assert!(matches!(boxed[0], ItemId::Fill(_)));
    let selection = vec![clicked, boxed[0]];
    let untouched = d
        .all_items()
        .into_iter()
        .find(|i| !selection.contains(i))
        .unwrap();
    let bounds_before = d.items_bounds(&selection).unwrap();

    // Move by an offset that does not round-trip in f32.
    let delta = Vec2::new(13.37, -0.1);
    history
        .execute(
            &mut doc,
            Box::new(TranslateItems::new(
                layer,
                FrameIndex(0),
                selection.clone(),
                delta,
            )),
        )
        .unwrap();
    let d = drawing(&doc, layer, 0);
    let bounds_after = d.items_bounds(&selection).unwrap();
    assert_eq!(bounds_after.0, bounds_before.0 + delta);
    assert_eq!(bounds_after.1, bounds_before.1 + delta);
    assert_eq!(
        d.item_bounds(untouched),
        drawing(&original, layer, 0).item_bounds(untouched)
    );
    // The moved items are found at their new place.
    assert_eq!(
        d.item_at(Vec2::new(230.0, 230.0) + delta, 0.0),
        Some(boxed[0])
    );
    let moved = doc.clone();

    // Undo restores exact positions; redo the exact moved state.
    assert!(history.undo(&mut doc).unwrap());
    assert_eq!(doc, original);
    assert!(history.redo(&mut doc).unwrap());
    assert_eq!(doc, moved);

    // A second move and a delete, each one step, all the way back.
    history
        .execute(
            &mut doc,
            Box::new(TranslateItems::new(
                layer,
                FrameIndex(0),
                selection.clone(),
                Vec2::new(-1.0, 10.0),
            )),
        )
        .unwrap();
    history
        .execute(
            &mut doc,
            Box::new(remove_items(layer, FrameIndex(0), &selection)),
        )
        .unwrap();
    assert_eq!(drawing(&doc, layer, 0).all_items(), vec![untouched]);
    for _ in 0..3 {
        assert!(history.undo(&mut doc).unwrap());
    }
    assert_eq!(doc, original);
    for _ in 0..3 {
        assert!(history.redo(&mut doc).unwrap());
    }
    assert_eq!(drawing(&doc, layer, 0).all_items(), vec![untouched]);
}

#[test]
fn moving_on_a_held_frame_moves_the_shared_drawing() {
    // Frame 5 holds the drawing keyed at frame 0, so moving "at frame 5"
    // edits that same drawing.
    let (mut doc, mut history, layer) = setup();
    let items = drawing(&doc, layer, 5).all_items();
    history
        .execute(
            &mut doc,
            Box::new(TranslateItems::new(
                layer,
                FrameIndex(5),
                items,
                Vec2::new(5.0, 5.0),
            )),
        )
        .unwrap();
    assert_eq!(drawing(&doc, layer, 0), drawing(&doc, layer, 5));
    assert!(history.undo(&mut doc).unwrap());
}
