//! The open project and the editor actions on it.
//!
//! [`Session`] owns the document, its undo history and the editor state.
//! Panels call its methods; every document change goes through
//! [`Session::execute`], so it is undoable.

use crate::onion::OnionSkin;
use crate::playback::Playback;
use crate::tools::ToolController;
use document::animation_core::{FrameIndex, LayerId};
use document::{CanvasSize, Document};
use editor::{Command, CommandError, EditorState, History};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// File extension of native project files.
pub const PROJECT_EXTENSION: &str = "rusting";

/// How long a status message stays visible.
pub const STATUS_DURATION: Duration = Duration::from_secs(5);

/// Work the host application must do for the UI (native dialogs, quitting).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Show an open-file dialog, then call [`Session::open_path`].
    OpenDialog,
    /// Show a save-file dialog, then call [`Session::save_to`].
    SaveAsDialog,
    /// Close the application. Unsaved changes were already confirmed.
    Quit,
    /// Show the export options for `ExportKind`, then a save dialog.
    Export(ExportKind),
}

/// What File → Export writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    /// The current frame as one PNG or JPEG.
    Frame,
    /// Every frame (or a range) as numbered PNG or JPEG files.
    Sequence,
    /// The animation as an H.264 MP4 video.
    Video,
}

/// An action that throws away the current document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscardAction {
    /// Start a new document.
    New,
    /// Open another document.
    Open,
    /// Quit the application.
    Quit,
}

/// A modal dialog shown over the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dialog {
    /// Something failed; the message explains what.
    Error {
        /// Short title.
        title: String,
        /// Full explanation.
        message: String,
    },
    /// Ask before discarding unsaved changes.
    ConfirmDiscard(DiscardAction),
}

/// A short message in the status bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// The text shown.
    pub text: String,
    /// Shown in the error colour.
    pub is_error: bool,
    /// When it was set.
    pub at: Instant,
}

/// Layout state of the timeline panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineView {
    /// Width of one frame column, in points.
    pub cell_width: f32,
}

impl TimelineView {
    /// Narrowest frame column.
    pub const MIN_CELL_WIDTH: f32 = 6.0;
    /// Widest frame column.
    pub const MAX_CELL_WIDTH: f32 = 48.0;
}

impl Default for TimelineView {
    fn default() -> Self {
        Self { cell_width: 16.0 }
    }
}

/// The open project and everything the editor does to it.
#[derive(Debug)]
pub struct Session {
    /// The open document. Change it only through [`Self::execute`].
    pub document: Document,
    /// Undo/redo history of `document`.
    pub history: History,
    /// Active tool, brush, layer and frame.
    pub state: EditorState,
    /// Where the document was loaded from or last saved to.
    pub path: Option<PathBuf>,
    /// Playback clock.
    pub playback: Playback,
    /// Onion-skin settings.
    pub onion: OnionSkin,
    /// Canvas tool settings and the gesture in progress.
    pub tools: ToolController,
    /// Timeline layout.
    pub timeline: TimelineView,
    /// Set by the canvas when Space was used to pan, so releasing Space does
    /// not also toggle playback.
    pub space_panned: bool,
    status: Option<Status>,
    dialog: Option<Dialog>,
    requests: Vec<Request>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// A session with a new, empty document.
    pub fn new() -> Self {
        Self::with_document(new_document(), None)
    }

    /// A session showing `document`, with empty history.
    pub fn with_document(document: Document, path: Option<PathBuf>) -> Self {
        let state = EditorState::for_document(&document);
        let playback = Playback::new(&document);
        Self {
            document,
            history: History::new(),
            state,
            path,
            playback,
            onion: OnionSkin::default(),
            tools: ToolController::default(),
            timeline: TimelineView::default(),
            space_panned: false,
            status: None,
            dialog: None,
            requests: Vec::new(),
        }
    }

    /// Replaces the document and resets history, selection and playback.
    /// Tool, brush, onion-skin and timeline settings are kept.
    fn replace_document(&mut self, document: Document, path: Option<PathBuf>) {
        let tool = self.state.tool;
        let brush = self.state.brush;
        self.state = EditorState::for_document(&document);
        self.state.tool = tool;
        self.state.brush = brush;
        self.playback = Playback::new(&document);
        self.document = document;
        self.history.clear();
        self.path = path;
        self.tools.cancel();
        self.sync_selection();
    }

    // ----- Commands -----------------------------------------------------

    /// Runs `command` through the history. On failure the error is shown in
    /// the status bar and returned.
    pub fn execute(&mut self, command: Box<dyn Command>) -> Result<(), CommandError> {
        let label = command.label().to_owned();
        let result = self.history.execute(&mut self.document, command);
        match &result {
            Ok(()) => self.after_change(),
            Err(err) => self.set_error(format!("{label}: {}", describe_error(err))),
        }
        result
    }

    /// Undoes the last command.
    pub fn undo(&mut self) {
        self.tools.cancel();
        let label = self.history.undo_label().map(str::to_owned);
        match self.history.undo(&mut self.document) {
            Ok(true) => {
                self.after_change();
                self.set_status(format!("Undo {}", label.unwrap_or_default()));
            }
            Ok(false) => self.set_status("Nothing to undo"),
            Err(err) => self.set_error(format!("Undo failed: {}", describe_error(&err))),
        }
    }

    /// Redoes the last undone command.
    pub fn redo(&mut self) {
        self.tools.cancel();
        let label = self.history.redo_label().map(str::to_owned);
        match self.history.redo(&mut self.document) {
            Ok(true) => {
                self.after_change();
                self.set_status(format!("Redo {}", label.unwrap_or_default()));
            }
            Ok(false) => self.set_status("Nothing to redo"),
            Err(err) => self.set_error(format!("Redo failed: {}", describe_error(&err))),
        }
    }

    /// Keeps the selection, playhead and playback valid after the document
    /// changed.
    fn after_change(&mut self) {
        let layers = self.document.layers();
        if self
            .state
            .active_layer
            .is_none_or(|id| layers.get(id).is_none())
        {
            self.state.active_layer = layers.iter().next_back().map(|l| l.id);
        }
        let frame = self.state.current_frame();
        self.state.set_current_frame(&self.document, frame);
        self.playback.sync(&self.document);
        self.sync_selection();
    }

    // ----- Frame and layer selection -------------------------------------

    /// The frame on the canvas and under the timeline playhead.
    pub fn current_frame(&self) -> FrameIndex {
        self.state.current_frame()
    }

    /// Moves the playhead (clamped to the document). Playback follows.
    pub fn set_frame(&mut self, frame: FrameIndex) {
        self.state.set_current_frame(&self.document, frame);
        self.playback.seek(self.state.current_frame());
        self.sync_selection();
    }

    /// Steps the playhead by `delta` frames, wrapping around the document.
    pub fn step(&mut self, delta: i64) {
        let count = i64::from(self.document.frame_count.max(1));
        let frame = (i64::from(self.current_frame().0) + delta).rem_euclid(count);
        self.set_frame(FrameIndex(u32::try_from(frame).unwrap_or(0)));
    }

    /// Jumps to the previous (`forward == false`) or next key on the active layer.
    pub fn jump_to_key(&mut self, forward: bool) {
        let Some(track) = self
            .state
            .active_layer
            .and_then(|id| self.document.track(id))
        else {
            return;
        };
        let frame = self.current_frame();
        let target = if forward {
            track.next_key_after(frame)
        } else {
            track.prev_key_before(frame)
        };
        if let Some((key, _)) = target {
            self.set_frame(key);
        }
    }

    /// Makes `layer` the layer drawing tools act on.
    pub fn select_layer(&mut self, layer: LayerId) {
        if self.document.layers().get(layer).is_some() {
            self.tools.cancel();
            self.state.active_layer = Some(layer);
            self.sync_selection();
        }
    }

    /// Advances playback. Call once per frame before drawing the UI.
    /// Returns true while playing.
    pub fn update_playback(&mut self) -> bool {
        self.playback.sync(&self.document);
        if let Some(frame) = self.playback.tick() {
            self.state.set_current_frame(&self.document, frame);
            self.sync_selection();
        }
        self.playback.is_playing()
    }

    /// Starts or stops playback from the current frame.
    pub fn toggle_playback(&mut self) {
        self.tools.cancel();
        self.playback.sync(&self.document);
        self.playback.toggle(self.current_frame());
    }

    // ----- Files ---------------------------------------------------------

    /// Whether there are unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.history.is_dirty()
    }

    /// The document's display name: the file name, or the document name.
    pub fn display_name(&self) -> String {
        self.path.as_deref().and_then(Path::file_name).map_or_else(
            || self.document.name.clone(),
            |n| n.to_string_lossy().into_owned(),
        )
    }

    /// The window title, with a `*` when there are unsaved changes.
    pub fn title(&self) -> String {
        let dirty = if self.is_dirty() { "*" } else { "" };
        format!("{}{dirty} — Rusting", self.display_name())
    }

    /// Runs `action`, asking first if that would lose unsaved changes.
    pub fn request(&mut self, action: DiscardAction) {
        if self.is_dirty() {
            self.dialog = Some(Dialog::ConfirmDiscard(action));
        } else {
            self.perform(action);
        }
    }

    /// Runs `action` without asking.
    pub(crate) fn perform(&mut self, action: DiscardAction) {
        match action {
            DiscardAction::New => {
                self.replace_document(new_document(), None);
                self.set_status("New document");
            }
            DiscardAction::Open => self.requests.push(Request::OpenDialog),
            DiscardAction::Quit => self.requests.push(Request::Quit),
        }
    }

    /// Opens the project at `path`. Failures are shown in an error dialog
    /// and leave the current document untouched.
    pub fn open_path(&mut self, path: &Path) -> bool {
        match file_format::load_with_report(path) {
            Ok((document, report)) => {
                self.replace_document(document, Some(path.to_path_buf()));
                if report.is_clean() {
                    self.set_status(format!("Opened {}", path.display()));
                } else {
                    tracing::warn!(fields = ?report.unknown_fields, "ignored unknown fields");
                    self.set_error(format!(
                        "Opened {}, but ignored {} unknown field(s) (saved by a newer version?)",
                        path.display(),
                        report.unknown_fields.len()
                    ));
                }
                true
            }
            Err(err) => {
                tracing::warn!(%err, path = %path.display(), "open failed");
                self.dialog = Some(Dialog::Error {
                    title: "Could not open file".into(),
                    message: format!("{}\n\n{err}", path.display()),
                });
                false
            }
        }
    }

    /// Saves to the current path, or asks for one.
    pub fn save(&mut self) {
        match self.path.clone() {
            Some(path) => {
                self.save_to(&path);
            }
            None => self.requests.push(Request::SaveAsDialog),
        }
    }

    /// Asks for a path and saves there.
    pub fn save_as(&mut self) {
        self.requests.push(Request::SaveAsDialog);
    }

    /// Saves to `path` (adding the project extension if it has none) and
    /// makes it the current path. Failures are shown in an error dialog.
    pub fn save_to(&mut self, path: &Path) -> bool {
        let mut path = path.to_path_buf();
        if path.extension().is_none() {
            path.set_extension(PROJECT_EXTENSION);
        }
        match file_format::save(&self.document, &path) {
            Ok(()) => {
                self.history.mark_saved();
                self.set_status(format!("Saved {}", path.display()));
                self.path = Some(path);
                true
            }
            Err(err) => {
                tracing::warn!(%err, path = %path.display(), "save failed");
                self.dialog = Some(Dialog::Error {
                    title: "Could not save file".into(),
                    message: format!("{}\n\n{err}", path.display()),
                });
                false
            }
        }
    }

    /// Asks the host to export (it shows the options and a save dialog).
    pub fn export(&mut self, kind: ExportKind) {
        self.requests.push(Request::Export(kind));
    }

    /// Shows an error dialog.
    pub fn show_error(&mut self, title: impl Into<String>, message: impl Into<String>) {
        self.dialog = Some(Dialog::Error {
            title: title.into(),
            message: message.into(),
        });
    }

    /// Requests for the host application, oldest first. Clears the queue.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    // ----- Messages ------------------------------------------------------

    /// Shows `text` in the status bar.
    pub fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            is_error: false,
            at: Instant::now(),
        });
    }

    /// Shows `text` in the status bar as an error.
    pub fn set_error(&mut self, text: impl Into<String>) {
        let text = text.into();
        tracing::info!(%text, "editor error");
        self.status = Some(Status {
            text,
            is_error: true,
            at: Instant::now(),
        });
    }

    /// The current status message, if it hasn't expired.
    pub fn status(&self) -> Option<&Status> {
        self.status
            .as_ref()
            .filter(|s| s.at.elapsed() < STATUS_DURATION)
    }

    /// The open modal dialog.
    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref()
    }

    /// Closes the modal dialog.
    pub fn close_dialog(&mut self) {
        self.dialog = None;
    }
}

/// A new document: 1920×1080 with one layer.
pub fn new_document() -> Document {
    let mut document = Document::new(
        "Untitled",
        CanvasSize {
            width: 1920,
            height: 1080,
        },
    );
    document.add_layer("Layer 1");
    document
}

/// A user-facing explanation of a command error.
pub fn describe_error(err: &CommandError) -> String {
    match err {
        CommandError::LayerLocked => "the layer is locked".into(),
        CommandError::KeyframeExists(frame) => {
            format!("frame {} already has a key", frame.display_number())
        }
        CommandError::KeyframeNotFound(frame) => {
            format!("frame {} has no key", frame.display_number())
        }
        CommandError::FrameOutOfRange { frame, frame_count } => format!(
            "frame {} is past the end ({frame_count} frames)",
            frame.display_number()
        ),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::commands::{AddLayer, RemoveLayer};

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rusting-ui-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn new_session_has_a_drawable_layer() {
        let s = Session::new();
        assert_eq!(s.document.layers().len(), 1);
        assert!(s.state.can_draw(&s.document));
        assert!(!s.is_dirty());
        assert_eq!(s.title(), "Untitled — Rusting");
    }

    #[test]
    fn removing_the_active_layer_selects_another_and_undo_keeps_it_valid() {
        let mut s = Session::new();
        let bottom = s.state.active_layer.unwrap();
        let add = AddLayer::new("Top");
        let top = add.layer_id();
        s.execute(Box::new(add)).unwrap();
        s.select_layer(top);
        s.execute(Box::new(RemoveLayer::new(top))).unwrap();
        assert_eq!(s.state.active_layer, Some(bottom));
        assert!(s.is_dirty());
        assert!(s.title().contains('*'));
        s.undo();
        s.undo();
        assert_eq!(s.state.active_layer, Some(bottom));
        s.redo();
        assert_eq!(s.document.layers().len(), 2);
    }

    #[test]
    fn failed_command_shows_an_error() {
        let mut s = Session::new();
        assert!(
            s.execute(Box::new(RemoveLayer::new(LayerId::new())))
                .is_err()
        );
        assert!(s.status().is_some_and(|st| st.is_error));
        assert!(!s.history.can_undo());
    }

    #[test]
    fn step_wraps_and_keys_jump() {
        let mut s = Session::new();
        s.step(-1);
        assert_eq!(s.current_frame(), FrameIndex(23));
        s.step(1);
        assert_eq!(s.current_frame(), FrameIndex(0));
        let layer = s.state.active_layer.unwrap();
        s.execute(Box::new(editor::commands::AddKeyframe::new(
            layer,
            FrameIndex(4),
            editor::commands::KeyframeKind::Drawing,
        )))
        .unwrap();
        s.jump_to_key(true);
        assert_eq!(s.current_frame(), FrameIndex(4));
        s.jump_to_key(true);
        assert_eq!(s.current_frame(), FrameIndex(4));
    }

    #[test]
    fn save_marks_clean_and_open_restores() {
        let mut s = Session::new();
        s.execute(Box::new(AddLayer::new("Second"))).unwrap();
        let path = temp_path("roundtrip");
        assert!(s.save_to(&path));
        assert!(!s.is_dirty());
        let saved = s.path.clone().unwrap();
        assert_eq!(saved.extension().unwrap(), PROJECT_EXTENSION);

        let mut other = Session::new();
        assert!(other.open_path(&saved));
        assert_eq!(other.document, s.document);
        assert!(!other.is_dirty());
        assert!(other.title().starts_with("roundtrip.rusting"));
    }

    #[test]
    fn corrupt_file_shows_error_and_keeps_document() {
        let path = temp_path("corrupt.rusting");
        std::fs::write(&path, "{ not json").unwrap();
        let mut s = Session::new();
        let before = s.document.clone();
        assert!(!s.open_path(&path));
        assert!(matches!(s.dialog(), Some(Dialog::Error { .. })));
        assert_eq!(s.document, before);
    }

    #[test]
    fn discarding_changes_asks_first() {
        let mut s = Session::new();
        s.request(DiscardAction::Open);
        assert_eq!(s.take_requests(), [Request::OpenDialog]);

        s.execute(Box::new(AddLayer::new("X"))).unwrap();
        s.request(DiscardAction::Quit);
        assert!(s.take_requests().is_empty());
        assert_eq!(
            s.dialog(),
            Some(&Dialog::ConfirmDiscard(DiscardAction::Quit))
        );
        s.perform(DiscardAction::New);
        assert!(!s.is_dirty());
        assert_eq!(s.document.layers().len(), 1);
    }

    #[test]
    fn export_asks_the_host_and_errors_show_a_dialog() {
        let mut s = Session::new();
        s.export(ExportKind::Video);
        assert_eq!(s.take_requests(), [Request::Export(ExportKind::Video)]);
        s.show_error("Export failed", "disk full");
        assert!(
            matches!(s.dialog(), Some(Dialog::Error { title, .. }) if title == "Export failed")
        );
    }

    #[test]
    fn save_without_path_asks_for_one() {
        let mut s = Session::new();
        s.save();
        assert_eq!(s.take_requests(), [Request::SaveAsDialog]);
    }
}
