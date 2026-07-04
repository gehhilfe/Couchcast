//! The editable controller → action button map.
//!
//! Stored as a plain list of bindings so it serializes to clean, human-editable
//! TOML (`[[mapping]]` tables) and is easy to render in the settings UI. Applied
//! in "capture mode" (overlay closed): each incoming [`PadButton`] is looked up
//! and, if bound, the resulting [`RemoteAction`] is forwarded to the target.

use serde::{Deserialize, Serialize};

use couchcast_transport::{Direction, PadButton, RemoteAction};

/// A single controller-button → action binding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub button: PadButton,
    pub action: RemoteAction,
}

/// An ordered list of [`Binding`]s. Serializes transparently as a TOML array of
/// tables, i.e. the config file simply contains repeated `[[mapping]]` entries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ButtonMap {
    pub bindings: Vec<Binding>,
}

impl ButtonMap {
    /// The action currently bound to `button`, if any.
    pub fn action_for(&self, button: PadButton) -> Option<&RemoteAction> {
        self.bindings
            .iter()
            .find(|b| b.button == button)
            .map(|b| &b.action)
    }

    /// Bind (or rebind) `button` to `action`.
    pub fn set(&mut self, button: PadButton, action: RemoteAction) {
        match self.bindings.iter_mut().find(|b| b.button == button) {
            Some(existing) => existing.action = action,
            None => self.bindings.push(Binding { button, action }),
        }
    }

    /// Remove any binding for `button`.
    pub fn clear(&mut self, button: PadButton) {
        self.bindings.retain(|b| b.button != button);
    }
}

impl Default for ButtonMap {
    /// A sensible default mapping for driving an Android TV / Fire TV UI with a
    /// game controller.
    fn default() -> Self {
        use PadButton::*;
        use RemoteAction as Act;
        let bindings = vec![
            Binding {
                button: DPadUp,
                action: Act::Navigate(Direction::Up),
            },
            Binding {
                button: DPadDown,
                action: Act::Navigate(Direction::Down),
            },
            Binding {
                button: DPadLeft,
                action: Act::Navigate(Direction::Left),
            },
            Binding {
                button: DPadRight,
                action: Act::Navigate(Direction::Right),
            },
            Binding {
                button: South,
                action: Act::Select,
            },
            Binding {
                button: East,
                action: Act::Back,
            },
            Binding {
                button: North,
                action: Act::Menu,
            },
            Binding {
                button: West,
                action: Act::PlayPause,
            },
            Binding {
                button: Start,
                action: Act::Menu,
            },
            Binding {
                button: Guide,
                action: Act::Home,
            },
            Binding {
                button: LeftBumper,
                action: Act::Rewind,
            },
            Binding {
                button: RightBumper,
                action: Act::FastForward,
            },
        ];
        Self { bindings }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_map_covers_dpad_and_face_buttons() {
        let map = ButtonMap::default();
        assert_eq!(
            map.action_for(PadButton::DPadUp),
            Some(&RemoteAction::Navigate(Direction::Up))
        );
        assert_eq!(
            map.action_for(PadButton::South),
            Some(&RemoteAction::Select)
        );
        assert_eq!(map.action_for(PadButton::East), Some(&RemoteAction::Back));
    }

    #[test]
    fn set_rebinds_in_place() {
        let mut map = ButtonMap::default();
        map.set(PadButton::South, RemoteAction::Home);
        assert_eq!(map.action_for(PadButton::South), Some(&RemoteAction::Home));
        // No duplicate binding was added.
        let count = map
            .bindings
            .iter()
            .filter(|b| b.button == PadButton::South)
            .count();
        assert_eq!(count, 1);
    }
}
