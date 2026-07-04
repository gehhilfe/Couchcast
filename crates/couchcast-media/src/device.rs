//! V4L2 capture-device enumeration via GStreamer's `DeviceMonitor`.
//!
//! Using the same GStreamer machinery that builds the pipeline (rather than raw
//! V4L2 ioctls) keeps device names and capabilities consistent with what the
//! pipeline will actually negotiate.

use gst::prelude::*;
use gstreamer as gst;

use crate::error::MediaError;
use crate::init_gstreamer;

/// A capture device the user can select.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureDevice {
    /// Human-readable name, e.g. "USB3 HDMI Capture".
    pub name: String,
    /// The V4L2 device node, e.g. `/dev/video0`.
    pub node: String,
    /// A stringified summary of the caps the device advertises, if available.
    pub caps: Option<String>,
}

/// Enumerate connected V4L2 video-source devices.
///
/// Cheap HDMI dongles frequently expose several `/dev/video*` nodes (only one of
/// which carries frames) and misreport formats, so callers should treat this as
/// a best-effort list to present in the UI, not gospel.
pub fn list_devices() -> Result<Vec<CaptureDevice>, MediaError> {
    init_gstreamer()?;

    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Video/Source"), None);
    monitor.start()?;
    let devices = monitor.devices();
    monitor.stop();

    let mut out = Vec::new();
    for device in devices.iter() {
        let Some(node) = device_node(device) else {
            continue;
        };
        out.push(CaptureDevice {
            name: device.display_name().to_string(),
            node,
            caps: device.caps().map(|c| c.to_string()),
        });
    }
    Ok(out)
}

/// Extract the `/dev/video*` node path from a device's properties, trying the
/// keys different GStreamer/V4L2/PipeWire providers use.
fn device_node(device: &gst::Device) -> Option<String> {
    let props = device.properties()?;
    for key in ["device.path", "api.v4l2.path", "object.path"] {
        if let Ok(value) = props.get::<String>(key) {
            return Some(value);
        }
    }
    None
}
