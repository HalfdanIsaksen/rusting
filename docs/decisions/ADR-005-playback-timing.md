# ADR-005: Playback timing

**Status:** Accepted (Phase 0, implemented in `feat/timeline-engine`)

## Decision
Playback derives the frame from elapsed wall-clock time, not from render iterations:
`frame = start + floor(elapsed_secs * fps)`, wrapped by the loop range.

- If rendering is slower than the document FPS, frames are **dropped** to keep sync with real time (and audio, later). The UI can show the effective FPS.
- If rendering is faster, the same frame is shown again. The app requests redraws only while playing.
- The clock takes an injected time source, so it is deterministic in tests.
