//! The "a new version is available" dialog (ADR-026). The desktop app finds
//! the release and decides when to show it; this only draws it.

/// A newer release to tell the user about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateNotice {
    /// The new version, e.g. `0.2.3`.
    pub version: String,
    /// The running version.
    pub current: String,
    /// One sentence about what the release adds, if it has one.
    pub summary: Option<String>,
    /// The release's web page, with its notes and downloads.
    pub url: String,
    /// The app was installed with Homebrew, so `brew upgrade` updates it
    /// instead of a download.
    pub homebrew: bool,
    /// The "check for updates at launch" setting, which the dialog can change.
    pub check_on_launch: bool,
}

/// The Homebrew command that updates the app.
pub const BREW_UPGRADE: &str = "brew upgrade rusting";

/// How the user answered the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateChoice {
    /// Open the release page (to download it, or read the notes).
    OpenPage,
    /// Close it; ask again at a later launch.
    Later,
    /// Don't ask about this version again.
    Skip,
}

/// Shows `notice` as a modal dialog. Returns the user's answer once they
/// give one. Escape or a click outside counts as [`UpdateChoice::Later`].
pub fn update_dialog(ctx: &egui::Context, notice: &mut UpdateNotice) -> Option<UpdateChoice> {
    let mut choice = None;
    let response = egui::Modal::new(egui::Id::unique("update_dialog")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.heading(format!("Rusting {} is available", notice.version));
        ui.add_space(6.0);
        ui.label(format!("You have version {}.", notice.current));
        if let Some(summary) = &notice.summary {
            ui.add_space(4.0);
            ui.label(summary);
        }
        ui.add_space(10.0);
        if notice.homebrew {
            ui.label("You installed Rusting with Homebrew. To update, run:");
            ui.horizontal(|ui| {
                ui.code(BREW_UPGRADE);
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(BREW_UPGRADE.to_owned());
                }
            });
            ui.add_space(10.0);
        }
        ui.horizontal(|ui| {
            let open = if notice.homebrew {
                "Release Notes"
            } else {
                "Download"
            };
            if ui.button(open).clicked() {
                choice = Some(UpdateChoice::OpenPage);
            }
            if ui.button("Later").clicked() {
                choice = Some(UpdateChoice::Later);
            }
            if ui.button("Skip This Version").clicked() {
                choice = Some(UpdateChoice::Skip);
            }
        });
        ui.add_space(6.0);
        ui.checkbox(
            &mut notice.check_on_launch,
            "Check for updates when Rusting starts",
        );
    });
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if choice.is_none() && (response.should_close() || escape) {
        choice = Some(UpdateChoice::Later);
    }
    choice
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(homebrew: bool) -> UpdateNotice {
        UpdateNotice {
            version: "9.0.0".into(),
            current: "0.2.2".into(),
            summary: Some("Adds things.".into()),
            url: "https://example.com".into(),
            homebrew,
            check_on_launch: true,
        }
    }

    fn texts(ctx: &egui::Context, notice: &mut UpdateNotice) -> (Option<UpdateChoice>, String) {
        let mut choice = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            choice = update_dialog(ui.ctx(), notice);
        });
        output.textures_delta.clear();
        let text = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        (choice, text)
    }

    #[test]
    fn shows_the_version_and_waits_for_an_answer() {
        let ctx = egui::Context::default();
        let mut n = notice(false);
        // The first frame only lays out; the second paints.
        let _ = texts(&ctx, &mut n);
        let (choice, text) = texts(&ctx, &mut n);
        assert_eq!(choice, None);
        assert!(text.contains("Rusting 9.0.0 is available"), "{text}");
        assert!(text.contains("Download"), "{text}");
        assert!(!text.contains(BREW_UPGRADE), "{text}");
    }

    #[test]
    fn homebrew_installs_show_the_brew_command() {
        let ctx = egui::Context::default();
        let mut n = notice(true);
        let _ = texts(&ctx, &mut n);
        let (_, text) = texts(&ctx, &mut n);
        assert!(text.contains(BREW_UPGRADE), "{text}");
        assert!(text.contains("Release Notes"), "{text}");
    }

    #[test]
    fn escape_means_later() {
        let ctx = egui::Context::default();
        let mut n = notice(false);
        let _ = texts(&ctx, &mut n);
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let mut choice = None;
        let mut output = ctx.run_ui(input, |ui| choice = update_dialog(ui.ctx(), &mut n));
        output.textures_delta.clear();
        assert_eq!(choice, Some(UpdateChoice::Later));
    }
}
