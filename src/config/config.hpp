#pragma once
//! Persistent configuration for Couchcast. Ported from `couchcast-config`.
//!
//! Stored as human-editable TOML under the XDG config directory
//! (`~/.config/couchcast/config.toml`). A missing file yields defaults; a corrupt
//! file is logged and replaced by defaults, so config never bricks the app.

#include <optional>
#include <string>

#include "config/mapping.hpp"
#include "transport/transport.hpp"

namespace couchcast::config {

/// Which transport backend to use for a target.
enum class TransportKind { Adb, BluetoothHid, Cec, Roku, Log };

const char* to_string(TransportKind k);
bool parse_transport_kind(const std::string& s, TransportKind& out);

/// The capture input format to request (a V4L2 pixel format or codec). Mirrors
/// media::CaptureCodec; kept separate so config stays free of GStreamer.
enum class CaptureCodec { Mjpeg, H264, Yuyv, Nv12, I420, P010, Bgr };

const char* to_string(CaptureCodec c);
bool parse_capture_codec(const std::string& s, CaptureCodec& out);

/// A capture device the user selected, remembered across runs.
struct DeviceRef {
    std::string name;
    std::string node;
    bool operator==(const DeviceRef&) const = default;
};

/// The device Couchcast forwards input to, and how to reach it.
struct TargetConfig {
    TransportKind transport = TransportKind::Adb;
    std::string address;
    bool operator==(const TargetConfig&) const = default;

    transport::TargetAddr to_target_addr() const {
        return transport::TargetAddr::network(address);
    }
};

/// Video/audio preferences for the capture pipeline.
struct MediaPrefs {
    std::optional<CaptureCodec> codec;
    std::optional<uint32_t> width;
    std::optional<uint32_t> height;
    std::optional<uint32_t> framerate;
    bool audio = true;
    bool hdr_output = true;
    bool operator==(const MediaPrefs&) const = default;
};

/// The full application configuration.
struct Config {
    std::optional<DeviceRef> last_device;
    MediaPrefs media;
    std::optional<TargetConfig> target;
    ButtonMap mapping = ButtonMap::make_default();

    /// The path Couchcast reads and writes.
    static std::string path();

    /// Load from `p`, returning defaults if the file is absent.
    static Config load_from(const std::string& p);

    /// Load from the standard path, falling back to defaults (with a logged
    /// warning) on any error.
    static Config load_or_default();

    /// Write to `p`, creating parent directories as needed. Returns false on error.
    bool save_to(const std::string& p) const;

    /// Write to the standard path.
    bool save() const;
};

/// The Couchcast config directory (`~/.config/couchcast` on Linux).
std::string config_dir();

}  // namespace couchcast::config
