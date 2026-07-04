//! Capture, display, and audio for Couchcast.
//!
//! Everything runs through a **single** GStreamer pipeline: a V4L2 video branch
//! (`v4l2src → decode → appsink`) and an optional PipeWire audio branch
//! (`pipewiresrc → autoaudiosink`). Sharing one pipeline (and therefore one
//! clock) is what gives A/V sync essentially for free — the decisive reason to
//! render video through GStreamer rather than a bare Wayland surface.
//!
//! The video branch terminates in an `appsink` that hands each decoded frame to
//! the app as a [`VideoFrame`] (see [`frame`]). The app owns the wgpu device and
//! uploads/imports the frame; this crate stays renderer-agnostic. See
//! `docs/ARCHITECTURE.md` for the latency knobs and the DMABUF/gamescope story.

use gstreamer as gst;

mod device;
mod error;
mod frame;
mod pipeline;

pub use device::{CaptureDevice, list_devices};
pub use error::MediaError;
pub use frame::{PixelFormat, PlaneRef, VideoFrame};
pub use pipeline::{CapturePipeline, PipelineConfig};

/// Initialize GStreamer itself. Idempotent and cheap; safe to call repeatedly.
pub(crate) fn init_gstreamer() -> Result<(), MediaError> {
    gst::init()?;
    Ok(())
}
