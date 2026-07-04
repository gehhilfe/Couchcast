#include "worker.hpp"

#include <asio.hpp>

#include <algorithm>
#include <chrono>
#include <cstdint>
#include <mutex>
#include <utility>

#include "log.hpp"
#include "transport/backends/adb.hpp"
#include "transport/backends/log_transport.hpp"

namespace couchcast {

using transport::RemoteAction;
using transport::TargetAddr;
using transport::Transport;

namespace {
// Build a transport for `kind`. Backends not compiled in fall back to the log
// transport (matching the Rust behavior).
std::unique_ptr<Transport> make_transport(config::TransportKind kind) {
    switch (kind) {
        case config::TransportKind::Adb:
            return std::make_unique<transport::backends::AdbTransport>();
        case config::TransportKind::Log:
            return std::make_unique<transport::backends::LogTransport>();
        default:
            CC_WARN("%s backend is not built into this binary; using the log transport",
                    config::to_string(kind));
            return std::make_unique<transport::backends::LogTransport>();
    }
}

// Drop Send commands once this many are already queued (backpressure), mirroring
// the Rust bounded-channel try_send behavior.
constexpr int SEND_QUEUE_LIMIT = 256;

// Reconnection backoff bounds. The target device is frequently unreachable at
// launch under Steam Gaming Mode (a streaming stick that is still waking, or a
// network that has not settled), so a one-shot connect would leave the transport
// dead until the user manually reconnects. Instead we retry with exponential
// backoff between these bounds, resetting to the minimum on success.
constexpr auto RECONNECT_MIN = std::chrono::milliseconds(1000);
constexpr auto RECONNECT_MAX = std::chrono::milliseconds(15000);
}  // namespace

struct TransportWorker::Impl {
    asio::io_context io;
    asio::executor_work_guard<asio::io_context::executor_type> work;
    asio::steady_timer retry_timer;
    std::thread thread;
    std::unique_ptr<Transport> current;
    std::atomic<int> pending_sends{0};

    // Reconnection state. These are touched only on the io_context thread (from
    // posted handlers and timer callbacks), so they need no separate lock.
    config::TransportKind desired_kind = config::TransportKind::Log;
    TargetAddr desired_addr;
    std::string backend_name;
    bool want_connected = false;
    std::chrono::milliseconds backoff = RECONNECT_MIN;
    std::uint64_t generation = 0;  // bumped to cancel a stale retry chain

    mutable std::mutex status_mutex;
    TransportStatus status;  // guarded by status_mutex

    void set_status(TransportStatus s) {
        std::lock_guard<std::mutex> lock(status_mutex);
        status = std::move(s);
    }

    // Try to connect once (io thread). On success resets the backoff; on failure
    // schedules a backed-off retry. `gen` guards against a superseding connect()/
    // disconnect() that has invalidated this retry chain.
    void attempt(std::uint64_t gen) {
        if (gen != generation || !want_connected) return;
        std::string target = desired_addr.value;
        set_status({ConnPhase::Connecting, backend_name, target, {}});

        if (current) current->disconnect();
        current = make_transport(desired_kind);
        if (current->connect(desired_addr)) {
            CC_INFO("connected: backend=%s addr=%s", current->name(),
                    desired_addr.debug().c_str());
            set_status({ConnPhase::Connected, backend_name, target, {}});
            backoff = RECONNECT_MIN;
            return;
        }

        auto delay = backoff;
        CC_WARN("connect failed: backend=%s addr=%s; retrying in %lldms",
                backend_name.c_str(), desired_addr.debug().c_str(),
                static_cast<long long>(delay.count()));
        set_status({ConnPhase::Failed, backend_name, target,
                    "unreachable or unauthorized; retrying"});
        backoff = std::min(backoff * 2, RECONNECT_MAX);
        retry_timer.expires_after(delay);
        retry_timer.async_wait([this, gen](const asio::error_code& ec) {
            if (ec) return;  // cancelled by a newer generation
            attempt(gen);
        });
    }

    // Begin a fresh connect cycle to the currently desired target (io thread).
    void start_cycle() {
        ++generation;
        backoff = RECONNECT_MIN;
        retry_timer.cancel();
        attempt(generation);
    }

    // Note that a live connection dropped and kick off reconnection exactly once.
    // Only transitions out of the Connected phase, which debounces the burst of
    // dropped sends that follows a drop.
    void note_connection_lost(const char* reason) {
        bool was_connected = false;
        {
            std::lock_guard<std::mutex> lock(status_mutex);
            if (status.phase == ConnPhase::Connected) {
                status.phase = ConnPhase::Failed;
                status.detail = reason;
                was_connected = true;
            }
        }
        if (was_connected && want_connected) start_cycle();
    }

    Impl() : work(asio::make_work_guard(io)), retry_timer(io) {
        current = std::make_unique<transport::backends::LogTransport>();
        thread = std::thread([this] { io.run(); });
    }

    ~Impl() {
        work.reset();
        io.stop();
        if (thread.joinable()) thread.join();
    }
};

TransportWorker::TransportWorker() : impl_(std::make_unique<Impl>()) {}
TransportWorker::~TransportWorker() = default;

void TransportWorker::connect(config::TransportKind kind, TargetAddr addr) {
    Impl* impl = impl_.get();
    std::string backend = config::to_string(kind);
    // Reflect "Connecting" immediately on the calling (UI) thread so the pill
    // updates this frame, before the worker thread picks up the job.
    impl->set_status({ConnPhase::Connecting, backend, addr.value, {}});
    asio::post(impl->io, [impl, kind, backend = std::move(backend),
                          addr = std::move(addr)]() mutable {
        impl->desired_kind = kind;
        impl->desired_addr = std::move(addr);
        impl->backend_name = std::move(backend);
        impl->want_connected = true;
        impl->start_cycle();
    });
}

void TransportWorker::send(RemoteAction action) {
    Impl* impl = impl_.get();
    if (impl->pending_sends.load() >= SEND_QUEUE_LIMIT) {
        CC_WARN("transport worker queue saturated; dropping action");
        return;
    }
    impl->pending_sends.fetch_add(1);
    asio::post(impl->io, [impl, action = std::move(action)] {
        impl->pending_sends.fetch_sub(1);
        if (impl->current && impl->current->is_connected()) {
            if (!impl->current->send(action)) {
                CC_WARN("forward failed; connection lost");
                impl->note_connection_lost("connection lost; reconnecting");
            }
        } else {
            CC_TRACE("dropping action; transport not connected");
            // A silently-dead link (e.g. the adb shell exited) surfaces here
            // rather than as a send() failure; recover from it too.
            impl->note_connection_lost("connection lost; reconnecting");
        }
    });
}

void TransportWorker::disconnect() {
    Impl* impl = impl_.get();
    impl->set_status({ConnPhase::Disconnected, {}, {}, {}});
    asio::post(impl->io, [impl] {
        impl->want_connected = false;
        ++impl->generation;  // cancel any pending retry chain
        impl->retry_timer.cancel();
        if (impl->current) impl->current->disconnect();
    });
}

TransportStatus TransportWorker::status() const {
    Impl* impl = impl_.get();
    std::lock_guard<std::mutex> lock(impl->status_mutex);
    return impl->status;
}

}  // namespace couchcast
