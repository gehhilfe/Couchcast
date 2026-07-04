//! Concrete [`Transport`](crate::Transport) implementations.
//!
//! Backends are feature-gated. The default build compiles only [`LogTransport`]
//! (always available, used for development without hardware) and
//! [`AdbTransport`] (the Fire TV / Android TV MVP backend). The remaining
//! backends are placeholders behind their own features so the extension point is
//! visible and documented.

mod log;
pub use log::LogTransport;

#[cfg(feature = "adb")]
mod adb;
#[cfg(feature = "adb")]
pub use adb::AdbTransport;

#[cfg(feature = "bluetooth")]
mod bluetooth;
#[cfg(feature = "bluetooth")]
pub use bluetooth::BluetoothHidTransport;

#[cfg(feature = "cec")]
mod cec;
#[cfg(feature = "cec")]
pub use cec::CecTransport;

#[cfg(feature = "roku")]
mod roku;
#[cfg(feature = "roku")]
pub use roku::RokuTransport;
