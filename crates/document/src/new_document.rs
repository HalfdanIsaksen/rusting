//! Settings for a new document, and the templates that prefill them.

use crate::{CanvasSize, Document};
use animation_core::Fps;
use std::fmt;

/// Largest canvas side, in pixels. GPUs guarantee textures of this size
/// (wgpu's default limit), so the canvas can always be rendered and exported.
pub const MAX_CANVAS_SIDE: u32 = 8192;
/// Highest frame rate a new document can have.
pub const MAX_FPS: u32 = 240;
/// Longest animation a new document can have, in frames.
pub const MAX_FRAME_COUNT: u32 = 100_000;

/// A named preset for [`NewDocumentSettings`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Template {
    /// Shown on the template card.
    pub name: &'static str,
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// Frames per second.
    pub fps: u32,
    /// Length in frames.
    pub frame_count: u32,
}

impl Template {
    /// Full HD video.
    pub const HD: Self = Self::new("HD 1080p", 1920, 1080, 24, 48);
    /// 4K UHD video.
    pub const UHD_4K: Self = Self::new("4K UHD", 3840, 2160, 24, 48);
    /// Square social post.
    pub const SQUARE: Self = Self::new("Square social", 1080, 1080, 30, 60);
    /// Vertical phone story / reel.
    pub const VERTICAL: Self = Self::new("Vertical / Story", 1080, 1920, 30, 60);
    /// Small, low-frame-rate GIF.
    pub const GIF: Self = Self::new("GIF / small", 640, 480, 12, 24);

    /// Every template, in the order the start menu lists them.
    pub const ALL: [Self; 5] = [
        Self::HD,
        Self::UHD_4K,
        Self::SQUARE,
        Self::VERTICAL,
        Self::GIF,
    ];

    const fn new(name: &'static str, width: u32, height: u32, fps: u32, frame_count: u32) -> Self {
        Self {
            name,
            width,
            height,
            fps,
            frame_count,
        }
    }

    /// Whether `settings` has this template's size, rate and length.
    pub fn matches(&self, settings: &NewDocumentSettings) -> bool {
        settings.width == self.width
            && settings.height == self.height
            && settings.fps == self.fps
            && settings.frame_count == self.frame_count
    }
}

/// What the user chose for a new document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewDocumentSettings {
    /// Document name (trimmed on creation).
    pub name: String,
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// Frames per second.
    pub fps: u32,
    /// Length in frames.
    pub frame_count: u32,
}

impl Default for NewDocumentSettings {
    /// "Untitled", with the HD template.
    fn default() -> Self {
        Self::from_template("Untitled", &Template::HD)
    }
}

/// Why [`NewDocumentSettings`] can't make a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsError {
    /// The name is empty or only whitespace.
    EmptyName,
    /// Width or height is 0 or above [`MAX_CANVAS_SIDE`].
    CanvasSize,
    /// Frame rate is 0 or above [`MAX_FPS`].
    Fps,
    /// Frame count is 0 or above [`MAX_FRAME_COUNT`].
    FrameCount,
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => f.write_str("Give the document a name."),
            Self::CanvasSize => write!(
                f,
                "Width and height must be between 1 and {MAX_CANVAS_SIDE} pixels."
            ),
            Self::Fps => write!(f, "Frame rate must be between 1 and {MAX_FPS} fps."),
            Self::FrameCount => write!(
                f,
                "The animation needs between 1 and {MAX_FRAME_COUNT} frames."
            ),
        }
    }
}

impl std::error::Error for SettingsError {}

impl NewDocumentSettings {
    /// Settings named `name` with `template`'s values.
    pub fn from_template(name: impl Into<String>, template: &Template) -> Self {
        let mut settings = Self {
            name: name.into(),
            width: 0,
            height: 0,
            fps: 0,
            frame_count: 0,
        };
        settings.apply_template(template);
        settings
    }

    /// Replaces size, rate and length with `template`'s; keeps the name.
    pub fn apply_template(&mut self, template: &Template) {
        self.width = template.width;
        self.height = template.height;
        self.fps = template.fps;
        self.frame_count = template.frame_count;
    }

    /// The template these settings match, or `None` for custom values.
    pub fn template(&self) -> Option<&'static Template> {
        Template::ALL.iter().find(|t| t.matches(self))
    }

    /// Animation length in seconds.
    pub fn duration_secs(&self) -> f64 {
        if self.fps == 0 {
            0.0
        } else {
            f64::from(self.frame_count) / f64::from(self.fps)
        }
    }

    /// Checks every value; the first problem found is returned.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.name.trim().is_empty() {
            return Err(SettingsError::EmptyName);
        }
        let side_ok = |side: u32| (1..=MAX_CANVAS_SIDE).contains(&side);
        if !side_ok(self.width) || !side_ok(self.height) {
            return Err(SettingsError::CanvasSize);
        }
        if !(1..=MAX_FPS).contains(&self.fps) {
            return Err(SettingsError::Fps);
        }
        if !(1..=MAX_FRAME_COUNT).contains(&self.frame_count) {
            return Err(SettingsError::FrameCount);
        }
        Ok(())
    }

    /// A new document with these settings and one empty layer, "Layer 1".
    pub fn create(&self) -> Result<Document, SettingsError> {
        self.validate()?;
        let mut document = Document::new(
            self.name.trim(),
            CanvasSize {
                width: self.width,
                height: self.height,
            },
        );
        document.fps = Fps::whole(self.fps).ok_or(SettingsError::Fps)?;
        document.frame_count = self.frame_count;
        document.add_layer("Layer 1");
        Ok(document)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_have_the_advertised_values() {
        let values: Vec<_> = Template::ALL
            .iter()
            .map(|t| (t.width, t.height, t.fps))
            .collect();
        assert_eq!(
            values,
            [
                (1920, 1080, 24),
                (3840, 2160, 24),
                (1080, 1080, 30),
                (1080, 1920, 30),
                (640, 480, 12),
            ]
        );
        for t in &Template::ALL {
            let settings = NewDocumentSettings::from_template("T", t);
            assert_eq!(settings.validate(), Ok(()), "{}", t.name);
            assert_eq!(settings.template(), Some(t));
        }
    }

    #[test]
    fn create_uses_every_setting() {
        let settings = NewDocumentSettings {
            name: "  Walk cycle ".into(),
            width: 800,
            height: 600,
            fps: 12,
            frame_count: 36,
        };
        let doc = settings.create().unwrap();
        assert_eq!(doc.name, "Walk cycle");
        assert_eq!(
            doc.canvas,
            CanvasSize {
                width: 800,
                height: 600
            }
        );
        assert_eq!(doc.fps, Fps::whole(12).unwrap());
        assert_eq!(doc.frame_count, 36);
        assert_eq!(doc.layers().len(), 1);
        assert_eq!(settings.duration_secs(), 3.0);
    }

    #[test]
    fn custom_values_match_no_template() {
        let mut settings = NewDocumentSettings::default();
        assert_eq!(settings.template(), Some(&Template::HD));
        settings.frame_count = 49;
        assert_eq!(settings.template(), None);
        settings.apply_template(&Template::GIF);
        assert_eq!(settings.template(), Some(&Template::GIF));
        assert_eq!(settings.name, "Untitled");
    }

    #[test]
    fn invalid_settings_are_rejected() {
        let ok = NewDocumentSettings::default();
        let check = |change: fn(&mut NewDocumentSettings), err| {
            let mut s = ok.clone();
            change(&mut s);
            assert_eq!(s.validate(), Err(err));
            assert_eq!(s.create(), Err(err));
        };
        check(|s| s.name = "   ".into(), SettingsError::EmptyName);
        check(|s| s.width = 0, SettingsError::CanvasSize);
        check(|s| s.height = 0, SettingsError::CanvasSize);
        check(|s| s.width = MAX_CANVAS_SIDE + 1, SettingsError::CanvasSize);
        check(|s| s.fps = 0, SettingsError::Fps);
        check(|s| s.fps = MAX_FPS + 1, SettingsError::Fps);
        check(|s| s.frame_count = 0, SettingsError::FrameCount);
        check(
            |s| s.frame_count = MAX_FRAME_COUNT + 1,
            SettingsError::FrameCount,
        );

        let mut edge = ok.clone();
        edge.width = 1;
        edge.height = MAX_CANVAS_SIDE;
        edge.fps = 1;
        edge.frame_count = 1;
        assert_eq!(edge.validate(), Ok(()));
    }
}
