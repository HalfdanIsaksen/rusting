# ADR-019: Motion tweens between object keys

**Status:** Proposed (feat/motion-tweens branch)

## Context
Users want an object to glide from one place to another without drawing every frame: place it on one key, place the same object somewhere else on a later key, select both and make a tween. The tween should run linear, ease in, ease out or ease in-out, and only objects can be tweened. It should animate position, scale, rotation and opacity. Objects (ADR-018) already keep their placement decomposed so two keys can be blended part by part.

## Options
| | Tween flag on the first instance key (chosen) | Separate `PropertyTrack<T>` per property (ADR-003's plan) | Tween as its own key type |
|---|---|---|---|
| Works with insert/remove/move frames and Convert to Object unchanged | Yes: it travels with the key | Needs a second set of tracks kept in step | Needs new rules for every edit |
| Object keeps playing its own animation through the tween | Yes: the first key's playback | Yes | Depends |
| Limited to objects by type | Yes: only `InstanceKey` has the field | No: would need extra checks | Yes |
| Per-property easing, motion paths | No | Yes | No |

## Decision
- **Easing.** `animation_core::Interpolation` gets `EaseIn` (`t³`), `EaseOut` (`1 − (1 − t)³`) and `EaseInOut` (cubic in for the first half, cubic out for the second). `factor` clamps progress to `0..=1` (NaN counts as 0). Every curve maps 0 to 0 and 1 to 1 and never falls. `Interpolation::TWEENS` lists the four kinds a tween can use, and `label()` gives their menu names.
- **Model.** `InstanceKey.tween: Option<Interpolation>` means "tween from this key to the next key on the track". `Some(Step)` is never stored: `SetTween` treats it as `None`, and files are rejected if they hold it.
- **When a tween is active.** A tween is active only while the next key on the same track is an instance of the **same** object (`Track::tween_from`, `Track::tween_at`, `Track::tweens`, `TweenSpan`). Otherwise the key holds still, as it did before. Moving or removing a key so the pair breaks leaves the tween stored but inactive, and it works again once the pair is whole. The timeline draws an inactive tween as a dashed line, and Tool Settings offers to remove it.
- **Evaluation.**
  - `Track::displayed_instance(frame)` returns the exposed key with its transform and opacity blended toward the next key by `ease.factor((f − k0) / (k1 − k0))`. Mode, `first_frame` and the tween stay the first key's, so the inner frame keeps counting from the first key, and the object keeps animating through the tween.
  - `Document::evaluate` and `instance_bounds` use it. Preview, export and canvas hit testing follow without changes. Tweens inside objects work too, because symbol tracks are evaluated the same way.
  - Frame `k1` shows the second key exactly.
  - **Rotation** blends linearly in radians as stored, with no shortest-path wrap. Keys at 0 and 3π/2 turn three quarters of the way round, not a quarter back. This keeps spins of a full turn or more possible.
  - Scale blends per axis, linearly.
- **Commands.**
  - `SetTween { layer, frame, Option<Interpolation> }` sets or removes the tween on the key at `frame`. Setting one is refused with `CommandError::NotTweenable` unless that key and the next key are instances of the same object. Removing one only needs the key to be an instance, so an inactive tween can be removed. `SetTween::can_tween` answers the same question for the UI.
  - **Editing inside a tween** must match what the user sees. Moving, scaling or rotating the object with the Selection or Transform tool on a frame inside a tweened span starts from the displayed (blended) transform. `SetInstanceTransform` then makes a new key on that frame from the displayed key: blended opacity, advanced playback, and the user's transform. The new key keeps the tween, so the motion now runs first key → new key → old end key, each part with the same ease.
  - Duplicate Frame and Convert to Object (for the key they add at a range start or end) also copy the displayed key.
- **UI.**
  - **Which keys the timeline menu acts on.** With a frame selection, it uses the selected layers (the active layer included). On each layer it picks every span from an object key to the next key that the selection overlaps by at least two frames. Selecting from one key to the next picks exactly that pair. Without a selection, it uses the right-clicked layer and the exposure holding the right-clicked frame. Locked layers are skipped.
  - The menu shows "Create Motion Tween ▸ Linear / Ease In / Ease Out / Ease In-Out" when a picked key can tween, with the current ease checked when all picked keys share it. It shows "Remove Tween" when a picked key has a tween. Several spans change in one undo step (`Batch`).
  - The timeline draws a tweened span as a purple band with an arrow to the next key.
  - Tool Settings shows a Motion tween picker (None or an ease) for the object under the playhead, with the frames the tween spans.
- **Files.** Format version 12. `tween` is written only when set (`"Linear"`, `"EaseIn"`, `"EaseOut"`, `"EaseInOut"`), so v11 files load unchanged and need no migration. Older builds must reject v12 files: they would drop the tweens and change the motion. Validation rejects `"Step"`. An unknown ease fails to parse. A tween on a drawing or empty key cannot be written, because those keys have no fields.

## Consequences
- One ease applies to all four properties. There is no per-property easing, Bezier curve or motion path yet. ADR-003's `PropertyTrack` stays a later option for layer-level property animation.
- Rotation does not take the shortest path. To turn from 350° to 10°, store 370°. The canvas transform tool stores the angle `from_affine` gives (−π..π), so rotating past 180° on the canvas can make a tween spin the long way.
- Splitting a tween with a new key keeps the ease on both halves. Eased motion therefore slows at the new key too. Use Linear for a constant speed through it.
- Opacity in Tool Settings edits the key the tween starts from, not the blended value shown on the frame.
- Onion skin still shows neighbouring drawing keys only. It does not show objects, so tweened motion is not onion-skinned.
- Moving a key that has an inactive tween next to a key of the same object turns the tween back on. This follows from keeping inactive tweens.
