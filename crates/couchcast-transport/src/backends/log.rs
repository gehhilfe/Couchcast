//! A no-op transport that just logs every action. Handy for developing the UI
//! and input pipeline without a real device attached.

use async_trait::async_trait;

use crate::capabilities::DeviceCapabilities;
use crate::event::RemoteAction;
use crate::transport::{Result, TargetAddr, Transport};

/// Logs every forwarded action at `info` level and otherwise does nothing.
/// Reports [`DeviceCapabilities::ANDROID_TV`] so nothing is filtered out.
#[derive(Debug, Default)]
pub struct LogTransport {
    connected: bool,
    target: Option<String>,
}

impl LogTransport {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Transport for LogTransport {
    fn name(&self) -> &'static str {
        "log"
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities::ANDROID_TV
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn connect(&mut self, target: &TargetAddr) -> Result<()> {
        self.target = Some(format!("{target:?}"));
        self.connected = true;
        tracing::info!(target = ?target, "log transport connected");
        Ok(())
    }

    async fn send(&mut self, action: RemoteAction) -> Result<()> {
        tracing::info!(target = ?self.target, action = %action.label(), "forward");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        tracing::info!("log transport disconnected");
        Ok(())
    }
}
