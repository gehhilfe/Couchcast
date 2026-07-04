//! The [`Transport`] trait and its supporting types.

use async_trait::async_trait;

use crate::capabilities::DeviceCapabilities;
use crate::event::RemoteAction;

/// How to reach a target device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetAddr {
    /// `host` or `host:port` for ADB-over-TCP (Fire TV / Android TV), Roku ECP,
    /// etc. Backends apply their own default port when one is omitted.
    Network(String),
    /// A USB device serial for ADB-over-USB.
    UsbSerial(String),
}

impl TargetAddr {
    /// Convenience constructor from a hostname/IP string.
    pub fn network(host: impl Into<String>) -> Self {
        TargetAddr::Network(host.into())
    }
}

/// Errors a transport can raise.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("transport is not connected")]
    NotConnected,
    #[error("action {0:?} is not supported by this target")]
    Unsupported(RemoteAction),
    #[error("backend `{0}` is not implemented yet")]
    NotImplemented(&'static str),
    #[error("failed to connect to {target}: {source}")]
    Connect {
        target: String,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

/// Transport result alias.
pub type Result<T> = std::result::Result<T, TransportError>;

/// A channel that forwards [`RemoteAction`]s to a single target device.
///
/// Implementations run inside the app's async transport worker (a dedicated
/// tokio runtime on its own thread); see `docs/ARCHITECTURE.md`. The trait is
/// object-safe via [`async_trait`], so the active backend is held as a
/// `Box<dyn Transport>` and swapped at runtime when the user picks a different
/// device.
#[async_trait]
pub trait Transport: Send {
    /// A stable identifier for the backend (e.g. `"adb"`).
    fn name(&self) -> &'static str;

    /// What this target can express. Callers should check
    /// [`DeviceCapabilities::supports`] before sending, or rely on the backend
    /// to drop unsupported actions.
    fn capabilities(&self) -> DeviceCapabilities;

    /// Whether a live connection is currently established.
    fn is_connected(&self) -> bool;

    /// Establish (or re-establish) the connection to `target`.
    async fn connect(&mut self, target: &TargetAddr) -> Result<()>;

    /// Forward a single action. Unsupported actions should be dropped (logged),
    /// not raised as errors, so a controller can be used freely regardless of
    /// the target's capabilities.
    async fn send(&mut self, action: RemoteAction) -> Result<()>;

    /// Tear the connection down cleanly.
    async fn disconnect(&mut self) -> Result<()>;
}
