//! File → Import: placing PNG/JPEG pictures on the canvas (ADR-015).
//!
//! The host shows the file dialog and reads the file
//! ([`crate::Request::ImportDialog`]); this module decides where the picture
//! goes and adds it as one undoable command.

use crate::session::{Request, Session};
use document::animation_core::Vec2;
use document::drawing::{Image, ImageData, ItemId};
use editor::Tool;
use editor::commands::AddImage;

impl Session {
    /// Asks the host to show the import dialog; it then calls
    /// [`Self::import_image`] for each picked file.
    pub fn import(&mut self) {
        self.requests.push(Request::ImportDialog);
    }

    /// Places `data` on the active layer at the current frame, centred on
    /// the canvas and shrunk (keeping its proportions) if it is larger than
    /// the canvas; one undo step. The picture is then selected with the
    /// Selection tool so it can be moved straight away. `name` is the file
    /// name, for messages. Returns false, with a status message, if the
    /// layer can't be drawn on.
    pub fn import_image(&mut self, name: &str, data: ImageData) -> bool {
        if let Some(reason) = self.draw_block_reason() {
            self.set_error(format!("Import {name}: {reason}"));
            return false;
        }
        let Some(layer) = self.state.active_layer else {
            return false;
        };
        let canvas = Vec2::new(
            self.document.canvas.width as f32,
            self.document.canvas.height as f32,
        );
        let size = fit_within(
            Vec2::new(data.width.max(1) as f32, data.height.max(1) as f32),
            canvas,
        );
        let image = Image::new(data, (canvas - size) * 0.5, size);
        let id = image.id;
        let frame = self.current_frame();
        if self
            .execute(Box::new(AddImage::new(layer, frame, image)))
            .is_err()
        {
            return false;
        }
        self.state.tool = Tool::Select;
        self.tools
            .set_selection((layer, frame), vec![ItemId::Image(id)]);
        self.set_status(format!("Imported {name}"));
        true
    }
}

/// `size` scaled down uniformly to fit inside `bounds`; unchanged if it
/// already fits.
fn fit_within(size: Vec2, bounds: Vec2) -> Vec2 {
    let scale = (bounds / size).min_element().min(1.0);
    size * scale
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::commands::SetLayerLocked;

    fn v(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    #[test]
    fn fit_keeps_proportions_and_never_enlarges() {
        assert_eq!(
            fit_within(v(100.0, 50.0), v(1920.0, 1080.0)),
            v(100.0, 50.0)
        );
        assert_eq!(
            fit_within(v(4000.0, 1000.0), v(2000.0, 2000.0)),
            v(2000.0, 500.0)
        );
        assert_eq!(
            fit_within(v(1000.0, 3000.0), v(2000.0, 1500.0)),
            v(500.0, 1500.0)
        );
    }

    #[test]
    fn import_menu_asks_the_host() {
        let mut s = Session::new();
        s.import();
        assert_eq!(s.take_requests(), [Request::ImportDialog]);
    }

    #[test]
    fn imported_image_is_centred_selected_and_undoable() {
        let mut s = Session::new();
        let canvas = v(
            s.document.canvas.width as f32,
            s.document.canvas.height as f32,
        );
        assert!(s.import_image("pic.png", ImageData::new(200, 100, vec![1])));
        let drawing = s.active_drawing().unwrap();
        let image = &drawing.images()[0];
        let (min, max) = image.bounds();
        assert!(((min + max) * 0.5 - canvas * 0.5).length() < 1e-3);
        assert_eq!(max - min, v(200.0, 100.0));
        assert_eq!(s.state.tool, Tool::Select);
        assert_eq!(s.tools.selection(), [ItemId::Image(image.id)]);

        s.undo();
        assert!(s.active_drawing().is_none_or(|d| d.images().is_empty()));
    }

    #[test]
    fn locked_layer_refuses_import() {
        let mut s = Session::new();
        let layer = s.state.active_layer.unwrap();
        s.execute(Box::new(SetLayerLocked::new(layer, true)))
            .unwrap();
        assert!(!s.import_image("pic.png", ImageData::new(2, 2, vec![1])));
        assert!(s.active_drawing().is_none_or(|d| d.images().is_empty()));
    }
}
