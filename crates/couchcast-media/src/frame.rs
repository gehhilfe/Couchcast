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

/// Pixel layout of a decoded frame. We negotiate NV12 out of the pipeline (the
/// format VA decoders and most dongles produce, and the one the wgpu shader
/// samples): a full-res Y plane plus a half-res interleaved UV plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 4:2:0, plane 0 = Y (one byte/px), plane 1 = interleaved U/V (two bytes per
    /// 2×2 block).
    Nv12,
}

/// The GStreamer caps string the video appsink negotiates. Kept next to the
/// [`PixelFormat`] it must stay in sync with.
pub(crate) const NEGOTIATED_FORMAT: &str = "NV12";

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
        PixelFormat::Nv12
    }

    /// Borrow plane `i` (0 = Y, 1 = UV for NV12), if present.
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
