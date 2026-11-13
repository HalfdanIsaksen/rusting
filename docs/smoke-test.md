# Manual smoke test

Run with `cargo run -p desktop`. Items marked *(MVP)* need features that aren't merged yet.

1. App launches without errors in the log. A window titled "Rusting" opens.
2. Resizing the window works. Minimising and restoring doesn't crash.
3. *(MVP)* The canvas shows its bounds and background. Zooming and panning keep content under the cursor.
4. *(MVP)* Drawing with the brush leaves strokes that survive zoom, pan, and resize.
5. *(MVP)* Add three layers. Drawing on one doesn't change the others. Hiding a layer hides its strokes. Reordering changes the stacking.
6. *(MVP)* A locked layer refuses new strokes.
7. *(MVP)* Add keyframes at frames 1, 5, and 9 with different drawings. Scrubbing shows the right drawing on each frame. Dragging a key in the timeline moves it; dropping on another key is refused.
8. *(MVP)* Play at 12 and 24 fps. Timing looks right and playback loops.
9. *(MVP)* Onion skin shows neighbouring drawings and doesn't change them.
10. *(MVP)* Undo and redo a stroke, a layer add or remove, and a keyframe add.
11. *(MVP)* Save, quit, and reopen. Everything above is preserved. Opening a corrupt file shows an error, not a crash.
12. Fill bucket (`K`): draw a closed shape and click inside it with Fill; it fills with the brush colour, under the lines, with no white edge. Draw a shape with a small gap: "No gaps" says "Area not closed — try a larger gap setting"; "Small gaps" fills it. Clicking a fill with another colour recolours it. Undo/redo, save and reopen keep fills.
13. Layer opacity: drag a layer's opacity field down to 30%; its strokes fade live while dragging. One undo brings back 100%. Type 55 into the field and press Enter.
14. Layer groups: select two layers and press the folder (group) button. A "Group 1" header appears above them, indented. Hiding, locking or fading the group affects both layers (drawing on a member of a locked group is refused). Fold and unfold with the arrow. Drag a third layer between the group's rows to add it, drag a member above the header to take it out, and drag the header to move the whole group. Right-click the header → Ungroup keeps the layers. Undo/redo, save and reopen keep groups.
15. Text (`T`): click the canvas and type "Hello, world"; the letters appear in the brush colour with a caret after them, and `B`, `E`, Space and `F` type characters instead of switching tools, playing or fitting. Backspace deletes, Enter starts a second line, `Cmd/Ctrl+Enter` finishes. One undo removes the whole text (and the key, if the frame had none). The "o" and "e" have open holes. Select the text with a click or a box: it moves, scales and rotates as one item, and the eraser cuts it. Change the font size and set the draw order to Behind to type under an existing rectangle. Esc while typing leaves nothing. Typing on a locked layer is refused. Save, reopen and export keep the text.
