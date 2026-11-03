//! The preview window's state: which frame it shows, and its own looping
//! playback that runs independently of the editor's playhead.
//!
//! While paused the preview follows the editor's current frame. Playing
//! loops through the whole document at its frame rate without moving the
//! editor's playhead; pausing goes back to following the editor.

use crate::playback::Playback;
use document::Document;
use document::animation_core::FrameIndex;
use document::timeline::{SystemTimeSource, TimeSource};

/// View → Preview: open state and the preview's own playback clock.
#[derive(Clone, Debug)]
pub struct Preview<T: TimeSource = SystemTimeSource> {
    open: bool,
    playback: Playback<T>,
    /// The frame the preview's playback is on, while playing.
    playing_frame: Option<FrameIndex>,
}

impl Preview<SystemTimeSource> {
    /// A closed, paused preview for `doc` using the system clock.
    pub fn new(doc: &Document) -> Self {
        Self::with_time(SystemTimeSource::new(), doc)
    }
}

impl<T: TimeSource> Preview<T> {
    /// A closed, paused preview for `doc` using `time`.
    pub fn with_time(time: T, doc: &Document) -> Self {
        let mut playback = Playback::with_time(time, doc);
        playback.set_looping(true);
        Self {
            open: false,
            playback,
            playing_frame: None,
        }
    }

    /// Whether the preview window is shown.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Shows or hides the preview window. Hiding it stops its playback.
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
        if !open {
            self.pause();
        }
    }

    /// Whether the preview is playing its own loop.
    pub fn is_playing(&self) -> bool {
        self.playback.is_playing()
    }

    /// Starts looping from `editor_frame`, or pauses if already playing.
    pub fn toggle(&mut self, doc: &Document, editor_frame: FrameIndex) {
        if self.is_playing() {
            self.pause();
        } else if self.open {
            self.playback.toggle(doc, editor_frame);
            self.playing_frame = Some(editor_frame);
        }
    }

    /// Stops playback; the preview goes back to following the editor.
    pub fn pause(&mut self) {
        self.playback.pause();
        self.playing_frame = None;
    }

    /// Advances the preview's playback. Call once per UI frame. Returns true
    /// while playing, so the caller keeps repainting.
    pub fn update(&mut self, doc: &Document) -> bool {
        if !self.open {
            self.pause();
            return false;
        }
        self.playback.sync(doc);
        if let Some(frame) = self.playback.tick() {
            self.playing_frame = Some(frame);
        }
        self.is_playing()
    }

    /// The frame to show: the preview's own while playing, else the
    /// editor's.
    pub fn frame(&self, editor_frame: FrameIndex) -> FrameIndex {
        self.playing_frame.unwrap_or(editor_frame)
    }

    /// Starts over for a new document, keeping the window open or closed.
    pub fn reset(&mut self, doc: &Document) {
        self.pause();
        self.playback.sync(doc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use document::CanvasSize;
    use document::animation_core::Fps;
    use document::timeline::ManualTimeSource;
    use std::time::Duration;

    fn doc(fps: u32, frames: u32) -> Document {
        let mut d = Document::new(
            "T",
            CanvasSize {
                width: 10,
                height: 10,
            },
        );
        d.fps = Fps::whole(fps).unwrap();
        d.frame_count = frames;
        d
    }

    fn open_preview(time: &ManualTimeSource, d: &Document) -> Preview<ManualTimeSource> {
        let mut p = Preview::with_time(time.clone(), d);
        p.set_open(true);
        p
    }

    #[test]
    fn paused_preview_follows_the_editor() {
        let time = ManualTimeSource::new();
        let d = doc(12, 24);
        let mut p = open_preview(&time, &d);
        assert!(!p.update(&d));
        assert_eq!(p.frame(FrameIndex(3)), FrameIndex(3));
        time.advance(Duration::from_secs(1));
        assert!(!p.update(&d));
        assert_eq!(p.frame(FrameIndex(7)), FrameIndex(7));
    }

    #[test]
    fn plays_from_the_editor_frame_and_loops() {
        let time = ManualTimeSource::new();
        let d = doc(12, 24);
        let mut p = open_preview(&time, &d);
        let editor = FrameIndex(20);
        p.toggle(&d, editor);
        assert!(p.update(&d));
        assert_eq!(p.frame(editor), FrameIndex(20));
        // 0.5 s at 12 fps = 6 frames: 20 + 6 wraps to 2.
        time.advance(Duration::from_millis(500));
        assert!(p.update(&d));
        assert_eq!(p.frame(editor), FrameIndex(2));
        // Several more loops later it is still playing.
        time.advance(Duration::from_secs(10));
        assert!(p.update(&d));
        assert_eq!(p.frame(editor), FrameIndex(2));
    }

    #[test]
    fn playing_ignores_editor_frame_changes() {
        let time = ManualTimeSource::new();
        let d = doc(10, 10);
        let mut p = open_preview(&time, &d);
        p.toggle(&d, FrameIndex(0));
        time.advance(Duration::from_millis(300));
        p.update(&d);
        // Whatever the editor is on, the preview shows its own frame.
        assert_eq!(p.frame(FrameIndex(0)), FrameIndex(3));
        assert_eq!(p.frame(FrameIndex(8)), FrameIndex(3));
    }

    #[test]
    fn pausing_returns_to_the_editor_frame() {
        let time = ManualTimeSource::new();
        let d = doc(10, 10);
        let mut p = open_preview(&time, &d);
        p.toggle(&d, FrameIndex(0));
        time.advance(Duration::from_millis(300));
        p.update(&d);
        p.toggle(&d, FrameIndex(5));
        assert!(!p.is_playing());
        time.advance(Duration::from_secs(1));
        assert!(!p.update(&d));
        assert_eq!(p.frame(FrameIndex(5)), FrameIndex(5));
    }

    #[test]
    fn closing_stops_playback_and_closed_preview_does_not_play() {
        let time = ManualTimeSource::new();
        let d = doc(10, 10);
        let mut p = open_preview(&time, &d);
        p.toggle(&d, FrameIndex(0));
        p.set_open(false);
        assert!(!p.is_playing());
        assert!(!p.update(&d));
        p.toggle(&d, FrameIndex(0));
        assert!(!p.is_playing());
        assert_eq!(p.frame(FrameIndex(4)), FrameIndex(4));
    }

    #[test]
    fn follows_document_fps_and_length() {
        let time = ManualTimeSource::new();
        let mut d = doc(10, 10);
        let mut p = open_preview(&time, &d);
        p.toggle(&d, FrameIndex(0));
        d.fps = Fps::whole(20).unwrap();
        d.frame_count = 4;
        p.update(&d);
        // 0.25 s at 20 fps = 5 frames, wrapped into 4.
        time.advance(Duration::from_millis(250));
        p.update(&d);
        assert_eq!(p.frame(FrameIndex(0)), FrameIndex(1));
    }
}
