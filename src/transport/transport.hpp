#pragma once
//! The Transport interface and its supporting types.
//!
//! Ported from `couchcast-transport::transport`. The Rust trait was async
//! (`async_trait`); here the methods are ordinary blocking calls, because they
//! run on the dedicated ASIO worker thread (see `worker.hpp`) rather than on an
//! async runtime. The UI thread never calls them directly.

#include <memory>
#include <string>

#include "transport/capabilities.hpp"
#include "transport/event.hpp"

namespace couchcast::transport {

/// How to reach a target device.
struct TargetAddr {
    enum class Kind { Network, UsbSerial };
    Kind kind = Kind::Network;
    std::string value;

    static TargetAddr network(std::string host) {
        return TargetAddr{Kind::Network, std::move(host)};
    }
    static TargetAddr usb_serial(std::string serial) {
        return TargetAddr{Kind::UsbSerial, std::move(serial)};
    }

    std::string debug() const {
        return (kind == Kind::Network ? "Network(" : "UsbSerial(") + value + ")";
    }
};

/// A channel that forwards RemoteActions to a single target device. Blocking;
/// invoked only from the transport worker thread.
class Transport {
   public:
    virtual ~Transport() = default;

    /// A stable identifier for the backend (e.g. "adb").
    virtual const char* name() const = 0;

    /// What this target can express.
    virtual DeviceCapabilities capabilities() const = 0;

    /// Whether a live connection is currently established.
    virtual bool is_connected() const = 0;

    /// Establish (or re-establish) the connection to `target`. Returns false on
    /// failure (logged by the backend).
    virtual bool connect(const TargetAddr& target) = 0;

    /// Forward a single action. Unsupported actions are dropped (logged), not
    /// treated as errors.
    virtual bool send(const RemoteAction& action) = 0;

    /// Tear the connection down cleanly.
    virtual void disconnect() = 0;
};

}  // namespace couchcast::transport
