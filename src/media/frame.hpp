#pragma once
//! Decoded video frames handed from the capture pipeline to the renderer.
//! Ported from `couchcast-media::frame`.
//!
//! Frames own a *copy* of their pixel data rather than pinning the GStreamer
//! sample. This is load-bearing: with a zero-copy v4l2 passthrough (raw NV12/
//! P010 flow straight from the capture buffer to the appsink), holding the
//! sample would keep a v4l2 mmap buffer checked out for however long the frame
//! sits in the render mailbox. The v4l2 pool is small (~4-5 buffers), so pinning
//! frames there starves `v4l2src` and collapses the capture rate. Copying out in
//! `from_sample` returns the buffer to the pool immediately.

#include <array>
#include <cstdint>
#include <optional>
#include <vector>

#include "media/device.hpp"

typedef struct _GstSample GstSample;

namespace couchcast::media {

/// Pixel layout of a decoded frame. Both are semi-planar 4:2:0; they differ only
/// in bit depth. NV12 = 8-bit SDR path; P010 = 10-bit HDR carrier.
enum class PixelFormat { Nv12, P010 };

const char* pixel_format_label(PixelFormat f);

/// The GStreamer raw format the appsink negotiates for `codec` (P010 stays
/// 10-bit; everything else converts to 8-bit NV12).
const char* negotiated_format(std::optional<CaptureCodec> codec);

/// A borrowed view of one plane's bytes and its row stride.
struct PlaneRef {
    const uint8_t* data;
    size_t size;
    size_t stride;
};

/// A decoded system-memory frame that owns its pixel data. Move-only. The
/// GStreamer sample it was built from is released before construction returns,
/// so a frame can be held indefinitely without back-pressuring capture.
class VideoFrame {
   public:
    VideoFrame() = default;
    ~VideoFrame() = default;
    VideoFrame(const VideoFrame&) = delete;
    VideoFrame& operator=(const VideoFrame&) = delete;
    VideoFrame(VideoFrame&&) noexcept = default;
    VideoFrame& operator=(VideoFrame&&) noexcept = default;

    /// Build from an appsink sample: copies the pixel planes out, then releases
    /// `sample` (always — including on the failure paths). Returns nullopt on
    /// failure.
    static std::optional<VideoFrame> from_sample(GstSample* sample);

    uint32_t width() const { return width_; }
    uint32_t height() const { return height_; }
    PixelFormat format() const { return format_; }

    /// Whether this frame should render through the HDR path (P010 + non-SDR or
    /// untagged transfer).
    bool is_hdr() const { return is_hdr_; }

    /// Borrow plane `i` (0 = Y, 1 = UV), if present.
    std::optional<PlaneRef> plane(size_t i) const;

   private:
    /// One owned plane: tightly packed at the source stride (kept so the copy
    /// into the Vulkan staging buffer can address rows by stride, unchanged).
    struct OwnedPlane {
        std::vector<uint8_t> data;
        size_t stride = 0;
    };

    uint32_t width_ = 0;
    uint32_t height_ = 0;
    PixelFormat format_ = PixelFormat::Nv12;
    bool is_hdr_ = false;
    size_t n_planes_ = 0;
    // NV12 and P010 are both 2-plane (Y + interleaved UV).
    std::array<OwnedPlane, 2> planes_{};
};

}  // namespace couchcast::media
