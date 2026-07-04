//! HDMI-CEC transport (deferred) — the only channel that literally rides the
//! HDMI link, so it is appealing "for free". In practice most HDMI *capture*
//! cards do not expose the CEC line, so this needs a Pulse-Eight style USB-CEC
//! adapter or a capture device that bridges CEC. The button set is limited to
//! the CEC user-control codes (navigation, media, power) — no text, no analog,
//! no gamepad. Good as universal fallback coverage, not a primary channel.
//!
//! Enable with the `cec` cargo feature; the real implementation pulls in
//! `cec-rs`/`libcec-sys` (which require system `libcec`). See `docs/ROADMAP.md`.

use async_trait::async_trait;

use crate::capabilities::DeviceCapabilities;
use crate::event::RemoteAction;
use crate::transport::{Result, TargetAddr, Transport, TransportError};

/// Placeholder for the HDMI-CEC backend.
#[derive(Debug, Default)]
pub struct CecTransport;

#[async_trait]
impl Transport for CecTransport {
    fn name(&self) -> &'static str {
        "cec"
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities::BASIC_REMOTE
    }

    fn is_connected(&self) -> bool {
        false
    }

    async fn connect(&mut self, _target: &TargetAddr) -> Result<()> {
        Err(TransportError::NotImplemented("cec"))
    }

    async fn send(&mut self, _action: RemoteAction) -> Result<()> {
        Err(TransportError::NotImplemented("cec"))
    }

    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }
}
