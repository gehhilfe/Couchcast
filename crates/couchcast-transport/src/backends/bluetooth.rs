//! Bluetooth-HID transport (deferred) — the host advertises itself as a
//! Bluetooth keyboard/gamepad so it works on Fire TV, Android TV **and** Apple TV
//! with no developer mode, pairing once like any BT remote.
//!
//! This is the best long-term "universal remote" path but is real engineering,
//! not a thin wrapper: BlueZ's classic BR/EDR HID *profile* support is
//! incomplete, so a full implementation registers an SDP record and opens the
//! HID L2CAP PSMs (0x11 control / 0x13 interrupt) itself, or registers a
//! `Profile1` over the system D-Bus. It also needs `--system-talk-name=org.bluez`
//! in the Flatpak sandbox and exclusive control of the BT adapter. Deferred until
//! the ADB backend proves the product — see `docs/ROADMAP.md`.
//!
//! Enable with the `bluetooth` cargo feature. Adding the real implementation
//! means pulling in `bluer` and implementing the HID report descriptors here.

use async_trait::async_trait;

use crate::capabilities::DeviceCapabilities;
use crate::event::RemoteAction;
use crate::transport::{Result, TargetAddr, Transport, TransportError};

/// Placeholder for the Bluetooth-HID backend.
#[derive(Debug, Default)]
pub struct BluetoothHidTransport;

#[async_trait]
impl Transport for BluetoothHidTransport {
    fn name(&self) -> &'static str {
        "bluetooth-hid"
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities::ANDROID_TV
    }

    fn is_connected(&self) -> bool {
        false
    }

    async fn connect(&mut self, _target: &TargetAddr) -> Result<()> {
        Err(TransportError::NotImplemented("bluetooth-hid"))
    }

    async fn send(&mut self, _action: RemoteAction) -> Result<()> {
        Err(TransportError::NotImplemented("bluetooth-hid"))
    }

    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }
}
