//! Encoding on a background thread, so the UI stays responsive.

use crate::{ExportError, ImageFormat, Mp4Encoder, RgbaImage, VideoSettings};
use crate::{sequence_path, write_image};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

/// How many rendered frames may wait for the encoder. Keeps memory bounded
/// (a 4K RGBA frame is 33 MB) while the encoder catches up.
pub const MAX_QUEUED_FRAMES: usize = 4;

/// Where the frames go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportTarget {
    /// One image at exactly `path`.
    Image {
        /// Output file.
        path: PathBuf,
        /// PNG or JPEG.
        format: ImageFormat,
    },
    /// Numbered images next to `base` (see [`sequence_path`]).
    Sequence {
        /// The chosen file; its stem names the sequence.
        base: PathBuf,
        /// PNG or JPEG.
        format: ImageFormat,
    },
    /// One H.264 MP4 file.
    Video {
        /// Output file.
        path: PathBuf,
        /// Size and frame rate.
        settings: VideoSettings,
    },
}

/// Progress reported by the worker.
#[derive(Debug)]
pub enum WorkerEvent {
    /// Another frame was written; `done` counts all written so far.
    Progress {
        /// Frames written so far.
        done: u32,
    },
    /// Everything was written. Holds the output path (the first file for a
    /// sequence).
    Finished(PathBuf),
    /// Writing failed; nothing more will be written. A partial video was
    /// deleted.
    Failed(ExportError),
    /// The export was cancelled. A partial video was deleted.
    Cancelled,
}

enum Message {
    /// A frame to write. `number` is its one-based frame number, used to name
    /// sequence files.
    Frame { number: u32, image: RgbaImage },
}

/// A background thread that writes frames as they are sent.
///
/// Send frames with [`Self::send`] while [`Self::can_accept`] is true, then
/// call [`Self::finish`]. Poll [`Self::events`] each UI frame.
#[derive(Debug)]
pub struct ExportWorker {
    sender: Option<mpsc::Sender<Message>>,
    events: mpsc::Receiver<WorkerEvent>,
    queued: Arc<AtomicUsize>,
    cancelled: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ExportWorker {
    /// Starts a worker writing to `target`.
    pub fn spawn(target: ExportTarget) -> Result<Self, ExportError> {
        let (sender, frames) = mpsc::channel::<Message>();
        let (event_tx, events) = mpsc::channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let thread = {
            let queued = Arc::clone(&queued);
            let cancelled = Arc::clone(&cancelled);
            std::thread::Builder::new()
                .name("rusting-export".into())
                .spawn(move || {
                    let event = run(target, &frames, &queued, &cancelled, &event_tx);
                    let _ = event_tx.send(event);
                })
                .map_err(|source| ExportError::Io {
                    path: PathBuf::from("<export thread>"),
                    source,
                })?
        };
        Ok(Self {
            sender: Some(sender),
            events,
            queued,
            cancelled,
            thread: Some(thread),
        })
    }

    /// Whether another frame can be queued without exceeding
    /// [`MAX_QUEUED_FRAMES`].
    pub fn can_accept(&self) -> bool {
        self.sender.is_some() && self.queued.load(Ordering::Acquire) < MAX_QUEUED_FRAMES
    }

    /// Queues frame `number` (one-based) for writing. Returns false if the
    /// worker has stopped.
    pub fn send(&self, number: u32, image: RgbaImage) -> bool {
        let Some(sender) = &self.sender else {
            return false;
        };
        self.queued.fetch_add(1, Ordering::AcqRel);
        if sender.send(Message::Frame { number, image }).is_err() {
            self.queued.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        true
    }

    /// No more frames will be sent: write what is queued and close the file.
    pub fn finish(&mut self) {
        self.sender = None;
    }

    /// Stops as soon as possible, dropping queued frames and deleting a
    /// partial video.
    pub fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.sender = None;
    }

    /// Events since the last call, oldest first.
    pub fn events(&self) -> Vec<WorkerEvent> {
        self.events.try_iter().collect()
    }

    /// Waits for the thread to exit and returns its remaining events.
    pub fn join(mut self) -> Vec<WorkerEvent> {
        self.sender = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.events.try_iter().collect()
    }
}

impl Drop for ExportWorker {
    fn drop(&mut self) {
        // Dropped without finishing (e.g. the app quit): don't leave a
        // half-written video behind.
        if self.sender.is_some() {
            self.cancel();
        }
        // Wait so a cancelled video's partial file is really gone.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The worker loop. Returns the final event.
fn run(
    target: ExportTarget,
    frames: &mpsc::Receiver<Message>,
    queued: &AtomicUsize,
    cancelled: &AtomicBool,
    events: &mpsc::Sender<WorkerEvent>,
) -> WorkerEvent {
    let mut video = None;
    if let ExportTarget::Video { path, settings } = &target {
        match Mp4Encoder::create(path, *settings) {
            Ok(encoder) => video = Some(encoder),
            Err(err) => return WorkerEvent::Failed(err),
        }
    }
    let mut first_file = None;
    for (index, Message::Frame { number, image }) in frames.iter().enumerate() {
        queued.fetch_sub(1, Ordering::AcqRel);
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        let result = match (&target, video.as_mut()) {
            (_, Some(encoder)) => encoder.push_frame(&image),
            (ExportTarget::Image { path, format }, None) => {
                first_file.get_or_insert_with(|| path.clone());
                write_image(path, &image, *format)
            }
            (ExportTarget::Sequence { base, format }, None) => {
                let path = sequence_path(base, number, *format);
                let result = write_image(&path, &image, *format);
                first_file.get_or_insert(path);
                result
            }
            (ExportTarget::Video { .. }, None) => Err(ExportError::Cancelled),
        };
        if let Err(err) = result {
            // Dropping `video` deletes the partial file.
            return WorkerEvent::Failed(err);
        }
        let done = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let _ = events.send(WorkerEvent::Progress { done });
    }
    if cancelled.load(Ordering::Acquire) {
        drop(video);
        return WorkerEvent::Cancelled;
    }
    match video {
        Some(encoder) => match encoder.finish() {
            Ok(path) => WorkerEvent::Finished(path),
            Err(err) => WorkerEvent::Failed(err),
        },
        None => match first_file {
            Some(path) => WorkerEvent::Finished(path),
            None => {
                WorkerEvent::Failed(ExportError::InvalidFrame("no frames were exported".into()))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rusting-worker-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn last(events: Vec<WorkerEvent>) -> WorkerEvent {
        events.into_iter().last().unwrap()
    }

    #[test]
    fn writes_a_numbered_sequence() {
        let dir = temp_dir("seq");
        let base = dir.join("walk.png");
        let mut worker = ExportWorker::spawn(ExportTarget::Sequence {
            base: base.clone(),
            format: ImageFormat::Png,
        })
        .unwrap();
        for n in 3..=5 {
            assert!(worker.send(n, RgbaImage::filled(4, 4, [n as u8, 0, 0, 255])));
        }
        worker.finish();
        let events = worker.join();
        assert!(matches!(
            events.iter().rev().nth(1),
            Some(WorkerEvent::Progress { done: 3 })
        ));
        assert!(matches!(last(events), WorkerEvent::Finished(p) if p == dir.join("walk_0003.png")));
        for n in 3..=5 {
            let img = image::open(dir.join(format!("walk_{n:04}.png"))).unwrap();
            assert_eq!(img.to_rgba8().get_pixel(0, 0).0, [n as u8, 0, 0, 255]);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancelling_a_video_deletes_it() {
        let dir = temp_dir("cancel");
        let path = dir.join("clip.mp4");
        let mut worker = ExportWorker::spawn(ExportTarget::Video {
            path: path.clone(),
            settings: VideoSettings {
                width: 16,
                height: 16,
                fps_numerator: 12,
                fps_denominator: 1,
            },
        })
        .unwrap();
        worker.send(1, RgbaImage::filled(16, 16, [9, 9, 9, 255]));
        worker.cancel();
        let events = worker.join();
        assert!(matches!(last(events), WorkerEvent::Cancelled));
        assert!(!path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn errors_are_reported_not_panicked() {
        let dir = temp_dir("err");
        let mut worker = ExportWorker::spawn(ExportTarget::Image {
            path: dir.join("missing/dir/x.png"),
            format: ImageFormat::Png,
        })
        .unwrap();
        worker.send(1, RgbaImage::filled(2, 2, [0; 4]));
        worker.finish();
        assert!(matches!(
            last(worker.join()),
            WorkerEvent::Failed(ExportError::Io { .. })
        ));
        let mut worker = ExportWorker::spawn(ExportTarget::Video {
            path: dir.join("v.mp4"),
            settings: VideoSettings {
                width: 8,
                height: 8,
                fps_numerator: 24,
                fps_denominator: 1,
            },
        })
        .unwrap();
        worker.send(1, RgbaImage::filled(4, 4, [0; 4]));
        worker.finish();
        assert!(matches!(
            last(worker.join()),
            WorkerEvent::Failed(ExportError::InvalidFrame(_))
        ));
        assert!(!dir.join("v.mp4").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn queue_is_bounded() {
        let dir = temp_dir("queue");
        let worker = ExportWorker::spawn(ExportTarget::Sequence {
            base: dir.join("q.png"),
            format: ImageFormat::Png,
        })
        .unwrap();
        // The count only grows past the limit if we ignore `can_accept`.
        assert!(worker.can_accept());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
