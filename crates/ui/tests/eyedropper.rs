//! The eyedropper through the editing session: picking from strokes, fills,
//! gradients and the background, and Alt-click from the drawing tools.

use document::animation_core::{Rgba, Vec2};
use editor::Tool;
use ui::Session;
use ui::shapes::GradientDirection;

const RED: Rgba = Rgba::rgb(1.0, 0.0, 0.0);
const GREEN: Rgba = Rgba::rgb(0.0, 1.0, 0.0);
const BLUE: Rgba = Rgba::rgb(0.0, 0.0, 1.0);

/// Draws a polyline with the brush in `color` at `opacity`.
fn draw(s: &mut Session, points: &[(f32, f32)], color: Rgba, opacity: f32) {
    s.state.tool = Tool::Brush;
    s.state.brush.width = 6.0;
    s.state.brush.color = color;
    s.state.brush.opacity = opacity;
    let p = |(x, y): (f32, f32)| Vec2::new(x, y);
    assert!(s.pointer_down(p(points[0]), 1.0));
    for pair in points.windows(2) {
        for i in 1..=20 {
            s.pointer_move(p(pair[0]).lerp(p(pair[1]), i as f32 / 20.0));
        }
    }
    s.pointer_up();
}

/// A red square outline (opacity 0.8) filled with green (opacity 0.6).
fn square_scene() -> Session {
    let mut s = Session::new();
    draw(
        &mut s,
        &[
            (100.0, 100.0),
            (200.0, 100.0),
            (200.0, 200.0),
            (100.0, 200.0),
            (100.0, 100.0),
        ],
        RED,
        0.8,
    );
    s.state.tool = Tool::Fill;
    s.state.brush.color = GREEN;
    s.state.brush.opacity = 0.6;
    assert!(s.pointer_down(Vec2::new(150.0, 150.0), 1.0));
    s.pointer_up();
    // Reset the colour so a pick is visible.
    s.state.brush.color = BLUE;
    s.state.brush.opacity = 1.0;
    s
}

fn click(s: &mut Session, at: Vec2) {
    assert!(s.pointer_down(at, 1.0));
    s.pointer_up();
}

#[test]
fn picks_from_a_stroke_without_an_undo_step() {
    let mut s = square_scene();
    let undo = s.history.undo_len();
    s.state.tool = Tool::Eyedropper;
    click(&mut s, Vec2::new(150.0, 100.0));
    assert_eq!(s.state.brush.color, RED);
    assert!((s.state.brush.opacity - 0.8).abs() < 1e-6);
    assert_eq!(s.state.tool, Tool::Eyedropper);
    assert_eq!(s.history.undo_len(), undo, "picking changes no document");
    assert!(!s.tools.is_active());
}

#[test]
fn picks_from_a_fill() {
    let mut s = square_scene();
    s.state.tool = Tool::Eyedropper;
    click(&mut s, Vec2::new(150.0, 150.0));
    assert_eq!(s.state.brush.color, GREEN);
    assert!((s.state.brush.opacity - 0.6).abs() < 1e-6);
}

#[test]
fn stroke_over_fill_picks_the_stroke() {
    let mut s = square_scene();
    // A blue line across the green fill.
    draw(&mut s, &[(120.0, 150.0), (180.0, 150.0)], BLUE, 1.0);
    s.state.brush.color = Rgba::BLACK;
    s.state.tool = Tool::Eyedropper;
    click(&mut s, Vec2::new(150.0, 150.0));
    assert_eq!(s.state.brush.color, BLUE);
    // Off the line, inside the square, the fill shows through.
    click(&mut s, Vec2::new(150.0, 180.0));
    assert_eq!(s.state.brush.color, GREEN);
}

#[test]
fn empty_canvas_picks_the_background() {
    let mut s = square_scene();
    s.document.background = Some(Rgba::rgb(0.2, 0.3, 0.4));
    s.state.brush.opacity = 0.5;
    s.state.tool = Tool::Eyedropper;
    click(&mut s, Vec2::new(600.0, 600.0));
    assert_eq!(s.state.brush.color, Rgba::rgb(0.2, 0.3, 0.4));
    assert_eq!(s.state.brush.opacity, 1.0);

    // A transparent background leaves the colour alone.
    s.document.background = None;
    s.state.brush.color = BLUE;
    click(&mut s, Vec2::new(600.0, 600.0));
    assert_eq!(s.state.brush.color, BLUE);
}

#[test]
fn works_on_a_locked_layer() {
    let mut s = square_scene();
    let layer = s.state.active_layer.expect("a layer");
    s.document.layer_mut(layer).expect("layer").locked = true;
    s.state.tool = Tool::Eyedropper;
    click(&mut s, Vec2::new(150.0, 150.0));
    assert_eq!(s.state.brush.color, GREEN);
}

#[test]
fn gradient_fill_picks_the_colour_at_the_point() {
    let mut s = Session::new();
    s.state.tool = Tool::Rectangle;
    let style = &mut s.tools.shape_style;
    style.stroke = false;
    style.fill_color = RED;
    style.gradient = true;
    style.gradient_color = BLUE;
    style.gradient_direction = GradientDirection::LeftToRight;
    assert!(s.pointer_down(Vec2::new(100.0, 100.0), 1.0));
    s.pointer_move(Vec2::new(300.0, 200.0));
    s.pointer_up();

    s.state.tool = Tool::Eyedropper;
    click(&mut s, Vec2::new(150.0, 150.0));
    let c = s.state.brush.color;
    assert!(
        (c.r - 0.75).abs() < 1e-4 && (c.b - 0.25).abs() < 1e-4,
        "{c:?}"
    );
}

#[test]
fn alt_click_with_brush_and_bucket_picks_without_switching_tools() {
    for tool in [Tool::Brush, Tool::Fill] {
        let mut s = square_scene();
        let undo = s.history.undo_len();
        s.state.tool = tool;
        s.tools.set_modifiers(false, true);
        click(&mut s, Vec2::new(150.0, 100.0));
        assert_eq!(s.state.tool, tool);
        assert_eq!(s.state.brush.color, RED, "{tool:?}");
        assert_eq!(s.history.undo_len(), undo, "{tool:?} drew or filled");
    }
}

#[test]
fn alt_click_with_a_shape_tool_picks_into_its_colours() {
    let mut s = square_scene();
    let undo = s.history.undo_len();
    s.state.tool = Tool::Ellipse;
    s.tools.set_modifiers(false, true);
    click(&mut s, Vec2::new(150.0, 100.0));
    click(&mut s, Vec2::new(150.0, 150.0));
    assert_eq!(s.state.tool, Tool::Ellipse);
    let style = s.tools.shape_style;
    assert_eq!(style.stroke_color, Rgba::new(1.0, 0.0, 0.0, 0.8));
    assert_eq!(style.fill_color, Rgba::new(0.0, 1.0, 0.0, 0.6));
    assert_eq!(s.history.undo_len(), undo);

    // An Alt-drag still draws from the centre.
    assert!(s.pointer_down(Vec2::new(500.0, 500.0), 1.0));
    s.pointer_move(Vec2::new(550.0, 540.0));
    s.pointer_up();
    assert_eq!(s.history.undo_len(), undo + 1);
}

#[test]
fn plain_click_with_the_bucket_still_fills() {
    let mut s = square_scene();
    let undo = s.history.undo_len();
    s.state.tool = Tool::Fill;
    s.tools.set_modifiers(false, false);
    click(&mut s, Vec2::new(150.0, 150.0));
    assert_eq!(s.history.undo_len(), undo + 1, "recoloured the fill");
}
