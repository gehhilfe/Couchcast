//! Roku ECP transport (deferred) — Roku's External Control Protocol is a plain
//! HTTP API on port 8060 (`POST /keypress/<Key>`, `/keydown`, `/keyup`) with
//! SSDP discovery and no pairing, so it is nearly free to add once the
//! [`Transport`](crate::Transport) trait exists. Roku-only, remote-grade button
//! set, ~20-60 ms round trips (fine for menus, not action games).
//!
//! Enable with the `roku` cargo feature; the real implementation pulls in an
//! HTTP client (`reqwest`) and an SSDP discovery helper. See `docs/ROADMAP.md`.

use async_trait::async_trait;

use crate::capabilities::DeviceCapabilities;
use crate::event::RemoteAction;
use crate::transport::{Result, TargetAddr, Transport, TransportError};

/// Placeholder for the Roku ECP backend.
#[derive(Debug, Default)]
pub struct RokuTransport;

#[async_trait]
impl Transport for RokuTransport {
    fn name(&self) -> &'static str {
        "roku"
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities::BASIC_REMOTE
    }

    fn is_connected(&self) -> bool {
        false
    }

    async fn connect(&mut self, _target: &TargetAddr) -> Result<()> {
        Err(TransportError::NotImplemented("roku"))
    }

    async fn send(&mut self, _action: RemoteAction) -> Result<()> {
        Err(TransportError::NotImplemented("roku"))
    }

    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }
}
