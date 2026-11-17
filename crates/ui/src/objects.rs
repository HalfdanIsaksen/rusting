//! Objects (symbols, ADR-018): making them from layers and frames or from
//! canvas items, placing them from the library, and their settings.

use crate::session::Session;
use document::animation_core::{FrameIndex, FrameRange, LayerId, SymbolId, Vec2};
use document::timeline::{InstanceKey, InstanceTransform};
use editor::Tool;
use editor::commands::{
    ConvertItemsToSymbol, ConvertToSymbol, InstanceProps, PlaceInstance, RemoveSymbol,
    RenameSymbol, SetInstanceProps,
};

/// The object shown on the active layer at the current frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InstanceAt {
    /// The layer.
    pub layer: LayerId,
    /// The frame of the instance key exposed at the current frame.
    pub key_frame: FrameIndex,
    /// The key.
    pub key: InstanceKey,
}

impl Session {
    /// The frames "Convert to Object" uses: the frame selection, or the
    /// whole animation when nothing is selected.
    pub fn object_range(&self) -> FrameRange {
        self.frame_selection().unwrap_or(FrameRange::new(
            FrameIndex(0),
            FrameIndex(self.document.frame_count),
        ))
    }

    /// Turns the selected layers' frames (see [`Self::object_range`]) into
    /// a new object in the library, placed on a new layer above them; one
    /// undo step. The new layer becomes the active one. Returns false, with
    /// a status message, if that is not possible.
    pub fn convert_layers_to_object(&mut self) -> bool {
        let layers = self.selected_layers();
        let range = self.object_range();
        let command = ConvertToSymbol::new(layers, range);
        let (layer, symbol) = (command.layer_id(), command.symbol_id());
        if self.execute(Box::new(command)).is_err() {
            return false;
        }
        self.clear_frame_selection();
        self.select_layer(layer);
        self.report_new_object(symbol);
        true
    }

    /// Turns the items selected on the canvas into a one-frame object,
    /// placed where they were on a new layer above (`F8`); one undo step.
    pub fn convert_selection_to_object(&mut self) -> bool {
        self.sync_selection();
        let Some((layer, frame)) = self.tools.selection_at() else {
            self.set_error("Convert to Object: select items on the canvas first");
            return false;
        };
        let items = self.tools.selection().to_vec();
        let command = ConvertItemsToSymbol::new(layer, frame, items);
        let (new_layer, symbol) = (command.layer_id(), command.symbol_id());
        if self.execute(Box::new(command)).is_err() {
            return false;
        }
        self.tools.cancel();
        self.select_layer(new_layer);
        self.select_instance();
        self.report_new_object(symbol);
        true
    }

    fn report_new_object(&mut self, symbol: SymbolId) {
        if let Some(name) = self.document.symbol(symbol).map(|s| s.name.clone()) {
            self.set_status(format!("Made {name}; it is in the Library under Objects"));
        }
    }

    /// Places object `symbol` on `layer` at `frame` (a drop on a timeline
    /// cell): it plays from there until the next key. One undo step.
    pub fn place_symbol_at(&mut self, symbol: SymbolId, layer: LayerId, frame: FrameIndex) -> bool {
        let key = InstanceKey::new(symbol);
        if self
            .execute(Box::new(PlaceInstance::new(layer, frame, key)))
            .is_err()
        {
            return false;
        }
        self.select_layer(layer);
        self.set_frame(frame);
        true
    }

    /// Places object `symbol` on a new layer above the active one at the
    /// current frame, its first frame centred on `centre` (document
    /// coordinates; the canvas centre if `None`). One undo step; the object
    /// is then selected with the Selection tool.
    pub fn place_symbol_on_canvas(&mut self, symbol: SymbolId, centre: Option<Vec2>) -> bool {
        let Some(name) = self.document.symbol(symbol).map(|s| s.name.clone()) else {
            return false;
        };
        let canvas = Vec2::new(
            self.document.canvas.width as f32,
            self.document.canvas.height as f32,
        );
        let centre = centre.unwrap_or(canvas * 0.5);
        let translation = centre - self.symbol_centre(symbol).unwrap_or(Vec2::ZERO);
        let key = InstanceKey::new(symbol)
            .with_transform(InstanceTransform::from_translation(translation));
        let frame = self.current_frame();
        let command =
            PlaceInstance::on_new_layer(&self.document, &name, self.state.active_layer, frame, key);
        let layer = command.layer_id();
        if self.execute(Box::new(command)).is_err() {
            return false;
        }
        self.select_layer(layer);
        self.state.tool = Tool::Select;
        self.select_instance();
        self.set_status(format!("Placed {name}"));
        true
    }

    /// The centre of what object `symbol` shows on its first frame, in its
    /// own coordinates.
    pub fn symbol_centre(&self, symbol: SymbolId) -> Option<Vec2> {
        let layer = self.state.active_layer.unwrap_or_default();
        let (min, max) = self
            .document
            .evaluate_symbol(symbol, FrameIndex(0), layer)
            .bounds()?;
        Some((min + max) * 0.5)
    }

    /// The size of what object `symbol` shows on its first frame (for the
    /// outline of a canvas drop).
    pub fn symbol_size(&self, symbol: SymbolId) -> Option<Vec2> {
        let layer = self.state.active_layer.unwrap_or_default();
        let (min, max) = self
            .document
            .evaluate_symbol(symbol, FrameIndex(0), layer)
            .bounds()?;
        Some(max - min)
    }

    /// Renames object `symbol`; one undo step. A blank or unchanged name
    /// does nothing.
    pub fn rename_symbol(&mut self, symbol: SymbolId, name: &str) {
        let name = name.trim();
        if name.is_empty() || self.document.symbol(symbol).is_none_or(|s| s.name == name) {
            return;
        }
        let _ = self.execute(Box::new(RenameSymbol::new(symbol, name)));
    }

    /// Deletes object `symbol` from the library; one undo step. Refused,
    /// with a status message, while it is placed somewhere.
    pub fn delete_symbol(&mut self, symbol: SymbolId) -> bool {
        let Some(name) = self.document.symbol(symbol).map(|s| s.name.clone()) else {
            return false;
        };
        let deleted = self.execute(Box::new(RemoveSymbol::new(symbol))).is_ok();
        if deleted {
            self.set_status(format!("Deleted {name} from the library"));
        }
        deleted
    }

    /// The object the active layer shows at the current frame, if any.
    pub fn instance_at_playhead(&self) -> Option<InstanceAt> {
        let layer = self.state.active_layer?;
        let (key_frame, key) = self
            .document
            .track(layer)?
            .instance_at(self.current_frame())?;
        Some(InstanceAt {
            layer,
            key_frame,
            key: *key,
        })
    }

    /// Changes Loop / Play once, first frame and opacity of the object
    /// under the playhead on the active layer. Changes with the same
    /// `gesture` (one slider drag) undo as one step.
    pub fn set_instance_props(&mut self, props: InstanceProps, gesture: Option<u64>) {
        let Some(at) = self.instance_at_playhead() else {
            return;
        };
        let mut command = SetInstanceProps::new(at.layer, self.current_frame(), props);
        if let Some(g) = gesture {
            command = command.in_gesture(g);
        }
        let _ = self.execute(Box::new(command));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use document::drawing::{BrushStyle, Stroke, StrokePoint};
    use document::timeline::{InstanceMode, Keyframe};
    use editor::commands::{AddKeyframe, AddStroke, KeyframeKind, SetFrameCount};

    fn stroke(x: f32) -> Stroke {
        Stroke::new(
            vec![
                StrokePoint::new(Vec2::new(x, 10.0)),
                StrokePoint::new(Vec2::new(x + 20.0, 30.0)),
            ],
            BrushStyle::default(),
        )
    }

    /// A session with 18 frames and a drawing on frames 0..12 of the
    /// active layer, one key per frame.
    fn twelve_frames() -> (Session, LayerId) {
        let mut s = Session::new();
        s.execute(Box::new(SetFrameCount::new(18))).unwrap();
        let layer = s.state.active_layer.unwrap();
        for f in 0..12 {
            s.execute(Box::new(AddKeyframe::new(
                layer,
                FrameIndex(f),
                KeyframeKind::Drawing,
            )))
            .unwrap();
            s.execute(Box::new(AddStroke::new(
                layer,
                FrameIndex(f),
                stroke(f as f32),
            )))
            .unwrap();
        }
        (s, layer)
    }

    #[test]
    fn layers_and_frames_become_an_object_that_loops_over_eighteen_frames() {
        let (mut s, layer) = twelve_frames();
        s.set_frame(FrameIndex(0));
        s.select_frames_to(FrameIndex(11));
        assert!(s.convert_layers_to_object());
        assert_eq!(s.frame_selection(), None);
        let symbol = *s.document.symbols().keys().next().unwrap();
        assert_eq!(s.document.symbol(symbol).unwrap().frame_count, 12);
        let object_layer = s.state.active_layer.unwrap();
        assert_ne!(object_layer, layer);
        // Make the object play to the end of the 18 frames.
        s.execute(Box::new(editor::commands::RemoveKeyframe::new(
            object_layer,
            FrameIndex(12),
        )))
        .unwrap();
        let x = |s: &Session, f: u32| {
            let list = s.document.evaluate(FrameIndex(f));
            let l = list.layers.last().unwrap();
            l.transform
                .transform_point2(l.drawing.strokes()[0].points[0].pos)
                .x
        };
        let shown: Vec<f32> = (0..18).map(|f| x(&s, f)).collect();
        let expected: Vec<f32> = (0..12).chain(0..6).map(|f| f as f32).collect();
        assert_eq!(shown, expected);

        s.set_frame(FrameIndex(3));
        let at = s.instance_at_playhead().unwrap();
        assert_eq!(at.key_frame, FrameIndex(0));
        let props = InstanceProps {
            mode: InstanceMode::Once,
            ..InstanceProps::of(&at.key)
        };
        s.set_instance_props(props, None);
        let shown: Vec<f32> = (0..18).map(|f| x(&s, f)).collect();
        let expected: Vec<f32> = (0..12).chain([11; 6]).map(|f| f as f32).collect();
        assert_eq!(shown, expected);

        // Undo everything back to plain frames.
        s.undo();
        s.undo();
        s.undo();
        assert!(s.document.symbols().is_empty());
    }

    #[test]
    fn objects_are_placed_from_the_library_and_renamed_or_deleted() {
        let (mut s, layer) = twelve_frames();
        s.select_frames_to(FrameIndex(3));
        assert!(s.convert_layers_to_object());
        let symbol = *s.document.symbols().keys().next().unwrap();

        // Drop on a timeline cell of the original layer.
        assert!(s.place_symbol_at(symbol, layer, FrameIndex(14)));
        let key = s.document.track(layer).unwrap().key_at(FrameIndex(14));
        assert_eq!(key.and_then(Keyframe::symbol), Some(symbol));

        // Drop on the canvas: a new layer, centred on the drop point.
        let layers = s.document.layers().len();
        s.set_frame(FrameIndex(5));
        assert!(s.place_symbol_on_canvas(symbol, Some(Vec2::new(300.0, 200.0))));
        assert_eq!(s.document.layers().len(), layers + 1);
        let new = s.state.active_layer.unwrap();
        let (min, max) = s.document.instance_bounds(new, FrameIndex(5)).unwrap();
        assert!(((min + max) * 0.5 - Vec2::new(300.0, 200.0)).length() < 1e-3);

        s.rename_symbol(symbol, "  Walk  ");
        assert_eq!(s.document.symbol(symbol).unwrap().name, "Walk");
        assert!(!s.delete_symbol(symbol));
        assert!(s.status().is_some());
    }
}
