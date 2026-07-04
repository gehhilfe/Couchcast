//! Input-forwarding transport layer for Couchcast.
//!
//! The device Couchcast controls (a Fire TV, Android TV box, Roku, …) is only
//! attached to the host through an HDMI capture card, so there is no return
//! channel "through" the video. Every way to send input back is therefore an
//! out-of-band network or radio link. This crate hides that behind a single
//! [`Transport`] trait and a device-agnostic [`RemoteAction`] vocabulary, so the
//! rest of the app never has to care whether a button press travels over ADB,
//! Bluetooth-HID, HDMI-CEC, or an HTTP remote API.
//!
//! ## Design
//!
//! * [`RemoteAction`] is what the UI/input layer produces — semantic intents
//!   ("navigate up", "select", "back") plus optional raw gamepad passthrough.
//! * [`DeviceCapabilities`] lets each backend advertise what it can actually do;
//!   [`RemoteAction`]s a target cannot express are dropped gracefully rather than
//!   erroring.
//! * Backends are [`cargo` features](https://doc.rust-lang.org/cargo/reference/features.html).
//!   Only [`backends::AdbTransport`] (Fire TV / Android TV) is fully built for
//!   the MVP; `bluetooth`, `cec`, and `roku` are feature-gated placeholders that
//!   demonstrate the extension point.
//!
//! See `docs/ARCHITECTURE.md` for the transport latency design (in particular why
//! the ADB backend holds a single persistent `adb shell` instead of forking
//! `adb` per keypress).

pub mod backends;
mod capabilities;
mod event;
mod transport;

pub use capabilities::DeviceCapabilities;
pub use event::{Direction, PadAxis, PadButton, RemoteAction};
pub use transport::{Result, TargetAddr, Transport, TransportError};
