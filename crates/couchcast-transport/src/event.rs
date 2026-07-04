//! The device-agnostic input vocabulary shared across the app.

use serde::{Deserialize, Serialize};

/// A directional navigation intent — the lowest common denominator every target
/// (Fire TV, Roku, a CEC TV, …) understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// Physical gamepad buttons, normalized to an Xbox-style layout — exactly how
/// `gilrs` and Steam Input present a controller. This is the *source* side of
/// the button map ([`couchcast-config`](../couchcast_config/index.html)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PadButton {
    /// A (bottom face button).
    South,
    /// B (right face button).
    East,
    /// Y (top face button).
    North,
    /// X (left face button).
    West,
    LeftBumper,
    RightBumper,
    LeftTrigger,
    RightTrigger,
    /// "View" / "Back" / minus.
    Select,
    /// "Menu" / "Start" / plus.
    Start,
    /// Guide / Steam / Home button.
    Guide,
    LeftStick,
    RightStick,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

/// Analog axes, normalized to `[-1.0, 1.0]` (triggers use `[0.0, 1.0]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PadAxis {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    LeftTrigger,
    RightTrigger,
}

/// A device-agnostic action to forward to the target.
///
/// Backends downgrade gracefully according to their [`DeviceCapabilities`]
/// (`super::DeviceCapabilities`): a dpad-only CEC target simply ignores
/// [`RemoteAction::Analog`] and [`RemoteAction::GamepadButton`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteAction {
    // --- Navigation (universal) ---
    Navigate(Direction),
    Select,
    Back,
    Home,
    Menu,

    // --- Media transport ---
    PlayPause,
    Play,
    Pause,
    Stop,
    Rewind,
    FastForward,
    Next,
    Previous,

    // --- Volume / power ---
    VolumeUp,
    VolumeDown,
    Mute,
    Power,

    /// Text entry into an on-screen field (search boxes, logins, …).
    Text(String),

    // --- Raw game-controller passthrough (targets that accept a real gamepad) ---
    GamepadButton {
        button: PadButton,
        pressed: bool,
    },
    Analog {
        axis: PadAxis,
        value: f32,
    },
}

impl RemoteAction {
    /// A short, human-readable label for logs and the mapping UI.
    pub fn label(&self) -> String {
        match self {
            RemoteAction::Navigate(d) => format!("Navigate {d:?}"),
            RemoteAction::Text(t) => format!("Text({t:?})"),
            RemoteAction::GamepadButton { button, pressed } => {
                format!("{button:?} {}", if *pressed { "down" } else { "up" })
            }
            RemoteAction::Analog { axis, value } => format!("{axis:?} = {value:.2}"),
            other => format!("{other:?}"),
        }
    }
}
