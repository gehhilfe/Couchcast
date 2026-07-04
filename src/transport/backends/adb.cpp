#include "transport/backends/adb.hpp"

#include <pwd.h>
#include <time.h>
#include <unistd.h>

#include <algorithm>
#include <cctype>
#include <chrono>
#include <cstdint>
#include <set>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

#include "log.hpp"

namespace couchcast::transport::backends {

namespace {
constexpr int DEFAULT_ADB_PORT = 5555;

// Linux evdev event types/codes we need (from <linux/input-event-codes.h>).
constexpr int EV_SYN = 0;
constexpr int EV_KEY = 1;
constexpr int SYN_REPORT = 0;
constexpr int FIRST_FD_SLOT = 3;  // 0,1,2 are stdin/stdout/stderr

// The monkey key-injection server: a device-side port and the host port we tunnel
// it to with `adb forward`. Fixed (not per-connection) — one target at a time.
constexpr uint16_t MONKEY_DEVICE_PORT = 26760;
constexpr uint16_t MONKEY_HOST_PORT = 26760;
// monkey's network server serves ONE client at a time and wedges permanently if
// probed before it is ready (a dead socket poisons its single client slot). So we
// wait for a freshly launched server to bind, then probe exactly once — never in a
// reconnect loop — and recover by relaunching, not reconnecting.
constexpr int MONKEY_STARTUP_WAIT_MS = 4000;  // let a fresh server bind before we touch it
constexpr int MONKEY_LAUNCH_CYCLES = 2;       // full kill+relaunch attempts
constexpr int MONKEY_RECOVERY_COOLDOWN_S = 5;  // min gap between mid-session recovery tries

void sleep_ms(int ms) {
    struct timespec ts{ms / 1000, (ms % 1000) * 1000000L};
    nanosleep(&ts, nullptr);
}

std::string trim(const std::string& s);  // defined below; used by parse_key_nodes

/// A stable key identifying a routable RemoteAction. Only Navigate varies by
/// direction; every other routable kind is fully determined by its Kind.
uint32_t action_key(const RemoteAction& a) {
    uint32_t k = static_cast<uint32_t>(a.kind) << 8;
    if (a.kind == RemoteAction::Kind::Navigate) {
        k |= static_cast<uint32_t>(a.direction);
    }
    return k;
}

/// Candidate Linux key codes for an action, most-preferred first. A device is
/// matched to the first candidate some evdev node advertises. Empty for actions
/// that stay on the `input` path (text, gamepad, analog).
std::vector<int> evdev_candidates(const RemoteAction& a) {
    using K = RemoteAction::Kind;
    switch (a.kind) {
        case K::Navigate:
            switch (a.direction) {
                case Direction::Up: return {103};     // KEY_UP
                case Direction::Down: return {108};   // KEY_DOWN
                case Direction::Left: return {105};   // KEY_LEFT
                case Direction::Right: return {106};  // KEY_RIGHT
            }
            return {};
        case K::Select: return {28, 353};       // KEY_ENTER, KEY_SELECT
        case K::Back: return {158};             // KEY_BACK
        case K::Home: return {172, 102};        // KEY_HOMEPAGE, KEY_HOME
        case K::Menu: return {139};             // KEY_MENU
        case K::PlayPause: return {164};        // KEY_PLAYPAUSE
        case K::Play: return {207, 200};        // KEY_PLAY, KEY_PLAYCD
        case K::Pause: return {119, 201};       // KEY_PAUSE, KEY_PAUSECD
        case K::Stop: return {166, 128};        // KEY_STOPCD, KEY_STOP
        case K::Rewind: return {168};           // KEY_REWIND
        case K::FastForward: return {208};      // KEY_FASTFORWARD
        case K::Next: return {163};             // KEY_NEXTSONG
        case K::Previous: return {165};         // KEY_PREVIOUSSONG
        case K::VolumeUp: return {115};         // KEY_VOLUMEUP
        case K::VolumeDown: return {114};       // KEY_VOLUMEDOWN
        case K::Mute: return {113};             // KEY_MUTE
        case K::Power: return {116};            // KEY_POWER
        default: return {};                     // Text, GamepadButton, Analog
    }
}

/// The nav/media actions we attempt to route through evdev, in a fixed order.
std::vector<RemoteAction> routable_actions() {
    using K = RemoteAction::Kind;
    std::vector<RemoteAction> v;
    for (auto d : {Direction::Up, Direction::Down, Direction::Left, Direction::Right}) {
        v.push_back(RemoteAction::navigate(d));
    }
    for (K k : {K::Select, K::Back, K::Home, K::Menu, K::PlayPause, K::Play,
                K::Pause, K::Stop, K::Rewind, K::FastForward, K::Next, K::Previous,
                K::VolumeUp, K::VolumeDown, K::Mute, K::Power}) {
        v.push_back(RemoteAction::simple(k));
    }
    return v;
}

/// Parse `getevent -pl` output into (node path, advertised EV_KEY codes). Codes
/// are printed as 4-digit hex under a `KEY (0001):` header, wrapping across
/// deeply-indented continuation lines; we accumulate hex tokens until the block
/// ends (a new labeled section or the next device).
std::vector<std::pair<std::string, std::set<int>>> parse_key_nodes(
    const std::string& text) {
    std::vector<std::pair<std::string, std::set<int>>> nodes;
    std::istringstream stream(text);
    std::string line;
    bool in_key = false;
    auto is_hex_token = [](const std::string& tok) {
        if (tok.empty() || tok.size() > 4) return false;
        for (char c : tok) {
            if (!std::isxdigit(static_cast<unsigned char>(c))) return false;
        }
        return true;
    };
    while (std::getline(stream, line)) {
        std::string t = trim(line);
        if (t.rfind("add device", 0) == 0) {
            in_key = false;
            auto colon = t.find(':');
            if (colon != std::string::npos) {
                std::string path = trim(t.substr(colon + 1));
                if (!path.empty()) nodes.emplace_back(path, std::set<int>{});
            }
            continue;
        }
        if (nodes.empty()) continue;
        bool key_header = t.rfind("KEY (", 0) == 0;
        if (key_header) {
            in_key = true;
            auto rp = t.find(')');
            t = (rp != std::string::npos) ? t.substr(rp + 1) : std::string{};
        } else if (!t.empty() && t.find('(') != std::string::npos &&
                   !is_hex_token(t.substr(0, t.find(' ')))) {
            // A different labeled section header (REL/ABS/SW/name:/…) ends KEY.
            in_key = false;
            continue;
        }
        if (!in_key) continue;
        std::istringstream toks(t);
        std::string tok;
        bool any = false;
        while (toks >> tok) {
            if (!is_hex_token(tok)) {
                in_key = false;
                break;
            }
            nodes.back().second.insert(std::stoi(tok, nullptr, 16));
            any = true;
        }
        (void)any;
    }
    return nodes;
}

/// Append one little-endian `struct input_event` to `buf`. The timeval is left
/// zeroed (the kernel accepts a zero timestamp); its width (`event_size - 8`)
/// is the only thing that differs between 64- and 32-bit devices.
void append_event(std::string& buf, int event_size, int type, int code, int value) {
    for (int i = 0; i < event_size - 8; ++i) buf.push_back('\0');
    buf.push_back(static_cast<char>(type & 0xff));
    buf.push_back(static_cast<char>((type >> 8) & 0xff));
    buf.push_back(static_cast<char>(code & 0xff));
    buf.push_back(static_cast<char>((code >> 8) & 0xff));
    uint32_t v = static_cast<uint32_t>(value);
    buf.push_back(static_cast<char>(v & 0xff));
    buf.push_back(static_cast<char>((v >> 8) & 0xff));
    buf.push_back(static_cast<char>((v >> 16) & 0xff));
    buf.push_back(static_cast<char>((v >> 24) & 0xff));
}

/// Octal-escape every byte as `\NNN` so the payload survives a single-quoted
/// `printf` format string with no literal `%` (which would start a conversion)
/// or NUL-termination surprises.
std::string octal_escape(const std::string& bytes) {
    std::string out;
    out.reserve(bytes.size() * 4);
    for (unsigned char b : bytes) {
        out.push_back('\\');
        out.push_back(static_cast<char>('0' + ((b >> 6) & 7)));
        out.push_back(static_cast<char>('0' + ((b >> 3) & 7)));
        out.push_back(static_cast<char>('0' + (b & 7)));
    }
    return out;
}

std::optional<std::string> keyevent(int code) {
    return "input keyevent " + std::to_string(code);
}

/// Map a normalized pad button to an Android KEYCODE_BUTTON_* value.
int gamepad_keycode(PadButton button) {
    switch (button) {
        case PadButton::South: return 96;
        case PadButton::East: return 97;
        case PadButton::West: return 99;
        case PadButton::North: return 100;
        case PadButton::LeftBumper: return 102;
        case PadButton::RightBumper: return 103;
        case PadButton::LeftTrigger: return 104;
        case PadButton::RightTrigger: return 105;
        case PadButton::Select: return 109;
        case PadButton::Start: return 108;
        case PadButton::Guide: return 110;
        case PadButton::LeftStick: return 106;
        case PadButton::RightStick: return 107;
        case PadButton::DPadUp: return 19;
        case PadButton::DPadDown: return 20;
        case PadButton::DPadLeft: return 21;
        case PadButton::DPadRight: return 22;
    }
    return 0;
}

/// The Android `KEYCODE_*` value an action maps to, or nullopt for actions with
/// no discrete keycode (text, analog, and gamepad-button *releases* — the app is
/// press-edge-triggered, so only presses ever arrive). Shared by the `input`
/// (`keyevent N`) and `monkey` (`press N`) paths so their coverage stays in sync.
std::optional<int> android_keycode(const RemoteAction& action) {
    using K = RemoteAction::Kind;
    switch (action.kind) {
        case K::Navigate:
            switch (action.direction) {
                case Direction::Up: return 19;
                case Direction::Down: return 20;
                case Direction::Left: return 21;
                case Direction::Right: return 22;
            }
            return std::nullopt;
        case K::Select: return 23;
        case K::Back: return 4;
        case K::Home: return 3;
        case K::Menu: return 82;
        case K::PlayPause: return 85;
        case K::Play: return 126;
        case K::Pause: return 127;
        case K::Stop: return 86;
        case K::Rewind: return 89;
        case K::FastForward: return 90;
        case K::Next: return 87;
        case K::Previous: return 88;
        case K::VolumeUp: return 24;
        case K::VolumeDown: return 25;
        case K::Mute: return 164;
        case K::Power: return 26;
        case K::GamepadButton:
            if (action.pressed) return gamepad_keycode(action.button);
            return std::nullopt;
        case K::Text:
        case K::Analog:
            return std::nullopt;
    }
    return std::nullopt;
}

/// Escape for Android `input text`: spaces become `%s`; a conservative allowlist
/// keeps everything else literal so no shell metacharacters slip through.
std::string escape_text(const std::string& text) {
    std::string out;
    out.reserve(text.size());
    for (char ch : text) {
        if (ch == ' ') {
            out += "%s";
        } else if (std::isalnum(static_cast<unsigned char>(ch)) || ch == '.' ||
                   ch == '-' || ch == '_') {
            out.push_back(ch);
        }
        // else: drop.
    }
    return out;
}

std::string to_lower(std::string s) {
    std::transform(s.begin(), s.end(), s.begin(),
                   [](unsigned char c) { return std::tolower(c); });
    return s;
}

std::string trim(const std::string& s) {
    size_t b = s.find_first_not_of(" \t\r\n");
    size_t e = s.find_last_not_of(" \t\r\n");
    if (b == std::string::npos) return {};
    return s.substr(b, e - b + 1);
}
}  // namespace

void AdbTransport::ensure_auth_key_env() {
    // An explicit setting wins — never override the user's own vendor keys.
    if (const char* existing = std::getenv("ANDROID_VENDOR_KEYS");
        existing && existing[0]) {
        return;
    }

    // Resolve the real home from passwd, not `$HOME`: the Flatpak sandbox and the
    // Steam Runtime container both remap `$HOME`, but keep the passwd entry's home
    // (`/home/<user>`) pointing at the host, where the authorized key lives.
    std::string home;
    if (const struct passwd* pw = getpwuid(getuid()); pw && pw->pw_dir) {
        home = pw->pw_dir;
    } else if (const char* h = std::getenv("HOME"); h && h[0]) {
        home = h;
    }
    if (home.empty()) return;

    std::string key = home + "/.android/adbkey";
    if (access(key.c_str(), R_OK) != 0) {
        // No host key to offer (device never paired here, or the sandbox can't
        // read it — grant `--filesystem=~/.android:ro`). adb falls back to its own
        // generated key, which prompts a fresh authorization on the device.
        CC_INFO("adb: no readable auth key at %s; relying on adb's own key",
                key.c_str());
        return;
    }

    setenv("ANDROID_VENDOR_KEYS", key.c_str(), 1);
    CC_INFO("adb: offering auth key %s (ANDROID_VENDOR_KEYS)", key.c_str());
}

std::string AdbTransport::serial_for(const TargetAddr& target) {
    if (target.kind == TargetAddr::Kind::Network) {
        if (target.value.find(':') != std::string::npos) return target.value;
        return target.value + ":" + std::to_string(DEFAULT_ADB_PORT);
    }
    return target.value;
}

std::optional<std::string> AdbTransport::action_to_line(const RemoteAction& action) {
    // Text is the one non-keycode action the `input` binary can still express.
    if (action.kind == RemoteAction::Kind::Text) {
        return "input text " + escape_text(action.text);
    }
    // Everything else is a discrete keyevent; analog has no `input` equivalent.
    if (auto code = android_keycode(action)) return keyevent(*code);
    return std::nullopt;
}

std::optional<std::string> AdbTransport::monkey_command(const RemoteAction& action) {
    // `press` is a down+up tap, which matches the app's press-edge semantics for
    // every key we forward. Text/analog stay on the `input` path.
    if (auto code = android_keycode(action)) return "press " + std::to_string(*code);
    return std::nullopt;
}

bool AdbTransport::connect(const TargetAddr& target) {
    std::string serial = serial_for(target);

    if (target.kind == TargetAddr::Kind::Network) {
        auto out = run_capture({adb_bin_, "connect", serial});
        if (!out) {
            CC_ERROR("failed to spawn `%s connect %s`", adb_bin_.c_str(),
                     serial.c_str());
            return false;
        }
        std::string lower = to_lower(out->stdout_text);
        if (lower.find("cannot") != std::string::npos ||
            lower.find("unable") != std::string::npos ||
            lower.find("failed") != std::string::npos) {
            CC_ERROR("adb connect %s failed: %s", serial.c_str(),
                     trim(out->stdout_text).c_str());
            return false;
        }
        CC_INFO("adb connect %s: %s", serial.c_str(),
                trim(out->stdout_text).c_str());
    }

    // `adb connect` reports success even for a device that is merely reachable
    // but still `unauthorized`/`offline` (e.g. "already connected to ..." from a
    // cached, un-accepted handshake). Gate on the transport state so we don't
    // open a shell that silently drops every command.
    auto state = run_capture({adb_bin_, "-s", serial, "get-state"});
    std::string state_text = state ? trim(state->stdout_text) : "";
    if (state_text != "device") {
        CC_ERROR("adb device %s not ready (state: %s); accept the debugging "
                 "prompt on the device",
                 serial.c_str(),
                 state_text.empty() ? "unreachable" : state_text.c_str());
        return false;
    }

    if (!shell_.spawn({adb_bin_, "-s", serial, "shell"})) {
        CC_ERROR("failed to open persistent adb shell for %s", serial.c_str());
        return false;
    }
    serial_ = serial;
    CC_INFO("adb persistent shell opened: serial=%s", serial.c_str());

    // Best-effort fast paths, tried in latency order. Both fall back to the
    // `input` binary on failure, so neither is fatal:
    //   1. raw evdev  — lowest latency, but blocked by SELinux without root;
    //   2. monkey     — warm-JVM server, works unrooted on stock Fire TV.
    setup_evdev(serial);
    setup_monkey(serial);
    return true;
}

void AdbTransport::setup_monkey(const std::string& serial) {
    // (Re)establish the host->device tunnel. `adb forward` accepts the host-side
    // connection immediately and only then dials the device, so a bare connect
    // says nothing about server health — every probe below must go to the
    // command level (`sleep 0` -> `OK`, a harmless no-op).
    run_capture({adb_bin_, "-s", serial, "forward", "--remove",
                 "tcp:" + std::to_string(MONKEY_HOST_PORT)});
    run_capture({adb_bin_, "-s", serial, "forward",
                 "tcp:" + std::to_string(MONKEY_HOST_PORT),
                 "tcp:" + std::to_string(MONKEY_DEVICE_PORT)});

    // A single connect + `sleep 0` handshake (a harmless no-op that returns `OK`).
    // Probed at most once per server instance — see the note on MONKEY_* above.
    auto probe = [&]() {
        if (!monkey_.connect(MONKEY_HOST_PORT, 1)) return false;
        std::string reply;
        if (monkey_.command("sleep 0", reply) && reply == "OK") return true;
        monkey_.close();
        return false;
    };

    // 1. Reuse a healthy server left by a prior session (one probe, no storm).
    //    Also recovers when our own teardown was skipped by a crash.
    if (probe()) {
        CC_INFO("adb: monkey warm-JVM fast path active, reused (host port %u)",
                MONKEY_HOST_PORT);
        monkey_supported_ = true;
        return;
    }
    monkey_.close();

    // 2. Nothing healthy listening — relaunch. Each cycle: kill any wedged or
    //    duplicate instance and *wait* for the port to free (else a fresh monkey
    //    can't bind), launch detached (setsid+nohup, so it outlives this one-shot
    //    shell), wait for it to bind, then probe exactly once.
    for (int cycle = 0; cycle < MONKEY_LAUNCH_CYCLES; ++cycle) {
        run_capture({adb_bin_, "-s", serial, "shell",
                     "kill $(pgrep -f 'monkey --port') 2>/dev/null; "
                     "i=0; while pgrep -f 'monkey --port' >/dev/null 2>&1 && "
                     "[ $i -lt 15 ]; do sleep 0.2; i=$((i+1)); done"});
        run_capture({adb_bin_, "-s", serial, "shell",
                     "setsid nohup monkey --port " +
                         std::to_string(MONKEY_DEVICE_PORT) +
                         " >/dev/null 2>&1 </dev/null &"});
        sleep_ms(MONKEY_STARTUP_WAIT_MS);
        if (probe()) {
            CC_INFO("adb: monkey warm-JVM fast path active (host port %u)",
                    MONKEY_HOST_PORT);
            monkey_supported_ = true;
            return;
        }
        monkey_.close();
    }
    CC_INFO("adb: monkey server did not come up; using `input` fallback");
    teardown_monkey();
}

bool AdbTransport::recover_monkey_if_needed() {
    if (monkey_.alive()) return true;
    // Only attempt recovery if monkey worked at least once (else the device
    // simply doesn't support it), the connection is live, and the cooldown since
    // the last try has elapsed (so a dead server doesn't block every keypress).
    if (!monkey_supported_ || !serial_) return false;
    auto now = std::chrono::steady_clock::now();
    if (now < monkey_next_retry_) return false;
    monkey_next_retry_ = now + std::chrono::seconds(MONKEY_RECOVERY_COOLDOWN_S);

    CC_INFO("adb: monkey fast path lost; attempting recovery");
    setup_monkey(*serial_);  // reuse-probe -> relaunch, as at connect time
    return monkey_.alive();
}

void AdbTransport::teardown_monkey() {
    monkey_.close();
    if (serial_) {
        run_capture({adb_bin_, "-s", *serial_, "forward", "--remove",
                     "tcp:" + std::to_string(MONKEY_HOST_PORT)});
        run_capture({adb_bin_, "-s", *serial_, "shell",
                     "kill $(pgrep -f 'monkey --port') 2>/dev/null"});
    }
}

void AdbTransport::setup_evdev(const std::string& serial) {
    routes_.clear();

    auto ev = run_capture({adb_bin_, "-s", serial, "shell", "getevent", "-pl"});
    if (!ev || ev->stdout_text.empty()) {
        CC_INFO("adb: getevent unavailable; using `input` fallback for all keys");
        return;
    }
    auto nodes = parse_key_nodes(ev->stdout_text);
    if (nodes.empty()) {
        CC_INFO("adb: no evdev key nodes found; using `input` fallback");
        return;
    }

    // struct input_event size follows the device shell's bitness (the timeval
    // width). Primary ABI is the reliable signal; default to 64-bit.
    auto abi = run_capture({adb_bin_, "-s", serial, "shell", "getprop",
                            "ro.product.cpu.abi"});
    event_size_ = (abi && abi->stdout_text.find("64") != std::string::npos) ? 24 : 16;

    // Resolve each action to (node, code): the first candidate code advertised by
    // some node. Collect the set of nodes we actually need.
    struct Pending {
        uint32_t key;
        std::string node;
        int code;
    };
    std::vector<Pending> pending;
    std::set<std::string> needed_nodes;
    for (const auto& action : routable_actions()) {
        bool resolved = false;
        for (int code : evdev_candidates(action)) {
            for (const auto& [path, codes] : nodes) {
                if (codes.count(code)) {
                    pending.push_back({action_key(action), path, code});
                    needed_nodes.insert(path);
                    resolved = true;
                    break;
                }
            }
            if (resolved) break;
        }
    }
    if (pending.empty()) {
        CC_INFO("adb: no nav/media key routable via evdev; using `input` fallback");
        return;
    }

    // Confirm the shell's own uid can write each node before we commit to the
    // fast path (adb `shell` is typically in the `input` group). The probe runs
    // as the same uid as the persistent shell, so `-w` is authoritative.
    std::ostringstream probe;
    probe << "for n in";
    for (const auto& n : needed_nodes) probe << ' ' << n;
    probe << "; do [ -w \"$n\" ] && echo \"$n\"; done";
    auto writ = run_capture({adb_bin_, "-s", serial, "shell", probe.str()});
    std::set<std::string> writable;
    if (writ) {
        std::istringstream ws(writ->stdout_text);
        std::string w;
        while (std::getline(ws, w)) {
            w = trim(w);
            if (!w.empty()) writable.insert(w);
        }
    }

    // Assign a shell fd slot to each writable node and open them all in one line.
    std::unordered_map<std::string, int> slot_of;
    std::ostringstream open_cmd;
    open_cmd << "exec";
    int next_slot = FIRST_FD_SLOT;
    for (const auto& n : needed_nodes) {
        if (!writable.count(n)) continue;
        slot_of[n] = next_slot;
        open_cmd << ' ' << next_slot << ">\"" << n << '"';
        ++next_slot;
    }
    if (slot_of.empty()) {
        CC_INFO("adb: evdev nodes not writable by shell uid; using `input` fallback");
        return;
    }
    if (!shell_.write_line(open_cmd.str())) {
        CC_WARN("adb: failed to open evdev fds; using `input` fallback");
        return;
    }

    for (const auto& p : pending) {
        auto it = slot_of.find(p.node);
        if (it != slot_of.end()) routes_[p.key] = {it->second, p.code};
    }
    CC_INFO("adb: evdev fast path active for %zu keys across %zu node(s)",
            routes_.size(), slot_of.size());
}

std::string AdbTransport::evdev_line_for(int linux_code, int event_size, int fd_slot) {
    std::string bytes;
    append_event(bytes, event_size, EV_KEY, linux_code, 1);   // press
    append_event(bytes, event_size, EV_SYN, SYN_REPORT, 0);
    append_event(bytes, event_size, EV_KEY, linux_code, 0);   // release
    append_event(bytes, event_size, EV_SYN, SYN_REPORT, 0);
    return "printf '" + octal_escape(bytes) + "' >&" + std::to_string(fd_slot);
}

std::optional<std::string> AdbTransport::evdev_line(const RemoteAction& action) const {
    auto it = routes_.find(action_key(action));
    if (it == routes_.end()) return std::nullopt;
    return evdev_line_for(it->second.linux_code, event_size_, it->second.fd_slot);
}

bool AdbTransport::send(const RemoteAction& action) {
    // Fast path 1: raw evdev injection for routed nav/media keys (rooted / Android
    // TV; SELinux blocks it on stock Fire TV).
    if (auto ev = evdev_line(action)) {
        CC_DEBUG("adb send (evdev): %s", action.label().c_str());
        if (!shell_.write_line(*ev)) {
            CC_WARN("adb shell pipe broken; marking disconnected");
            return false;
        }
        return true;
    }

    // Fast path 2: the warm-JVM monkey server, recovering it first if a heavy app
    // on the device (e.g. Netflix launching) killed it mid-session. On a socket
    // break we fall through to `input` for this one press rather than dropping it;
    // the next press retries recovery (rate-limited by a cooldown).
    if (auto cmd = monkey_command(action)) {
        if (recover_monkey_if_needed()) {
            CC_DEBUG("adb send (monkey): %s -> %s", action.label().c_str(),
                     cmd->c_str());
            std::string reply;
            if (monkey_.command(*cmd, reply)) return true;
            CC_WARN("adb: monkey socket broke; using `input` for this press");
        }
    }

    // Fallback: the `input` binary (pays a JVM cold-start; used for text, analog,
    // and whenever no fast path is available).
    auto line = action_to_line(action);
    if (!line) {
        CC_TRACE("adb: action not mappable, dropped: %s", action.label().c_str());
        return true;
    }
    CC_DEBUG("adb send: %s", line->c_str());
    if (!shell_.write_line(*line)) {
        CC_WARN("adb shell pipe broken; marking disconnected");
        return false;
    }
    return true;
}

void AdbTransport::disconnect() {
    teardown_monkey();  // uses serial_, so before the reset below
    monkey_supported_ = false;
    shell_.kill();
    serial_.reset();
    routes_.clear();
    CC_INFO("adb transport disconnected");
}

}  // namespace couchcast::transport::backends
