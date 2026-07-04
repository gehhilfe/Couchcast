//! An optional on-screen HUD that lists the controller buttons currently held.
//!
//! It is compiled only with the `debug-input-hud` Cargo feature:
//!
//! ```sh
//! cargo run -p couchcast --features debug-input-hud
//! ```
//!
//! The HUD is a debugging aid for confirming exactly what Steam Input hands the
//! app — for example that the Guide/Steam button now reaches Couchcast (instead
//! of popping the Steam overlay) under Gaming Mode / Big Picture. When the
//! feature is off, [`ButtonHud`] is a zero-sized no-op, so the call sites in
//! `app.rs` stay free of `#[cfg]` clutter.

use std::collections::HashSet;

use couchcast_transport::PadButton;

#[cfg(feature = "debug-input-hud")]
pub use enabled::ButtonHud;

#[cfg(not(feature = "debug-input-hud"))]
pub use disabled::ButtonHud;

#[cfg(feature = "debug-input-hud")]
mod enabled {
    use super::*;

    /// Fixed display order so the HUD text doesn't reshuffle as the (unordered)
    /// pressed set changes. Lists every [`PadButton`] variant.
    const DISPLAY_ORDER: &[PadButton] = &[
        PadButton::DPadUp,
        PadButton::DPadDown,
        PadButton::DPadLeft,
        PadButton::DPadRight,
        PadButton::North,
        PadButton::South,
        PadButton::West,
        PadButton::East,
        PadButton::LeftBumper,
        PadButton::RightBumper,
        PadButton::LeftTrigger,
        PadButton::RightTrigger,
        PadButton::LeftStick,
        PadButton::RightStick,
        PadButton::Select,
        PadButton::Start,
        PadButton::Guide,
    ];

    /// Tracks the connected controllers and the buttons currently held, and
    /// draws them as a translucent overlay in the top-left via egui.
    ///
    /// The connected-pad line is the key diagnostic: an empty "Pads" line means
    /// `gilrs` sees no controller at all (Steam isn't presenting a virtual
    /// gamepad — e.g. the shortcut is on a keyboard/mouse layout), whereas a pad
    /// listed with no buttons reacting means Steam isn't routing input to this
    /// window (a focus-tracking problem).
    #[derive(Default)]
    pub struct ButtonHud {
        devices: Vec<String>,
        held: Vec<&'static str>,
    }

    impl ButtonHud {
        pub fn new() -> Self {
            Self::default()
        }

        /// Refresh the held-button line from the current set of held buttons.
        pub fn update(&mut self, pressed: &HashSet<PadButton>) {
            self.held = DISPLAY_ORDER
                .iter()
                .filter(|b| pressed.contains(b))
                .map(|b| button_label(*b))
                .collect();
        }

        /// Refresh the connected-controllers line (call on connect/disconnect).
        pub fn set_devices(&mut self, names: &[String]) {
            self.devices = names.to_vec();
        }

        /// Draw the HUD as a top-left overlay window.
        pub fn draw(&self, ctx: &egui::Context) {
            let pads = if self.devices.is_empty() {
                "(none — Steam is not presenting a gamepad)".to_owned()
            } else {
                self.devices.join(", ")
            };
            let buttons = if self.held.is_empty() {
                "—".to_owned()
            } else {
                self.held.join("  ")
            };
            egui::Area::new(egui::Id::new("couchcast-button-hud"))
                .anchor(egui::Align2::LEFT_TOP, egui::vec2(12.0, 12.0))
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_black_alpha(184))
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .corner_radius(8.0)
                        .show(ui, |ui| {
                            ui.monospace(format!("Pads: {pads}"));
                            ui.monospace(format!("Buttons: {buttons}"));
                        });
                });
        }
    }

    /// A short, controller-agnostic name for the HUD. Face buttons use their
    /// Xbox letters (least obvious from the enum name); everything else is a
    /// compact abbreviation.
    fn button_label(button: PadButton) -> &'static str {
        match button {
            PadButton::South => "A",
            PadButton::East => "B",
            PadButton::North => "Y",
            PadButton::West => "X",
            PadButton::LeftBumper => "LB",
            PadButton::RightBumper => "RB",
            PadButton::LeftTrigger => "LT",
            PadButton::RightTrigger => "RT",
            PadButton::Select => "Select",
            PadButton::Start => "Start",
            PadButton::Guide => "Guide",
            PadButton::LeftStick => "L3",
            PadButton::RightStick => "R3",
            PadButton::DPadUp => "Up",
            PadButton::DPadDown => "Down",
            PadButton::DPadLeft => "Left",
            PadButton::DPadRight => "Right",
        }
    }
}

#[cfg(not(feature = "debug-input-hud"))]
mod disabled {
    use super::*;

    /// No-op stand-in compiled when the `debug-input-hud` feature is off. It is
    /// zero-sized and every method optimizes away, keeping the call sites in
    /// `app.rs` identical across both builds.
    #[derive(Default)]
    pub struct ButtonHud;

    impl ButtonHud {
        pub fn new() -> Self {
            Self
        }

        pub fn update(&mut self, _pressed: &HashSet<PadButton>) {}

        pub fn set_devices(&mut self, _names: &[String]) {}

        pub fn draw(&self, _ctx: &egui::Context) {}
    }
}
