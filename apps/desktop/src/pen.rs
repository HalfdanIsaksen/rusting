//! Pen pressure from drawing tablets (ADR-025).
//!
//! The canvas reads pen pressure from egui touch events: the force of a touch
//! is the pressure of the pointer events that follow it (`shell.rs`). On
//! Windows, winit already reports a pen as touches with a force, and
//! egui-winit turns them into touch events plus emulated pointer events.
//!
//! On macOS, winit reports a tablet pen as a plain mouse. AppKit does carry
//! the pressure on each mouse event from a tablet, so a local event monitor
//! reads it, and [`PenInput`] adds a touch event with that force just before
//! egui-winit sees the matching winit event. The monitor runs right before
//! AppKit hands the same event to winit's view, which reports it to us at
//! once, so the pressure it stored belongs to that event.
//!
//! Elsewhere (Linux) the pen draws like a mouse, at full pressure.

use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, WindowEvent};

/// The touch device the macOS pen is reported as.
const PEN_DEVICE: egui::TouchDeviceId = egui::TouchDeviceId(0x0070_656e);

/// Turns tablet mouse events into egui touch events carrying the pressure.
pub struct PenInput {
    /// The AppKit event monitor, kept alive for the app's lifetime.
    #[cfg(target_os = "macos")]
    _monitor: Option<objc2::rc::Retained<objc2::runtime::AnyObject>>,
    /// A pen stroke is in progress (the left button went down with a pen).
    down: bool,
    /// The pressure last reported during the stroke.
    force: f32,
    /// The latest cursor position, in physical pixels.
    cursor: PhysicalPosition<f64>,
}

impl PenInput {
    /// Starts listening for tablet pressure (macOS; elsewhere it does
    /// nothing). Call on the main thread once the event loop runs.
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            _monitor: macos::install(),
            ..Self::without_monitor()
        }
    }

    /// No pen stroke yet, and no AppKit event monitor.
    fn without_monitor() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            _monitor: None,
            down: false,
            force: 1.0,
            cursor: PhysicalPosition::new(0.0, 0.0),
        }
    }

    /// The touch event to add before egui-winit handles `event`, if `event`
    /// is part of a pen stroke.
    pub fn touch_for(&mut self, event: &WindowEvent, pixels_per_point: f32) -> Option<egui::Event> {
        self.step(event, tablet_pressure(), pixels_per_point)
    }

    /// [`Self::touch_for`] with the pressure of the current AppKit event
    /// (`None` when it didn't come from a tablet).
    fn step(
        &mut self,
        event: &WindowEvent,
        pressure: Option<f32>,
        pixels_per_point: f32,
    ) -> Option<egui::Event> {
        let phase = match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = *position;
                if !self.down {
                    return None;
                }
                egui::TouchPhase::Move
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if pressure.is_some() => {
                self.down = true;
                egui::TouchPhase::Start
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } if self.down => {
                self.down = false;
                egui::TouchPhase::End
            }
            _ => return None,
        };
        if let Some(p) = pressure.filter(|p| p.is_finite()) {
            self.force = p.clamp(0.0, 1.0);
        }
        let ppp = pixels_per_point.max(f32::EPSILON);
        Some(egui::Event::Touch {
            device_id: PEN_DEVICE,
            id: egui::TouchId(0),
            phase,
            pos: egui::pos2(self.cursor.x as f32 / ppp, self.cursor.y as f32 / ppp),
            force: Some(self.force),
        })
    }
}

/// The pen pressure of the AppKit event being handled, if it came from a
/// tablet.
#[cfg(target_os = "macos")]
fn tablet_pressure() -> Option<f32> {
    macos::pressure()
}

/// Only macOS needs the pressure added (see the module docs).
#[cfg(not(target_os = "macos"))]
fn tablet_pressure() -> Option<f32> {
    None
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventSubtype, NSEventType};
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// The last left-button or tablet event's pressure as `f32` bits; NaN
    /// when that event came from a mouse or trackpad.
    static PRESSURE: AtomicU32 = AtomicU32::new(f32::NAN.to_bits());

    pub fn pressure() -> Option<f32> {
        let p = f32::from_bits(PRESSURE.load(Ordering::Relaxed));
        p.is_finite().then_some(p)
    }

    /// Records the pressure of every left-button and tablet event, passing
    /// each event on unchanged.
    pub fn install() -> Option<Retained<AnyObject>> {
        let mask = NSEventMask::LeftMouseDown
            | NSEventMask::LeftMouseDragged
            | NSEventMask::LeftMouseUp
            | NSEventMask::TabletPoint;
        let handler = block2::RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit hands the monitor a valid event for the
            // duration of the call.
            #[allow(unsafe_code)]
            let e = unsafe { event.as_ref() };
            // `subtype` is valid for mouse and tablet events, which is all
            // the mask lets through.
            let tablet = e.r#type() == NSEventType::TabletPoint
                || e.subtype() == NSEventSubtype::TabletPoint;
            let p = if tablet { e.pressure() } else { f32::NAN };
            PRESSURE.store(p.to_bits(), Ordering::Relaxed);
            event.as_ptr()
        });
        // SAFETY: the handler returns the event it was given, a valid
        // pointer.
        #[allow(unsafe_code)]
        let monitor =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &handler) };
        if monitor.is_none() {
            tracing::warn!("pen pressure: AppKit refused the event monitor");
        }
        monitor
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::event::DeviceId;

    fn moved(x: f64) -> WindowEvent {
        WindowEvent::CursorMoved {
            device_id: DeviceId::dummy(),
            position: PhysicalPosition::new(x, 20.0),
        }
    }

    fn button(state: ElementState) -> WindowEvent {
        WindowEvent::MouseInput {
            device_id: DeviceId::dummy(),
            state,
            button: MouseButton::Left,
        }
    }

    fn touch(event: Option<egui::Event>) -> Option<(egui::TouchPhase, egui::Pos2, f32)> {
        match event? {
            egui::Event::Touch {
                phase, pos, force, ..
            } => Some((phase, pos, force?)),
            _ => None,
        }
    }

    #[test]
    fn a_tablet_stroke_becomes_touches_with_its_pressure() {
        use egui::TouchPhase::{End, Move, Start};
        let mut pen = PenInput::without_monitor();
        assert_eq!(touch(pen.step(&moved(10.0), Some(0.3), 2.0)), None);
        assert_eq!(
            touch(pen.step(&button(ElementState::Pressed), Some(0.3), 2.0)),
            Some((Start, egui::pos2(5.0, 10.0), 0.3))
        );
        assert_eq!(
            touch(pen.step(&moved(30.0), Some(0.8), 2.0)),
            Some((Move, egui::pos2(15.0, 10.0), 0.8))
        );
        assert_eq!(
            touch(pen.step(&button(ElementState::Released), Some(0.0), 2.0)),
            Some((End, egui::pos2(15.0, 10.0), 0.0))
        );
        // After the stroke, moves are plain hovering.
        assert_eq!(touch(pen.step(&moved(40.0), Some(0.5), 2.0)), None);
    }

    #[test]
    fn a_mouse_adds_no_touches() {
        let mut pen = PenInput::without_monitor();
        for event in [
            moved(10.0),
            button(ElementState::Pressed),
            moved(30.0),
            button(ElementState::Released),
        ] {
            assert_eq!(touch(pen.step(&event, None, 1.0)), None);
        }
    }
}
