//! Controller (and, later, keyboard) input reading for Couchcast.
//!
//! Under SteamOS Gaming Mode, Steam Input grabs the physical controller and
//! re-presents it as a virtual Xbox-style pad (the "Steam Virtual Gamepad",
//! Valve `28DE:11FF`). We deliberately read **that** normalized pad via `gilrs`
//! rather than the raw physical device — reading the raw node fights Steam's
//! remap and produces double/ghost input.
//!
//! This crate turns `gilrs` events into a small, UI-toolkit-free, device-agnostic
//! [`PadEvent`] stream expressed in `couchcast-transport`'s vocabulary. The app
//! drives [`InputManager::poll`] from the winit event loop and routes the events
//! two ways:
//!
//! * **Menu open** → a [`NavDir`] (from the D-pad or left stick, with
//!   [`NavRepeater`] hold-to-repeat) moves the owned menu cursor.
//! * **Menu closed** ("capture mode") → the button map turns each press into a
//!   [`RemoteAction`](couchcast_transport::RemoteAction) forwarded to the target.
//!
//! Keyboard reading (which gamepad crates ignore) will use `evdev`; see
//! `docs/ROADMAP.md`.

use gilrs::{Axis, Button, EventType, Gilrs};

use couchcast_transport::{PadAxis, PadButton};

/// Errors initializing the input subsystem.
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("failed to initialize gamepad input: {0}")]
    Init(String),
}

/// A normalized controller event, free of any `gilrs` types.
#[derive(Debug, Clone, PartialEq)]
pub enum PadEvent {
    /// A face/shoulder/dpad button changed state.
    Button { button: PadButton, pressed: bool },
    /// An analog axis moved (value normalized to `[-1.0, 1.0]`).
    Axis { axis: PadAxis, value: f32 },
    /// A controller was connected.
    Connected { name: String },
    /// A controller was disconnected.
    Disconnected { name: String },
}

/// A high-level UI navigation intent derived from a [`PadEvent`] — a direction
/// plus confirm/cancel. (The menu drives its cursor from [`NavDir`]; this richer
/// intent is kept for reuse.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavEvent {
    Up,
    Down,
    Left,
    Right,
    /// Activate the focused widget (A / South).
    Activate,
    /// Close the overlay / go back (B / East).
    Back,
}

/// Reads controllers and yields normalized [`PadEvent`]s.
pub struct InputManager {
    gilrs: Gilrs,
}

impl InputManager {
    /// Initialize the input subsystem. On Linux this opens the udev/evdev
    /// backend; it needs read access to `/dev/input/event*` (granted for a
    /// Steam-launched app on SteamOS).
    pub fn new() -> Result<Self, InputError> {
        let gilrs = Gilrs::new().map_err(|e| InputError::Init(e.to_string()))?;
        Ok(Self { gilrs })
    }

    /// Drain all pending controller events. Call this every frame/tick from the
    /// main loop; it never blocks.
    pub fn poll(&mut self) -> Vec<PadEvent> {
        let mut out = Vec::new();
        while let Some(event) = self.gilrs.next_event() {
            match event.event {
                EventType::ButtonPressed(button, _) => {
                    if let Some(b) = map_button(button) {
                        out.push(PadEvent::Button {
                            button: b,
                            pressed: true,
                        });
                    }
                }
                EventType::ButtonReleased(button, _) => {
                    if let Some(b) = map_button(button) {
                        out.push(PadEvent::Button {
                            button: b,
                            pressed: false,
                        });
                    }
                }
                EventType::AxisChanged(axis, value, _) => {
                    if let Some(a) = map_axis(axis) {
                        out.push(PadEvent::Axis { axis: a, value });
                    }
                }
                EventType::Connected => {
                    let name = self.gilrs.gamepad(event.id).name().to_owned();
                    tracing::info!(%name, "controller connected");
                    out.push(PadEvent::Connected { name });
                }
                EventType::Disconnected => {
                    let name = self.gilrs.gamepad(event.id).name().to_owned();
                    tracing::info!(%name, "controller disconnected");
                    out.push(PadEvent::Disconnected { name });
                }
                _ => {}
            }
        }
        out
    }

    /// Names of the controllers currently connected. Under Gaming Mode this is
    /// usually a single "Steam Virtual Gamepad".
    pub fn connected_names(&self) -> Vec<String> {
        self.gilrs
            .gamepads()
            .map(|(_, gamepad)| gamepad.name().to_owned())
            .collect()
    }
}

/// Derive a UI navigation intent from a pad event (button presses only).
pub fn nav_from_pad(event: &PadEvent) -> Option<NavEvent> {
    let PadEvent::Button {
        button,
        pressed: true,
    } = event
    else {
        return None;
    };
    Some(match button {
        PadButton::DPadUp => NavEvent::Up,
        PadButton::DPadDown => NavEvent::Down,
        PadButton::DPadLeft => NavEvent::Left,
        PadButton::DPadRight => NavEvent::Right,
        PadButton::South => NavEvent::Activate,
        PadButton::East => NavEvent::Back,
        _ => return None,
    })
}

/// A pure directional intent (no Activate/Back), used by the menu's hold-to-repeat
/// cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavDir {
    Up,
    Down,
    Left,
    Right,
}

/// Left-stick deflection past this magnitude counts as a directional press.
pub const LEFT_STICK_DEADZONE: f32 = 0.5;

/// Convert a left-stick position to a directional intent, or `None` inside the
/// dead zone. The dominant axis wins. `gilrs` reports `LeftStickY` as +up/-down.
pub fn stick_to_nav(x: f32, y: f32) -> Option<NavDir> {
    if x.abs() < LEFT_STICK_DEADZONE && y.abs() < LEFT_STICK_DEADZONE {
        return None;
    }
    if x.abs() >= y.abs() {
        Some(if x > 0.0 { NavDir::Right } else { NavDir::Left })
    } else {
        Some(if y > 0.0 { NavDir::Up } else { NavDir::Down })
    }
}

/// Turns a *held* direction into a stream of discrete nav steps: one immediately
/// on press, then — after an initial delay — repeats at a steady rate while held.
/// This gives menu navigation the auto-repeat every game UI has, instead of one
/// step per physical press.
pub struct NavRepeater {
    dir: Option<NavDir>,
    next_fire: Option<std::time::Instant>,
    initial_delay: std::time::Duration,
    interval: std::time::Duration,
}

impl Default for NavRepeater {
    fn default() -> Self {
        Self {
            dir: None,
            next_fire: None,
            initial_delay: std::time::Duration::from_millis(400),
            interval: std::time::Duration::from_millis(90),
        }
    }
}

impl NavRepeater {
    /// Advance the repeater. `desired` is the direction currently held (from the
    /// D-pad or left stick), or `None` if neutral. Returns `Some(dir)` on the
    /// ticks a step should be applied.
    pub fn tick(&mut self, now: std::time::Instant, desired: Option<NavDir>) -> Option<NavDir> {
        match desired {
            None => {
                self.dir = None;
                self.next_fire = None;
                None
            }
            Some(d) if self.dir != Some(d) => {
                // New direction → fire once and arm the initial delay.
                self.dir = Some(d);
                self.next_fire = Some(now + self.initial_delay);
                Some(d)
            }
            Some(d) => match self.next_fire {
                Some(nf) if now >= nf => {
                    self.next_fire = Some(now + self.interval);
                    Some(d)
                }
                _ => None,
            },
        }
    }
}

/// Map a `gilrs` button to our normalized [`PadButton`]. `gilrs` already applies
/// SDL_GameControllerDB mappings, so `LeftTrigger`/`RightTrigger` are the bumpers
/// (L1/R1) and `LeftTrigger2`/`RightTrigger2` are the analog triggers (L2/R2).
fn map_button(button: Button) -> Option<PadButton> {
    Some(match button {
        Button::South => PadButton::South,
        Button::East => PadButton::East,
        Button::North => PadButton::North,
        Button::West => PadButton::West,
        Button::LeftTrigger => PadButton::LeftBumper,
        Button::RightTrigger => PadButton::RightBumper,
        Button::LeftTrigger2 => PadButton::LeftTrigger,
        Button::RightTrigger2 => PadButton::RightTrigger,
        Button::Select => PadButton::Select,
        Button::Start => PadButton::Start,
        Button::Mode => PadButton::Guide,
        Button::LeftThumb => PadButton::LeftStick,
        Button::RightThumb => PadButton::RightStick,
        Button::DPadUp => PadButton::DPadUp,
        Button::DPadDown => PadButton::DPadDown,
        Button::DPadLeft => PadButton::DPadLeft,
        Button::DPadRight => PadButton::DPadRight,
        // C / Z / Unknown have no place in an Xbox layout.
        _ => return None,
    })
}

/// Map a `gilrs` axis to our normalized [`PadAxis`]. The D-pad-as-axis variants
/// are ignored because the D-pad is surfaced as buttons.
fn map_axis(axis: Axis) -> Option<PadAxis> {
    Some(match axis {
        Axis::LeftStickX => PadAxis::LeftStickX,
        Axis::LeftStickY => PadAxis::LeftStickY,
        Axis::RightStickX => PadAxis::RightStickX,
        Axis::RightStickY => PadAxis::RightStickY,
        Axis::LeftZ => PadAxis::LeftTrigger,
        Axis::RightZ => PadAxis::RightTrigger,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpad_maps_to_nav() {
        let ev = PadEvent::Button {
            button: PadButton::DPadLeft,
            pressed: true,
        };
        assert_eq!(nav_from_pad(&ev), Some(NavEvent::Left));
    }

    #[test]
    fn south_activates_east_goes_back() {
        assert_eq!(
            nav_from_pad(&PadEvent::Button {
                button: PadButton::South,
                pressed: true
            }),
            Some(NavEvent::Activate)
        );
        assert_eq!(
            nav_from_pad(&PadEvent::Button {
                button: PadButton::East,
                pressed: true
            }),
            Some(NavEvent::Back)
        );
    }

    #[test]
    fn button_release_is_not_navigation() {
        let ev = PadEvent::Button {
            button: PadButton::DPadUp,
            pressed: false,
        };
        assert_eq!(nav_from_pad(&ev), None);
    }

    #[test]
    fn stick_deadzone_and_dominant_axis() {
        assert_eq!(stick_to_nav(0.1, -0.2), None);
        assert_eq!(stick_to_nav(0.9, 0.1), Some(NavDir::Right));
        assert_eq!(stick_to_nav(-0.9, 0.1), Some(NavDir::Left));
        assert_eq!(stick_to_nav(0.1, 0.9), Some(NavDir::Up));
        assert_eq!(stick_to_nav(0.1, -0.9), Some(NavDir::Down));
    }

    #[test]
    fn repeater_fires_once_then_repeats_while_held() {
        use std::time::{Duration, Instant};
        let mut r = NavRepeater::default();
        let t0 = Instant::now();
        // First press fires immediately.
        assert_eq!(r.tick(t0, Some(NavDir::Down)), Some(NavDir::Down));
        // Still held, before the initial delay → no fire.
        assert_eq!(
            r.tick(t0 + Duration::from_millis(100), Some(NavDir::Down)),
            None
        );
        // Past the initial delay → repeat.
        assert_eq!(
            r.tick(t0 + Duration::from_millis(450), Some(NavDir::Down)),
            Some(NavDir::Down)
        );
        // Release clears state.
        assert_eq!(r.tick(t0 + Duration::from_millis(500), None), None);
    }

    #[test]
    fn repeater_refires_on_direction_change() {
        use std::time::Instant;
        let mut r = NavRepeater::default();
        let t0 = Instant::now();
        assert_eq!(r.tick(t0, Some(NavDir::Up)), Some(NavDir::Up));
        // Immediately switching direction fires again.
        assert_eq!(r.tick(t0, Some(NavDir::Left)), Some(NavDir::Left));
    }

    #[test]
    fn gilrs_bumpers_map_to_our_bumpers() {
        assert_eq!(map_button(Button::LeftTrigger), Some(PadButton::LeftBumper));
        assert_eq!(
            map_button(Button::LeftTrigger2),
            Some(PadButton::LeftTrigger)
        );
        assert_eq!(map_button(Button::Mode), Some(PadButton::Guide));
    }
}
