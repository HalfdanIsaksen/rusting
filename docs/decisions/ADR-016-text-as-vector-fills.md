# ADR-016: Text becomes vector fills

**Status:** Proposed (text-tool branch)

## Context
Users want to put words on the canvas (titles, speech, labels) with a Text tool. Everything in a frame is a stroke or a fill today, and every tool (select, lasso, transform, eraser, bucket, eyedropper, onion skin, export) works on those two kinds. A text item would need a font, a layout and a renderer path of its own.

## Options
| | Text item in the document (editable) | Glyph outlines as fills (chosen) | Glyphs rasterized into an image |
|---|---|---|---|
| Edit the words later | Yes | No (retype it) | No |
| Data / file format change | New item kind, format bump, every tool and the renderer learn it | None: an ordinary `Fill` | Needs image items |
| Select, move, transform, erase, export | Each must learn text | Already work | Transform blurs; erase is per pixel |
| Looks the same on every machine | Only if the font is in the file | Yes: the outlines are the data | Yes, at one resolution |
| Scales without blur | Yes | Yes | No |

## Decision
- **Geometry.** `drawing::text::Font` parses a TrueType/OpenType font with `ttf-parser` (pure Rust, no egui/winit/wgpu, so the core-crate rule holds). `Font::layout(text, size, origin)` returns a `TextLayout` with a `FillShape` of every glyph outline in document coordinates, the caret position and the line height. Lines break at `\n` and are left-aligned; glyphs advance by their horizontal advance. `size` is the em height in document units.
- **Curves.** Quadratic and cubic segments are flattened adaptively so the polygon stays within about `size / 1000` units (at least 0.005) of the true outline, up to 64 segments per curve.
- **Holes.** Font winding conventions differ (TrueType and CFF wind opposite ways). Each glyph's contours are re-oriented by nesting depth: a contour inside an even number of the glyph's other contours is an outer boundary (positive signed area), inside an odd number a hole (negative). That is what `FillShape` and `tessellate_fill` expect, so the counters of "o", "A", "B" and "8" stay open, and the even-odd `contains` used by hit-testing agrees.
- **One fill per text block.** The whole typed block becomes one `Fill`. It then selects, moves, transforms and erases as one item, which is what a user expects of a word; per-glyph fills would make a click pick one letter and a marquee pick dozens. The partial eraser can still split it into pieces. One fill with many contours triangulates fine: holes are assigned to the smallest outer contour that contains them.
- **Font.** Ubuntu Light, taken from `epaint_default_fonts` (the font crate egui already ships, same version as egui, so no new download and no runtime fetch). It is licensed under the Ubuntu Font Licence 1.0, which allows embedding and redistribution with the software; the licence text ships in that crate (`fonts/UFL.txt`). Only `ui` names the font (`ui::tools::TEXT_FONT`); `drawing` takes font bytes and only uses the font crate in its tests.
- **Command.** No new command: the commit is `AddFill` (which creates a drawing key on the frame when needed, undoes exactly and refuses locked layers) wrapped in a `Batch` labelled "Text", so it is one undo step. Placement follows the tool's draw order like the other tools (ADR-010, ADR-011). Colour and opacity are the brush colour and opacity, shared with the brush and the paint bucket.
- **No same-colour merge.** "Merge same colour" (ADR-012) re-traces merged shapes on a 0.5-unit grid, which would blur small glyphs, so text is never merged.
- **Typing.** The text tool keeps an insertion point (layer, frame, origin, text) outside the document, with a live preview fill and a caret. While it exists the keyboard goes to the text: the shortcut handler feeds `Text`/`Paste` events to it and drops every key event, and the canvas ignores Space-pan and `F`. Enter adds a line, Backspace deletes, Esc cancels; Cmd/Ctrl+Enter, a click elsewhere, another tool, another layer or frame, undo/redo and save commit.

## Consequences
- Committed text cannot be edited as text. Retype it, or undo and type again.
- One font, one weight, no kerning (Ubuntu Light keeps its kerning in GPOS, which is not applied), no shaping (ligatures, right-to-left and complex scripts), no wrapping or alignment. Characters the font lacks draw as its missing-glyph box.
- The caret is always at the end of the text; there is no cursor movement or selection inside the text being typed.
- Text adds no data model or file format change; files with text open in older builds.
- Overlapping contours inside one glyph (rare in static fonts like this one) would paint their overlap twice at partial opacity.
