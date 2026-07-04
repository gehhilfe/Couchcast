#include "media/frame.hpp"

#include <gst/app/gstappsink.h>
#include <gst/gst.h>
#include <gst/video/video.h>

#include <algorithm>

#include "log.hpp"

namespace couchcast::media {

const char* pixel_format_label(PixelFormat f) {
    return f == PixelFormat::P010 ? "P010" : "NV12";
}

const char* negotiated_format(std::optional<CaptureCodec> codec) {
    if (codec && *codec == CaptureCodec::P010) return "P010_10LE";
    return "NV12";
}

namespace {
PixelFormat from_video_format(GstVideoFormat f) {
    return f == GST_VIDEO_FORMAT_P010_10LE ? PixelFormat::P010 : PixelFormat::Nv12;
}

// P010 + PQ/HLG (or untagged Unknown) renders HDR; a common SDR transfer is SDR.
bool compute_is_hdr(const GstVideoFrame& frame) {
    if (GST_VIDEO_FRAME_FORMAT(&frame) != GST_VIDEO_FORMAT_P010_10LE) return false;
    switch (frame.info.colorimetry.transfer) {
        case GST_VIDEO_TRANSFER_GAMMA10:
        case GST_VIDEO_TRANSFER_GAMMA18:
        case GST_VIDEO_TRANSFER_GAMMA20:
        case GST_VIDEO_TRANSFER_GAMMA22:
        case GST_VIDEO_TRANSFER_GAMMA28:
        case GST_VIDEO_TRANSFER_BT709:
        case GST_VIDEO_TRANSFER_SMPTE240M:
        case GST_VIDEO_TRANSFER_SRGB:
        case GST_VIDEO_TRANSFER_LOG100:
        case GST_VIDEO_TRANSFER_LOG316:
        case GST_VIDEO_TRANSFER_BT2020_12:
        case GST_VIDEO_TRANSFER_ADOBERGB:
            return false;
        default:
            return true;
    }
}
}  // namespace

std::optional<VideoFrame> VideoFrame::from_sample(GstSample* sample) {
    if (!sample) return std::nullopt;

    GstBuffer* buffer = gst_sample_get_buffer(sample);
    GstCaps* caps = gst_sample_get_caps(sample);
    if (!buffer || !caps) {
        gst_sample_unref(sample);
        return std::nullopt;
    }

    GstVideoInfo info;
    if (!gst_video_info_from_caps(&info, caps)) {
        gst_sample_unref(sample);
        return std::nullopt;
    }

    GstVideoFrame vframe;
    if (!gst_video_frame_map(&vframe, &info, buffer, GST_MAP_READ)) {
        CC_WARN("failed to map video frame");
        gst_sample_unref(sample);
        return std::nullopt;
    }

    VideoFrame vf;
    vf.width_ = static_cast<uint32_t>(GST_VIDEO_FRAME_WIDTH(&vframe));
    vf.height_ = static_cast<uint32_t>(GST_VIDEO_FRAME_HEIGHT(&vframe));
    vf.format_ = from_video_format(GST_VIDEO_FRAME_FORMAT(&vframe));
    vf.is_hdr_ = compute_is_hdr(vframe);

    guint n = GST_VIDEO_FRAME_N_PLANES(&vframe);
    vf.n_planes_ = std::min<size_t>(n, vf.planes_.size());
    for (size_t i = 0; i < vf.n_planes_; ++i) {
        size_t stride = static_cast<size_t>(GST_VIDEO_FRAME_PLANE_STRIDE(&vframe, i));
        // Plane 0 (Y) is full height; plane 1 (UV) is half height for 4:2:0.
        size_t plane_h = (i == 0) ? vf.height_ : (vf.height_ + 1) / 2;
        const uint8_t* src =
            static_cast<const uint8_t*>(GST_VIDEO_FRAME_PLANE_DATA(&vframe, i));
        vf.planes_[i].stride = stride;
        vf.planes_[i].data.assign(src, src + stride * plane_h);
    }

    // Copy done; the capture buffer is returned to the pool as soon as we unref.
    gst_video_frame_unmap(&vframe);
    gst_sample_unref(sample);
    return vf;
}

std::optional<PlaneRef> VideoFrame::plane(size_t i) const {
    if (i >= n_planes_) return std::nullopt;
    const OwnedPlane& p = planes_[i];
    return PlaneRef{p.data.data(), p.data.size(), p.stride};
}

}  // namespace couchcast::media
