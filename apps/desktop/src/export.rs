//! File → Export: the options dialog, the progress window and the export job.
//!
//! Frames are rendered on the main thread with the renderer's offscreen path
//! (a few per UI frame, within a time budget) and handed to an
//! [`ExportWorker`] thread that encodes and writes them, so the editor stays
//! responsive during long exports. The job works on a snapshot of the
//! document, so editing during an export doesn't change what is written.

use document::animation_core::{FrameIndex, Rgba};
use document::{Document, RenderList};
use export::{ExportTarget, ExportWorker, ImageFormat, RgbaImage, VideoSettings, WorkerEvent};
use renderer::{DrawListBuilder, OffscreenFrame, OffscreenRenderer, RendererError, output_size};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use ui::{ExportKind, Session};

/// Most time spent rendering export frames per UI frame.
const FRAME_BUDGET: Duration = Duration::from_millis(25);

/// Scale presets offered in the dialog, in percent.
const SCALE_PRESETS: [u32; 4] = [25, 50, 100, 200];

/// Image file type for frame and sequence exports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    /// Lossless, optional transparency.
    Png,
    /// Lossy, always opaque.
    Jpeg,
}

/// The choices in the export dialog.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExportSettings {
    /// What is exported.
    pub kind: ExportKind,
    /// PNG or JPEG (frame and sequence exports).
    pub file_type: FileType,
    /// Output size in percent of the canvas size.
    pub scale_percent: u32,
    /// First frame, one-based (sequence and video).
    pub first: u32,
    /// Last frame, one-based and inclusive (sequence and video).
    pub last: u32,
    /// JPEG quality, 1..=100.
    pub jpeg_quality: u8,
    /// PNG only: leave the background transparent.
    pub transparent: bool,
}

impl ExportSettings {
    fn image_format(&self) -> ImageFormat {
        match self.file_type {
            FileType::Png => ImageFormat::Png,
            FileType::Jpeg => ImageFormat::Jpeg {
                quality: self.jpeg_quality,
            },
        }
    }

    /// The extension of the chosen file.
    pub fn extension(&self) -> &'static str {
        match self.kind {
            ExportKind::Video => "mp4",
            _ => self.image_format().extension(),
        }
    }

    fn scale(&self) -> f32 {
        self.scale_percent as f32 / 100.0
    }

    /// The canvas colour to render: `None` (transparent) only for PNG with
    /// transparency on. JPEG and MP4 have no alpha, so they always get the
    /// document background (white if the document has none).
    fn background(&self, document: &Document) -> Option<Rgba> {
        let transparent =
            self.transparent && self.kind != ExportKind::Video && self.file_type == FileType::Png;
        if transparent {
            None
        } else {
            Some(document.background.unwrap_or(Rgba::WHITE))
        }
    }

    /// Zero-based frames to export, by sprite sheet row. Only a sprite
    /// sheet has more than one row: one per tag, or the frame range if the
    /// document has no tags.
    fn rows(&self, doc: &Document, current: FrameIndex) -> Vec<Vec<u32>> {
        let first = self.first.max(1);
        let range = (first - 1..self.last.max(first)).collect();
        match self.kind {
            ExportKind::Frame => vec![vec![current.0]],
            ExportKind::SpriteSheet if !doc.tags.is_empty() => doc
                .tags
                .iter()
                .map(|t| (t.range.start.0..t.range.end.0.min(doc.frame_count)).collect())
                .filter(|row: &Vec<u32>| !row.is_empty())
                .collect(),
            _ => vec![range],
        }
    }
}

/// How an export ended.
#[derive(Debug)]
pub enum Outcome {
    /// Written; the path of the (first) file and how many frames.
    Done(PathBuf, u32),
    /// Something failed; the message says what.
    Failed(String),
    /// The user cancelled.
    Cancelled,
}

/// An export in progress.
struct Job {
    kind: ExportKind,
    document: Document,
    frames: Vec<u32>,
    next: usize,
    scale: f32,
    background: Option<Rgba>,
    worker: ExportWorker,
    written: u32,
    cancelling: bool,
    finished_sending: bool,
}

impl Job {
    fn total(&self) -> u32 {
        self.frames.len() as u32
    }

    /// Renders queued frames within the time budget and collects worker
    /// progress. Returns the outcome once the export has ended.
    fn pump(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        offscreen: &mut OffscreenRenderer,
    ) -> Option<Outcome> {
        let start = Instant::now();
        while !self.cancelling
            && self.next < self.frames.len()
            && self.worker.can_accept()
            && start.elapsed() < FRAME_BUDGET
        {
            let frame = FrameIndex(self.frames[self.next]);
            let image = match render_frame(
                device,
                queue,
                offscreen,
                &self.document,
                frame,
                self.scale,
                self.background,
            ) {
                Ok(image) => image,
                Err(err) => {
                    self.worker.cancel();
                    return Some(Outcome::Failed(format!("Rendering failed: {err}")));
                }
            };
            if !self.worker.send(frame.display_number(), image) {
                break;
            }
            self.next += 1;
        }
        if self.next >= self.frames.len() && !self.finished_sending {
            self.worker.finish();
            self.finished_sending = true;
        }
        for event in self.worker.events() {
            match event {
                WorkerEvent::Progress { done } => self.written = done,
                WorkerEvent::Finished(path) => return Some(Outcome::Done(path, self.written)),
                WorkerEvent::Failed(err) => return Some(Outcome::Failed(err.to_string())),
                WorkerEvent::Cancelled => return Some(Outcome::Cancelled),
            }
        }
        None
    }

    fn cancel(&mut self) {
        self.cancelling = true;
        self.worker.cancel();
    }
}

/// Renders `frame` of `document` at `scale` the way the canvas shows it
/// (MSAA, layer opacity, hidden layers left out), without editor overlays.
pub fn render_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    offscreen: &mut OffscreenRenderer,
    document: &Document,
    frame: FrameIndex,
    scale: f32,
    background: Option<Rgba>,
) -> Result<RgbaImage, RendererError> {
    let list: RenderList<'_> = document.evaluate(frame);
    let mut builder = DrawListBuilder::new(device, offscreen.stroke_cache_mut());
    builder.render_list(&list);
    let draws = builder.finish();
    let pixels = offscreen.render(
        device,
        queue,
        &OffscreenFrame {
            canvas_size: (document.canvas.width, document.canvas.height),
            scale,
            background,
            strokes: &draws,
        },
    )?;
    Ok(RgbaImage {
        width: pixels.width,
        height: pixels.height,
        pixels: pixels.rgba,
    })
}

/// The export dialog, progress window and running job.
pub struct ExportPanel {
    /// The options dialog, while open.
    dialog: Option<ExportSettings>,
    /// The options the user confirmed, waiting for a save path.
    confirmed: Option<ExportSettings>,
    /// The last confirmed options, to start the next dialog from.
    last: Option<ExportSettings>,
    job: Option<Job>,
}

impl ExportPanel {
    /// No dialog, no job.
    pub fn new() -> Self {
        Self {
            dialog: None,
            confirmed: None,
            last: None,
            job: None,
        }
    }

    /// Whether an export is running.
    pub fn is_running(&self) -> bool {
        self.job.is_some()
    }

    /// Opens the options dialog for `kind`.
    pub fn open(&mut self, kind: ExportKind, session: &Session) {
        if self.job.is_some() {
            return;
        }
        let doc = &session.document;
        let frame_count = doc.frame_count.max(1);
        let mut settings = self.last.unwrap_or(ExportSettings {
            kind,
            file_type: FileType::Png,
            scale_percent: 100,
            first: 1,
            last: frame_count,
            jpeg_quality: 90,
            transparent: doc.background.is_none(),
        });
        settings.kind = kind;
        settings.first = settings.first.clamp(1, frame_count);
        settings.last = if self.last.is_some() {
            settings.last.clamp(settings.first, frame_count)
        } else {
            frame_count
        };
        self.dialog = Some(settings);
    }

    /// Options confirmed in the dialog, to ask for a save path with.
    pub fn take_confirmed(&mut self) -> Option<ExportSettings> {
        self.confirmed.take()
    }

    /// Starts exporting `session`'s document with `settings` to `path`.
    /// Returns an error message if the export could not start.
    pub fn start(
        &mut self,
        settings: ExportSettings,
        path: &Path,
        session: &Session,
    ) -> Result<(), String> {
        let document = session.document.clone();
        let rows = settings.rows(&document, session.current_frame());
        let scale = settings.scale();
        let (width, height) = output_size((document.canvas.width, document.canvas.height), scale);
        let target = match settings.kind {
            ExportKind::Frame => ExportTarget::Image {
                path: path.to_owned(),
                format: settings.image_format(),
            },
            ExportKind::Sequence => ExportTarget::Sequence {
                base: path.to_owned(),
                format: settings.image_format(),
            },
            ExportKind::Video => ExportTarget::Video {
                path: path.to_owned(),
                settings: VideoSettings {
                    width,
                    height,
                    fps_numerator: document.fps.numerator(),
                    fps_denominator: document.fps.denominator(),
                },
            },
            ExportKind::SpriteSheet => ExportTarget::SpriteSheet {
                path: path.to_owned(),
                format: settings.image_format(),
                rows: rows.iter().map(|r| r.len() as u32).collect(),
            },
        };
        let frames: Vec<u32> = rows.into_iter().flatten().collect();
        if frames.is_empty() {
            return Err("No frames to export".to_owned());
        }
        let worker = ExportWorker::spawn(target).map_err(|e| e.to_string())?;
        self.job = Some(Job {
            kind: settings.kind,
            background: settings.background(&document),
            document,
            next: 0,
            frames,
            scale,
            worker,
            written: 0,
            cancelling: false,
            finished_sending: false,
        });
        Ok(())
    }

    /// Advances the running export. Returns how it ended, once it has.
    pub fn pump(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        offscreen: &mut OffscreenRenderer,
    ) -> Option<Outcome> {
        let outcome = self.job.as_mut()?.pump(device, queue, offscreen);
        if outcome.is_some() {
            self.job = None;
        }
        outcome
    }

    /// Draws the options dialog or the progress window.
    pub fn ui(&mut self, ctx: &egui::Context, session: &Session) {
        if let Some(job) = &mut self.job {
            progress_window(ctx, job);
            ctx.request_repaint();
            return;
        }
        let Some(settings) = &mut self.dialog else {
            return;
        };
        let mut action = None;
        let response = egui::Modal::new(egui::Id::unique("export_dialog")).show(ctx, |ui| {
            action = options_ui(ui, settings, &session.document);
        });
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if response.should_close() || escape {
            action = Some(false);
        }
        match action {
            Some(true) => {
                self.confirmed = Some(*settings);
                self.last = Some(*settings);
                self.dialog = None;
            }
            Some(false) => self.dialog = None,
            None => {}
        }
    }
}

/// The options form. Returns `Some(true)` to export, `Some(false)` to cancel.
fn options_ui(ui: &mut egui::Ui, s: &mut ExportSettings, doc: &Document) -> Option<bool> {
    ui.set_min_width(340.0);
    ui.heading(match s.kind {
        ExportKind::Frame => "Export Frame",
        ExportKind::Sequence => "Export Image Sequence",
        ExportKind::Video => "Export Video (MP4)",
        ExportKind::SpriteSheet => "Export Sprite Sheet",
    });
    ui.add_space(8.0);
    let frame_count = doc.frame_count.max(1);
    egui::Grid::new("export_options")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            if s.kind != ExportKind::Video {
                ui.label("Format");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut s.file_type, FileType::Png, "PNG");
                    ui.selectable_value(&mut s.file_type, FileType::Jpeg, "JPG");
                });
                ui.end_row();
            }

            ui.label("Scale");
            ui.horizontal(|ui| {
                for preset in SCALE_PRESETS {
                    ui.selectable_value(&mut s.scale_percent, preset, format!("{preset}%"));
                }
                ui.add(
                    egui::DragValue::new(&mut s.scale_percent)
                        .range(5..=800)
                        .suffix("%"),
                );
            });
            ui.end_row();

            ui.label("Size");
            let (w, h) = output_size((doc.canvas.width, doc.canvas.height), s.scale());
            ui.label(format!("{w} × {h} px"));
            ui.end_row();

            if s.kind == ExportKind::SpriteSheet && !doc.tags.is_empty() {
                ui.label("Rows");
                ui.label(format!("{} (one per tag)", doc.tags.len()));
                ui.end_row();
            } else if s.kind != ExportKind::Frame {
                ui.label("Frames");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut s.first).range(1..=frame_count));
                    ui.label("to");
                    ui.add(egui::DragValue::new(&mut s.last).range(1..=frame_count));
                    if ui.small_button("All").clicked() {
                        s.first = 1;
                        s.last = frame_count;
                    }
                });
                s.last = s.last.max(s.first);
                ui.end_row();
            }

            if s.kind == ExportKind::Video {
                ui.label("Frame rate");
                ui.label(format!("{:.3} fps (document)", doc.fps.as_f64()));
                ui.end_row();
            }

            if s.kind != ExportKind::Video && s.file_type == FileType::Jpeg {
                ui.label("Quality");
                ui.add(egui::Slider::new(&mut s.jpeg_quality, 1..=100));
                ui.end_row();
            }

            if s.kind != ExportKind::Video && s.file_type == FileType::Png {
                ui.label("Background");
                ui.checkbox(&mut s.transparent, "Transparent");
                ui.end_row();
            }
        });
    if s.kind == ExportKind::Video || s.file_type == FileType::Jpeg {
        ui.add_space(4.0);
        ui.weak("No transparency: frames are drawn on the document background.");
    }
    if s.kind == ExportKind::Sequence {
        ui.weak("Files are numbered by frame, e.g. name_0001.png.");
    }
    if s.kind == ExportKind::SpriteSheet && doc.tags.is_empty() {
        ui.weak("No frame tags: the frames go on one row. Tag frames to get a row per tag.");
    }
    ui.add_space(10.0);
    let mut action = None;
    ui.horizontal(|ui| {
        if ui.button("Export…").clicked() {
            action = Some(true);
        }
        if ui.button("Cancel").clicked() {
            action = Some(false);
        }
    });
    action
}

fn progress_window(ctx: &egui::Context, job: &mut Job) {
    let total = job.total().max(1);
    egui::Window::new("Exporting")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_min_width(300.0);
            let what = match job.kind {
                ExportKind::Frame => "frame",
                ExportKind::Sequence => "image sequence",
                ExportKind::Video => "video",
                ExportKind::SpriteSheet => "sprite sheet",
            };
            if job.cancelling {
                ui.label("Cancelling…");
            } else {
                ui.label(format!("Writing {what}: frame {} of {total}", job.written));
            }
            ui.add(egui::ProgressBar::new(job.written as f32 / total as f32).show_percentage());
            ui.add_space(6.0);
            if ui
                .add_enabled(!job.cancelling, egui::Button::new("Cancel"))
                .clicked()
            {
                job.cancel();
            }
        });
}

/// Shows a save dialog for `settings`, defaulting to the document name.
/// The returned path has the right extension.
pub fn ask_path(settings: &ExportSettings, session: &Session) -> Option<PathBuf> {
    let ext = settings.extension();
    let (filter, title) = match settings.kind {
        ExportKind::Frame => (ext.to_uppercase(), "Export Frame"),
        ExportKind::Sequence => (ext.to_uppercase(), "Export Image Sequence"),
        ExportKind::Video => ("MP4 video".to_owned(), "Export Video"),
        ExportKind::SpriteSheet => (ext.to_uppercase(), "Export Sprite Sheet"),
    };
    let stem = session
        .path
        .as_deref()
        .and_then(Path::file_stem)
        .map_or_else(
            || session.document.name.clone(),
            |s| s.to_string_lossy().into_owned(),
        );
    let name = match settings.kind {
        ExportKind::Frame => format!(
            "{stem}_{:04}.{ext}",
            session.current_frame().display_number()
        ),
        _ => format!("{stem}.{ext}"),
    };
    let mut dialog = rfd::FileDialog::new()
        .set_title(title)
        .add_filter(filter, &[ext])
        .set_file_name(name);
    if let Some(dir) = session.path.as_deref().and_then(Path::parent) {
        dialog = dialog.set_directory(dir);
    }
    let mut path = dialog.save_file()?;
    let has_ext = path.extension().is_some_and(|e| {
        e.eq_ignore_ascii_case(ext) || (ext == "jpg" && e.eq_ignore_ascii_case("jpeg"))
    });
    if !has_ext {
        path.set_extension(ext);
    }
    Some(path)
}

/// Shows how an export ended in the status bar or an error dialog.
pub fn report(outcome: Outcome, session: &mut Session) {
    match outcome {
        Outcome::Done(path, frames) => {
            let what = if frames == 1 {
                String::new()
            } else {
                format!(" ({frames} frames)")
            };
            session.set_status(format!("Exported {}{what}", path.display()));
        }
        Outcome::Failed(message) => {
            tracing::warn!(%message, "export failed");
            session.show_error("Export failed", message);
        }
        Outcome::Cancelled => session.set_status("Export cancelled"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(kind: ExportKind) -> ExportSettings {
        ExportSettings {
            kind,
            file_type: FileType::Png,
            scale_percent: 50,
            first: 3,
            last: 5,
            jpeg_quality: 90,
            transparent: true,
        }
    }

    #[test]
    fn frames_and_background_follow_the_options() {
        let mut doc = ui::session::new_document();
        doc.background = Some(Rgba::rgb(0.1, 0.2, 0.3));
        let s = settings(ExportKind::Sequence);
        assert_eq!(s.rows(&doc, FrameIndex(9)), [vec![2, 3, 4]]);
        assert_eq!(
            settings(ExportKind::Frame).rows(&doc, FrameIndex(9)),
            [vec![9]]
        );
        let sheet = settings(ExportKind::SpriteSheet);
        assert_eq!(sheet.rows(&doc, FrameIndex(9)), [vec![2, 3, 4]]);
        let tag = |name: &str, s, e| document::FrameTag {
            name: name.into(),
            range: document::animation_core::FrameRange::new(FrameIndex(s), FrameIndex(e)),
        };
        doc.frame_count = 10;
        doc.tags = vec![tag("walk", 0, 3), tag("jump", 8, 12)];
        assert_eq!(sheet.rows(&doc, FrameIndex(9)), [vec![0, 1, 2], vec![8, 9]]);
        assert_eq!(s.background(&doc), None);
        let jpg = ExportSettings {
            file_type: FileType::Jpeg,
            ..s
        };
        assert_eq!(jpg.background(&doc), doc.background);
        assert_eq!(jpg.extension(), "jpg");
        let video = settings(ExportKind::Video);
        assert_eq!(video.background(&doc), doc.background);
        assert_eq!(video.extension(), "mp4");
        doc.background = None;
        assert_eq!(video.background(&doc), Some(Rgba::WHITE));
    }

    /// A few frames of a red stroke moving over a background, plus a hidden
    /// magenta layer that must not appear.
    fn demo_session() -> Session {
        use document::animation_core::Vec2;
        use document::drawing::{BrushStyle, DrawingContent, Stroke, StrokePoint, VectorDrawing};
        use document::timeline::Keyframe;
        let mut doc = document::Document::new(
            "demo",
            document::CanvasSize {
                width: 64,
                height: 33,
            },
        );
        doc.background = Some(Rgba::rgb(0.2, 0.6, 0.2));
        doc.frame_count = 6;
        let line = |x: f32, color| {
            let mut d = VectorDrawing::new();
            d.push_stroke(Stroke::new(
                vec![
                    StrokePoint::new(Vec2::new(x, 4.0)),
                    StrokePoint::new(Vec2::new(x, 29.0)),
                ],
                BrushStyle {
                    color,
                    width: 8.0,
                    opacity: 1.0,
                },
            ));
            DrawingContent::Vector(d)
        };
        let layer = doc.add_layer("ball");
        for f in 0..6u32 {
            let id = doc.add_drawing(line(8.0 + f as f32 * 8.0, Rgba::rgb(1.0, 0.0, 0.0)));
            doc.track_mut(layer)
                .expect("track")
                .set_key(FrameIndex(f), Keyframe::Drawing(id));
        }
        let hidden = doc.add_layer("hidden");
        let id = doc.add_drawing(line(32.0, Rgba::rgb(1.0, 0.0, 1.0)));
        doc.track_mut(hidden)
            .expect("track")
            .set_key(FrameIndex(0), Keyframe::Drawing(id));
        doc.layer_mut(hidden).expect("layer").visible = false;
        Session::with_document(doc, None)
    }

    /// Runs an export to the end through the same calls the app makes.
    fn run(
        settings: ExportSettings,
        path: &Path,
        session: &Session,
        gpu: &(wgpu::Device, wgpu::Queue),
    ) -> Outcome {
        let mut panel = ExportPanel::new();
        panel.start(settings, path, session).expect("start");
        let mut offscreen = OffscreenRenderer::new(&gpu.0);
        for _ in 0..10_000 {
            if let Some(outcome) = panel.pump(&gpu.0, &gpu.1, &mut offscreen) {
                return outcome;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("export did not finish");
    }

    /// Exports PNG, JPG, a PNG sequence and an MP4 on a real GPU and checks
    /// the files. Skips when there is no GPU adapter. Set
    /// `RUSTING_EXPORT_OUT` to keep the files in that directory.
    #[test]
    fn exports_png_jpg_sequence_and_mp4() {
        let Ok(gpu) = pollster::block_on(renderer::headless_device()) else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let keep = std::env::var_os("RUSTING_EXPORT_OUT").map(PathBuf::from);
        let dir = keep.clone().unwrap_or_else(|| {
            std::env::temp_dir().join(format!("rusting-desktop-export-{}", std::process::id()))
        });
        std::fs::create_dir_all(&dir).expect("dir");
        let mut session = demo_session();
        session.set_frame(FrameIndex(2));
        let base = ExportSettings {
            kind: ExportKind::Frame,
            file_type: FileType::Png,
            scale_percent: 200,
            first: 1,
            last: 6,
            jpeg_quality: 90,
            transparent: false,
        };

        // Frame 3 as PNG at 200%: red line at x = 24 (48 px), background
        // elsewhere, hidden layer absent.
        let png = dir.join("frame.png");
        assert!(matches!(run(base, &png, &session, &gpu), Outcome::Done(..)));
        let img = image::open(&png).expect("png").to_rgba8();
        assert_eq!(img.dimensions(), (128, 66));
        assert_eq!(img.get_pixel(48, 33).0, [255, 0, 0, 255]);
        let bg = img.get_pixel(100, 33).0;
        assert_eq!(bg[3], 255);
        assert!(
            bg[1] > bg[0] && bg[1] > bg[2],
            "background is green: {bg:?}"
        );

        // Transparent PNG: alpha 0 away from the stroke.
        let clear = dir.join("clear.png");
        let transparent = ExportSettings {
            transparent: true,
            ..base
        };
        assert!(matches!(
            run(transparent, &clear, &session, &gpu),
            Outcome::Done(..)
        ));
        let img = image::open(&clear).expect("png").to_rgba8();
        assert_eq!(img.get_pixel(100, 33).0[3], 0);
        assert_eq!(img.get_pixel(48, 33).0, [255, 0, 0, 255]);

        // JPG at 100%.
        let jpg = dir.join("frame.jpg");
        let jpeg = ExportSettings {
            file_type: FileType::Jpeg,
            scale_percent: 100,
            ..base
        };
        assert!(matches!(run(jpeg, &jpg, &session, &gpu), Outcome::Done(..)));
        let img = image::open(&jpg).expect("jpg").to_rgb8();
        assert_eq!(img.dimensions(), (64, 33));
        let p = img.get_pixel(24, 16).0;
        assert!(p[0] > 200 && p[1] < 60 && p[2] < 60, "{p:?}");

        // Frames 2..=4 as a numbered sequence.
        let seq = ExportSettings {
            kind: ExportKind::Sequence,
            first: 2,
            last: 4,
            scale_percent: 100,
            ..base
        };
        let outcome = run(seq, &dir.join("walk.png"), &session, &gpu);
        assert!(matches!(outcome, Outcome::Done(_, 3)), "{outcome:?}");
        for n in 2..=4 {
            assert!(dir.join(format!("walk_{n:04}.png")).exists());
        }
        assert!(!dir.join("walk_0001.png").exists());

        // All six frames as MP4 (33 px tall: padded to 34).
        let mp4 = dir.join("clip.mp4");
        let video = ExportSettings {
            kind: ExportKind::Video,
            ..base
        };
        let outcome = run(video, &mp4, &session, &gpu);
        assert!(matches!(outcome, Outcome::Done(_, 6)), "{outcome:?}");
        let bytes = std::fs::read(&mp4).expect("mp4");
        assert_eq!(&bytes[4..8], b"ftyp");
        assert!(bytes.windows(4).any(|w| w == b"moov"));

        if keep.is_none() {
            std::fs::remove_dir_all(dir).expect("cleanup");
        }
    }

    #[test]
    fn cancelling_a_video_export_removes_the_file() {
        let Ok(gpu) = pollster::block_on(renderer::headless_device()) else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let dir =
            std::env::temp_dir().join(format!("rusting-desktop-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let session = demo_session();
        let path = dir.join("cancel.mp4");
        let mut panel = ExportPanel::new();
        let settings = ExportSettings {
            kind: ExportKind::Video,
            file_type: FileType::Png,
            scale_percent: 100,
            first: 1,
            last: 6,
            jpeg_quality: 90,
            transparent: false,
        };
        panel.start(settings, &path, &session).expect("start");
        let mut offscreen = OffscreenRenderer::new(&gpu.0);
        assert!(panel.pump(&gpu.0, &gpu.1, &mut offscreen).is_none());
        panel.job.as_mut().expect("job").cancel();
        let mut outcome = None;
        for _ in 0..10_000 {
            outcome = panel.pump(&gpu.0, &gpu.1, &mut offscreen);
            if outcome.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(matches!(outcome, Some(Outcome::Cancelled)), "{outcome:?}");
        assert!(!path.exists());
        std::fs::remove_dir_all(dir).expect("cleanup");
    }
}
