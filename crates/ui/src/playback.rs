//! Playback state: a [`PlaybackClock`] kept in sync with the document.

use document::Document;
use document::animation_core::{Fps, FrameIndex, FrameRange};
use document::timeline::{PlaybackClock, SystemTimeSource, TimeSource};

/// Plays the document's frames in real time (ADR-005).
#[derive(Clone, Debug)]
pub struct Playback<T: TimeSource = SystemTimeSource> {
    clock: PlaybackClock<T>,
    /// Frames dropped since playback started, for the status bar.
    dropped: u64,
}

impl Playback<SystemTimeSource> {
    /// A paused, looping clock for `doc` using the system clock.
    pub fn new(doc: &Document) -> Self {
        Self::with_time(SystemTimeSource::new(), doc)
    }
}

impl<T: TimeSource> Playback<T> {
    /// A paused, looping clock for `doc` using `time`.
    pub fn with_time(time: T, doc: &Document) -> Self {
        Self {
            clock: PlaybackClock::new(time, doc.fps, range_of(doc)),
            dropped: 0,
        }
    }

    /// Picks up frame-rate and length changes from `doc`.
    pub fn sync(&mut self, doc: &Document) {
        if self.clock.fps() != doc.fps {
            self.clock.set_fps(doc.fps);
        }
        let range = range_of(doc);
        if self.clock.range() != range {
            self.clock.set_range(range);
        }
    }

    /// Whether playback is running.
    pub fn is_playing(&self) -> bool {
        self.clock.is_playing()
    }

    /// Whether playback wraps around at the end.
    pub fn looping(&self) -> bool {
        self.clock.looping()
    }

    /// Turns looping on or off.
    pub fn set_looping(&mut self, looping: bool) {
        self.clock.set_looping(looping);
    }

    /// The clock's frame rate.
    pub fn fps(&self) -> Fps {
        self.clock.fps()
    }

    /// Frames dropped since playback started (rendering was too slow).
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Plays from `from` if paused, pauses if playing.
    pub fn toggle(&mut self, from: FrameIndex) {
        if self.clock.is_playing() {
            self.clock.pause();
        } else {
            self.clock.seek(from);
            self.dropped = 0;
            self.clock.play();
        }
    }

    /// Stops playback, holding the current frame.
    pub fn pause(&mut self) {
        self.clock.pause();
    }

    /// Moves the clock to `frame`.
    pub fn seek(&mut self, frame: FrameIndex) {
        self.clock.seek(frame);
    }

    /// The frame to show now, or `None` when paused.
    pub fn tick(&mut self) -> Option<FrameIndex> {
        if !self.clock.is_playing() {
            return None;
        }
        let tick = self.clock.tick();
        self.dropped += tick.dropped;
        Some(tick.frame)
    }
}

fn range_of(doc: &Document) -> FrameRange {
    FrameRange::first_n(doc.frame_count.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use document::CanvasSize;
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

    #[test]
    fn plays_in_real_time_and_loops() {
        let time = ManualTimeSource::new();
        let d = doc(12, 24);
        let mut p = Playback::with_time(time.clone(), &d);
        assert_eq!(p.tick(), None);
        p.toggle(FrameIndex(0));
        assert!(p.is_playing());
        time.advance(Duration::from_millis(500));
        assert_eq!(p.tick(), Some(FrameIndex(6)));
        time.advance(Duration::from_secs(2));
        // 2.5 s at 12 fps = 30 frames, wrapped into 24.
        assert_eq!(p.tick(), Some(FrameIndex(6)));
        p.toggle(FrameIndex(0));
        assert!(!p.is_playing());
    }

    #[test]
    fn follows_document_fps_and_stops_at_end_without_loop() {
        let time = ManualTimeSource::new();
        let mut d = doc(12, 10);
        let mut p = Playback::with_time(time.clone(), &d);
        d.fps = Fps::whole(24).unwrap();
        p.sync(&d);
        assert_eq!(p.fps(), d.fps);
        p.set_looping(false);
        p.toggle(FrameIndex(0));
        assert_eq!(p.tick(), Some(FrameIndex(0)));
        // A slow render: 5 frames pass, 4 of them are never shown.
        time.advance(Duration::from_millis(5 * 1000 / 24 + 1));
        assert_eq!(p.tick(), Some(FrameIndex(5)));
        assert_eq!(p.dropped(), 4);
        time.advance(Duration::from_secs(1));
        assert_eq!(p.tick(), Some(FrameIndex(9)));
        assert!(!p.is_playing());
    }
}
