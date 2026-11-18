//! The library panel: every file imported into the project, in sections by
//! kind (ADR-017), and the objects made in it (ADR-018).
//!
//! Double-click a row to rename it. Right-click it to place it (a picture on
//! the canvas, a sound on a new audio layer at the current frame, an object
//! on a new layer on the canvas), use it (a brush, ADR-022), export it (a
//! brush) or delete it (only while nothing in the animation uses it). Drag a row to place it where it is dropped: a picture
//! or sound row sets an [`AssetDrag`] payload, an object row an
//! [`ObjectDrag`]. The host's canvas takes pictures and objects with
//! `Response::dnd_release_payload` and calls [`Session::place_asset`] or
//! [`Session::place_symbol_on_canvas`] with the drop point. The timeline
//! takes an [`ObjectDrag`] on a frame cell, and a sound on its frames to
//! start a new audio layer at the frame it is dropped on
//! ([`Session::place_audio`]).

use crate::icons::{self, IconFn};
use crate::session::Session;
use document::animation_core::{AssetId, SymbolId};
use document::{Asset, AssetKind, Document, Symbol};
use egui::{Align2, Rect, Sense, TextStyle, vec2};

/// Drag-and-drop payload of a library picture row being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssetDrag(pub AssetId);

/// Drag-and-drop payload of a library object row being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectDrag(pub SymbolId);

/// What a library row stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Entry {
    /// An imported file (ADR-017).
    Asset(AssetId),
    /// An object (ADR-018).
    Object(SymbolId),
}

/// Where a section's rows come from.
enum Source {
    /// Library assets of the kinds this returns true for.
    Assets(fn(&AssetKind) -> bool),
    /// The document's objects (symbols), kept apart from the assets.
    Objects,
}

/// One section of the panel.
struct Section {
    title: &'static str,
    /// What to say when the section is empty.
    empty: &'static str,
    icon: IconFn,
    source: Source,
}

/// The sections, top to bottom. A new asset kind adds an entry here.
const SECTIONS: &[Section] = &[
    Section {
        title: "Images",
        empty: "File → Import… adds PNG and JPEG pictures here.",
        icon: icons::picture,
        source: Source::Assets(|kind| matches!(kind, AssetKind::Image(_))),
    },
    Section {
        title: "Audio",
        empty: "File → Import… adds MP3 and WAV sounds here.",
        icon: icons::speaker,
        source: Source::Assets(|kind| matches!(kind, AssetKind::Audio(_))),
    },
    Section {
        title: "Brushes",
        empty: "Drawing with a built-in brush, or File → Import… of a .rustbrush file, adds \
                brushes here.",
        icon: icons::brush,
        source: Source::Assets(|kind| matches!(kind, AssetKind::Brush(_))),
    },
    Section {
        title: "Objects",
        empty: "Select layers and frames (or items on the canvas) and choose Convert to \
                Object (F8) to make one.",
        icon: icons::object,
        source: Source::Objects,
    },
];

/// One row to draw.
struct Row {
    entry: Entry,
    name: String,
    /// Tooltip description, e.g. "Image, 640×480".
    description: String,
    in_use: bool,
    /// A library sound: placed on the timeline, not on the canvas.
    is_sound: bool,
    /// A library brush: used by the Brush tool, not placed (ADR-022).
    is_brush: bool,
}

impl Section {
    /// The rows of this section, in library order.
    fn rows(&self, doc: &Document) -> Vec<Row> {
        match self.source {
            Source::Assets(holds) => {
                let used = doc.used_assets();
                doc.assets()
                    .values()
                    .filter(|a| holds(&a.kind))
                    .map(|a| Row {
                        entry: Entry::Asset(a.id),
                        name: a.name.clone(),
                        description: describe(a),
                        in_use: used.contains(&a.id),
                        is_sound: a.audio().is_some(),
                        is_brush: a.brush().is_some(),
                    })
                    .collect()
            }
            Source::Objects => {
                let mut symbols: Vec<&Symbol> = doc.symbols().values().collect();
                symbols.sort_by(|a, b| a.name.cmp(&b.name));
                symbols
                    .into_iter()
                    .map(|s| Row {
                        entry: Entry::Object(s.id),
                        name: s.name.clone(),
                        description: describe_object(s),
                        in_use: doc.symbol_in_use(s.id),
                        is_sound: false,
                        is_brush: false,
                    })
                    .collect()
            }
        }
    }
}

const ROW_HEIGHT: f32 = 22.0;

/// The row being renamed, kept in egui's temporary memory between frames.
#[derive(Clone, Default)]
struct Renaming {
    target: Option<Entry>,
    text: String,
    focus_requested: bool,
}

/// What a row asked for this frame; applied after the list is drawn.
enum Action {
    Place(Entry),
    ExportBrush(AssetId),
    Delete(Entry),
    Rename(Entry, String),
}

/// Whether `entry` still exists.
fn exists(doc: &Document, entry: Entry) -> bool {
    match entry {
        Entry::Asset(id) => doc.asset(id).is_some(),
        Entry::Object(id) => doc.symbol(id).is_some(),
    }
}

/// Draws the library panel.
pub fn panel(ui: &mut egui::Ui, session: &mut Session) {
    // The pane's tab carries the "Library" title (ADR-021).
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Import…")
                .on_hover_text(
                    "Add PNG or JPEG pictures, MP3 or WAV sounds or .rustbrush brushes to the \
                     library",
                )
                .clicked()
            {
                session.import();
            }
        });
    });
    ui.separator();

    let rename_id = ui.scope_id().with("asset_renaming");
    let mut renaming: Renaming = ui.data(|d| d.get_temp(rename_id)).unwrap_or_default();
    if renaming
        .target
        .is_some_and(|entry| !exists(&session.document, entry))
    {
        renaming = Renaming::default();
    }
    let mut action = None;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for section in SECTIONS {
                let rows = section.rows(&session.document);
                egui::CollapsingHeader::new(format!("{} ({})", section.title, rows.len()))
                    .id_salt(section.title)
                    .default_open(true)
                    .show(ui, |ui| {
                        if rows.is_empty() {
                            ui.weak(section.empty);
                        }
                        for r in &rows {
                            if let Some(a) = row(ui, section, r, &mut renaming) {
                                action = Some(a);
                            }
                        }
                    });
            }
        });
    ui.data_mut(|d| d.insert_temp(rename_id, renaming));

    match action {
        Some(Action::ExportBrush(id)) => session.export_brush(id),
        Some(Action::Place(Entry::Asset(id))) => {
            if session
                .document
                .asset(id)
                .is_some_and(|a| a.brush().is_some())
            {
                session.use_brush(id);
            } else if session
                .document
                .asset(id)
                .is_some_and(|a| a.audio().is_some())
            {
                let frame = session.current_frame();
                session.place_audio(id, frame);
            } else {
                session.place_asset(id, None);
            }
        }
        Some(Action::Place(Entry::Object(id))) => {
            session.place_symbol_on_canvas(id, None);
        }
        Some(Action::Delete(Entry::Asset(id))) => {
            session.delete_asset(id);
        }
        Some(Action::Delete(Entry::Object(id))) => {
            session.delete_symbol(id);
        }
        Some(Action::Rename(Entry::Asset(id), name)) => session.rename_asset(id, &name),
        Some(Action::Rename(Entry::Object(id), name)) => session.rename_symbol(id, &name),
        None => {}
    }
}

/// Draws one row: icon and name, or the rename field.
fn row(ui: &mut egui::Ui, section: &Section, r: &Row, renaming: &mut Renaming) -> Option<Action> {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let icon_rect = Rect::from_min_size(
        rect.left_center() - vec2(0.0, 8.0) + vec2(2.0, 0.0),
        vec2(16.0, 16.0),
    );
    let name_rect = Rect::from_x_y_ranges(icon_rect.right() + 6.0..=rect.right(), rect.y_range());
    let visuals = ui.visuals().clone();

    if renaming.target == Some(r.entry) {
        (section.icon)(ui.painter(), icon_rect, visuals.text_color());
        let edit = ui.put(
            name_rect.shrink2(vec2(0.0, 1.0)),
            egui::TextEdit::singleline(&mut renaming.text).margin(vec2(3.0, 1.0)),
        );
        if !renaming.focus_requested {
            edit.request_focus();
            renaming.focus_requested = true;
        }
        let (enter, escape) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if escape {
            *renaming = Renaming::default();
        } else if edit.lost_focus() || enter {
            let name = std::mem::take(&mut renaming.text);
            *renaming = Renaming::default();
            return Some(Action::Rename(r.entry, name));
        }
        return None;
    }

    let drop_hint = match r.entry {
        Entry::Asset(_) if r.is_brush => "Right-click → Use Brush to draw with it",
        Entry::Asset(_) if r.is_sound => "Drag onto the timeline to place it there",
        Entry::Asset(_) => "Drag onto the canvas to place it there",
        Entry::Object(_) => "Drag onto the canvas or onto a timeline cell to place it there",
    };
    let response = ui
        .interact(
            rect,
            ui.scope_id().with(("library_row", r.entry)),
            Sense::click_and_drag(),
        )
        .on_hover_text(format!(
            "{} ({}). {drop_hint}, double-click to rename, right-click for more.",
            r.name, r.description
        ));
    match r.entry {
        // Brushes are picked, not dropped anywhere.
        Entry::Asset(_) if r.is_brush => {}
        Entry::Asset(id) => response.dnd_set_drag_payload(AssetDrag(id)),
        Entry::Object(id) => response.dnd_set_drag_payload(ObjectDrag(id)),
    }
    if response.hovered() || response.dragged() {
        ui.painter()
            .rect_filled(rect, 2.0, visuals.widgets.hovered.weak_bg_fill);
    }
    if response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    let color = visuals.text_color();
    (section.icon)(ui.painter(), icon_rect, color);
    ui.painter_at(name_rect).text(
        name_rect.left_center(),
        Align2::LEFT_CENTER,
        &r.name,
        TextStyle::Body.resolve(ui.style()),
        color,
    );

    let mut action = None;
    if response.double_clicked() {
        *renaming = Renaming {
            target: Some(r.entry),
            text: r.name.clone(),
            focus_requested: false,
        };
    }
    response.context_menu(|ui| {
        let (place, place_hint) = match r.entry {
            Entry::Asset(_) if r.is_brush => (
                "Use Brush",
                "Draw with it: picks the Brush tool with this brush",
            ),
            Entry::Asset(_) if r.is_sound => (
                "Add to Timeline",
                "Put it on a new audio layer, starting at the current frame",
            ),
            Entry::Asset(_) => (
                "Place on Canvas",
                "Place it centred on the canvas, on the current layer and frame",
            ),
            Entry::Object(_) => (
                "Place on Canvas",
                "Place it centred on the canvas, on a new layer at this frame",
            ),
        };
        if ui.button(place).on_hover_text(place_hint).clicked() {
            action = Some(Action::Place(r.entry));
            ui.close();
        }
        if let Entry::Asset(id) = r.entry
            && r.is_brush
            && ui
                .button("Export Brush…")
                .on_hover_text("Save it as a .rustbrush file to share or use in other projects")
                .clicked()
        {
            action = Some(Action::ExportBrush(id));
            ui.close();
        }
        if ui.button("Rename").clicked() {
            *renaming = Renaming {
                target: Some(r.entry),
                text: r.name.clone(),
                focus_requested: false,
            };
            ui.close();
        }
        ui.separator();
        let delete = ui
            .add_enabled(!r.in_use, egui::Button::new("Delete"))
            .on_disabled_hover_text(if r.is_sound {
                "It is used in the animation: delete its audio layers first"
            } else if r.is_brush {
                "It is used in the animation: delete the strokes drawn with it first"
            } else {
                "It is used in the animation: delete its placed copies first"
            });
        if delete.clicked() {
            action = Some(Action::Delete(r.entry));
            ui.close();
        }
    });
    action
}

/// A short description of an asset for its tooltip, e.g. "Image, 640×480".
fn describe(asset: &Asset) -> String {
    match &asset.kind {
        AssetKind::Image(data) => format!("Image, {}×{}", data.width, data.height),
        AssetKind::Audio(data) => format!(
            "Audio, {}, {}",
            data.format.label(),
            format_duration(data.duration)
        ),
        AssetKind::Brush(data) => format!(
            "Brush, {}×{} tip{}",
            data.tip.width,
            data.tip.height,
            if data.grain.is_some() {
                " with paper grain"
            } else {
                ""
            }
        ),
    }
}

/// A short description of an object for its tooltip, e.g. "Object, 12
/// frames, 2 layers".
fn describe_object(symbol: &Symbol) -> String {
    let plural = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
    format!(
        "Object, {}, {}",
        plural(symbol.frame_count as usize, "frame"),
        plural(symbol.layers.len(), "layer")
    )
}

/// A length in seconds as `m:ss.s`, e.g. "1:05.3".
pub fn format_duration(secs: f64) -> String {
    let tenths = (secs.max(0.0) * 10.0).round() as u64;
    format!("{}:{:02}.{}", tenths / 600, tenths / 10 % 60, tenths % 10)
}

#[cfg(test)]
mod tests {
    use super::*;
    use document::drawing::ImageData;

    #[test]
    fn durations_read_as_minutes_and_seconds() {
        assert_eq!(format_duration(0.0), "0:00.0");
        assert_eq!(format_duration(65.34), "1:05.3");
        assert_eq!(format_duration(599.96), "10:00.0");
    }

    fn draw(session: &mut Session) {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| panel(ui, session));
        output.textures_delta.clear();
    }

    #[test]
    fn sounds_get_their_own_section() {
        let mut s = Session::new();
        let sound = document::AudioData {
            format: document::AudioFormat::Mp3,
            sample_rate: 44_100,
            channels: 2,
            duration: 65.3,
            peaks: Vec::new(),
            bytes: std::sync::Arc::from(vec![1u8]),
        };
        s.import_audio("song.mp3", sound).unwrap();
        let asset = s.document.assets().values().next().unwrap();
        let sections: Vec<&str> = SECTIONS
            .iter()
            .filter(|sec| !sec.rows(&s.document).is_empty())
            .map(|sec| sec.title)
            .collect();
        assert_eq!(sections, ["Audio"]);
        assert_eq!(describe(asset), "Audio, MP3, 1:05.3");
        let before = s.document.clone();
        draw(&mut s);
        assert_eq!(s.document, before);
    }

    #[test]
    fn panel_lists_assets_without_changing_the_document() {
        let mut s = Session::new();
        draw(&mut s);
        assert!(s.import_image("pic.png", ImageData::new(4, 4, vec![1])));
        let before = s.document.clone();
        draw(&mut s);
        assert_eq!(s.document, before);
        let rows: usize = SECTIONS.iter().map(|sec| sec.rows(&s.document).len()).sum();
        assert_eq!(rows, 1);
    }

    #[test]
    fn objects_have_their_own_section() {
        let mut s = Session::new();
        let mut symbol = Symbol::new("Ball", 12);
        symbol.push_layer(
            document::scene::Layer::new("in"),
            document::timeline::Track::new(),
        );
        s.document.insert_symbol(symbol);
        draw(&mut s);
        let objects = SECTIONS.iter().find(|sec| sec.title == "Objects").unwrap();
        let rows = objects.rows(&s.document);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Ball");
        assert_eq!(rows[0].description, "Object, 12 frames, 1 layer");
        assert!(!rows[0].in_use);
    }
}
