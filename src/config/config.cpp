#include "config/config.hpp"

#include <sys/stat.h>
#include <sys/types.h>

#include <cstdlib>
#include <fstream>
#include <sstream>

#define TOML_EXCEPTIONS 0
#include <toml++/toml.hpp>

#include "log.hpp"

namespace couchcast::config {

using transport::Direction;
using transport::RemoteAction;

const char* to_string(TransportKind k) {
    switch (k) {
        case TransportKind::Adb: return "adb";
        case TransportKind::BluetoothHid: return "bluetooth-hid";
        case TransportKind::Cec: return "cec";
        case TransportKind::Roku: return "roku";
        case TransportKind::Log: return "log";
    }
    return "adb";
}

bool parse_transport_kind(const std::string& s, TransportKind& out) {
    for (auto k : {TransportKind::Adb, TransportKind::BluetoothHid, TransportKind::Cec,
                   TransportKind::Roku, TransportKind::Log}) {
        if (s == to_string(k)) {
            out = k;
            return true;
        }
    }
    return false;
}

const char* to_string(CaptureCodec c) {
    switch (c) {
        case CaptureCodec::Mjpeg: return "mjpeg";
        case CaptureCodec::H264: return "h264";
        case CaptureCodec::Yuyv: return "yuyv";
        case CaptureCodec::Nv12: return "nv12";
        case CaptureCodec::I420: return "i420";
        case CaptureCodec::P010: return "p010";
        case CaptureCodec::Bgr: return "bgr";
    }
    return "mjpeg";
}

bool parse_capture_codec(const std::string& s, CaptureCodec& out) {
    for (auto c : {CaptureCodec::Mjpeg, CaptureCodec::H264, CaptureCodec::Yuyv,
                   CaptureCodec::Nv12, CaptureCodec::I420, CaptureCodec::P010,
                   CaptureCodec::Bgr}) {
        if (s == to_string(c)) {
            out = c;
            return true;
        }
    }
    return false;
}

namespace {

// --- RemoteAction <-> string (mapping serialization) ---
// Simple unit kinds and Navigate map to a snake_case string; Text carries its
// payload in a separate `text` key. Gamepad/analog passthrough is not persisted
// (it never appears in a button map).
std::optional<std::string> action_name(const RemoteAction& a) {
    using K = RemoteAction::Kind;
    switch (a.kind) {
        case K::Navigate:
            return std::string("navigate_") + transport::to_string(a.direction);
        case K::Select: return "select";
        case K::Back: return "back";
        case K::Home: return "home";
        case K::Menu: return "menu";
        case K::PlayPause: return "play_pause";
        case K::Play: return "play";
        case K::Pause: return "pause";
        case K::Stop: return "stop";
        case K::Rewind: return "rewind";
        case K::FastForward: return "fast_forward";
        case K::Next: return "next";
        case K::Previous: return "previous";
        case K::VolumeUp: return "volume_up";
        case K::VolumeDown: return "volume_down";
        case K::Mute: return "mute";
        case K::Power: return "power";
        case K::Text: return "text";
        default: return std::nullopt;
    }
}

// Retained for when config-driven button mapping is re-enabled; the loader
// currently forces the built-in default, so this inverse parser is unused.
[[maybe_unused]] std::optional<RemoteAction> action_from(const std::string& name,
                                                         const std::string& text) {
    using K = RemoteAction::Kind;
    if (name.rfind("navigate_", 0) == 0) {
        Direction d;
        if (transport::parse_direction(name.substr(9), d))
            return RemoteAction::navigate(d);
        return std::nullopt;
    }
    if (name == "select") return RemoteAction::simple(K::Select);
    if (name == "back") return RemoteAction::simple(K::Back);
    if (name == "home") return RemoteAction::simple(K::Home);
    if (name == "menu") return RemoteAction::simple(K::Menu);
    if (name == "play_pause") return RemoteAction::simple(K::PlayPause);
    if (name == "play") return RemoteAction::simple(K::Play);
    if (name == "pause") return RemoteAction::simple(K::Pause);
    if (name == "stop") return RemoteAction::simple(K::Stop);
    if (name == "rewind") return RemoteAction::simple(K::Rewind);
    if (name == "fast_forward") return RemoteAction::simple(K::FastForward);
    if (name == "next") return RemoteAction::simple(K::Next);
    if (name == "previous") return RemoteAction::simple(K::Previous);
    if (name == "volume_up") return RemoteAction::simple(K::VolumeUp);
    if (name == "volume_down") return RemoteAction::simple(K::VolumeDown);
    if (name == "mute") return RemoteAction::simple(K::Mute);
    if (name == "power") return RemoteAction::simple(K::Power);
    if (name == "text") return RemoteAction::make_text(text);
    return std::nullopt;
}

bool make_dirs(const std::string& dir) {
    // Create `dir` and any missing parents (like `mkdir -p`).
    std::string path;
    for (size_t i = 0; i < dir.size(); ++i) {
        path.push_back(dir[i]);
        if (dir[i] == '/' || i + 1 == dir.size()) {
            if (path == "/" || path.empty()) continue;
            std::string p = path;
            if (p.back() == '/') p.pop_back();
            if (mkdir(p.c_str(), 0755) != 0 && errno != EEXIST) return false;
        }
    }
    return true;
}

}  // namespace

std::string config_dir() {
    const char* xdg = std::getenv("XDG_CONFIG_HOME");
    std::string base;
    if (xdg && xdg[0]) {
        base = xdg;
    } else {
        const char* home = std::getenv("HOME");
        base = std::string(home ? home : ".") + "/.config";
    }
    return base + "/couchcast";
}

std::string Config::path() { return config_dir() + "/config.toml"; }

Config Config::load_from(const std::string& p) {
    std::ifstream in(p);
    if (!in) return Config{};  // absent -> defaults
    std::stringstream ss;
    ss << in.rdbuf();

    toml::parse_result res = toml::parse(ss.str());
    if (!res) {
        CC_WARN("parsing config: %s", std::string(res.error().description()).c_str());
        return Config{};
    }
    const toml::table& tbl = res.table();

    Config cfg;

    if (auto* dev = tbl["last_device"].as_table()) {
        DeviceRef d;
        d.name = (*dev)["name"].value_or("");
        d.node = (*dev)["node"].value_or("");
        if (!d.node.empty()) cfg.last_device = d;
    }

    if (auto* media = tbl["media"].as_table()) {
        if (auto v = (*media)["codec"].value<std::string>()) {
            CaptureCodec c;
            if (parse_capture_codec(*v, c)) cfg.media.codec = c;
        }
        if (auto v = (*media)["width"].value<int64_t>())
            cfg.media.width = static_cast<uint32_t>(*v);
        if (auto v = (*media)["height"].value<int64_t>())
            cfg.media.height = static_cast<uint32_t>(*v);
        if (auto v = (*media)["framerate"].value<int64_t>())
            cfg.media.framerate = static_cast<uint32_t>(*v);
        cfg.media.audio = (*media)["audio"].value_or(true);
        // hdr_output defaults on for configs written before the field existed.
        cfg.media.hdr_output = (*media)["hdr_output"].value_or(true);
    }

    if (auto* tgt = tbl["target"].as_table()) {
        TargetConfig t;
        if (auto v = (*tgt)["transport"].value<std::string>()) {
            parse_transport_kind(*v, t.transport);
        }
        t.address = (*tgt)["address"].value_or("");
        cfg.target = t;
    }

    // For now the button mapping is always the built-in default: any `mapping`
    // array in the config is ignored so behavior stays consistent across setups.
    // (cfg.mapping is initialized to ButtonMap::make_default().)

    return cfg;
}

Config Config::load_or_default() {
    // load_from already falls back to defaults on error; keep the same behavior.
    return load_from(path());
}

bool Config::save_to(const std::string& p) const {
    // Build the TOML document.
    toml::table root;

    if (last_device) {
        root.insert("last_device", toml::table{{"name", last_device->name},
                                               {"node", last_device->node}});
    }

    toml::table media;
    if (this->media.codec) media.insert("codec", to_string(*this->media.codec));
    if (this->media.width) media.insert("width", static_cast<int64_t>(*this->media.width));
    if (this->media.height) media.insert("height", static_cast<int64_t>(*this->media.height));
    if (this->media.framerate)
        media.insert("framerate", static_cast<int64_t>(*this->media.framerate));
    media.insert("audio", this->media.audio);
    media.insert("hdr_output", this->media.hdr_output);
    root.insert("media", std::move(media));

    if (target) {
        root.insert("target", toml::table{{"transport", to_string(target->transport)},
                                          {"address", target->address}});
    }

    toml::array mapping;
    for (const auto& b : this->mapping.bindings) {
        auto name = action_name(b.action);
        if (!name) continue;
        toml::table row;
        row.insert("button", transport::to_string(b.button));
        row.insert("action", *name);
        if (b.action.kind == RemoteAction::Kind::Text) row.insert("text", b.action.text);
        mapping.push_back(std::move(row));
    }
    root.insert("mapping", std::move(mapping));

    // Ensure the parent directory exists.
    auto slash = p.find_last_of('/');
    if (slash != std::string::npos) {
        if (!make_dirs(p.substr(0, slash))) {
            CC_WARN("writing config: could not create %s", p.substr(0, slash).c_str());
            return false;
        }
    }

    std::ofstream out(p, std::ios::trunc);
    if (!out) {
        CC_WARN("writing config: could not open %s", p.c_str());
        return false;
    }
    out << toml::toml_formatter{root};
    return true;
}

bool Config::save() const { return save_to(path()); }

}  // namespace couchcast::config
