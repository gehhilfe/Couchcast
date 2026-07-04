//! Controller (and, later, keyboard) input reading for Couchcast.
//!
//! Under SteamOS Gaming Mode, Steam Input grabs the physical controller and
//! re-presents it as a virtual Xbox-style pad (the "Steam Virtual Gamepad",
//! Valve `28DE:11FF`). We deliberately read **that** normalized pad via `gilrs`
//! rather than the raw physical device — reading the raw node fights Steam's
//! remap and produces double/ghost input.
//!
//! This crate turns `gilrs` events into a small, GTK-free, device-agnostic
//! [`PadEvent`] stream expressed in `couchcast-transport`'s vocabulary. The app
//! drives [`InputManager::poll`] from the glib main loop and routes the events
//! two ways:
//!
//! * **Overlay open** → [`nav_from_pad`] converts them to [`NavEvent`]s that move
//!   GTK focus (D-pad-only menu navigation).
//! * **Overlay closed** ("capture mode") → the button map turns them into
//!   [`RemoteAction`](couchcast_transport::RemoteAction)s forwarded to the target.
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

/// A high-level UI navigation intent derived from a [`PadEvent`], used to drive
/// GTK focus when the settings overlay is open.
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
    fn gilrs_bumpers_map_to_our_bumpers() {
        assert_eq!(map_button(Button::LeftTrigger), Some(PadButton::LeftBumper));
        assert_eq!(
            map_button(Button::LeftTrigger2),
            Some(PadButton::LeftTrigger)
        );
        assert_eq!(map_button(Button::Mode), Some(PadButton::Guide));
    }
}
