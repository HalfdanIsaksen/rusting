//! The open project and the editor actions on it.
//!
//! [`Session`] owns the document, its undo history and the editor state.
//! Panels call its methods; every document change goes through
//! [`Session::execute`], so it is undoable.

use crate::onion::OnionSkin;
use crate::playback::Playback;
use crate::preview::Preview;
use crate::tools::ToolController;
use document::animation_core::{FrameIndex, LayerId};
use document::{CanvasAnchor, CanvasSize, Document, NewDocumentSettings, SettingsError};
use editor::commands::{DuplicateFrame, ResizeCanvas};
use editor::{Command, CommandError, EditorState, History};
use std::collections::BTreeSet;
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

/// The start menu: settings for a new document, shown instead of the editor
/// at launch and for File → New.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartMenu {
    /// The values in the form.
    pub settings: NewDocumentSettings,
    /// The user picked the "Custom" card, so it stays highlighted even while
    /// the values still match a template.
    pub custom: bool,
    /// Whether there is a document behind the menu to go back to (File →
    /// New). At launch there is none, so the menu can't be cancelled.
    pub can_cancel: bool,
    /// Give the name field keyboard focus on the next frame.
    pub focus_name: bool,
}

impl StartMenu {
    /// A menu prefilled with `settings`.
    pub fn new(settings: NewDocumentSettings, can_cancel: bool) -> Self {
        Self {
            settings,
            custom: false,
            can_cancel,
            focus_name: true,
        }
    }
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

/// Which panels around the canvas are shown (the View menu). Hidden panels
/// give their space to the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewSettings {
    /// The tool strip on the left.
    pub toolbar: bool,
    /// The timeline at the bottom, with its layer column.
    pub timeline: bool,
    /// The tool settings panel on the right.
    pub tool_settings: bool,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            toolbar: true,
            timeline: true,
            tool_settings: true,
        }
    }
}

/// What the "Resize Canvas" dialog is editing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResizeCanvasDialog {
    /// New width in pixels.
    pub width: u32,
    /// New height in pixels.
    pub height: u32,
    /// The point the canvas grows or shrinks from.
    pub anchor: CanvasAnchor,
}

impl ResizeCanvasDialog {
    /// Starts from `size`, anchored at the centre.
    pub fn new(size: CanvasSize) -> Self {
        Self {
            width: size.width,
            height: size.height,
            anchor: CanvasAnchor::Center,
        }
    }

    /// The size entered.
    pub fn size(&self) -> CanvasSize {
        CanvasSize {
            width: self.width,
            height: self.height,
        }
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
    /// View → Preview: the preview window and its own playback.
    pub preview: Preview,
    /// Canvas tool settings and the gesture in progress.
    pub tools: ToolController,
    /// Timeline layout.
    pub timeline: TimelineView,
    /// Which panels are shown. Kept when the document changes.
    pub view: ViewSettings,
    /// Set by the canvas when Space was used to pan, so releasing Space does
    /// not also toggle playback.
    pub space_panned: bool,
    /// The open "Resize Canvas" dialog, if any.
    pub resize_canvas: Option<ResizeCanvasDialog>,
    /// Layers selected in the timeline besides the active one.
    layer_selection: BTreeSet<LayerId>,
    status: Option<Status>,
    dialog: Option<Dialog>,
    requests: Vec<Request>,
    start_menu: Option<StartMenu>,
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
        let preview = Preview::new(&document);
        Self {
            document,
            history: History::new(),
            state,
            path,
            playback,
            onion: OnionSkin::default(),
            preview,
            tools: ToolController::default(),
            timeline: TimelineView::default(),
            view: ViewSettings::default(),
            space_panned: false,
            resize_canvas: None,
            layer_selection: BTreeSet::new(),
            status: None,
            dialog: None,
            requests: Vec::new(),
            start_menu: None,
        }
    }

    /// Replaces the document and resets history, selection and playback.
    /// Tool, brush, onion-skin, timeline and view settings are kept.
    fn replace_document(&mut self, document: Document, path: Option<PathBuf>) {
        let tool = self.state.tool;
        let brush = self.state.brush;
        self.state = EditorState::for_document(&document);
        self.layer_selection.clear();
        self.state.tool = tool;
        self.state.brush = brush;
        self.playback = Playback::new(&document);
        self.preview.reset(&document);
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
        self.layer_selection.retain(|&id| layers.get(id).is_some());
        let frame = self.state.current_frame();
        self.state.set_current_frame(&self.document, frame);
        self.playback.sync(&self.document);
        self.sync_selection();
    }

    // ----- Canvas ---------------------------------------------------------

    /// Opens the "Resize Canvas" dialog, prefilled with the current size.
    pub fn open_resize_canvas(&mut self) {
        self.resize_canvas = Some(ResizeCanvasDialog::new(self.document.canvas));
    }

    /// Resizes the canvas to `size`, keeping artwork in place relative to
    /// `anchor`. One undo step; does nothing if the size is unchanged.
    pub fn resize_canvas(&mut self, size: CanvasSize, anchor: CanvasAnchor) {
        if size == self.document.canvas {
            return;
        }
        self.tools.cancel();
        if self
            .execute(Box::new(ResizeCanvas::new(size, anchor)))
            .is_ok()
        {
            self.set_status(format!("Canvas resized to {}×{}", size.width, size.height));
        }
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

    /// Duplicates the frame under the playhead on every layer (one undo
    /// step) and moves the playhead onto the copy.
    pub fn duplicate_frame(&mut self) {
        if self.tools.is_active() {
            return;
        }
        self.tools.cancel();
        self.playback.pause();
        let command = DuplicateFrame::new(self.current_frame());
        let new_frame = command.new_frame();
        if self.execute(Box::new(command)).is_ok() {
            self.set_frame(new_frame);
            self.set_status(format!(
                "Duplicated to frame {}",
                new_frame.display_number()
            ));
        }
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

    /// Selects only `layer` and makes it the layer drawing tools act on.
    pub fn select_layer(&mut self, layer: LayerId) {
        if self.document.layers().get(layer).is_some() {
            self.layer_selection.clear();
            self.set_active_layer(layer);
        }
    }

    /// Makes `layer` the layer drawing tools act on, keeping the rest of the
    /// layer selection.
    pub fn set_active_layer(&mut self, layer: LayerId) {
        if self.document.layers().get(layer).is_some() {
            self.tools.cancel();
            self.state.active_layer = Some(layer);
            self.sync_selection();
        }
    }

    /// Selects `layers` with `active` as the active one.
    pub fn select_layers(&mut self, active: LayerId, layers: impl IntoIterator<Item = LayerId>) {
        self.select_layer(active);
        self.layer_selection.extend(layers);
    }

    /// Whether `layer` is selected in the timeline (the active layer always is).
    pub fn is_layer_selected(&self, layer: LayerId) -> bool {
        self.state.active_layer == Some(layer) || self.layer_selection.contains(&layer)
    }

    /// The selected layers, top of the stack first.
    pub fn selected_layers(&self) -> Vec<LayerId> {
        self.document
            .layers()
            .iter()
            .rev()
            .map(|l| l.id)
            .filter(|&id| self.is_layer_selected(id))
            .collect()
    }

    /// Cmd/Ctrl+click: adds `layer` to the selection and makes it active, or
    /// takes it out again. The last selected layer stays selected.
    pub fn toggle_layer_selected(&mut self, layer: LayerId) {
        if !self.is_layer_selected(layer) {
            self.layer_selection.extend(self.state.active_layer);
            self.layer_selection.insert(layer);
            self.set_active_layer(layer);
            return;
        }
        self.layer_selection.remove(&layer);
        if self.state.active_layer == Some(layer)
            && let Some(other) = self.selected_layers().into_iter().find(|&id| id != layer)
        {
            self.set_active_layer(other);
        }
    }

    /// Shift+click: selects every layer from the active one to `layer`. The
    /// active layer stays the anchor.
    pub fn select_layer_range(&mut self, layer: LayerId) {
        let layers = self.document.layers();
        let (Some(to), Some(from)) = (
            layers.index_of(layer),
            self.state.active_layer.and_then(|id| layers.index_of(id)),
        ) else {
            return self.select_layer(layer);
        };
        self.layer_selection = layers
            .iter()
            .skip(from.min(to))
            .take(from.abs_diff(to) + 1)
            .map(|l| l.id)
            .collect();
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

    /// The frame the preview window shows: its own while it plays, else
    /// the editor's current frame.
    pub fn preview_frame(&self) -> FrameIndex {
        self.preview.frame(self.current_frame())
    }

    /// Starts or stops the preview window's own looping playback.
    pub fn toggle_preview_playback(&mut self) {
        let frame = self.current_frame();
        self.preview.toggle(&self.document, frame);
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

    /// The window title, with a `*` when there are unsaved changes. Just
    /// the app name while the start menu is up.
    pub fn title(&self) -> String {
        if self.start_menu.is_some() {
            return "Rusting".to_owned();
        }
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
                // Start from the current document's size, rate and length.
                let doc = &self.document;
                let fps = doc.fps.as_f64().round().max(1.0) as u32;
                let settings = NewDocumentSettings {
                    name: "Untitled".to_owned(),
                    width: doc.canvas.width,
                    height: doc.canvas.height,
                    fps,
                    frame_count: doc.frame_count.max(1),
                };
                self.show_start_menu(StartMenu::new(settings, true));
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
                self.start_menu = None;
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

    /// Shows the start menu instead of the editor.
    pub fn show_start_menu(&mut self, menu: StartMenu) {
        self.tools.cancel();
        self.playback.pause();
        self.start_menu = Some(menu);
    }

    /// The start menu, if it is showing.
    pub fn start_menu(&self) -> Option<&StartMenu> {
        self.start_menu.as_ref()
    }

    /// Mutable access to the start menu's form.
    pub fn start_menu_mut(&mut self) -> Option<&mut StartMenu> {
        self.start_menu.as_mut()
    }

    /// Goes back to the editor without a new document. Does nothing if the
    /// menu can't be cancelled (at launch).
    pub fn cancel_start_menu(&mut self) {
        if self.start_menu.as_ref().is_some_and(|m| m.can_cancel) {
            self.start_menu = None;
        }
    }

    /// Replaces the document with a new one made from the start menu's
    /// settings and closes the menu. Invalid settings leave everything as is.
    pub fn create_from_start_menu(&mut self) -> Result<(), SettingsError> {
        let Some(menu) = &self.start_menu else {
            return Ok(());
        };
        let document = menu.settings.create()?;
        let name = document.name.clone();
        self.replace_document(document, None);
        self.start_menu = None;
        self.set_status(format!("New document “{name}”"));
        Ok(())
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
    fn resize_canvas_is_one_undo_step() {
        let mut s = Session::new();
        s.open_resize_canvas();
        let dialog = s.resize_canvas.take().unwrap();
        assert_eq!(dialog.size(), s.document.canvas);
        assert_eq!(dialog.anchor, CanvasAnchor::Center);

        let new = CanvasSize {
            width: 800,
            height: 600,
        };
        s.resize_canvas(new, CanvasAnchor::TopLeft);
        assert_eq!(s.document.canvas, new);
        assert!(s.is_dirty());
        assert_eq!(s.history.undo_label(), Some("Resize Canvas"));
        s.undo();
        assert_eq!(s.document.canvas.width, 1920);
        assert!(!s.is_dirty());

        // The same size is not recorded.
        let same = s.document.canvas;
        s.resize_canvas(same, CanvasAnchor::Center);
        assert!(!s.history.can_undo());
    }

    #[test]
    fn view_shows_all_panels_by_default_and_survives_opening_a_file() {
        let mut s = Session::new();
        assert_eq!(s.view, ViewSettings::default());
        assert!(s.view.toolbar && s.view.timeline && s.view.tool_settings);

        let path = temp_path("view.rusting");
        assert!(s.save_to(&path));
        s.view.toolbar = false;
        assert!(s.open_path(&path));
        assert!(!s.view.toolbar);
        assert!(s.view.timeline);
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
    fn layer_selection_toggles_ranges_and_survives_removal() {
        let mut s = Session::new();
        let a = s.state.active_layer.unwrap();
        let [b, c] = ["B", "C"].map(|name| {
            let add = AddLayer::new(name);
            let id = add.layer_id();
            s.execute(Box::new(add)).unwrap();
            id
        });
        // Stack bottom to top: a, b, c.
        s.select_layer(a);
        s.toggle_layer_selected(c);
        assert_eq!(s.state.active_layer, Some(c));
        assert_eq!(s.selected_layers(), [c, a]);
        s.toggle_layer_selected(c);
        assert_eq!(s.state.active_layer, Some(a));
        assert_eq!(s.selected_layers(), [a]);
        s.toggle_layer_selected(a);
        assert_eq!(s.selected_layers(), [a], "the last layer stays selected");

        s.select_layer_range(c);
        assert_eq!(s.selected_layers(), [c, b, a]);
        assert_eq!(s.state.active_layer, Some(a));
        s.select_layer(b);
        assert_eq!(s.selected_layers(), [b]);

        s.select_layers(c, [a]);
        s.execute(Box::new(RemoveLayer::new(a))).unwrap();
        assert_eq!(s.selected_layers(), [c]);
        s.undo();
        assert_eq!(s.selected_layers(), [c], "undo does not reselect");
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
        assert!(s.start_menu().is_some());
        s.create_from_start_menu().unwrap();
        assert!(!s.is_dirty());
        assert_eq!(s.document.layers().len(), 1);
    }

    #[test]
    fn start_menu_creates_a_document_with_its_settings() {
        let mut s = Session::new();
        let mut menu = StartMenu::new(NewDocumentSettings::default(), false);
        menu.settings = NewDocumentSettings {
            name: "Bounce".into(),
            width: 640,
            height: 480,
            fps: 12,
            frame_count: 30,
        };
        s.show_start_menu(menu);
        assert_eq!(s.title(), "Rusting");

        // Can't cancel at launch: there's no document to go back to.
        s.cancel_start_menu();
        assert!(s.start_menu().is_some());

        s.create_from_start_menu().unwrap();
        assert!(s.start_menu().is_none());
        let doc = &s.document;
        assert_eq!(doc.name, "Bounce");
        assert_eq!((doc.canvas.width, doc.canvas.height), (640, 480));
        assert_eq!(doc.fps, document::animation_core::Fps::whole(12).unwrap());
        assert_eq!(doc.frame_count, 30);
        assert_eq!(s.playback.fps(), doc.fps);
        assert!(s.state.can_draw(doc));
        assert_eq!(s.title(), "Bounce — Rusting");
        // The timeline is as long as the document.
        s.step(-1);
        assert_eq!(s.current_frame(), FrameIndex(29));
    }

    #[test]
    fn invalid_start_menu_settings_keep_the_menu_open() {
        let mut s = Session::new();
        let before = s.document.clone();
        let mut menu = StartMenu::new(NewDocumentSettings::default(), true);
        menu.settings.frame_count = 0;
        s.show_start_menu(menu);
        assert_eq!(s.create_from_start_menu(), Err(SettingsError::FrameCount));
        assert!(s.start_menu().is_some());
        assert_eq!(s.document, before);
        s.cancel_start_menu();
        assert!(s.start_menu().is_none());
        assert_eq!(s.document, before);
    }

    #[test]
    fn file_new_prefills_from_the_current_document() {
        let mut s = Session::new();
        s.document.canvas = CanvasSize {
            width: 800,
            height: 600,
        };
        s.document.frame_count = 10;
        s.request(DiscardAction::New);
        let menu = s.start_menu().unwrap();
        assert!(menu.can_cancel);
        assert_eq!(
            (menu.settings.width, menu.settings.height, menu.settings.fps),
            (800, 600, 24)
        );
        assert_eq!(menu.settings.frame_count, 10);
    }

    #[test]
    fn opening_a_file_closes_the_start_menu() {
        let mut s = Session::new();
        let path = temp_path("from-start-menu");
        assert!(s.save_to(&path));
        let saved = s.path.clone().unwrap();
        let mut other = Session::new();
        other.show_start_menu(StartMenu::new(NewDocumentSettings::default(), false));
        assert!(other.open_path(&saved));
        assert!(other.start_menu().is_none());
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
