#include "media/device.hpp"

#include <fcntl.h>
#include <linux/videodev2.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include <gst/gst.h>

#include <algorithm>
#include <cerrno>
#include <cmath>
#include <cstring>

#include "log.hpp"

namespace couchcast::media {

const char* source_caps(CaptureCodec c) {
    switch (c) {
        case CaptureCodec::Mjpeg: return "image/jpeg";
        case CaptureCodec::H264: return "video/x-h264";
        case CaptureCodec::Yuyv: return "video/x-raw,format=YUY2";
        case CaptureCodec::Nv12: return "video/x-raw,format=NV12";
        case CaptureCodec::I420: return "video/x-raw,format=I420";
        case CaptureCodec::P010: return "video/x-raw,format=P010_10LE";
        case CaptureCodec::Bgr: return "video/x-raw,format=BGR";
    }
    return "video/x-raw";
}

const char* codec_label(CaptureCodec c) {
    switch (c) {
        case CaptureCodec::Mjpeg: return "MJPEG";
        case CaptureCodec::H264: return "H.264";
        case CaptureCodec::Yuyv: return "YUYV";
        case CaptureCodec::Nv12: return "NV12";
        case CaptureCodec::I420: return "I420";
        case CaptureCodec::P010: return "P010";
        case CaptureCodec::Bgr: return "BGR";
    }
    return "?";
}

std::optional<CaptureCodec> codec_from_fourcc(const std::array<char, 4>& f) {
    auto eq = [&](const char* s) { return std::memcmp(f.data(), s, 4) == 0; };
    if (eq("MJPG")) return CaptureCodec::Mjpeg;
    if (eq("H264")) return CaptureCodec::H264;
    if (eq("YUYV")) return CaptureCodec::Yuyv;
    if (eq("NV12")) return CaptureCodec::Nv12;
    if (eq("YU12")) return CaptureCodec::I420;
    if (eq("P010")) return CaptureCodec::P010;
    if (eq("BGR3")) return CaptureCodec::Bgr;
    return std::nullopt;
}

std::vector<CaptureCodec> codecs(const std::vector<CaptureFormat>& formats) {
    std::vector<CaptureCodec> out;
    for (CaptureCodec c : CODEC_ORDER) {
        if (std::any_of(formats.begin(), formats.end(),
                        [&](const CaptureFormat& f) { return f.codec == c; })) {
            out.push_back(c);
        }
    }
    return out;
}

std::vector<std::pair<uint32_t, uint32_t>> resolutions(
    const std::vector<CaptureFormat>& formats, CaptureCodec codec) {
    std::vector<std::pair<uint32_t, uint32_t>> res;
    for (const auto& f : formats) {
        if (f.codec == codec) res.emplace_back(f.width, f.height);
    }
    std::sort(res.begin(), res.end(), [](const auto& a, const auto& b) {
        uint64_t pa = static_cast<uint64_t>(a.first) * a.second;
        uint64_t pb = static_cast<uint64_t>(b.first) * b.second;
        if (pa != pb) return pa > pb;
        return a > b;
    });
    res.erase(std::unique(res.begin(), res.end()), res.end());
    return res;
}

std::vector<uint32_t> framerates(const std::vector<CaptureFormat>& formats,
                                 CaptureCodec codec,
                                 std::pair<uint32_t, uint32_t> res) {
    for (const auto& f : formats) {
        if (f.codec == codec && std::pair<uint32_t, uint32_t>(f.width, f.height) == res)
            return f.framerates;
    }
    return {};
}

namespace {

std::optional<uint32_t> fraction_fps(uint32_t num, uint32_t den) {
    if (num == 0) return std::nullopt;
    double fps = std::round(static_cast<double>(den) / static_cast<double>(num));
    if (fps >= 1.0) return static_cast<uint32_t>(fps);
    return std::nullopt;
}

/// Read a device's supported CaptureFormats directly from V4L2 (best-effort).
std::vector<CaptureFormat> enumerate_formats(const std::string& node) {
    std::vector<CaptureFormat> out;
    int fd = open(node.c_str(), O_RDONLY | O_NONBLOCK);
    if (fd < 0) return out;

    for (uint32_t fi = 0;; ++fi) {
        v4l2_fmtdesc fmt{};
        fmt.index = fi;
        fmt.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        if (ioctl(fd, VIDIOC_ENUM_FMT, &fmt) != 0) break;

        std::array<char, 4> fourcc = {
            static_cast<char>(fmt.pixelformat & 0xff),
            static_cast<char>((fmt.pixelformat >> 8) & 0xff),
            static_cast<char>((fmt.pixelformat >> 16) & 0xff),
            static_cast<char>((fmt.pixelformat >> 24) & 0xff)};
        auto codec = codec_from_fourcc(fourcc);
        if (!codec) continue;

        for (uint32_t si = 0;; ++si) {
            v4l2_frmsizeenum size{};
            size.index = si;
            size.pixel_format = fmt.pixelformat;
            if (ioctl(fd, VIDIOC_ENUM_FRAMESIZES, &size) != 0) break;

            uint32_t width = 0, height = 0;
            if (size.type == V4L2_FRMSIZE_TYPE_DISCRETE) {
                width = size.discrete.width;
                height = size.discrete.height;
            } else {
                width = size.stepwise.max_width;
                height = size.stepwise.max_height;
            }

            std::vector<uint32_t> rates;
            for (uint32_t ii = 0;; ++ii) {
                v4l2_frmivalenum iv{};
                iv.index = ii;
                iv.pixel_format = fmt.pixelformat;
                iv.width = width;
                iv.height = height;
                if (ioctl(fd, VIDIOC_ENUM_FRAMEINTERVALS, &iv) != 0) break;
                std::optional<uint32_t> fps;
                if (iv.type == V4L2_FRMIVAL_TYPE_DISCRETE) {
                    fps = fraction_fps(iv.discrete.numerator, iv.discrete.denominator);
                } else {
                    // Fastest rate = smallest interval = `min`.
                    fps = fraction_fps(iv.stepwise.min.numerator,
                                       iv.stepwise.min.denominator);
                }
                if (fps) rates.push_back(*fps);
                if (iv.type != V4L2_FRMIVAL_TYPE_DISCRETE) break;
            }
            std::sort(rates.begin(), rates.end(), std::greater<>());
            rates.erase(std::unique(rates.begin(), rates.end()), rates.end());

            out.push_back(CaptureFormat{*codec, width, height, std::move(rates)});

            if (size.type != V4L2_FRMSIZE_TYPE_DISCRETE) break;
        }
    }

    close(fd);
    return out;
}

/// Extract the /dev/video* node path from a GstDevice's properties.
std::optional<std::string> device_node(GstDevice* device) {
    GstStructure* props = gst_device_get_properties(device);
    if (!props) return std::nullopt;
    std::optional<std::string> result;
    for (const char* key : {"device.path", "api.v4l2.path", "object.path"}) {
        const gchar* val = gst_structure_get_string(props, key);
        if (val) {
            result = val;
            break;
        }
    }
    gst_structure_free(props);
    return result;
}

}  // namespace

std::vector<CaptureDevice> list_devices() {
    if (!gst_is_initialized()) gst_init(nullptr, nullptr);

    GstDeviceMonitor* monitor = gst_device_monitor_new();
    gst_device_monitor_add_filter(monitor, "Video/Source", nullptr);
    gst_device_monitor_start(monitor);
    GList* devices = gst_device_monitor_get_devices(monitor);
    gst_device_monitor_stop(monitor);

    std::vector<CaptureDevice> out;
    for (GList* l = devices; l != nullptr; l = l->next) {
        GstDevice* device = GST_DEVICE(l->data);
        auto node = device_node(device);
        if (!node) continue;

        CaptureDevice cd;
        gchar* name = gst_device_get_display_name(device);
        cd.name = name ? name : "";
        g_free(name);
        cd.node = *node;

        GstCaps* caps = gst_device_get_caps(device);
        if (caps) {
            gchar* s = gst_caps_to_string(caps);
            if (s) {
                cd.caps = s;
                g_free(s);
            }
            gst_caps_unref(caps);
        }
        cd.formats = enumerate_formats(*node);
        out.push_back(std::move(cd));
    }

    g_list_free_full(devices, gst_object_unref);
    gst_object_unref(monitor);
    return out;
}

}  // namespace couchcast::media
