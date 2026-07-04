//! Capture-device enumeration.
//!
//! Device discovery (names + node paths) uses GStreamer's `DeviceMonitor`, but
//! the per-device *format* list is read straight from V4L2 via ioctls (the `v4l`
//! crate). GStreamer's probe reports only a subset of a device's formats — it
//! silently drops some pixel formats such as 10-bit `P010`, even though `v4l2src`
//! can capture them — so we go to the kernel directly to get the full, honest
//! list of formats / resolutions / framerates.

use gst::prelude::*;
use gstreamer as gst;
use v4l::frameinterval::FrameIntervalEnum;
use v4l::framesize::FrameSizeEnum;
use v4l::video::Capture;

use crate::error::MediaError;
use crate::init_gstreamer;

/// The capture *input* format a device delivers, i.e. a V4L2 pixel format or
/// compressed codec. Selecting one pins it on the source caps in front of
/// `decodebin`; `None` lets the pipeline auto-negotiate (the default). Raw 10-bit
/// `P010` stays 10-bit end to end and drives the renderer's HDR path (BT.2020 +
/// PQ decode, tone-mapped to the display); every other codec goes through the
/// 8-bit NV12 SDR path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureCodec {
    /// Motion-JPEG (`image/jpeg`, V4L2 `MJPG`).
    Mjpeg,
    /// H.264 (`video/x-h264`, V4L2 `H264`).
    H264,
    /// Packed 4:2:2 YUYV (V4L2 `YUYV`, GStreamer `YUY2`).
    Yuyv,
    /// Semi-planar 8-bit 4:2:0 (V4L2/GStreamer `NV12`).
    Nv12,
    /// Planar 8-bit 4:2:0 (V4L2 `YU12`, GStreamer `I420`).
    I420,
    /// Semi-planar 10-bit 4:2:0 (V4L2 `P010`, GStreamer `P010_10LE`).
    P010,
    /// Packed 24-bit BGR (V4L2 `BGR3`, GStreamer `BGR`).
    Bgr,
}

impl CaptureCodec {
    /// Codecs in a stable display order, used to filter down to what a device offers.
    const ORDER: [CaptureCodec; 7] = [
        CaptureCodec::Mjpeg,
        CaptureCodec::H264,
        CaptureCodec::Yuyv,
        CaptureCodec::Nv12,
        CaptureCodec::I420,
        CaptureCodec::P010,
        CaptureCodec::Bgr,
    ];

    /// The leading GStreamer source caps this codec pins in front of `decodebin`
    /// (media type plus, for raw formats, the exact `format=`). Width/height/
    /// framerate are appended by the pipeline builder.
    pub fn source_caps(&self) -> &'static str {
        match self {
            CaptureCodec::Mjpeg => "image/jpeg",
            CaptureCodec::H264 => "video/x-h264",
            CaptureCodec::Yuyv => "video/x-raw,format=YUY2",
            CaptureCodec::Nv12 => "video/x-raw,format=NV12",
            CaptureCodec::I420 => "video/x-raw,format=I420",
            CaptureCodec::P010 => "video/x-raw,format=P010_10LE",
            CaptureCodec::Bgr => "video/x-raw,format=BGR",
        }
    }

    /// A short human label for the menu.
    pub fn label(&self) -> &'static str {
        match self {
            CaptureCodec::Mjpeg => "MJPEG",
            CaptureCodec::H264 => "H.264",
            CaptureCodec::Yuyv => "YUYV",
            CaptureCodec::Nv12 => "NV12",
            CaptureCodec::I420 => "I420",
            CaptureCodec::P010 => "P010",
            CaptureCodec::Bgr => "BGR",
        }
    }

    /// Map a V4L2 fourcc (e.g. `b"NV12"`) to a codec, if recognized.
    pub fn from_fourcc(fourcc: [u8; 4]) -> Option<Self> {
        match &fourcc {
            b"MJPG" => Some(CaptureCodec::Mjpeg),
            b"H264" => Some(CaptureCodec::H264),
            b"YUYV" => Some(CaptureCodec::Yuyv),
            b"NV12" => Some(CaptureCodec::Nv12),
            b"YU12" => Some(CaptureCodec::I420),
            b"P010" => Some(CaptureCodec::P010),
            b"BGR3" => Some(CaptureCodec::Bgr),
            _ => None,
        }
    }
}

/// A capture format the device advertises: a codec at a fixed resolution with the
/// framerates it supports there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureFormat {
    pub codec: CaptureCodec,
    pub width: u32,
    pub height: u32,
    /// Supported framerates in fps, sorted descending, deduped.
    pub framerates: Vec<u32>,
}

/// The distinct codecs offered across `formats`, in stable display order.
pub fn codecs(formats: &[CaptureFormat]) -> Vec<CaptureCodec> {
    CaptureCodec::ORDER
        .into_iter()
        .filter(|c| formats.iter().any(|f| f.codec == *c))
        .collect()
}

/// The distinct resolutions offered for `codec`, sorted descending by pixel count.
pub fn resolutions(formats: &[CaptureFormat], codec: CaptureCodec) -> Vec<(u32, u32)> {
    let mut res: Vec<(u32, u32)> = formats
        .iter()
        .filter(|f| f.codec == codec)
        .map(|f| (f.width, f.height))
        .collect();
    res.sort_unstable_by(|a, b| (b.0 * b.1).cmp(&(a.0 * a.1)).then(b.cmp(a)));
    res.dedup();
    res
}

/// The framerates offered for `codec` at `(width, height)`, sorted descending.
pub fn framerates(formats: &[CaptureFormat], codec: CaptureCodec, res: (u32, u32)) -> Vec<u32> {
    formats
        .iter()
        .find(|f| f.codec == codec && (f.width, f.height) == res)
        .map(|f| f.framerates.clone())
        .unwrap_or_default()
}

/// A capture device the user can select.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureDevice {
    /// Human-readable name, e.g. "USB3 HDMI Capture".
    pub name: String,
    /// The V4L2 device node, e.g. `/dev/video0`.
    pub node: String,
    /// A stringified summary of the caps the device advertises, if available.
    pub caps: Option<String>,
    /// Structured capture formats read from V4L2 (best-effort; empty for a
    /// non-capture node or a device we lack permission to open).
    pub formats: Vec<CaptureFormat>,
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
            node: node.clone(),
            caps: device.caps().as_ref().map(|c| c.to_string()),
            formats: enumerate_formats(&node),
        });
    }
    Ok(out)
}

/// Read a device's supported [`CaptureFormat`]s directly from V4L2.
///
/// For each recognized pixel format we enumerate its discrete frame sizes and,
/// per size, its frame intervals (rounded to whole fps). Stepwise/continuous
/// ranges — rare on capture cards — collapse to their maximum. Any error (a
/// non-capture node, a busy device, missing permissions) yields an empty list so
/// the device is still selectable, just without format choices.
fn enumerate_formats(node: &str) -> Vec<CaptureFormat> {
    let Ok(dev) = v4l::Device::with_path(node) else {
        return Vec::new();
    };
    let Ok(descriptions) = dev.enum_formats() else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for desc in descriptions {
        let Some(codec) = CaptureCodec::from_fourcc(desc.fourcc.repr) else {
            continue;
        };
        let Ok(sizes) = dev.enum_framesizes(desc.fourcc) else {
            continue;
        };
        for size in sizes {
            let (width, height) = match size.size {
                FrameSizeEnum::Discrete(d) => (d.width, d.height),
                FrameSizeEnum::Stepwise(s) => (s.max_width, s.max_height),
            };
            let mut framerates: Vec<u32> = dev
                .enum_frameintervals(desc.fourcc, width, height)
                .unwrap_or_default()
                .iter()
                .filter_map(interval_to_fps)
                .collect();
            framerates.sort_unstable_by(|a, b| b.cmp(a));
            framerates.dedup();
            out.push(CaptureFormat {
                codec,
                width,
                height,
                framerates,
            });
        }
    }
    out
}

/// The whole-fps rate of a frame interval (`fps = 1 / interval`); stepwise ranges
/// collapse to their fastest (minimum interval / maximum rate).
fn interval_to_fps(iv: &v4l::frameinterval::FrameInterval) -> Option<u32> {
    let fraction_fps = |num: u32, den: u32| -> Option<u32> {
        if num == 0 {
            return None;
        }
        let fps = (den as f64 / num as f64).round();
        (fps >= 1.0).then_some(fps as u32)
    };
    match &iv.interval {
        FrameIntervalEnum::Discrete(f) => fraction_fps(f.numerator, f.denominator),
        // Fastest rate = smallest interval = `min`.
        FrameIntervalEnum::Stepwise(s) => fraction_fps(s.min.numerator, s.min.denominator),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourcc_and_source_caps_round_trip() {
        // V4L2 fourcc → codec → GStreamer source caps for the raw formats a
        // typical capture card exposes, including 10-bit P010.
        assert_eq!(
            CaptureCodec::from_fourcc(*b"NV12"),
            Some(CaptureCodec::Nv12)
        );
        assert_eq!(
            CaptureCodec::from_fourcc(*b"P010"),
            Some(CaptureCodec::P010)
        );
        assert_eq!(
            CaptureCodec::from_fourcc(*b"YU12"),
            Some(CaptureCodec::I420)
        );
        assert_eq!(
            CaptureCodec::from_fourcc(*b"MJPG"),
            Some(CaptureCodec::Mjpeg)
        );
        assert_eq!(CaptureCodec::from_fourcc(*b"XXXX"), None);

        assert_eq!(CaptureCodec::Nv12.source_caps(), "video/x-raw,format=NV12");
        assert_eq!(
            CaptureCodec::P010.source_caps(),
            "video/x-raw,format=P010_10LE"
        );
        assert_eq!(CaptureCodec::Mjpeg.source_caps(), "image/jpeg");
    }

    #[test]
    fn helpers_group_by_codec_in_stable_order() {
        let formats = vec![
            CaptureFormat {
                codec: CaptureCodec::Mjpeg,
                width: 1920,
                height: 1080,
                framerates: vec![60, 30],
            },
            CaptureFormat {
                codec: CaptureCodec::Nv12,
                width: 1280,
                height: 720,
                framerates: vec![60, 30],
            },
            CaptureFormat {
                codec: CaptureCodec::P010,
                width: 1920,
                height: 1080,
                framerates: vec![60, 30, 25],
            },
            CaptureFormat {
                codec: CaptureCodec::P010,
                width: 1280,
                height: 720,
                framerates: vec![60],
            },
        ];

        // Codecs come back in ORDER (Mjpeg before the raw formats), not input order.
        assert_eq!(
            codecs(&formats),
            vec![CaptureCodec::Mjpeg, CaptureCodec::Nv12, CaptureCodec::P010]
        );
        // P010 resolutions, largest first.
        assert_eq!(
            resolutions(&formats, CaptureCodec::P010),
            vec![(1920, 1080), (1280, 720)]
        );
        assert_eq!(
            framerates(&formats, CaptureCodec::P010, (1920, 1080)),
            vec![60, 30, 25]
        );
    }
}
