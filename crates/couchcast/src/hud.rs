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

#[cfg(feature = "debug-input-hud")]
use std::cell::RefCell;

use gtk4 as gtk;

use couchcast_transport::PadButton;

#[cfg(feature = "debug-input-hud")]
pub use enabled::ButtonHud;

#[cfg(not(feature = "debug-input-hud"))]
pub use disabled::ButtonHud;

#[cfg(feature = "debug-input-hud")]
mod enabled {
    use super::*;
    use gtk::prelude::*;

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

    /// Styling for the HUD label: a translucent dark pill, monospace, top-left,
    /// so it stays legible over live video.
    const CSS: &str = "\
.couchcast-button-hud {
  font-family: monospace;
  font-size: 13px;
  color: #f5f5f5;
  background-color: rgba(0, 0, 0, 0.72);
  padding: 6px 10px;
  border-radius: 8px;
}
";

    /// A small label pinned to the top-left of the overlay showing the connected
    /// controllers and the buttons currently held down.
    ///
    /// The connected-pad line is the key diagnostic: an empty "Pads" line means
    /// `gilrs` sees no controller at all (Steam isn't presenting a virtual
    /// gamepad — e.g. the shortcut is on a keyboard/mouse layout), whereas a pad
    /// listed with no buttons reacting means Steam isn't routing input to this
    /// window (a focus-tracking problem).
    pub struct ButtonHud {
        label: gtk::Label,
        devices: RefCell<Vec<String>>,
        held: RefCell<Vec<&'static str>>,
    }

    impl ButtonHud {
        pub fn new() -> Self {
            install_css();
            let label = gtk::Label::builder()
                .halign(gtk::Align::Start)
                .valign(gtk::Align::Start)
                .margin_top(12)
                .margin_start(12)
                .build();
            label.add_css_class("couchcast-button-hud");
            let hud = Self {
                label,
                devices: RefCell::new(Vec::new()),
                held: RefCell::new(Vec::new()),
            };
            hud.render();
            hud
        }

        /// Add the HUD label to the window's overlay so it floats over the video.
        pub fn attach(&self, overlay: &gtk::Overlay) {
            overlay.add_overlay(&self.label);
        }

        /// Refresh the held-button line from the current set of held buttons.
        pub fn update(&self, pressed: &HashSet<PadButton>) {
            *self.held.borrow_mut() = DISPLAY_ORDER
                .iter()
                .filter(|b| pressed.contains(b))
                .map(|b| button_label(*b))
                .collect();
            self.render();
        }

        /// Refresh the connected-controllers line (call on connect/disconnect).
        pub fn set_devices(&self, names: &[String]) {
            *self.devices.borrow_mut() = names.to_vec();
            self.render();
        }

        /// Repaint the label from the current device + held-button state.
        fn render(&self) {
            let devices = self.devices.borrow();
            let pads = if devices.is_empty() {
                "(none — Steam is not presenting a gamepad)".to_owned()
            } else {
                devices.join(", ")
            };
            let held = self.held.borrow();
            let buttons = if held.is_empty() {
                "—".to_owned()
            } else {
                held.join("  ")
            };
            self.label
                .set_text(&format!("Pads: {pads}\nButtons: {buttons}"));
        }
    }

    impl Default for ButtonHud {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Register the HUD stylesheet once against the default display.
    fn install_css() {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
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

        pub fn attach(&self, _overlay: &gtk::Overlay) {}

        pub fn update(&self, _pressed: &HashSet<PadButton>) {}

        pub fn set_devices(&self, _names: &[String]) {}
    }
}
