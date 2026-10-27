//! Wall-clock driven playback (ADR-005).
//!
//! The shown frame is derived from elapsed time, never from how often the app
//! renders: `frame = start + floor(elapsed * fps)`, wrapped into the play
//! range. A slow renderer therefore skips frames to stay in sync, and a fast
//! one shows the same frame again.
//!
//! Time comes from a [`TimeSource`], so tests can drive the clock by hand.

use animation_core::{Fps, FrameIndex, FrameRange};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A monotonic clock. `now` is the time since an arbitrary fixed origin.
pub trait TimeSource {
    /// The current time. Must never go backwards.
    fn now(&self) -> Duration;
}

/// The real clock, backed by [`std::time::Instant`].
#[derive(Clone, Copy, Debug)]
pub struct SystemTimeSource {
    origin: Instant,
}

impl SystemTimeSource {
    /// A clock whose origin is the moment it was created.
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemTimeSource {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeSource for SystemTimeSource {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// A clock that only moves when told to. Clones share the same time, so a
/// test can keep a handle while the [`PlaybackClock`] owns another.
#[derive(Clone, Debug, Default)]
pub struct ManualTimeSource {
    nanos: Arc<AtomicU64>,
}

impl ManualTimeSource {
    /// A manual clock at time zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves time forward by `by`.
    pub fn advance(&self, by: Duration) {
        let by = u64::try_from(by.as_nanos()).unwrap_or(u64::MAX);
        // Saturate so a huge step cannot wrap the clock around.
        let mut current = self.nanos.load(Ordering::Relaxed);
        while let Err(actual) = self.nanos.compare_exchange_weak(
            current,
            current.saturating_add(by),
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            current = actual;
        }
    }
}

impl TimeSource for ManualTimeSource {
    fn now(&self) -> Duration {
        Duration::from_nanos(self.nanos.load(Ordering::Relaxed))
    }
}

/// The result of one [`PlaybackClock::tick`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tick {
    /// The frame to show now.
    pub frame: FrameIndex,
    /// Frames skipped since the previous tick because rendering was too slow
    /// to show each one. Zero on the first tick after play, seek or a setting
    /// change.
    pub dropped: u64,
    /// Whether a non-looping playback reached the end of its range on this
    /// tick. The clock pauses itself when this happens.
    pub finished: bool,
}

/// Turns elapsed wall-clock time into the frame to show.
///
/// Typical use: call [`PlaybackClock::tick`] once per redraw while
/// [`PlaybackClock::is_playing`], and request another redraw while it stays
/// playing. Scrubbing calls [`PlaybackClock::seek`].
#[derive(Clone, Debug)]
pub struct PlaybackClock<T: TimeSource = SystemTimeSource> {
    time: T,
    fps: Fps,
    range: FrameRange,
    looping: bool,
    /// The frame shown at zero elapsed time.
    start_frame: FrameIndex,
    /// Play time accumulated before the current play stretch.
    elapsed_before: Duration,
    /// When the current play stretch began, or `None` while paused.
    started_at: Option<Duration>,
    /// Elapsed frame count at the last tick, for counting dropped frames.
    last_tick: Option<u64>,
}

impl<T: TimeSource> PlaybackClock<T> {
    /// A paused clock at the start of `range`, looping by default.
    pub fn new(time: T, fps: Fps, range: FrameRange) -> Self {
        Self {
            time,
            fps,
            range,
            looping: true,
            start_frame: range.start,
            elapsed_before: Duration::ZERO,
            started_at: None,
            last_tick: None,
        }
    }

    /// The time source this clock reads.
    pub fn time_source(&self) -> &T {
        &self.time
    }

    /// The playback frame rate.
    pub fn fps(&self) -> Fps {
        self.fps
    }

    /// The frames playback stays within.
    pub fn range(&self) -> FrameRange {
        self.range
    }

    /// Whether playback wraps to the range start after the last frame.
    pub fn looping(&self) -> bool {
        self.looping
    }

    /// Whether the clock is running.
    pub fn is_playing(&self) -> bool {
        self.started_at.is_some()
    }

    /// The frame to show now.
    pub fn current_frame(&self) -> FrameIndex {
        self.resolve(self.frames_elapsed()).0
    }

    /// Starts playback from the current frame.
    ///
    /// If the current frame is outside the range, or a non-looping playback
    /// already reached the end, playback restarts from the range start. Does
    /// nothing if already playing or the range is empty.
    pub fn play(&mut self) {
        if self.is_playing() || self.range.is_empty() {
            return;
        }
        let (frame, finished, _) = self.resolve(self.frames_elapsed());
        if finished || !self.range.contains(frame) {
            self.start_frame = self.range.start;
            self.elapsed_before = Duration::ZERO;
        }
        self.started_at = Some(self.time.now());
        self.last_tick = None;
    }

    /// Stops the clock, holding the current frame. Resuming continues from
    /// the exact same point in time, including the part-way frame.
    pub fn pause(&mut self) {
        if let Some(started) = self.started_at.take() {
            self.elapsed_before += self.time.now().saturating_sub(started);
        }
    }

    /// Plays if paused, pauses if playing.
    pub fn toggle(&mut self) {
        if self.is_playing() {
            self.pause();
        } else {
            self.play();
        }
    }

    /// Jumps to `frame`. While playing, playback continues from there; a
    /// frame outside the play range goes to the range start instead.
    pub fn seek(&mut self, frame: FrameIndex) {
        let frame = if self.is_playing() && !self.range.contains(frame) {
            self.range.start
        } else {
            frame
        };
        self.rebase(frame);
    }

    /// Changes the frame rate without moving the current frame.
    pub fn set_fps(&mut self, fps: Fps) {
        let frame = self.current_frame();
        self.fps = fps;
        self.rebase(frame);
    }

    /// Changes the play range. The current frame is kept if it lies inside
    /// the new range; while playing, it otherwise moves to the range start.
    /// An empty range stops playback.
    pub fn set_range(&mut self, range: FrameRange) {
        let mut frame = self.current_frame();
        self.range = range;
        if range.is_empty() {
            self.pause();
        } else if self.is_playing() && !range.contains(frame) {
            frame = range.start;
        }
        self.rebase(frame);
    }

    /// Turns looping on or off without moving the current frame.
    pub fn set_looping(&mut self, looping: bool) {
        let frame = self.current_frame();
        self.looping = looping;
        self.rebase(frame);
    }

    /// Reads the clock for one redraw: the frame to show, how many frames
    /// were skipped since the last tick, and whether playback just finished.
    /// A finished non-looping playback pauses itself.
    pub fn tick(&mut self) -> Tick {
        let (frame, finished, n) = self.resolve(self.frames_elapsed());
        let dropped = self
            .last_tick
            .map_or(0, |last| n.saturating_sub(last).saturating_sub(1));
        self.last_tick = Some(n);
        if finished {
            self.pause();
        }
        Tick {
            frame,
            dropped,
            finished,
        }
    }

    /// Restarts the time base at `frame`, keeping the play state.
    fn rebase(&mut self, frame: FrameIndex) {
        self.start_frame = frame;
        self.elapsed_before = Duration::ZERO;
        if self.started_at.is_some() {
            self.started_at = Some(self.time.now());
        }
        self.last_tick = None;
    }

    /// Total play time since the last rebase.
    fn elapsed(&self) -> Duration {
        let running = self
            .started_at
            .map_or(Duration::ZERO, |s| self.time.now().saturating_sub(s));
        self.elapsed_before + running
    }

    /// `floor(elapsed * fps)`, computed exactly in integer nanoseconds.
    fn frames_elapsed(&self) -> u64 {
        let nanos = self.elapsed().as_nanos();
        let frames = nanos * u128::from(self.fps.numerator())
            / (u128::from(self.fps.denominator()) * 1_000_000_000);
        u64::try_from(frames).unwrap_or(u64::MAX)
    }

    /// Maps `n` elapsed frames to `(frame, finished, effective n)`. The
    /// effective count stops growing once a non-looping playback finishes.
    fn resolve(&self, n: u64) -> (FrameIndex, bool, u64) {
        let start = u64::from(self.start_frame.0);
        let raw = start.saturating_add(n);
        let (s, e) = (u64::from(self.range.start.0), u64::from(self.range.end.0));
        let to_frame = |f: u64| FrameIndex(u32::try_from(f).unwrap_or(u32::MAX));

        if self.range.is_empty() || !self.range.contains(self.start_frame) || raw < e {
            // Outside the range (only possible while paused) or still inside.
            return (to_frame(raw), false, n);
        }
        if self.looping {
            (to_frame(s + (raw - s) % (e - s)), false, n)
        } else {
            (to_frame(e - 1), true, e - 1 - start)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fps(n: u32, d: u32) -> Fps {
        Fps::new(n, d).expect("valid fps")
    }

    fn range(s: u32, e: u32) -> FrameRange {
        FrameRange::new(FrameIndex(s), FrameIndex(e))
    }

    fn clock(f: Fps, r: FrameRange) -> (PlaybackClock<ManualTimeSource>, ManualTimeSource) {
        let time = ManualTimeSource::new();
        (PlaybackClock::new(time.clone(), f, r), time)
    }

    /// Duration of `frames` frames at a whole-number rate, plus `extra_ns`.
    fn frames_at(rate: u32, frames: u64, extra_ns: u64) -> Duration {
        Duration::from_nanos(frames * 1_000_000_000 / u64::from(rate) + extra_ns)
    }

    #[test]
    fn starts_paused_at_range_start() {
        let (c, t) = clock(Fps::FPS_24, range(5, 50));
        t.advance(Duration::from_secs(1));
        assert!(!c.is_playing());
        assert_eq!(c.current_frame(), FrameIndex(5));
    }

    #[test]
    fn frame_is_floor_of_elapsed_time() {
        let (mut c, t) = clock(Fps::FPS_24, range(0, 1000));
        c.play();
        // One frame at 24 fps is 41_666_666.67 ns.
        t.advance(Duration::from_nanos(41_666_666));
        assert_eq!(c.current_frame(), FrameIndex(0));
        t.advance(Duration::from_nanos(1));
        assert_eq!(c.current_frame(), FrameIndex(1));
        t.advance(Duration::from_nanos(1_000_000_000 - 41_666_667));
        assert_eq!(c.current_frame(), FrameIndex(24));
    }

    #[test]
    fn ntsc_rates_are_exact() {
        let (mut c, t) = clock(fps(24000, 1001), range(0, u32::MAX));
        c.play();
        t.advance(Duration::from_secs(1));
        assert_eq!(c.current_frame(), FrameIndex(23)); // 23.976
        t.advance(Duration::from_secs(1000)); // 1001 s total
        assert_eq!(c.current_frame(), FrameIndex(24000));

        let (mut c, t) = clock(fps(30000, 1001), range(0, u32::MAX));
        c.play();
        t.advance(Duration::from_secs(1001));
        assert_eq!(c.current_frame(), FrameIndex(30000));
        // One nanosecond short of 30000 frames stays on 29999.
        let (mut c, t) = clock(fps(30000, 1001), range(0, u32::MAX));
        c.play();
        t.advance(Duration::from_secs(1001) - Duration::from_nanos(1));
        assert_eq!(c.current_frame(), FrameIndex(29999));
    }

    #[test]
    fn looping_wraps_into_range() {
        let (mut c, t) = clock(Fps::FPS_24, range(10, 14));
        c.play();
        let seen: Vec<u32> = (0..10)
            .map(|i| {
                if i > 0 {
                    t.advance(frames_at(24, 1, 1));
                }
                c.tick().frame.0
            })
            .collect();
        assert_eq!(seen, [10, 11, 12, 13, 10, 11, 12, 13, 10, 11]);
        assert!(c.is_playing());
    }

    #[test]
    fn non_looping_stops_on_last_frame_then_restarts() {
        let (mut c, t) = clock(Fps::FPS_24, range(10, 14));
        c.set_looping(false);
        c.play();
        t.advance(frames_at(24, 3, 1));
        let tick = c.tick();
        assert_eq!((tick.frame, tick.finished), (FrameIndex(13), false));
        t.advance(frames_at(24, 5, 0));
        let tick = c.tick();
        assert_eq!(tick.frame, FrameIndex(13));
        assert!(tick.finished);
        assert_eq!(tick.dropped, 0, "frames past the end are not dropped");
        assert!(!c.is_playing());

        // Holds the last frame while paused; play restarts from the top.
        t.advance(Duration::from_secs(5));
        assert_eq!(c.current_frame(), FrameIndex(13));
        c.play();
        assert_eq!(c.current_frame(), FrameIndex(10));
    }

    #[test]
    fn pause_and_resume_keep_position_and_phase() {
        let (mut c, t) = clock(Fps::FPS_24, range(0, 100));
        c.play();
        t.advance(frames_at(24, 5, 20_000_000)); // 5 frames + about half a frame
        c.pause();
        assert_eq!(c.current_frame(), FrameIndex(5));
        t.advance(Duration::from_secs(10));
        assert_eq!(c.current_frame(), FrameIndex(5));
        c.play();
        assert_eq!(c.current_frame(), FrameIndex(5));
        // The half frame already played counts: frame 6 comes early.
        t.advance(Duration::from_nanos(22_000_000));
        assert_eq!(c.current_frame(), FrameIndex(6));
    }

    #[test]
    fn toggle_switches_state() {
        let (mut c, _) = clock(Fps::FPS_24, range(0, 10));
        c.toggle();
        assert!(c.is_playing());
        c.toggle();
        assert!(!c.is_playing());
    }

    #[test]
    fn slow_render_drops_frames() {
        let (mut c, t) = clock(Fps::FPS_24, range(0, 1000));
        c.play();
        assert_eq!(
            c.tick(),
            Tick {
                frame: FrameIndex(0),
                dropped: 0,
                finished: false
            }
        );
        // A render that took five frame durations: playback jumps ahead.
        t.advance(frames_at(24, 5, 1));
        assert_eq!(
            c.tick(),
            Tick {
                frame: FrameIndex(5),
                dropped: 4,
                finished: false
            }
        );
        // Back to normal speed.
        t.advance(frames_at(24, 1, 0));
        assert_eq!(
            c.tick(),
            Tick {
                frame: FrameIndex(6),
                dropped: 0,
                finished: false
            }
        );
    }

    #[test]
    fn fast_render_repeats_frames() {
        let (mut c, t) = clock(Fps::FPS_24, range(0, 1000));
        c.play();
        for _ in 0..5 {
            t.advance(Duration::from_millis(5));
            let tick = c.tick();
            assert_eq!((tick.frame, tick.dropped), (FrameIndex(0), 0));
        }
    }

    #[test]
    fn dropped_frames_count_across_a_loop_wrap() {
        let (mut c, t) = clock(Fps::FPS_24, range(0, 4));
        c.play();
        c.tick();
        t.advance(frames_at(24, 6, 1));
        assert_eq!(
            c.tick(),
            Tick {
                frame: FrameIndex(2),
                dropped: 5,
                finished: false
            }
        );
    }

    #[test]
    fn changing_fps_while_playing_does_not_jump() {
        let (mut c, t) = clock(Fps::FPS_24, range(0, 1000));
        c.play();
        t.advance(frames_at(24, 10, 20_000_000));
        assert_eq!(c.current_frame(), FrameIndex(10));
        c.set_fps(fps(12, 1));
        assert_eq!(c.current_frame(), FrameIndex(10));
        t.advance(frames_at(12, 1, 0)); // 83_333_333 ns, just short of a frame
        assert_eq!(c.current_frame(), FrameIndex(10));
        t.advance(Duration::from_nanos(1));
        assert_eq!(c.current_frame(), FrameIndex(11));
    }

    #[test]
    fn seek_while_playing_continues_from_target() {
        let (mut c, t) = clock(Fps::FPS_24, range(0, 100));
        c.play();
        t.advance(Duration::from_secs(1));
        c.seek(FrameIndex(50));
        assert_eq!(c.current_frame(), FrameIndex(50));
        assert!(c.is_playing());
        t.advance(frames_at(24, 2, 1));
        assert_eq!(c.current_frame(), FrameIndex(52));
        // Seeking outside the range while playing goes to the range start.
        c.seek(FrameIndex(500));
        assert_eq!(c.current_frame(), FrameIndex(0));
    }

    #[test]
    fn paused_seek_may_leave_range_and_play_snaps_back() {
        let (mut c, t) = clock(Fps::FPS_24, range(10, 20));
        c.seek(FrameIndex(3));
        assert_eq!(c.current_frame(), FrameIndex(3));
        t.advance(Duration::from_secs(1));
        assert_eq!(c.current_frame(), FrameIndex(3));
        c.play();
        assert_eq!(c.current_frame(), FrameIndex(10));
    }

    #[test]
    fn set_range_and_looping_keep_current_frame() {
        let (mut c, t) = clock(Fps::FPS_24, range(0, 100));
        c.play();
        t.advance(frames_at(24, 30, 1));
        c.set_range(range(20, 40));
        assert_eq!(c.current_frame(), FrameIndex(30));
        c.set_looping(false);
        assert_eq!(c.current_frame(), FrameIndex(30));
        // A range that excludes the current frame moves playback to its start.
        c.set_range(range(60, 70));
        assert_eq!(c.current_frame(), FrameIndex(60));
        // An empty range stops playback.
        c.set_range(range(5, 5));
        assert!(!c.is_playing());
    }

    #[test]
    fn empty_range_cannot_play() {
        let (mut c, _) = clock(Fps::FPS_24, range(3, 3));
        c.play();
        assert!(!c.is_playing());
        assert_eq!(c.current_frame(), FrameIndex(3));
    }

    #[test]
    fn system_time_source_is_monotonic() {
        let s = SystemTimeSource::new();
        let a = s.now();
        let b = s.now();
        assert!(b >= a);
        let c: PlaybackClock = PlaybackClock::new(s, Fps::FPS_24, range(0, 10));
        assert_eq!(c.current_frame(), FrameIndex(0));
    }
}
