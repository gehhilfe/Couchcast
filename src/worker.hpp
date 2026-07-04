#pragma once
//! The transport worker: a dedicated thread running an ASIO io_context that owns
//! the active Transport and performs all input forwarding off the main thread.
//! Ported from `couchcast::worker` (which used a tokio runtime).
//!
//! The UI thread never blocks on transport I/O — it posts work to the io_context
//! (non-blocking) and drops sends under backpressure rather than stalling.

#include <atomic>
#include <memory>
#include <thread>

#include "config/config.hpp"
#include "transport/transport.hpp"

// Forward-declare so the ASIO header only appears in the .cpp.
namespace asio {
class io_context;
}

namespace couchcast {

/// Lifecycle of the active transport connection, as observed from the UI thread.
enum class ConnPhase { Disconnected, Connecting, Connected, Failed };

/// A thread-safe snapshot of the worker's connection state, polled by the UI so
/// it can reflect success/failure instead of showing "Connecting..." forever.
struct TransportStatus {
    ConnPhase phase = ConnPhase::Disconnected;
    std::string backend;  ///< Backend id, e.g. "adb" (empty when disconnected).
    std::string target;   ///< Human-readable address the worker is/was reaching.
    std::string detail;   ///< Failure reason when phase == Failed; else empty.
};

class TransportWorker {
   public:
    TransportWorker();
    ~TransportWorker();
    TransportWorker(const TransportWorker&) = delete;
    TransportWorker& operator=(const TransportWorker&) = delete;

    /// Connect (or reconnect) to a target using `kind`.
    void connect(config::TransportKind kind, transport::TargetAddr addr);

    /// Forward an action. Non-blocking; dropped if the queue is saturated.
    void send(transport::RemoteAction action);

    /// Disconnect the current transport.
    void disconnect();

    /// A thread-safe snapshot of the current connection state. Safe to call from
    /// the UI thread every frame.
    TransportStatus status() const;

   private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

}  // namespace couchcast
