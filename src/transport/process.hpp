#pragma once
//! Minimal POSIX process helpers for the ADB transport, replacing Rust's
//! `tokio::process`. One helper runs a command to completion capturing stdout
//! (`adb connect`); the other holds a long-lived child open with a writable
//! stdin pipe (the persistent `adb shell`). A third holds a TCP line-oriented
//! request/response connection open (the `monkey --port` server, reached via
//! `adb forward`).

#include <cstdint>
#include <optional>
#include <string>
#include <vector>

namespace couchcast::transport {

struct CommandOutput {
    int exit_code = -1;
    std::string stdout_text;
};

/// Run `argv` to completion, capturing stdout. Returns nullopt if the process
/// could not be spawned.
std::optional<CommandOutput> run_capture(const std::vector<std::string>& argv);

/// A spawned child process with a pipe to its stdin. stdout/stderr are dropped.
/// Killed and reaped on destruction.
class ChildProcess {
   public:
    ChildProcess() = default;
    ~ChildProcess();
    ChildProcess(const ChildProcess&) = delete;
    ChildProcess& operator=(const ChildProcess&) = delete;
    ChildProcess(ChildProcess&& other) noexcept;
    ChildProcess& operator=(ChildProcess&& other) noexcept;

    /// Spawn `argv` with a stdin pipe. Returns false on failure.
    bool spawn(const std::vector<std::string>& argv);

    /// Write `line` followed by a newline to the child's stdin. Returns false if
    /// the pipe has broken (the caller then marks itself disconnected).
    bool write_line(const std::string& line);

    /// True if the child was spawned, still has an open stdin pipe, and the
    /// process has not exited. The exit check (`waitpid(WNOHANG)`) matters for
    /// `adb shell`, which exits immediately when the device is `unauthorized` or
    /// `offline` — without it a dead shell would masquerade as connected.
    bool alive() const;

    /// Kill and reap the child, closing the pipe.
    void kill();

   private:
    // Mutable so the const `alive()` can reap a child that has already exited
    // (transitioning pid_ back to -1) without lying about its liveness.
    mutable int pid_ = -1;
    mutable int stdin_fd_ = -1;
};

/// A blocking, line-oriented TCP client to `127.0.0.1:port` — the host end of an
/// `adb forward` tunnel to the device's `monkey --port` server. `command()`
/// writes one request line and reads back monkey's one-line reply (`OK`/`ERROR`),
/// giving a per-keypress health signal. A short receive timeout keeps a wedged
/// server from stalling the transport worker thread.
class TcpLineClient {
   public:
    TcpLineClient() = default;
    ~TcpLineClient();
    TcpLineClient(const TcpLineClient&) = delete;
    TcpLineClient& operator=(const TcpLineClient&) = delete;
    TcpLineClient(TcpLineClient&& other) noexcept;
    TcpLineClient& operator=(TcpLineClient&& other) noexcept;

    /// Connect to `127.0.0.1:port`, retrying up to `attempts` times (the monkey
    /// server takes a second or two to bind after launch). Returns false if no
    /// attempt succeeded within the given `recv_timeout_ms` per-call read budget.
    bool connect(uint16_t port, int attempts, int recv_timeout_ms = 500);

    /// Send `line` (a newline is appended) and read one reply line into `reply`
    /// (trailing CR/LF stripped). Returns false if the socket is closed or the
    /// write/read fails — the caller then disables the fast path.
    bool command(const std::string& line, std::string& reply);

    bool alive() const { return fd_ >= 0; }
    void close();

   private:
    int fd_ = -1;
};

}  // namespace couchcast::transport
