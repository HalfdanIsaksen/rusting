//! Keyboard shortcuts.

use crate::session::{DiscardAction, Session};
use editor::Tool;
use egui::{Key, KeyboardShortcut, Modifiers};

/// New document.
pub const NEW: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
/// Open a document.
pub const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
/// Save.
pub const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
/// Save under a new name.
pub const SAVE_AS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
/// Undo.
pub const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
/// Redo.
pub const REDO: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
/// Redo (Windows/Linux habit).
pub const REDO_ALT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);
/// Select everything on the active layer's current drawing.
pub const SELECT_ALL: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::A);
/// Quit.
pub const QUIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Q);

/// Show or hide the tool strip.
pub const TOGGLE_TOOLBAR: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Num1);
/// Show or hide the timeline.
pub const TOGGLE_TIMELINE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Num2);

/// Handles global shortcuts. Plain-key shortcuts are skipped while a text
/// field has keyboard focus.
pub fn handle(ctx: &egui::Context, session: &mut Session) {
    let typing = ctx.egui_wants_keyboard_input();
    let modal_open = session.dialog().is_some() || session.resize_canvas.is_some();

    // Most specific first: Shift+Cmd+S before Cmd+S (egui ignores extra Shift).
    let (save_as, save, new, open, quit, toolbar, timeline) = ctx.input_mut(|i| {
        (
            i.consume_shortcut(&SAVE_AS),
            i.consume_shortcut(&SAVE),
            i.consume_shortcut(&NEW),
            i.consume_shortcut(&OPEN),
            i.consume_shortcut(&QUIT),
            i.consume_shortcut(&TOGGLE_TOOLBAR),
            i.consume_shortcut(&TOGGLE_TIMELINE),
        )
    });
    if !modal_open {
        session.view.toolbar ^= toolbar;
        session.view.timeline ^= timeline;
        if save_as {
            session.save_as();
        } else if save {
            session.save();
        }
        if new {
            session.request(DiscardAction::New);
        }
        if open {
            session.request(DiscardAction::Open);
        }
        if quit {
            session.request(DiscardAction::Quit);
        }
    }
    if typing || modal_open {
        return;
    }

    let (redo, undo) = ctx.input_mut(|i| {
        let redo = i.consume_shortcut(&REDO) | i.consume_shortcut(&REDO_ALT);
        (redo, i.consume_shortcut(&UNDO))
    });
    if redo {
        session.redo();
    } else if undo {
        session.undo();
    }

    let gesture = session.tools.is_active();
    if ctx.input_mut(|i| i.consume_shortcut(&SELECT_ALL)) && !gesture {
        session.state.tool = Tool::Select;
        session.select_all();
    }
    // With a selection, arrows nudge it instead of moving the playhead.
    let selecting = session.state.tool == Tool::Select && !session.tools.selection().is_empty();
    let (select, escape, delete, shift) = ctx.input(|i| {
        (
            i.modifiers.is_none() && i.key_pressed(Key::V),
            i.key_pressed(Key::Escape),
            i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
            i.modifiers.shift,
        )
    });
    if escape && !gesture {
        session.clear_selection();
    }
    if delete && selecting && !gesture {
        session.delete_selection();
    }
    let (brush, eraser, fill, left, right, up, down, space_down, space_up) = ctx.input_mut(|i| {
        let plain = i.modifiers.is_none();
        let space = |want_pressed: bool| {
            i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key { key: Key::Space, pressed, repeat: false, .. }
                        if *pressed == want_pressed
                )
            })
        };
        let result = (
            plain && i.key_pressed(Key::B),
            plain && i.key_pressed(Key::E),
            plain && i.key_pressed(Key::K),
            i.num_presses(Key::ArrowLeft),
            i.num_presses(Key::ArrowRight),
            i.num_presses(Key::ArrowUp),
            i.num_presses(Key::ArrowDown),
            space(true),
            space(false),
        );
        // Space belongs to playback and panning, not to a focused button.
        i.events.retain(|e| {
            !matches!(
                e,
                egui::Event::Key {
                    key: Key::Space,
                    ..
                }
            )
        });
        result
    });
    if !gesture {
        if select {
            session.state.tool = Tool::Select;
        }
        if brush {
            session.state.tool = Tool::Brush;
        }
        if eraser {
            session.state.tool = Tool::Eraser;
        }
        if fill {
            session.state.tool = Tool::Fill;
        }
    }
    if selecting {
        let step = if shift { 10.0 } else { 1.0 };
        let dx = right as f32 - left as f32;
        let dy = down as f32 - up as f32;
        if !gesture && (dx != 0.0 || dy != 0.0) {
            session.nudge_selection(document::animation_core::Vec2::new(dx, dy) * step);
        }
        return finish_space(session, space_down, space_up, gesture);
    }
    let steps = right as i64 - left as i64;
    if steps != 0 && !gesture {
        session.playback.pause();
        session.step(steps);
    }
    if up > 0 && !gesture {
        session.playback.pause();
        session.jump_to_key(false);
    }
    if down > 0 && !gesture {
        session.playback.pause();
        session.jump_to_key(true);
    }
    finish_space(session, space_down, space_up, gesture);
}

/// Space is also "hold to pan" on the canvas: toggle playback on release,
/// unless the canvas used it for panning.
fn finish_space(session: &mut Session, space_down: bool, space_up: bool, gesture: bool) {
    if space_down {
        session.space_panned = false;
    }
    if space_up && !session.space_panned && !gesture {
        session.toggle_playback();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(session: &mut Session, shortcut: KeyboardShortcut) {
        let ctx = egui::Context::default();
        let event = |pressed| egui::Event::Key {
            key: shortcut.logical_key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: shortcut.modifiers,
        };
        let input = egui::RawInput {
            events: vec![event(true), event(false)],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| handle(ui.ctx(), session));
        output.textures_delta.clear();
    }

    #[test]
    fn view_shortcuts_toggle_panels() {
        let mut s = Session::new();
        press(&mut s, TOGGLE_TOOLBAR);
        assert!(!s.view.toolbar);
        assert!(s.view.timeline);
        press(&mut s, TOGGLE_TIMELINE);
        assert!(!s.view.timeline);
        press(&mut s, TOGGLE_TOOLBAR);
        press(&mut s, TOGGLE_TIMELINE);
        assert_eq!(s.view, crate::session::ViewSettings::default());
    }

    #[test]
    fn view_shortcuts_do_not_clash() {
        let all = [
            NEW, OPEN, SAVE, SAVE_AS, UNDO, REDO, REDO_ALT, SELECT_ALL, QUIT,
        ];
        for view in [TOGGLE_TOOLBAR, TOGGLE_TIMELINE] {
            assert!(all.iter().all(|s| s.logical_key != view.logical_key));
        }
    }
}
