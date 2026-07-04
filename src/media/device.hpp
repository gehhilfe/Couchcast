#pragma once
//! Capture-device enumeration. Ported from `couchcast-media::device`.
//!
//! Device discovery (names + node paths) uses GStreamer's DeviceMonitor, but the
//! per-device format list is read straight from V4L2 via ioctls (GStreamer's
//! probe silently drops formats such as 10-bit P010 that v4l2src can capture).

#include <array>
#include <cstdint>
#include <optional>
#include <string>
#include <vector>

namespace couchcast::media {

/// The capture *input* format a device delivers.
enum class CaptureCodec { Mjpeg, H264, Yuyv, Nv12, I420, P010, Bgr };

/// Codecs in stable display order.
inline constexpr std::array<CaptureCodec, 7> CODEC_ORDER = {
    CaptureCodec::Mjpeg, CaptureCodec::H264, CaptureCodec::Yuyv, CaptureCodec::Nv12,
    CaptureCodec::I420,  CaptureCodec::P010, CaptureCodec::Bgr};

/// The leading GStreamer source caps this codec pins in front of decodebin.
const char* source_caps(CaptureCodec c);

/// A short human label for the menu.
const char* codec_label(CaptureCodec c);

/// Map a V4L2 fourcc (e.g. "NV12") to a codec, if recognized.
std::optional<CaptureCodec> codec_from_fourcc(const std::array<char, 4>& fourcc);

/// A capture format the device advertises.
struct CaptureFormat {
    CaptureCodec codec;
    uint32_t width;
    uint32_t height;
    std::vector<uint32_t> framerates;  // sorted descending, deduped
    bool operator==(const CaptureFormat&) const = default;
};

/// The distinct codecs offered across `formats`, in stable display order.
std::vector<CaptureCodec> codecs(const std::vector<CaptureFormat>& formats);

/// The distinct resolutions offered for `codec`, sorted descending by pixel count.
std::vector<std::pair<uint32_t, uint32_t>> resolutions(
    const std::vector<CaptureFormat>& formats, CaptureCodec codec);

/// The framerates offered for `codec` at `res`, sorted descending.
std::vector<uint32_t> framerates(const std::vector<CaptureFormat>& formats,
                                 CaptureCodec codec,
                                 std::pair<uint32_t, uint32_t> res);

/// A capture device the user can select.
struct CaptureDevice {
    std::string name;
    std::string node;
    std::optional<std::string> caps;
    std::vector<CaptureFormat> formats;
};

/// Enumerate connected V4L2 video-source devices (best-effort).
std::vector<CaptureDevice> list_devices();

}  // namespace couchcast::media
