//! The eraser on fills through the editing session: whole mode removes the
//! fills it touches, partial mode cuts them with a live preview, and either
//! way strokes and fills change in one undo step.

use document::animation_core::{FrameIndex, Rgba, Vec2};
use document::drawing::{Fill, Stroke, VectorDrawing};
use editor::Tool;
use editor::commands::SetLayerLocked;
use ui::{EraserMode, Session};

fn v(x: f32, y: f32) -> Vec2 {
    Vec2::new(x, y)
}

fn drawing(s: &Session) -> Option<&VectorDrawing> {
    let layer = s.state.active_layer?;
    s.document
        .drawing_at(layer, FrameIndex(0))
        .and_then(|id| s.document.drawing(id))
        .and_then(|d| d.as_vector())
}

fn fills(s: &Session) -> Vec<Fill> {
    drawing(s).map_or_else(Vec::new, |d| d.fills().to_vec())
}

fn strokes(s: &Session) -> Vec<Stroke> {
    drawing(s).map_or_else(Vec::new, |d| d.strokes().to_vec())
}

/// Draws a polyline with the brush.
fn draw(s: &mut Session, points: &[(f32, f32)]) {
    s.state.tool = Tool::Brush;
    let p = |(x, y): (f32, f32)| v(x, y);
    assert!(s.pointer_down(p(points[0]), 1.0));
    for pair in points.windows(2) {
        for i in 1..=20 {
            s.pointer_move(p(pair[0]).lerp(p(pair[1]), i as f32 / 20.0));
        }
    }
    s.pointer_up();
}

fn click_fill(s: &mut Session, at: Vec2, color: Rgba) {
    s.state.brush.color = color;
    s.state.tool = Tool::Fill;
    assert!(s.pointer_down(at, 1.0));
    s.pointer_up();
}

/// Two filled squares: red at (100..200), blue at (300..400), outlined with
/// width-4 strokes.
fn two_filled_squares() -> Session {
    let mut s = Session::new();
    s.state.brush.width = 4.0;
    for x in [100.0, 300.0] {
        draw(
            &mut s,
            &[
                (x, 100.0),
                (x + 100.0, 100.0),
                (x + 100.0, 200.0),
                (x, 200.0),
                (x, 100.0),
            ],
        );
    }
    click_fill(&mut s, v(150.0, 150.0), Rgba::rgb(1.0, 0.0, 0.0));
    click_fill(&mut s, v(350.0, 150.0), Rgba::rgb(0.0, 0.0, 1.0));
    assert_eq!(fills(&s).len(), 2);
    assert_eq!(strokes(&s).len(), 2);
    s.state.tool = Tool::Eraser;
    s
}

/// Drags the eraser through `points`.
fn erase(s: &mut Session, points: &[Vec2]) {
    assert!(s.pointer_down(points[0], 1.0));
    for p in &points[1..] {
        s.pointer_move(*p);
    }
}

#[test]
fn whole_mode_removes_touched_fills_and_strokes_in_one_step() {
    let mut s = two_filled_squares();
    let (before_fills, before_strokes) = (fills(&s), strokes(&s));
    let undo_len = s.history.undo_len();
    s.tools.eraser_mode = EraserMode::Stroke;
    s.tools.eraser_radius = 3.0;

    // A dab inside the red fill only: the fill, not its outline.
    erase(&mut s, &[v(150.0, 150.0)]);
    assert_eq!(s.tools.erasing_fills(), [before_fills[0].id]);
    assert!(s.tools.erasing().is_empty());
    assert!(s.tools.fill_erase_preview(before_fills[0].id).is_empty());
    s.pointer_up();
    assert_eq!(fills(&s), before_fills[1..]);
    assert_eq!(strokes(&s), before_strokes);
    assert_eq!(s.history.undo_label(), Some("Erase Fill"));
    s.undo();
    assert_eq!(fills(&s), before_fills);

    // A swipe across the blue square catches its outline and its fill.
    erase(&mut s, &[v(350.0, 50.0), v(350.0, 150.0)]);
    assert_eq!(s.tools.erasing_fills(), [before_fills[1].id]);
    assert_eq!(s.tools.erasing(), [before_strokes[1].id]);
    assert_eq!(fills(&s), before_fills, "document untouched while erasing");
    s.pointer_up();
    assert_eq!(fills(&s), before_fills[..1]);
    assert_eq!(strokes(&s), before_strokes[..1]);
    assert_eq!(s.history.undo_len(), undo_len + 1, "one undo step");
    assert_eq!(s.history.undo_label(), Some("Erase"));
    s.undo();
    assert_eq!((fills(&s), strokes(&s)), (before_fills, before_strokes));
}

#[test]
fn partial_mode_cuts_fills_with_preview_and_one_undo_step() {
    let mut s = two_filled_squares();
    let (before_fills, before_strokes) = (fills(&s), strokes(&s));
    let (red, blue) = (before_fills[0].clone(), before_fills[1].clone());
    let undo_len = s.history.undo_len();
    assert_eq!(s.tools.eraser_mode, EraserMode::Partial);
    s.tools.eraser_radius = 5.0;

    // Top to bottom through the red square: cuts the fill in two and its
    // outline at the top and bottom edges.
    let path: Vec<Vec2> = (0..=20).map(|i| v(150.0, 50.0 + i as f32 * 10.0)).collect();
    erase(&mut s, &path);
    assert_eq!(s.tools.erasing_fills(), [red.id]);
    assert_eq!(s.tools.erasing(), [before_strokes[0].id]);
    let preview = s.tools.fill_erase_preview(red.id).to_vec();
    assert_eq!(preview.len(), 2);
    assert!(s.tools.fill_erase_preview(blue.id).is_empty());
    assert_eq!(fills(&s), before_fills, "document untouched while erasing");
    assert_eq!(s.history.undo_len(), undo_len);

    s.pointer_up();
    assert!(s.tools.erasing_fills().is_empty());
    let after = fills(&s);
    assert_eq!(after.len(), 3);
    assert_eq!(after[..2], preview[..], "commits what the preview showed");
    assert_eq!(after[2], blue, "fill order kept, other fill untouched");
    for piece in &after[..2] {
        assert_ne!(piece.id, red.id);
        assert_eq!((piece.color, piece.opacity), (red.color, red.opacity));
        assert!(!piece.contains(v(150.0, 150.0)));
    }
    assert!(after[..2].iter().any(|p| p.contains(v(120.0, 150.0))));
    assert!(after[..2].iter().any(|p| p.contains(v(180.0, 150.0))));
    assert!(strokes(&s).len() > 2, "the outline was cut too");
    assert_eq!(
        s.history.undo_len(),
        undo_len + 1,
        "strokes and fills in one step"
    );
    assert_eq!(s.history.undo_label(), Some("Erase"));

    s.undo();
    assert_eq!((fills(&s), strokes(&s)), (before_fills, before_strokes));
    s.redo();
    assert_eq!(fills(&s), after);

    // The pieces survive save and load.
    let text = file_format::to_string(&s.document).unwrap();
    assert_eq!(file_format::from_str(&text).unwrap(), s.document);
}

#[test]
fn partial_dab_inside_punches_a_hole_and_leaves_strokes() {
    let mut s = two_filled_squares();
    let before_strokes = strokes(&s);
    let red = fills(&s)[0].clone();
    s.tools.eraser_radius = 8.0;
    // Several small moves; the fill is rasterized once and cut step by step.
    erase(
        &mut s,
        &[
            v(140.0, 150.0),
            v(145.0, 150.0),
            v(150.0, 150.0),
            v(155.0, 150.0),
        ],
    );
    s.pointer_up();
    let after = fills(&s);
    assert_eq!(after.len(), 2);
    let holed = &after[0];
    assert_eq!(holed.contours.len(), 2, "outline and hole");
    assert!(!holed.contains(v(150.0, 150.0)));
    assert!(holed.contains(v(110.0, 110.0)));
    assert_eq!(holed.color, red.color);
    assert_eq!(strokes(&s), before_strokes);
}

#[test]
fn erasing_outside_fills_changes_no_fill() {
    let mut s = two_filled_squares();
    let before = fills(&s);
    s.tools.eraser_radius = 5.0;
    // Between the squares, touching neither.
    erase(&mut s, &[v(250.0, 50.0), v(250.0, 250.0)]);
    assert!(s.tools.erasing_fills().is_empty());
    s.pointer_up();
    assert_eq!(fills(&s), before);
}

#[test]
fn locked_layer_refuses_the_eraser() {
    let mut s = two_filled_squares();
    let before = fills(&s);
    let undo_len = s.history.undo_len();
    let layer = s.state.active_layer.unwrap();
    s.execute(Box::new(SetLayerLocked::new(layer, true)))
        .unwrap();
    for mode in [EraserMode::Partial, EraserMode::Stroke] {
        s.tools.eraser_mode = mode;
        assert!(!s.pointer_down(v(150.0, 150.0), 1.0));
        s.pointer_move(v(160.0, 150.0));
        s.pointer_up();
        assert!(s.tools.erasing_fills().is_empty());
    }
    assert_eq!(fills(&s), before);
    assert_eq!(s.history.undo_len(), undo_len + 1, "only the lock");
}

#[test]
fn fast_partial_drag_cuts_a_clean_channel_in_one_step() {
    let mut s = two_filled_squares();
    let before_fills = fills(&s);
    let red = before_fills[0].clone();
    let undo_len = s.history.undo_len();
    s.tools.eraser_radius = 6.0;
    // Few, widely spaced pointer moves: in at the left edge, a zigzag, out
    // at the bottom.
    let path = [
        v(90.0, 130.0),
        v(150.0, 115.0),
        v(170.0, 170.0),
        v(125.0, 160.0),
        v(140.0, 215.0),
    ];
    erase(&mut s, &path);
    let preview = s.tools.fill_erase_preview(red.id).to_vec();
    assert!(!preview.is_empty());
    s.pointer_up();
    let after = fills(&s);
    assert_eq!(after[..preview.len()], preview[..], "commits the preview");
    assert_eq!(s.history.undo_len(), undo_len + 1, "one undo step");

    // Nothing of the red fill is left along the swept path, in the shape or
    // in its triangles.
    let pieces = &after[..preview.len()];
    let meshes: Vec<_> = pieces
        .iter()
        .map(document::drawing::tessellate_fill)
        .collect();
    for pair in path.windows(2) {
        for i in 0..=100 {
            let q = pair[0].lerp(pair[1], i as f32 / 100.0);
            assert!(!pieces.iter().any(|f| f.contains(q)), "fill left at {q}");
            for mesh in &meshes {
                for t in mesh.indices.as_chunks::<3>().0 {
                    let [a, b, c] = t.map(|i| Vec2::from(mesh.vertices[i as usize]));
                    let inside = (b - a).perp_dot(q - a) > 0.0
                        && (c - b).perp_dot(q - b) > 0.0
                        && (a - c).perp_dot(q - c) > 0.0;
                    assert!(!inside, "mesh bridges the cut at {q}");
                }
            }
        }
    }
    assert!(pieces.iter().any(|f| f.contains(v(110.0, 190.0))));
    assert!(pieces.iter().any(|f| f.contains(v(190.0, 110.0))));

    s.undo();
    assert_eq!(fills(&s), before_fills);
}
