//! A long editing session through `History`, saved to disk and reloaded.
//! Every frame must evaluate identically, and undo/redo must stay valid after
//! saving (saving does not garbage-collect the in-memory document).

use document::animation_core::{Fps, FrameIndex, Vec2};
use document::drawing::{BrushStyle, Stroke, StrokePoint};
use document::{CanvasSize, Document, RenderList};
use editor::commands::{
    AddKeyframe, AddLayer, AddStroke, KeyframeKind, MoveLayer, RemoveKeyframe, RemoveLayer,
    RemoveStroke, RenameLayer, SetFps, SetFrameCount, SetLayerLocked, SetLayerVisible,
};
use editor::{Command, CommandError, History};

/// A deterministic squiggle so strokes differ from each other.
fn stroke(seed: u32) -> Stroke {
    let s = seed as f32;
    let points = (0..8)
        .map(|i| {
            let t = i as f32;
            StrokePoint {
                pos: Vec2::new(10.0 + s * 3.0 + t * 5.0, 20.0 + (t + s).sin() * 15.0),
                pressure: 0.25 + 0.75 * ((t + s) * 0.37).fract(),
            }
        })
        .collect();
    Stroke::new(
        points,
        BrushStyle {
            width: 1.0 + (seed % 5) as f32,
            ..BrushStyle::default()
        },
    )
}

fn run(history: &mut History, doc: &mut Document, cmd: impl Command + 'static) {
    let label = format!("{cmd:?}");
    history
        .execute(doc, Box::new(cmd))
        .unwrap_or_else(|e| panic!("{label}: {e}"));
}

/// Owned summary of a render list, so documents can be compared after `doc`
/// is mutated.
fn snapshot(list: &RenderList<'_>) -> String {
    format!("{list:?}")
}

fn all_frames(doc: &Document) -> Vec<String> {
    (0..doc.frame_count)
        .map(|f| snapshot(&doc.evaluate(FrameIndex(f))))
        .collect()
}

fn assert_same_on_every_frame(a: &Document, b: &Document) {
    assert_eq!(a.frame_count, b.frame_count);
    for f in 0..a.frame_count {
        assert_eq!(
            a.evaluate(FrameIndex(f)),
            b.evaluate(FrameIndex(f)),
            "frame {}",
            FrameIndex(f)
        );
    }
}

#[test]
fn long_session_survives_save_and_load() {
    let mut doc = Document::new(
        "Session",
        CanvasSize {
            width: 1280,
            height: 720,
        },
    );
    let mut h = History::new();

    run(&mut h, &mut doc, SetFrameCount::new(48));
    run(&mut h, &mut doc, SetFps::new(Fps::whole(12).expect("fps")));

    let layers: Vec<_> = ["Background", "Character", "Effects", "Scratch"]
        .into_iter()
        .map(|name| {
            let cmd = AddLayer::new(name);
            let id = cmd.layer_id();
            run(&mut h, &mut doc, cmd);
            id
        })
        .collect();
    let (bg, character, fx, scratch) = (layers[0], layers[1], layers[2], layers[3]);

    // Background: one held drawing with several strokes.
    let mut seed = 0;
    for _ in 0..5 {
        seed += 1;
        run(
            &mut h,
            &mut doc,
            AddStroke::new(bg, FrameIndex(0), stroke(seed)),
        );
    }

    // Character: a new drawing every 4 frames (strokes create the keys).
    let mut erased = None;
    for frame in (0..48).step_by(4) {
        for _ in 0..3 {
            seed += 1;
            let cmd = AddStroke::new(character, FrameIndex(frame), stroke(seed));
            if frame == 8 && erased.is_none() {
                erased = Some(cmd.stroke_id());
            }
            run(&mut h, &mut doc, cmd);
        }
    }
    let erased = erased.expect("stroke on frame 8");
    run(
        &mut h,
        &mut doc,
        RemoveStroke::new(character, FrameIndex(10), erased),
    );

    // Effects: explicit keys, an empty gap, a blank drawing filled later.
    run(
        &mut h,
        &mut doc,
        AddKeyframe::new(fx, FrameIndex(6), KeyframeKind::Drawing),
    );
    for _ in 0..2 {
        seed += 1;
        run(
            &mut h,
            &mut doc,
            AddStroke::new(fx, FrameIndex(7), stroke(seed)),
        );
    }
    run(
        &mut h,
        &mut doc,
        AddKeyframe::new(fx, FrameIndex(12), KeyframeKind::Empty),
    );
    run(
        &mut h,
        &mut doc,
        AddKeyframe::new(fx, FrameIndex(20), KeyframeKind::Drawing),
    );
    seed += 1;
    run(
        &mut h,
        &mut doc,
        AddStroke::new(fx, FrameIndex(30), stroke(seed)),
    );
    run(
        &mut h,
        &mut doc,
        AddKeyframe::new(fx, FrameIndex(40), KeyframeKind::Drawing),
    );
    // Leaves an unreferenced drawing in the pool (needed for undo).
    run(&mut h, &mut doc, RemoveKeyframe::new(fx, FrameIndex(40)));

    // Layer properties and order.
    run(&mut h, &mut doc, RenameLayer::new(fx, "FX"));
    run(&mut h, &mut doc, MoveLayer::new(fx, 0));
    run(&mut h, &mut doc, SetLayerVisible::new(bg, false));
    run(&mut h, &mut doc, SetLayerLocked::new(character, true));
    assert_eq!(
        h.execute(
            &mut doc,
            Box::new(AddStroke::new(character, FrameIndex(0), stroke(999)))
        ),
        Err(CommandError::LayerLocked)
    );

    // A layer that is drawn on, then removed (its drawing stays for undo).
    seed += 1;
    run(
        &mut h,
        &mut doc,
        AddStroke::new(scratch, FrameIndex(3), stroke(seed)),
    );
    run(&mut h, &mut doc, RemoveLayer::new(scratch));

    // Some undo/redo churn in the middle of the session.
    h.undo(&mut doc).expect("undo");
    h.undo(&mut doc).expect("undo");
    h.redo(&mut doc).expect("redo");
    h.redo(&mut doc).expect("redo");

    assert!(doc.has_unreferenced_drawings());
    let rendered_before_save = all_frames(&doc);
    assert!(
        rendered_before_save.iter().any(|f| f.contains("pressure")),
        "session should draw something"
    );

    // Save, reload.
    let dir = std::env::temp_dir().join(format!("rusting-session-{}", doc.id));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("session.rusting");
    file_format::save(&doc, &path).expect("save");
    h.mark_saved();
    assert!(!h.is_dirty());
    let (loaded, report) = file_format::load_with_report(&path).expect("load");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(report.is_clean(), "{report:?}");

    // Same picture on every frame; only unreferenced drawings differ.
    assert_same_on_every_frame(&doc, &loaded);
    assert!(!loaded.has_unreferenced_drawings());
    let mut collected = doc.clone();
    collected.garbage_collect_drawings();
    assert_eq!(loaded, collected);

    // Saving did not break undo: rewind everything, then replay it.
    let replayed_frames = {
        let mut rewound = doc.clone();
        while h.undo(&mut rewound).expect("undo after save") {}
        assert!(rewound.layers().is_empty());
        assert!(rewound.drawings().is_empty());
        assert!(h.is_dirty());
        while h.redo(&mut rewound).expect("redo after save") {}
        assert!(!h.is_dirty());
        assert_eq!(rewound, doc);
        all_frames(&rewound)
    };
    assert_eq!(replayed_frames, rendered_before_save);

    // A loaded document saves to the same bytes again.
    assert_eq!(
        file_format::to_string(&loaded).expect("serialize"),
        file_format::to_string(&doc).expect("serialize")
    );
}
