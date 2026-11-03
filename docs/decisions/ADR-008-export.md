# ADR-008: Export (PNG/JPG, image sequences, MP4)

**Status:** Proposed (export branch)

## Context
Users need to get their animation out of Rusting: one frame as an image, every frame as numbered images, and the animation as a video. Exports must look exactly like the canvas (MSAA, layer opacity, hidden layers left out) but at document resolution, not viewport size. Users should not have to install anything, and long exports must not freeze the editor.

## Options for MP4
| | OpenH264 (bundled) + pure-Rust muxer | `ffmpeg` on PATH | Platform encoders (AVFoundation, Media Foundation) |
|---|---|---|---|
| User installs something | No: `openh264` builds Cisco's C++ source with `cc` | Yes, and we must detect a missing binary | No |
| Build needs | A C/C++ compiler (already needed by other crates) | Nothing | Per-OS code and `unsafe` FFI |
| Codec quality | Constrained Baseline H.264: fine for flat-colour animation | Best (x264, any codec) | Good |
| Cross-platform | Yes | Yes, if installed | Separate code per OS |
| Licensing | BSD-2 source. Cisco's patent licence covers only their prebuilt binary, so a shipped build may need a review | Depends on the user's ffmpeg build | OS-provided |

## Decision
- **New GPU-free crate `export`** (depends on nothing in the workspace): `RgbaImage` (straight-alpha sRGB RGBA8), `write_image` (PNG/JPEG via `image`), `flatten`, `sequence_path` (`name_0001.png`), `yuv::rgba_to_yuv420`, `Mp4Encoder`, and `ExportWorker` (a thread that writes frames as they arrive).
- **MP4: `openh264` (feature `source`) + `mp4` crate.** Frames are padded to even sizes by repeating the last column or row, converted to limited-range **BT.709** I420 (signalled in the stream's VUI), and encoded with frame skipping off (so every frame gets a sample) and a keyframe every ~2 s. Annex B output is split into NAL units: SPS/PPS go into the `avcC` box, and the rest are written as length-prefixed samples. The track and movie timescale is `fps.num × ceil(1000 / fps.num)`, with a per-frame duration of `fps.den` scaled the same way, so 24000/1001 and whole rates are exact. The file is deleted if the encoder is dropped before `finish` (cancel, error, or app quit).
- **ffmpeg fallback: not needed.** The bundled route builds and works on macOS (verified with the system decoder via `avmediainfo` and QuickLook). If it ever fails to build on a target, piping raw frames to `ffmpeg` is the fallback.
- **Rendering (`renderer`):** `OffscreenRenderer` has its own `CanvasRenderer` (own target, uniform buffers and mesh cache, so it never disturbs the editor canvas). It renders with a viewport that maps the canvas onto the whole target at `canvas × scale`, without the workspace, shadow, outline or checkerboard (`Chrome::Bare`, clear to transparent). Then it copies the resolved texture into a buffer with rows padded to 256 bytes, waits for the map, and strips the padding.
- **Colour:** blending happens in linear space into an `Rgba8UnormSrgb` target, exactly like the canvas. The read-back bytes are therefore sRGB-encoded, which is what PNG and JPEG expect. On a transparent clear they are *premultiplied*, so `unpremultiply_srgb` decodes, divides by alpha and re-encodes. Opaque pixels are unchanged. JPEG and MP4 always render onto the document background (white if the document has none), so their alpha is 1. PNG renders transparent only if the user ticks "Transparent".
- **Same content as the canvas:** `renderer::DrawListBuilder` (mesh caching plus `stroke_color` and `fill_color`) replaces the item-building code that was in `apps/desktop/src/main.rs`. The canvas adds onion skins, erase and move previews and the stroke in progress on top of the same rules. Export calls `DrawListBuilder::render_list(&doc.evaluate(frame))`.
- **UX (`apps/desktop/src/export.rs`):** File → Export → Frame / Image Sequence / Video sends `ui::Request::Export(kind)`. The desktop shows an options modal (format, scale 25/50/100/200 % or custom, frame range, JPG quality, PNG transparency), then an `rfd` save dialog. A job snapshots the document, renders frames on the main thread within a 25 ms budget per UI frame, and sends them to the worker. At most 4 frames are queued, so memory stays bounded. A non-modal progress window has a Cancel button. Errors appear in the existing error dialog, and success appears in the status bar.

## Consequences
- Builds compile OpenH264's C++ source (adds ~20 s to a clean build).
- Baseline profile only, with no B-frames; files are somewhat larger than x264 would make at the same quality.
- MP4 dimensions are limited to 65535 per side (the `mp4` crate stores them as u16). Odd sizes gain one pixel.
- MOV export is the same H.264 stream and boxes with a QuickTime `ftyp` brand (`qt  `), chosen by the `.mov` extension. No alpha.
- Rendering waits for the GPU on each export frame, which is fine at a few frames per UI frame.
- OpenH264 prints a harmless warning to stderr ("bitrate can't be controlled … without enabling skip frame"), because frame skipping is off on purpose.
