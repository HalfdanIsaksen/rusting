# Manual smoke test

Run with `cargo run -p desktop`. Items marked *(MVP)* need features that aren't merged yet.

1. App launches without errors in the log. A window titled "Rusting" opens.
2. Resizing the window works. Minimising and restoring doesn't crash.
3. *(MVP)* The canvas shows its bounds and background. Zooming and panning keep content under the cursor.
4. *(MVP)* Drawing with the brush leaves strokes that survive zoom, pan, and resize.
5. *(MVP)* Add three layers. Drawing on one doesn't change the others. Hiding a layer hides its strokes. Reordering changes the stacking.
6. *(MVP)* A locked layer refuses new strokes.
7. *(MVP)* Add keyframes at frames 1, 5, and 9 with different drawings. Scrubbing shows the right drawing on each frame.
8. *(MVP)* Play at 12 and 24 fps. Timing looks right and playback loops.
9. *(MVP)* Onion skin shows neighbouring drawings and doesn't change them.
10. *(MVP)* Undo and redo a stroke, a layer add or remove, and a keyframe add.
11. *(MVP)* Save, quit, and reopen. Everything above is preserved. Opening a corrupt file shows an error, not a crash.
