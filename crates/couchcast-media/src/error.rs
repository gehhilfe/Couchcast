//! Media subsystem error type.

use gstreamer as gst;

/// Errors from GStreamer initialization, device enumeration, or the pipeline.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("GStreamer error: {0}")]
    Glib(#[from] gst::glib::Error),
    #[error("GStreamer error: {0}")]
    Bool(#[from] gst::glib::BoolError),
    #[error("pipeline state change failed: {0}")]
    StateChange(#[from] gst::StateChangeError),
    #[error("pipeline description did not produce a gst::Pipeline")]
    NotAPipeline,
    #[error("pipeline is missing the `{0}` element")]
    MissingElement(&'static str),
}
