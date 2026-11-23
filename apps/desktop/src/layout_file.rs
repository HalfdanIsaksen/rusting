//! Saves the panel layout between launches (ADR-021), as JSON in the
//! per-user configuration folder:
//!
//! - macOS: `~/Library/Application Support/Rusting/panel-layout.json`
//! - Windows: `%APPDATA%\Rusting\panel-layout.json`
//! - Linux and others: `$XDG_CONFIG_HOME/rusting/panel-layout.json`, or
//!   `~/.config/rusting/panel-layout.json`
//!
//! `RUSTING_CONFIG_DIR` overrides the folder. Reading and writing never fail
//! the app: a missing, unreadable or invalid file means the default layout.

use std::path::{Path, PathBuf};

/// File name of the saved layout.
const FILE_NAME: &str = "panel-layout.json";

/// The per-user configuration folder, if the environment names one.
pub fn config_dir() -> Option<PathBuf> {
    let var = |name| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = var("RUSTING_CONFIG_DIR") {
        return Some(dir);
    }
    if cfg!(target_os = "macos") {
        var("HOME").map(|home| home.join("Library/Application Support/Rusting"))
    } else if cfg!(windows) {
        var("APPDATA").map(|dir| dir.join("Rusting"))
    } else {
        var("XDG_CONFIG_HOME")
            .or_else(|| var("HOME").map(|home| home.join(".config")))
            .map(|dir| dir.join("rusting"))
    }
}

/// Where the layout is saved.
pub fn path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(FILE_NAME))
}

/// Restores the layout saved at `path` into `session`, if there is a valid one.
pub fn load(session: &mut ui::Session, path: &Path) {
    match std::fs::read_to_string(path) {
        Ok(json) => {
            if !session.restore_panel_layout(&json) {
                tracing::warn!(path = %path.display(), "ignoring invalid panel layout");
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => tracing::warn!(path = %path.display(), %err, "could not read panel layout"),
    }
}

/// Saves `session`'s layout to `path`, replacing the file in one step.
pub fn save(session: &mut ui::Session, path: &Path) {
    let json = session.panel_layout_json();
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    };
    if let Err(err) = write() {
        tracing::warn!(path = %path.display(), %err, "could not save panel layout");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rusting-layout-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn saved_layout_is_restored() {
        let path = temp_dir("roundtrip").join("nested").join(FILE_NAME);
        let mut session = ui::Session::new();
        session.view.library = false;
        save(&mut session, &path);

        let mut restored = ui::Session::new();
        load(&mut restored, &path);
        assert!(!restored.view.library);
        assert!(restored.view.timeline);
        assert_eq!(restored.panel_layout_json(), session.panel_layout_json());
    }

    #[test]
    fn missing_or_corrupt_file_keeps_the_default_layout() {
        let dir = temp_dir("corrupt");
        let default = ui::Session::new().panel_layout_json();

        let mut session = ui::Session::new();
        load(&mut session, &dir.join(FILE_NAME));
        assert_eq!(session.panel_layout_json(), default);

        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(FILE_NAME);
        for junk in ["", "{", "{\"version\": 1}", "\u{0}\u{1}binary"] {
            std::fs::write(&path, junk).expect("write");
            let mut session = ui::Session::new();
            load(&mut session, &path);
            assert_eq!(session.panel_layout_json(), default, "{junk:?}");
            assert_eq!(session.view, ui::ViewSettings::default());
        }
    }
}
