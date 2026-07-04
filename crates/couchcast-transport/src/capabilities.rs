//! What a given target can actually do, so unsupported actions are dropped
//! rather than erroring.

use crate::event::RemoteAction;

/// The set of [`RemoteAction`] categories a target understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceCapabilities {
    /// D-pad navigation + select/back/home/menu.
    pub navigation: bool,
    /// Play/pause/stop/seek/skip.
    pub media_keys: bool,
    /// Volume up/down/mute.
    pub volume: bool,
    /// Power on/off/toggle.
    pub power: bool,
    /// On-screen text entry.
    pub text_input: bool,
    /// Analog stick / trigger passthrough.
    pub analog: bool,
    /// Raw gamepad button passthrough (for games on the target).
    pub raw_gamepad: bool,
}

impl DeviceCapabilities {
    /// A target that can do nothing (useful as a base to build up from).
    pub const NONE: Self = Self {
        navigation: false,
        media_keys: false,
        volume: false,
        power: false,
        text_input: false,
        analog: false,
        raw_gamepad: false,
    };

    /// A typical Android TV / Fire TV target reached over ADB — it accepts the
    /// full range of actions.
    pub const ANDROID_TV: Self = Self {
        navigation: true,
        media_keys: true,
        volume: true,
        power: true,
        text_input: true,
        analog: true,
        raw_gamepad: true,
    };

    /// A basic HDMI-CEC / infrared style remote: navigation, media and power,
    /// but no text or analog.
    pub const BASIC_REMOTE: Self = Self {
        navigation: true,
        media_keys: true,
        volume: true,
        power: true,
        text_input: false,
        analog: false,
        raw_gamepad: false,
    };

    /// Whether this target can express `action`.
    pub fn supports(&self, action: &RemoteAction) -> bool {
        use RemoteAction::*;
        match action {
            Navigate(_) | Select | Back | Home | Menu => self.navigation,
            PlayPause | Play | Pause | Stop | Rewind | FastForward | Next | Previous => {
                self.media_keys
            }
            VolumeUp | VolumeDown | Mute => self.volume,
            Power => self.power,
            Text(_) => self.text_input,
            Analog { .. } => self.analog,
            GamepadButton { .. } => self.raw_gamepad,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Direction, PadAxis, PadButton};

    #[test]
    fn android_tv_supports_everything_common() {
        let caps = DeviceCapabilities::ANDROID_TV;
        assert!(caps.supports(&RemoteAction::Navigate(Direction::Up)));
        assert!(caps.supports(&RemoteAction::Text("hi".into())));
        assert!(caps.supports(&RemoteAction::GamepadButton {
            button: PadButton::South,
            pressed: true,
        }));
    }

    #[test]
    fn basic_remote_drops_text_and_analog() {
        let caps = DeviceCapabilities::BASIC_REMOTE;
        assert!(caps.supports(&RemoteAction::Select));
        assert!(!caps.supports(&RemoteAction::Text("hi".into())));
        assert!(!caps.supports(&RemoteAction::Analog {
            axis: PadAxis::LeftStickX,
            value: 0.5,
        }));
    }
}
