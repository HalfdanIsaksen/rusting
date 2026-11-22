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
    /// Where playback started; picks the tag to loop while playing.
    from: FrameIndex,
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
            clock: PlaybackClock::new(time, doc.fps, range_of(doc, FrameIndex(0))),
            dropped: 0,
            from: FrameIndex(0),
        }
    }

    /// Picks up frame-rate, length and tag changes from `doc`. The play
    /// range is the tag holding the start frame (the current frame while
    /// paused), so playback loops inside it.
    pub fn sync(&mut self, doc: &Document) {
        if self.clock.fps() != doc.fps {
            self.clock.set_fps(doc.fps);
        }
        let at = if self.is_playing() {
            self.from
        } else {
            self.clock.current_frame()
        };
        let range = range_of(doc, at);
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

    /// Plays from `from` if paused (looping inside its tag, if any), pauses
    /// if playing.
    pub fn toggle(&mut self, doc: &Document, from: FrameIndex) {
        if self.clock.is_playing() {
            self.clock.pause();
        } else {
            self.clock.seek(from);
            self.from = from;
            self.sync(doc);
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

    /// The frames playback shows after `frame`, in order, at most `count`:
    /// through the end of the play range, then from its start again when
    /// looping. Never `frame` itself. Empty while paused.
    pub fn upcoming(&self, frame: FrameIndex, count: usize) -> Vec<FrameIndex> {
        let range = self.clock.range();
        let (start, end) = (range.start.0, range.end.0);
        if !self.is_playing() || start >= end {
            return Vec::new();
        }
        let len = (end - start) as usize;
        let mut out = Vec::new();
        let mut next = frame.0;
        while out.len() < count.min(len) {
            next = next.saturating_add(1);
            if next < start || next >= end {
                if !self.looping() {
                    break;
                }
                next = start;
            }
            if next == frame.0 {
                break;
            }
            out.push(FrameIndex(next));
        }
        out
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

/// A change in playback that sound has to follow (ADR-020). The host gets
/// one per drawn frame from [`crate::Session::playback_event`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackEvent {
    /// Playback started at this frame.
    Started(FrameIndex),
    /// While playing, the playhead jumped to this frame (a loop wrapped,
    /// or the user moved the playhead).
    Seeked(FrameIndex),
    /// Playback stopped.
    Stopped,
    /// While paused, the playhead moved to this frame (scrubbing or
    /// stepping).
    Scrubbed(FrameIndex),
}

/// Turns the playing flag and the current frame, sampled once per drawn
/// frame, into [`PlaybackEvent`]s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlaybackWatch {
    last: Option<(bool, FrameIndex)>,
}

impl PlaybackWatch {
    /// The event since the last call, given whether playback runs now and
    /// the frame shown. Moving forward by up to a quarter second (at least
    /// two frames) between calls is normal playback; anything else while
    /// playing is a seek.
    pub fn update(&mut self, playing: bool, frame: FrameIndex, fps: Fps) -> Option<PlaybackEvent> {
        let last = self.last.replace((playing, frame));
        let Some((was_playing, was)) = last else {
            return playing.then_some(PlaybackEvent::Started(frame));
        };
        match (was_playing, playing) {
            (false, true) => Some(PlaybackEvent::Started(frame)),
            (true, false) => Some(PlaybackEvent::Stopped),
            (true, true) => {
                let max_step = (fps.as_f64() * 0.25).ceil().max(2.0) as u32;
                (frame < was || frame.0 - was.0 > max_step).then_some(PlaybackEvent::Seeked(frame))
            }
            (false, false) => (frame != was).then_some(PlaybackEvent::Scrubbed(frame)),
        }
    }

    /// Forgets the last state, e.g. when another document is opened.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

fn range_of(doc: &Document, frame: FrameIndex) -> FrameRange {
    let all = FrameRange::first_n(doc.frame_count.max(1));
    doc.tag_at(frame).map_or(all, |t| {
        FrameRange::new(t.range.start.min(all.end), t.range.end.min(all.end))
    })
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
    fn watch_reports_start_stop_seek_and_scrub() {
        use PlaybackEvent::*;
        let fps = Fps::whole(24).unwrap();
        let mut w = PlaybackWatch::default();
        let mut at = |playing, f| w.update(playing, FrameIndex(f), fps);
        assert_eq!(at(false, 0), None);
        assert_eq!(at(false, 0), None);
        assert_eq!(at(false, 3), Some(Scrubbed(FrameIndex(3))));
        assert_eq!(at(true, 3), Some(Started(FrameIndex(3))));
        assert_eq!(at(true, 4), None);
        assert_eq!(at(true, 10), None, "6 frames is a quarter second");
        assert_eq!(at(true, 17), Some(Seeked(FrameIndex(17))));
        assert_eq!(at(true, 0), Some(Seeked(FrameIndex(0))), "loop wrapped");
        assert_eq!(at(false, 0), Some(Stopped));
        let mut fresh = PlaybackWatch::default();
        assert_eq!(
            fresh.update(true, FrameIndex(5), fps),
            Some(Started(FrameIndex(5)))
        );
        fresh.reset();
        assert_eq!(fresh.update(false, FrameIndex(9), fps), None);
    }

    #[test]
    fn plays_in_real_time_and_loops() {
        let time = ManualTimeSource::new();
        let d = doc(12, 24);
        let mut p = Playback::with_time(time.clone(), &d);
        assert_eq!(p.tick(), None);
        p.toggle(&d, FrameIndex(0));
        assert!(p.is_playing());
        time.advance(Duration::from_millis(500));
        assert_eq!(p.tick(), Some(FrameIndex(6)));
        time.advance(Duration::from_secs(2));
        // 2.5 s at 12 fps = 30 frames, wrapped into 24.
        assert_eq!(p.tick(), Some(FrameIndex(6)));
        p.toggle(&d, FrameIndex(0));
        assert!(!p.is_playing());
    }

    #[test]
    fn loops_inside_the_tag_it_starts_in() {
        let time = ManualTimeSource::new();
        let mut d = doc(12, 24);
        d.tags.push(document::FrameTag {
            name: "walk".into(),
            range: FrameRange::new(FrameIndex(4), FrameIndex(8)),
        });
        let mut p = Playback::with_time(time.clone(), &d);
        p.toggle(&d, FrameIndex(5));
        p.sync(&d);
        time.advance(Duration::from_millis(250)); // 3 frames: 5 → 8, wraps to 4
        assert_eq!(p.tick(), Some(FrameIndex(4)));
        p.toggle(&d, FrameIndex(0));
        // Outside any tag the whole animation plays, through the tag.
        p.toggle(&d, FrameIndex(2));
        p.sync(&d);
        time.advance(Duration::from_millis(500));
        assert_eq!(p.tick(), Some(FrameIndex(8)));
    }

    #[test]
    fn upcoming_frames_follow_the_play_range() {
        let time = ManualTimeSource::new();
        let mut d = doc(12, 6);
        let mut p = Playback::with_time(time.clone(), &d);
        let frames = |v: &[u32]| v.iter().map(|&f| FrameIndex(f)).collect::<Vec<_>>();
        assert!(p.upcoming(FrameIndex(0), 3).is_empty(), "paused");
        p.toggle(&d, FrameIndex(0));
        assert_eq!(p.upcoming(FrameIndex(1), 3), frames(&[2, 3, 4]));
        // Wraps when looping, and never comes back round to the frame shown.
        assert_eq!(p.upcoming(FrameIndex(4), 3), frames(&[5, 0, 1]));
        assert_eq!(p.upcoming(FrameIndex(2), 99), frames(&[3, 4, 5, 0, 1]));
        p.set_looping(false);
        assert_eq!(p.upcoming(FrameIndex(4), 3), frames(&[5]));
        // Inside a tag, only the tag's frames.
        p.toggle(&d, FrameIndex(0));
        p.set_looping(true);
        d.tags.push(document::FrameTag {
            name: "walk".into(),
            range: FrameRange::new(FrameIndex(1), FrameIndex(3)),
        });
        p.toggle(&d, FrameIndex(1));
        assert_eq!(p.upcoming(FrameIndex(2), 3), frames(&[1]));
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
        p.toggle(&d, FrameIndex(0));
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
