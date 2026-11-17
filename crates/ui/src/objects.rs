//! Objects (symbols, ADR-018): making them from layers and frames or from
//! canvas items, placing them from the library, their settings, and motion
//! tweens between their keys (ADR-019).

use crate::session::Session;
use document::Document;
use document::animation_core::{FrameIndex, FrameRange, Interpolation, LayerId, SymbolId, Vec2};
use document::timeline::{InstanceKey, InstanceTransform};
use editor::commands::{
    Batch, ConvertItemsToSymbol, ConvertToSymbol, InstanceProps, PlaceInstance, RemoveSymbol,
    RenameSymbol, SetInstanceProps, SetTween,
};
use editor::{Command, Tool};

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

/// The motion tweens a timeline menu can create or remove (ADR-019), as
/// the (layer, frame) of the key each tween starts on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TweenTargets {
    /// Keys followed by a key of the same object: a tween can start there.
    pub creatable: Vec<(LayerId, FrameIndex)>,
    /// Keys that have a tween, active or not.
    pub removable: Vec<(LayerId, FrameIndex)>,
    /// The ease every creatable key already tweens with, if they all share
    /// one.
    pub current: Option<Interpolation>,
}

/// The motion tween setting of the object under the playhead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TweenAt {
    /// The layer.
    pub layer: LayerId,
    /// The instance key exposed at the playhead (where the tween starts).
    pub key_frame: FrameIndex,
    /// Its tween, active or not.
    pub tween: Option<Interpolation>,
    /// The next key's frame if it places the same object, so a tween
    /// from `key_frame` is (or would be) active and ends there.
    pub end: Option<FrameIndex>,
}

/// Whether the key on `start` (followed by the next key on `next`, or by
/// nothing until `frame_count`) is picked by frame selection `range`: a
/// one-frame range picks the exposure holding it; a longer one picks every
/// span `start..=next` it overlaps by at least two frames, so selecting
/// "from one key to the next" picks exactly that pair.
fn span_picked(
    start: FrameIndex,
    next: Option<FrameIndex>,
    frame_count: u32,
    range: FrameRange,
) -> bool {
    let end = next.map_or(frame_count.max(start.0 + 1), |n| n.0);
    if range.len() <= 1 {
        return (start.0..end).contains(&range.start.0);
    }
    let last = next.map_or(end - 1, |n| n.0);
    let lo = start.0.max(range.start.0);
    let hi = last.min(range.end.0 - 1);
    hi > lo
}

/// The instance keys of `layer` that `range` picks, with their tween and
/// whether they can start one.
fn picked_keys(
    doc: &Document,
    layer: LayerId,
    range: FrameRange,
) -> Vec<(FrameIndex, Option<Interpolation>, bool)> {
    let Some(track) = doc.track(layer) else {
        return Vec::new();
    };
    track
        .keys()
        .filter_map(|(frame, key)| {
            let instance = key.instance()?;
            let next = track.next_key_after(frame).map(|(f, _)| f);
            span_picked(frame, next, doc.frame_count, range).then(|| {
                (
                    frame,
                    instance.tween,
                    SetTween::can_tween(doc, layer, frame),
                )
            })
        })
        .collect()
}

impl Session {
    /// The motion tweens the timeline's context menu acts on. With a frame
    /// selection: the spans of instance keys the selection covers on the
    /// selected layers (active layer included). Without one: the exposure
    /// holding `cell` (the right-clicked layer and frame), or the active
    /// layer at the playhead. Locked layers are skipped.
    pub fn tween_targets(&self, cell: Option<(LayerId, FrameIndex)>) -> TweenTargets {
        let (layers, range) = match (self.frame_selection(), cell) {
            (Some(range), _) => (self.selected_layers(), range),
            (None, Some((layer, frame))) => {
                (vec![layer], FrameRange::new(frame, FrameIndex(frame.0 + 1)))
            }
            (None, None) => {
                let frame = self.current_frame();
                (
                    self.state.active_layer.into_iter().collect(),
                    FrameRange::new(frame, FrameIndex(frame.0 + 1)),
                )
            }
        };
        let mut targets = TweenTargets::default();
        let mut eases = Vec::new();
        for layer in layers {
            if !self.document.layers().is_editable(layer) {
                continue;
            }
            for (frame, tween, can) in picked_keys(&self.document, layer, range) {
                if can {
                    targets.creatable.push((layer, frame));
                    eases.push(tween);
                }
                if tween.is_some() {
                    targets.removable.push((layer, frame));
                }
            }
        }
        targets.current = match eases.split_first() {
            Some((first, rest)) if rest.iter().all(|e| e == first) => *first,
            _ => None,
        };
        targets
    }

    /// Sets (`Some`) or removes (`None`) the motion tween on each key in
    /// `keys`; one undo step. Returns whether it changed anything.
    pub fn set_tweens(
        &mut self,
        keys: &[(LayerId, FrameIndex)],
        ease: Option<Interpolation>,
    ) -> bool {
        let commands: Vec<Box<dyn Command>> = keys
            .iter()
            .map(|&(layer, frame)| Box::new(SetTween::new(layer, frame, ease)) as Box<dyn Command>)
            .collect();
        let label = match ease {
            Some(_) => "Create Motion Tween",
            None => "Remove Tween",
        };
        let ok = match commands.len() {
            0 => return false,
            1 => commands
                .into_iter()
                .next()
                .is_some_and(|c| self.execute(c).is_ok()),
            _ => self.execute(Box::new(Batch::new(label, commands))).is_ok(),
        };
        if ok {
            let what = match ease {
                Some(e) => format!("{} motion tween", e.label()),
                None => "Removed the motion tween".to_owned(),
            };
            let plural = if keys.len() > 1 { "s" } else { "" };
            self.set_status(match ease {
                Some(_) => format!("{what}{plural} created"),
                None => format!("{what}{plural}"),
            });
        }
        ok
    }

    /// The motion tween setting of the object the active layer shows at
    /// the playhead, if it shows one.
    pub fn tween_at_playhead(&self) -> Option<TweenAt> {
        let at = self.instance_at_playhead()?;
        let track = self.document.track(at.layer)?;
        let end = track
            .next_key_after(at.key_frame)
            .filter(|(_, k)| k.instance().is_some_and(|i| i.symbol == at.key.symbol))
            .map(|(f, _)| f);
        Some(TweenAt {
            layer: at.layer,
            key_frame: at.key_frame,
            tween: at.key.tween,
            end,
        })
    }

    /// Sets or removes the motion tween from the object key under the
    /// playhead (Tool Settings); one undo step.
    pub fn set_tween_at_playhead(&mut self, ease: Option<Interpolation>) -> bool {
        let Some(at) = self.tween_at_playhead() else {
            return false;
        };
        if at.tween == ease {
            return false;
        }
        self.set_tweens(&[(at.layer, at.key_frame)], ease)
    }

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

    /// A session whose active layer places one object on frames 0, 6 and
    /// 12 of 18 (x = 0, 60, 120).
    fn three_object_keys() -> (Session, LayerId) {
        let (mut s, layer) = twelve_frames();
        s.select_frames_to(FrameIndex(0));
        assert!(s.convert_layers_to_object());
        let object = s.state.active_layer.unwrap();
        let symbol = *s.document.symbols().keys().next().unwrap();
        s.execute(Box::new(editor::commands::RemoveKeyframe::new(
            object,
            FrameIndex(1),
        )))
        .unwrap();
        for f in [0, 6, 12] {
            let key = InstanceKey::new(symbol).with_transform(InstanceTransform::from_translation(
                Vec2::new(10.0 * f as f32, 0.0),
            ));
            s.execute(Box::new(PlaceInstance::new(object, FrameIndex(f), key)))
                .unwrap();
        }
        let _ = layer;
        (s, object)
    }

    #[test]
    fn selecting_from_key_to_key_creates_one_tween() {
        let (mut s, layer) = three_object_keys();
        s.set_frame(FrameIndex(0));
        s.select_frames_to(FrameIndex(6));
        let targets = s.tween_targets(None);
        assert_eq!(targets.creatable, [(layer, FrameIndex(0))]);
        assert!(targets.removable.is_empty());
        assert_eq!(targets.current, None);
        assert!(s.set_tweens(&targets.creatable, Some(Interpolation::EaseInOut)));
        let x = |s: &Session, f: u32| {
            s.document
                .track(layer)
                .unwrap()
                .displayed_instance(FrameIndex(f))
                .unwrap()
                .1
                .transform
                .translation
                .x
        };
        assert_eq!(x(&s, 3), 30.0);
        assert!(x(&s, 1) < 10.0);
        assert_eq!(x(&s, 8), 60.0);
        let targets = s.tween_targets(None);
        assert_eq!(targets.current, Some(Interpolation::EaseInOut));
        assert_eq!(targets.removable, [(layer, FrameIndex(0))]);

        // Selecting across all three keys picks both spans: one undo step.
        s.select_frames_to(FrameIndex(17));
        let targets = s.tween_targets(None);
        assert_eq!(
            targets.creatable,
            [(layer, FrameIndex(0)), (layer, FrameIndex(6))]
        );
        assert_eq!(targets.current, None);
        let undo = s.history.undo_len();
        assert!(s.set_tweens(&targets.creatable, Some(Interpolation::Linear)));
        assert_eq!(s.history.undo_len(), undo + 1);
        assert_eq!(x(&s, 9), 90.0);
        // The last key has nothing to tween to.
        s.clear_frame_selection();
        assert!(
            s.tween_targets(Some((layer, FrameIndex(14))))
                .creatable
                .is_empty()
        );
        // Right-click inside a tweened span, then remove it.
        let targets = s.tween_targets(Some((layer, FrameIndex(8))));
        assert_eq!(targets.removable, [(layer, FrameIndex(6))]);
        assert!(s.set_tweens(&targets.removable, None));
        assert_eq!(x(&s, 9), 60.0);
        s.undo();
        assert_eq!(x(&s, 9), 90.0);
    }

    #[test]
    fn drawing_layers_and_locked_layers_offer_no_tween() {
        let (mut s, layer) = twelve_frames();
        s.set_frame(FrameIndex(0));
        s.select_frames_to(FrameIndex(6));
        assert_eq!(s.tween_targets(None), TweenTargets::default());
        let (mut s, object) = three_object_keys();
        s.set_frame(FrameIndex(0));
        s.select_frames_to(FrameIndex(6));
        s.execute(Box::new(editor::commands::SetLayerLocked::new(
            object, true,
        )))
        .unwrap();
        assert!(s.tween_targets(None).creatable.is_empty());
        let _ = layer;
    }

    #[test]
    fn tool_settings_edit_the_tween_under_the_playhead() {
        let (mut s, layer) = three_object_keys();
        s.set_frame(FrameIndex(3));
        let at = s.tween_at_playhead().unwrap();
        assert_eq!(
            (at.layer, at.key_frame, at.tween, at.end),
            (layer, FrameIndex(0), None, Some(FrameIndex(6)))
        );
        assert!(s.set_tween_at_playhead(Some(Interpolation::EaseIn)));
        assert!(!s.set_tween_at_playhead(Some(Interpolation::EaseIn)));
        assert_eq!(
            s.tween_at_playhead().unwrap().tween,
            Some(Interpolation::EaseIn)
        );
        assert!(s.set_tween_at_playhead(None));
        assert_eq!(s.tween_at_playhead().unwrap().tween, None);
        // The last key has no partner.
        s.set_frame(FrameIndex(14));
        assert_eq!(s.tween_at_playhead().unwrap().end, None);
        assert!(!s.set_tween_at_playhead(Some(Interpolation::Linear)));
        assert!(s.status().unwrap().is_error);
    }

    #[test]
    fn span_picking_rules() {
        let r = |a: u32, b: u32| FrameRange::new(FrameIndex(a), FrameIndex(b));
        let f = FrameIndex;
        // One frame: the exposure holding it.
        assert!(span_picked(f(0), Some(f(6)), 18, r(5, 6)));
        assert!(!span_picked(f(0), Some(f(6)), 18, r(6, 7)));
        assert!(span_picked(f(12), None, 18, r(17, 18)));
        // Key to key, inclusive: that pair only.
        assert!(span_picked(f(0), Some(f(6)), 18, r(0, 7)));
        assert!(!span_picked(f(6), Some(f(12)), 18, r(0, 7)));
        // Two frames inside one span.
        assert!(span_picked(f(0), Some(f(6)), 18, r(2, 4)));
        // A span the selection only touches at its last key is not picked.
        assert!(!span_picked(f(0), Some(f(6)), 18, r(6, 10)));
    }
}
