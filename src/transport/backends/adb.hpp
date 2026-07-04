#pragma once
//! ADB-over-TCP transport for Fire TV / Android TV — the MVP backend.
//!
//! Ported from `couchcast-transport::backends::adb`. The load-bearing design is
//! preserved: one long-lived `adb shell` opened at connect time, with command
//! lines streamed into its stdin, avoiding per-keypress `adb` connection setup.
//!
//! On top of that, nav/media keys take a low-latency evdev fast path: at connect
//! we discover which `/dev/input/eventN` node advertises each Linux key code
//! (`getevent -pl`), hold those nodes open in the shell (`exec N>node`), and per
//! press stream a raw `struct input_event` sequence straight to the kernel — no
//! `input` binary, so no ~half-second JVM cold-start. Anything not routable
//! (gamepad, text, or a key no node advertises) falls back to `input keyevent`.
//! See `docs/ARCHITECTURE.md`.

#include <chrono>
#include <cstdint>
#include <optional>
#include <string>
#include <unordered_map>

#include "transport/process.hpp"
#include "transport/transport.hpp"

namespace couchcast::transport::backends {

class AdbTransport final : public Transport {
   public:
    AdbTransport() = default;
    explicit AdbTransport(std::string adb_bin) : adb_bin_(std::move(adb_bin)) {}

    const char* name() const override { return "adb"; }
    DeviceCapabilities capabilities() const override {
        return DeviceCapabilities::android_tv();
    }
    bool is_connected() const override { return shell_.alive(); }
    bool connect(const TargetAddr& target) override;
    bool send(const RemoteAction& action) override;
    void disconnect() override;

    /// Point `adb` at the user's already-authorized auth key by exporting
    /// `ANDROID_VENDOR_KEYS`, so device authentication survives an environment
    /// that hides or relocates `~/.android/adbkey` — chiefly the Flatpak sandbox
    /// (redirected `$HOME`) and the Steam Runtime container launched from
    /// gamescope. The real home is resolved from the passwd database (robust to a
    /// clobbered `$HOME`); no-op if `ANDROID_VENDOR_KEYS` is already set or no key
    /// exists. Call once at startup, before any thread or `adb` process starts
    /// (`setenv` must not race a concurrent `getenv`, and the adb *server*
    /// captures the key set only at its launch).
    static void ensure_auth_key_env();

    // Exposed for unit tests.
    static std::string serial_for(const TargetAddr& target);
    static std::optional<std::string> action_to_line(const RemoteAction& action);
    /// The `monkey --port` command for `action` (`press <keycode>`), or nullopt
    /// if the action has no keycode (text/analog/gamepad-release). Reuses the same
    /// Android keycodes as the `input` path.
    static std::optional<std::string> monkey_command(const RemoteAction& action);
    /// Build the persistent-shell line that injects a full key tap (down, SYN,
    /// up, SYN) for Linux key code `linux_code` into the evdev fd opened as
    /// `fd_slot`. `event_size` is 24 on 64-bit devices, 16 on 32-bit. Exposed to
    /// lock the on-the-wire byte layout in tests.
    static std::string evdev_line_for(int linux_code, int event_size, int fd_slot);

   private:
    /// One resolved evdev fast-path binding: the shell fd the node is held open
    /// as, and the Linux key code that node accepts for this action.
    struct EvdevRoute {
        int fd_slot;
        int linux_code;
    };

    /// Discover writable evdev nodes for our nav/media keys and hold them open in
    /// the persistent shell. Leaves `routes_` empty (→ `input` fallback) if the
    /// device exposes no usable node. Called at the end of a successful connect.
    void setup_evdev(const std::string& serial);

    /// The evdev fast-path line for `action`, or nullopt if it is not routable on
    /// this device (caller then falls back to the `input` path).
    std::optional<std::string> evdev_line(const RemoteAction& action) const;

    /// Launch a `monkey --port` server on the device, tunnel it with `adb
    /// forward`, and open a socket to it — a warm-JVM key-injection path that
    /// avoids the `input` binary's per-call cold-start. Best-effort: leaves the
    /// monkey socket closed (→ `input` fallback) on any failure.
    void setup_monkey(const std::string& serial);

    /// Tear down the monkey server, forward, and socket (best-effort).
    void teardown_monkey();

    /// If monkey was working but its socket has since broken (e.g. Android killed
    /// the process under memory pressure when a heavy app launched), try to
    /// re-establish it — but no more than once per cooldown so a permanently dead
    /// server doesn't stall every keypress. Returns true if the fast path is live.
    bool recover_monkey_if_needed();

    std::string adb_bin_ = "adb";
    std::optional<std::string> serial_;
    ChildProcess shell_;

    // evdev fast path, populated by setup_evdev().
    int event_size_ = 24;  // sizeof(struct input_event): 24 on 64-bit, 16 on 32-bit
    std::unordered_map<uint32_t, EvdevRoute> routes_;  // action_key -> binding

    // monkey warm-JVM fast path, populated by setup_monkey().
    TcpLineClient monkey_;
    bool monkey_supported_ = false;  // monkey came up at least once this connection
    std::chrono::steady_clock::time_point monkey_next_retry_{};  // recovery backoff gate
};

}  // namespace couchcast::transport::backends
