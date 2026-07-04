//! The transport worker: a dedicated thread running a tokio runtime that owns the
//! active [`Transport`] and performs all (async, potentially blocking) input
//! forwarding off the winit main thread.
//!
//! The UI thread never blocks on transport I/O — it hands [`WorkerCmd`]s over a
//! bounded channel with a non-blocking `try_send`, dropping input under
//! backpressure rather than stalling the render loop.

use couchcast_config::TransportKind;
use couchcast_transport::{
    RemoteAction, TargetAddr, Transport,
    backends::{AdbTransport, LogTransport},
};
use tokio::sync::mpsc;

/// A command sent from the winit main thread to the transport worker.
pub enum WorkerCmd {
    /// Switch to `kind` and connect it to `addr`.
    Connect {
        kind: TransportKind,
        addr: TargetAddr,
    },
    /// Forward a single action to the connected target.
    Send(RemoteAction),
    /// Disconnect the current transport.
    Disconnect,
}

/// Handle to the transport worker thread.
pub struct TransportWorker {
    tx: mpsc::Sender<WorkerCmd>,
}

impl TransportWorker {
    /// Spawn the worker thread and its tokio runtime.
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel(256);
        std::thread::Builder::new()
            .name("couchcast-transport".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build transport runtime");
                rt.block_on(run(rx));
            })
            .expect("failed to spawn transport thread");
        Self { tx }
    }

    /// Connect (or reconnect) to a target using `kind`.
    pub fn connect(&self, kind: TransportKind, addr: TargetAddr) {
        self.dispatch(WorkerCmd::Connect { kind, addr });
    }

    /// Forward an action. Non-blocking; dropped if the queue is full.
    pub fn send(&self, action: RemoteAction) {
        self.dispatch(WorkerCmd::Send(action));
    }

    /// Disconnect the current transport.
    pub fn disconnect(&self) {
        self.dispatch(WorkerCmd::Disconnect);
    }

    fn dispatch(&self, cmd: WorkerCmd) {
        if let Err(e) = self.tx.try_send(cmd) {
            tracing::warn!("transport worker queue full or closed: {e}");
        }
    }
}

async fn run(mut rx: mpsc::Receiver<WorkerCmd>) {
    let mut transport: Box<dyn Transport> = Box::new(LogTransport::new());
    while let Some(cmd) = rx.recv().await {
        match cmd {
            WorkerCmd::Connect { kind, addr } => {
                let _ = transport.disconnect().await;
                transport = make_transport(kind);
                match transport.connect(&addr).await {
                    Ok(()) => tracing::info!(backend = transport.name(), ?addr, "connected"),
                    Err(e) => tracing::error!("connect failed: {e}"),
                }
            }
            WorkerCmd::Send(action) => {
                if transport.is_connected() {
                    if let Err(e) = transport.send(action).await {
                        tracing::warn!("forward failed: {e}");
                    }
                } else {
                    tracing::trace!("dropping action; transport not connected");
                }
            }
            WorkerCmd::Disconnect => {
                let _ = transport.disconnect().await;
            }
        }
    }
    tracing::debug!("transport worker shutting down");
}

/// Build a transport for `kind`. Backends whose cargo feature is not compiled
/// into this binary fall back to the logging transport.
fn make_transport(kind: TransportKind) -> Box<dyn Transport> {
    match kind {
        TransportKind::Adb => Box::new(AdbTransport::new()),
        TransportKind::Log => Box::new(LogTransport::new()),
        other => {
            tracing::warn!(
                "{other:?} backend is not built into this binary; using the log transport"
            );
            Box::new(LogTransport::new())
        }
    }
}
