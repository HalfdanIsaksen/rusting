//! The fill bucket through the editing session: click to fill, gap closing,
//! recolouring, undo/redo and save/load.

use document::animation_core::{FrameIndex, Rgba, Vec2};
use document::drawing::{GapClosing, VectorDrawing};
use editor::Tool;
use ui::{FillStyle, Session};

fn drawing(s: &Session) -> Option<&VectorDrawing> {
    let layer = s.state.active_layer?;
    s.document
        .drawing_at(layer, FrameIndex(0))
        .and_then(|id| s.document.drawing(id))
        .and_then(|d| d.as_vector())
}

fn fills(s: &Session) -> usize {
    drawing(s).map_or(0, |d| d.fills().len())
}

/// Draws a polyline with the brush.
fn draw(s: &mut Session, points: &[(f32, f32)]) {
    s.state.tool = Tool::Brush;
    let p = |(x, y): (f32, f32)| Vec2::new(x, y);
    assert!(s.pointer_down(p(points[0]), 1.0));
    for pair in points.windows(2) {
        for i in 1..=20 {
            s.pointer_move(p(pair[0]).lerp(p(pair[1]), i as f32 / 20.0));
        }
    }
    s.pointer_up();
}

fn click_fill(s: &mut Session, at: Vec2) {
    s.state.tool = Tool::Fill;
    assert!(s.pointer_down(at, 1.0));
    s.pointer_move(at);
    s.pointer_up();
    assert!(!s.tools.is_active(), "fill is a click, not a gesture");
}

#[test]
fn fill_closed_square_then_undo_redo_and_recolour() {
    let mut s = Session::new();
    s.state.brush.width = 4.0;
    draw(
        &mut s,
        &[
            (100.0, 100.0),
            (200.0, 100.0),
            (200.0, 200.0),
            (100.0, 200.0),
            (100.0, 100.0),
        ],
    );
    let red = Rgba::rgb(1.0, 0.0, 0.0);
    s.state.brush.color = red;
    s.state.brush.opacity = 0.5;
    click_fill(&mut s, Vec2::new(150.0, 150.0));
    assert_eq!(fills(&s), 1);
    let fill = &drawing(&s).unwrap().fills()[0];
    assert_eq!((fill.color, fill.opacity), (red, 0.5));
    assert!(fill.contains(Vec2::new(150.0, 150.0)));

    s.undo();
    assert_eq!(fills(&s), 0);
    s.redo();
    assert_eq!(fills(&s), 1);

    // Clicking the fill again with another colour recolours it in one step.
    let undo_steps = s.history.undo_len();
    s.state.brush.color = Rgba::rgb(0.0, 0.0, 1.0);
    click_fill(&mut s, Vec2::new(150.0, 150.0));
    assert_eq!(fills(&s), 1);
    assert_eq!(drawing(&s).unwrap().fills()[0].color.b, 1.0);
    assert_eq!(s.history.undo_len(), undo_steps + 1);
    s.undo();
    assert_eq!(drawing(&s).unwrap().fills()[0].color, red);

    // Fills survive save and load.
    let text = file_format::to_string(&s.document).unwrap();
    let loaded = file_format::from_str(&text).unwrap();
    assert_eq!(loaded, s.document);
}

#[test]
fn small_gap_needs_gap_closing_and_explains_failure() {
    let mut s = Session::new();
    s.state.brush.width = 4.0;
    // Square with a ~5 unit gap in the top edge.
    draw(
        &mut s,
        &[
            (154.5, 100.0),
            (200.0, 100.0),
            (200.0, 200.0),
            (100.0, 200.0),
            (100.0, 100.0),
            (145.5, 100.0),
        ],
    );
    let undo_steps = s.history.undo_len();
    s.tools.fill_gap = GapClosing::None;
    click_fill(&mut s, Vec2::new(150.0, 150.0));
    assert_eq!(fills(&s), 0);
    assert_eq!(s.history.undo_len(), undo_steps, "nothing recorded");
    let status = s.status().unwrap();
    assert!(
        status.is_error && status.text.contains("not closed"),
        "{}",
        status.text
    );

    s.tools.fill_gap = GapClosing::Small;
    click_fill(&mut s, Vec2::new(150.0, 150.0));
    assert_eq!(fills(&s), 1);
}

#[test]
fn fill_on_empty_frame_does_nothing() {
    let mut s = Session::new();
    click_fill(&mut s, Vec2::new(10.0, 10.0));
    assert_eq!(fills(&s), 0);
    assert!(!s.history.can_undo());
    assert!(s.status().unwrap().is_error);
}

#[test]
fn gradient_fill_spans_the_area_and_survives_save() {
    let mut s = Session::new();
    s.state.brush.width = 4.0;
    draw(
        &mut s,
        &[
            (100.0, 100.0),
            (200.0, 100.0),
            (200.0, 200.0),
            (100.0, 200.0),
            (100.0, 100.0),
        ],
    );
    s.tools.fill_style = FillStyle::Gradient;
    s.tools.gradient_color = Rgba::rgb(0.0, 0.0, 1.0);
    // The first click only sets the start; the second sets the end and fills.
    click_fill(&mut s, Vec2::new(120.0, 150.0));
    assert_eq!(fills(&s), 0);
    assert_eq!(s.tools.gradient_start(), Some(Vec2::new(120.0, 150.0)));
    click_fill(&mut s, Vec2::new(180.0, 150.0));
    assert_eq!(s.tools.gradient_start(), None);
    let g = drawing(&s).unwrap().fills()[0].gradient.unwrap();
    assert_eq!(g.end_color, Rgba::rgb(0.0, 0.0, 1.0));
    assert_eq!(
        (g.start, g.end),
        (Vec2::new(120.0, 150.0), Vec2::new(180.0, 150.0))
    );

    // Esc drops a pending start.
    click_fill(&mut s, Vec2::new(150.0, 150.0));
    s.clear_selection();
    assert_eq!(s.tools.gradient_start(), None);

    // Clicking it with a normal fill makes it solid again.
    s.tools.fill_style = FillStyle::Normal;
    click_fill(&mut s, Vec2::new(150.0, 150.0));
    assert_eq!(drawing(&s).unwrap().fills()[0].gradient, None);
    s.undo();

    let text = file_format::to_string(&s.document).unwrap();
    assert!(text.contains("gradient"));
    assert_eq!(file_format::from_str(&text).unwrap(), s.document);
}
