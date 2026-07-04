//! Decoded video frames handed from the capture pipeline to the renderer.
//!
//! `couchcast-media` stays renderer-agnostic: it maps/exports frames and exposes
//! plain plane bytes + strides, so the app (which owns the wgpu device) does the
//! GPU upload without this crate depending on wgpu. The sysmem [`VideoFrame::Mapped`]
//! path is the physically-optimal route for USB capture dongles (frames already
//! land in RAM); the zero-copy DMABUF variant is added in a later stage.

use gstreamer as gst;
use gstreamer_video as gst_video;
use gstreamer_video::prelude::*;

use crate::error::MediaError;

/// Pixel layout of a decoded frame. Both variants are semi-planar 4:2:0 — a
/// full-res Y plane plus a half-res interleaved UV plane — and differ only in bit
/// depth. The default SDR path negotiates NV12 (8-bit, what VA decoders and most
/// dongles produce); the HDR path negotiates P010 (10-bit) so wide-gamut / PQ
/// content survives to the renderer instead of being crushed to 8-bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 8-bit 4:2:0: plane 0 = Y (one byte/px), plane 1 = interleaved U/V (two
    /// bytes per 2×2 block). Sampled as `R8`/`Rg8` textures.
    Nv12,
    /// 10-bit 4:2:0 (`P010_10LE`): each sample is a little-endian 16-bit word with
    /// the 10 bits left-justified. Same plane layout as NV12 but two bytes per
    /// component; sampled as `R16`/`Rg16` textures. The carrier for HDR capture.
    P010,
}

impl PixelFormat {
    /// A short label for logs and the debug overlay.
    pub fn label(&self) -> &'static str {
        match self {
            PixelFormat::Nv12 => "NV12",
            PixelFormat::P010 => "P010",
        }
    }

    /// Map a decoded GStreamer format to our renderer-facing pixel format. Only
    /// the two formats the appsink ever negotiates are distinguished; anything
    /// else falls back to NV12 (it is byte-compatible with the 8-bit path).
    fn from_video_format(f: gst_video::VideoFormat) -> Self {
        match f {
            gst_video::VideoFormat::P01010le => PixelFormat::P010,
            _ => PixelFormat::Nv12,
        }
    }
}

/// The GStreamer raw format the video appsink negotiates for `codec`. Raw 10-bit
/// P010 stays 10-bit (`P010_10LE`) so the HDR path keeps its precision; every
/// other codec is converted to 8-bit `NV12`. Kept next to [`PixelFormat`], which
/// it must stay in sync with.
pub(crate) fn negotiated_format(codec: Option<crate::device::CaptureCodec>) -> &'static str {
    match codec {
        Some(crate::device::CaptureCodec::P010) => "P010_10LE",
        _ => "NV12",
    }
}

/// A borrowed view of one plane's bytes and its row stride.
pub struct PlaneRef<'a> {
    pub data: &'a [u8],
    pub stride: usize,
}

/// A decoded frame ready to upload. Owns whatever keeps its pixels alive.
pub enum VideoFrame {
    /// System-memory frame: the buffer is mapped readable and uploaded per plane.
    Mapped(MappedFrame),
}

/// A mapped, readable system-memory frame. Holds the GStreamer mapping alive for
/// as long as the plane data is borrowed.
pub struct MappedFrame {
    frame: gst_video::VideoFrame<gst_video::video_frame::Readable>,
}

impl VideoFrame {
    /// Extract a [`VideoFrame`] from an appsink sample. Currently supports the
    /// system-memory NV12 path.
    pub fn from_sample(sample: &gst::Sample) -> Result<Self, MediaError> {
        let buffer = sample
            .buffer_owned()
            .ok_or(MediaError::MissingElement("sample buffer"))?;
        let caps = sample
            .caps()
            .ok_or(MediaError::MissingElement("sample caps"))?;
        let info = gst_video::VideoInfo::from_caps(caps)
            .map_err(|_| MediaError::MissingElement("video caps info"))?;

        let frame = gst_video::VideoFrame::from_buffer_readable(buffer, &info)
            .map_err(|_| MediaError::MissingElement("mappable frame"))?;
        Ok(VideoFrame::Mapped(MappedFrame { frame }))
    }

    pub fn width(&self) -> u32 {
        match self {
            VideoFrame::Mapped(m) => m.frame.width(),
        }
    }

    pub fn height(&self) -> u32 {
        match self {
            VideoFrame::Mapped(m) => m.frame.height(),
        }
    }

    pub fn format(&self) -> PixelFormat {
        match self {
            VideoFrame::Mapped(m) => PixelFormat::from_video_format(m.frame.format()),
        }
    }

    /// Whether this frame should be rendered through the HDR path (PQ EOTF,
    /// BT.2020 primaries, tone-map to the display) rather than the plain SDR path.
    ///
    /// True only for 10-bit P010. Cheap HDMI capture dongles frequently deliver
    /// P010 *without* tagging colorimetry, so an untagged (`Unknown`) transfer is
    /// treated as HDR — P010 is used almost exclusively to carry HDR10. A P010
    /// stream explicitly tagged with an SDR transfer is rendered as SDR.
    pub fn is_hdr(&self) -> bool {
        match self {
            VideoFrame::Mapped(m) => {
                if !matches!(m.frame.format(), gst_video::VideoFormat::P01010le) {
                    return false;
                }
                // Anything explicitly tagged with a common SDR transfer renders
                // SDR; PQ / HLG (and untagged `Unknown`) fall through to HDR. The
                // HDR transfers are `v1_18`-gated in the bindings, so we match the
                // ungated SDR set and let `_` cover the rest rather than naming
                // them (which would force a newer GStreamer feature).
                use gst_video::VideoTransferFunction as T;
                !matches!(
                    m.frame.info().colorimetry().transfer(),
                    T::Gamma10
                        | T::Gamma18
                        | T::Gamma20
                        | T::Gamma22
                        | T::Gamma28
                        | T::Bt709
                        | T::Smpte240m
                        | T::Srgb
                        | T::Log100
                        | T::Log316
                        | T::Bt202012
                        | T::Adobergb
                )
            }
        }
    }

    /// Borrow plane `i` (0 = Y, 1 = UV), if present.
    pub fn plane(&self, i: usize) -> Option<PlaneRef<'_>> {
        match self {
            VideoFrame::Mapped(m) => {
                let data = m.frame.plane_data(i as u32).ok()?;
                let stride = *m.frame.plane_stride().get(i)? as usize;
                Some(PlaneRef { data, stride })
            }
        }
    }
}
