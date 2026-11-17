//! The library panel: every file imported into the project, in sections by
//! kind (ADR-017).
//!
//! Double-click a row to rename it. Right-click it to place it (a picture on
//! the canvas, a sound on a new audio layer at the current frame) or delete
//! it (only while nothing in the animation uses it). Drag a picture onto the
//! canvas to place it where it is dropped: the row sets an [`AssetDrag`]
//! payload, and the host's canvas takes it with
//! `Response::dnd_release_payload::<AssetDrag>()` and calls
//! [`Session::place_asset`] with the drop point. Drag a sound onto the
//! timeline's frames to start a new audio layer at the frame it is dropped
//! on ([`Session::place_audio`]).

use crate::icons::{self, IconFn};
use crate::session::Session;
use document::animation_core::AssetId;
use document::{Asset, AssetKind};
use egui::{Align2, Rect, Sense, TextStyle, vec2};

/// Drag-and-drop payload of a library row being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssetDrag(pub AssetId);

/// One section of the panel: the assets of one kind.
struct Section {
    title: &'static str,
    /// What to say when the section is empty.
    empty: &'static str,
    icon: IconFn,
    holds: fn(&AssetKind) -> bool,
}

/// The sections, top to bottom. A new asset kind adds an entry here.
const SECTIONS: &[Section] = &[
    Section {
        title: "Images",
        empty: "File → Import… adds PNG and JPEG pictures here.",
        icon: icons::picture,
        holds: |kind| matches!(kind, AssetKind::Image(_)),
    },
    Section {
        title: "Audio",
        empty: "File → Import… adds MP3 and WAV sounds here.",
        icon: icons::speaker,
        holds: |kind| matches!(kind, AssetKind::Audio(_)),
    },
];

const ROW_HEIGHT: f32 = 22.0;

/// The row being renamed, kept in egui's temporary memory between frames.
#[derive(Clone, Default)]
struct Renaming {
    target: Option<AssetId>,
    text: String,
    focus_requested: bool,
}

/// What a row asked for this frame; applied after the list is drawn.
enum Action {
    Place(AssetId),
    Delete(AssetId),
    Rename(AssetId, String),
}

/// Draws the library panel.
pub fn panel(ui: &mut egui::Ui, session: &mut Session) {
    ui.horizontal(|ui| {
        ui.heading("Library");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Import…")
                .on_hover_text("Add PNG or JPEG pictures or MP3 or WAV sounds to the library")
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
        .is_some_and(|id| session.document.asset(id).is_none())
    {
        renaming = Renaming::default();
    }
    let used = session.document.used_assets();
    let mut action = None;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for section in SECTIONS {
                let assets: Vec<&Asset> = session
                    .document
                    .assets()
                    .values()
                    .filter(|a| (section.holds)(&a.kind))
                    .collect();
                egui::CollapsingHeader::new(format!("{} ({})", section.title, assets.len()))
                    .id_salt(section.title)
                    .default_open(true)
                    .show(ui, |ui| {
                        if assets.is_empty() {
                            ui.weak(section.empty);
                        }
                        for asset in assets {
                            let in_use = used.contains(&asset.id);
                            if let Some(a) = row(ui, section, asset, in_use, &mut renaming) {
                                action = Some(a);
                            }
                        }
                    });
            }
        });
    ui.data_mut(|d| d.insert_temp(rename_id, renaming));

    match action {
        Some(Action::Place(id)) => {
            if session
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
        Some(Action::Delete(id)) => {
            session.delete_asset(id);
        }
        Some(Action::Rename(id, name)) => session.rename_asset(id, &name),
        None => {}
    }
}

/// Draws one asset row: icon and name, or the rename field.
fn row(
    ui: &mut egui::Ui,
    section: &Section,
    asset: &Asset,
    in_use: bool,
    renaming: &mut Renaming,
) -> Option<Action> {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let icon_rect = Rect::from_min_size(
        rect.left_center() - vec2(0.0, 8.0) + vec2(2.0, 0.0),
        vec2(16.0, 16.0),
    );
    let name_rect = Rect::from_x_y_ranges(icon_rect.right() + 6.0..=rect.right(), rect.y_range());
    let visuals = ui.visuals().clone();

    if renaming.target == Some(asset.id) {
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
            return Some(Action::Rename(asset.id, name));
        }
        return None;
    }

    let is_sound = asset.audio().is_some();
    let response = ui
        .interact(
            rect,
            ui.scope_id().with(("library_asset", asset.id)),
            Sense::click_and_drag(),
        )
        .on_hover_text(format!(
            "{} ({}). Drag onto the {} to place it there, double-click to rename, \
             right-click for more.",
            asset.name,
            describe(asset),
            if is_sound { "timeline" } else { "canvas" }
        ));
    response.dnd_set_drag_payload(AssetDrag(asset.id));
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
        &asset.name,
        TextStyle::Body.resolve(ui.style()),
        color,
    );

    let mut action = None;
    if response.double_clicked() {
        *renaming = Renaming {
            target: Some(asset.id),
            text: asset.name.clone(),
            focus_requested: false,
        };
    }
    response.context_menu(|ui| {
        let (place, place_tip) = if is_sound {
            (
                "Add to Timeline",
                "Put it on a new audio layer, starting at the current frame",
            )
        } else {
            (
                "Place on Canvas",
                "Place it centred on the canvas, on the current layer and frame",
            )
        };
        if ui.button(place).on_hover_text(place_tip).clicked() {
            action = Some(Action::Place(asset.id));
            ui.close();
        }
        if ui.button("Rename").clicked() {
            *renaming = Renaming {
                target: Some(asset.id),
                text: asset.name.clone(),
                focus_requested: false,
            };
            ui.close();
        }
        ui.separator();
        let delete = ui
            .add_enabled(!in_use, egui::Button::new("Delete"))
            .on_disabled_hover_text(if is_sound {
                "It is used in the animation: delete its audio layers first"
            } else {
                "It is used in the animation: delete its placed copies first"
            });
        if delete.clicked() {
            action = Some(Action::Delete(asset.id));
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
    }
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
            .filter(|sec| (sec.holds)(&asset.kind))
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
        assert!(
            SECTIONS
                .iter()
                .any(|sec| (sec.holds)(&s.document.assets().values().next().unwrap().kind))
        );
    }
}
