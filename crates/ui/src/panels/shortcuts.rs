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
/// Quit.
pub const QUIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Q);

/// Handles global shortcuts. Plain-key shortcuts are skipped while a text
/// field has keyboard focus.
pub fn handle(ctx: &egui::Context, session: &mut Session) {
    let typing = ctx.egui_wants_keyboard_input();
    let modal_open = session.dialog().is_some();

    // Most specific first: Shift+Cmd+S before Cmd+S (egui ignores extra Shift).
    let (save_as, save, new, open, quit) = ctx.input_mut(|i| {
        (
            i.consume_shortcut(&SAVE_AS),
            i.consume_shortcut(&SAVE),
            i.consume_shortcut(&NEW),
            i.consume_shortcut(&OPEN),
            i.consume_shortcut(&QUIT),
        )
    });
    if !modal_open {
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
            i.key_pressed(Key::ArrowUp),
            i.key_pressed(Key::ArrowDown),
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
    let steps = right as i64 - left as i64;
    if steps != 0 && !gesture {
        session.playback.pause();
        session.step(steps);
    }
    if up && !gesture {
        session.playback.pause();
        session.jump_to_key(false);
    }
    if down && !gesture {
        session.playback.pause();
        session.jump_to_key(true);
    }
    // Space is also "hold to pan" on the canvas: toggle playback on release,
    // unless the canvas used it for panning.
    if space_down {
        session.space_panned = false;
    }
    if space_up && !session.space_panned && !gesture {
        session.toggle_playback();
    }
}
