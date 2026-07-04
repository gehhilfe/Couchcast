//! Capture, display, and audio for Couchcast.
//!
//! Everything runs through a **single** GStreamer pipeline: a V4L2 video branch
//! (`v4l2src → decode → gtk4paintablesink`) and an optional PipeWire audio branch
//! (`pipewiresrc → autoaudiosink`). Sharing one pipeline (and therefore one
//! clock) is what gives A/V sync essentially for free — the decisive reason to
//! render video through GStreamer rather than a bare Wayland surface.
//!
//! Video is handed to GTK zero-copy through [`gtk4paintablesink`](https://gstreamer.freedesktop.org/documentation/gtk4/index.html),
//! whose [`gdk::Paintable`](gtk4::gdk::Paintable) the UI drops into a
//! `gtk::Picture`. See `docs/ARCHITECTURE.md` for the latency knobs and the
//! DMABUF/gamescope story.

use gstreamer as gst;

mod device;
mod error;
mod pipeline;

pub use device::{CaptureDevice, list_devices};
pub use error::MediaError;
pub use pipeline::{CapturePipeline, PipelineConfig};

/// Initialize GStreamer itself. Idempotent and cheap; safe to call repeatedly.
pub(crate) fn init_gstreamer() -> Result<(), MediaError> {
    gst::init()?;
    Ok(())
}

/// Initialize GStreamer and register the in-process `gtk4paintablesink` element
/// from `gst-plugin-gtk4` (it is not a system plugin — it ships as a Rust crate
/// registered statically). Idempotent.
pub(crate) fn init_video_sink() -> Result<(), MediaError> {
    init_gstreamer()?;
    gstgtk4::plugin_register_static()?;
    Ok(())
}
