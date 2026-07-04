//! The single shared capture pipeline.

use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;

use crate::device::CaptureCodec;
use crate::error::MediaError;
use crate::frame::VideoFrame;
use crate::init_gstreamer;

/// Parameters for building the capture pipeline.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// The V4L2 device node to capture from, e.g. `/dev/video0`.
    pub device_node: String,
    /// Capture input format to request from the device; `None` = let `decodebin`
    /// auto-negotiate. When set, width/height/framerate are negotiated at the
    /// source (before decode) rather than scaled afterwards.
    pub codec: Option<CaptureCodec>,
    /// Preferred capture width; `None` = device default.
    pub width: Option<u32>,
    /// Preferred capture height.
    pub height: Option<u32>,
    /// Preferred framerate in fps.
    pub framerate: Option<u32>,
    /// Whether to include the PipeWire audio branch.
    pub audio: bool,
}

impl PipelineConfig {
    /// A config for `device_node` with audio on and no format overrides.
    pub fn new(device_node: impl Into<String>) -> Self {
        Self {
            device_node: device_node.into(),
            codec: None,
            width: None,
            height: None,
            framerate: None,
            audio: true,
        }
    }
}

/// A live capture pipeline. Video is delivered as [`VideoFrame`]s through a
/// callback registered with [`CapturePipeline::set_frame_callback`]; audio (if
/// enabled) plays straight to the default output. Stops itself on drop.
pub struct CapturePipeline {
    pipeline: gst::Pipeline,
    appsink: gst_app::AppSink,
    has_audio: bool,
    /// Keeps the bus watch alive — dropping the guard removes the watch.
    bus_guard: Option<gst::bus::BusWatchGuard>,
}

impl CapturePipeline {
    /// Build the pipeline for `config`. If audio is requested but the audio
    /// branch cannot be constructed (e.g. no PipeWire), it falls back to a
    /// video-only pipeline rather than failing outright.
    pub fn new(config: &PipelineConfig) -> Result<Self, MediaError> {
        init_gstreamer()?;
        match Self::build(config, config.audio) {
            Ok(pipeline) => Ok(pipeline),
            Err(e) if config.audio => {
                tracing::warn!("pipeline with audio failed ({e}); retrying video-only");
                Self::build(config, false)
            }
            Err(e) => Err(e),
        }
    }

    fn build(config: &PipelineConfig, with_audio: bool) -> Result<Self, MediaError> {
        let description = build_description(config, with_audio);
        tracing::debug!(%description, "building capture pipeline");

        let element = gst::parse::launch(&description)?;
        let pipeline = element
            .downcast::<gst::Pipeline>()
            .map_err(|_| MediaError::NotAPipeline)?;

        let sink = pipeline
            .by_name("videosink")
            .ok_or(MediaError::MissingElement("videosink"))?;
        let appsink = sink
            .downcast::<gst_app::AppSink>()
            .map_err(|_| MediaError::MissingElement("appsink"))?;

        Ok(Self {
            pipeline,
            appsink,
            has_audio: with_audio,
            bus_guard: None,
        })
    }

    /// Register a callback fired on the GStreamer streaming thread for each
    /// decoded frame. Replaces the old `gdk::Paintable`: the app wires this to a
    /// single-slot mailbox + an event-loop wakeup, then imports/uploads the
    /// frame on its own (GPU-owning) thread.
    pub fn set_frame_callback(&self, cb: impl Fn(VideoFrame) + Send + 'static) {
        self.appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    match VideoFrame::from_sample(&sample) {
                        Ok(frame) => cb(frame),
                        Err(e) => tracing::warn!("failed to extract frame: {e}"),
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
    }

    /// Whether the audio branch is present in this pipeline.
    pub fn has_audio(&self) -> bool {
        self.has_audio
    }

    /// The underlying pipeline, e.g. to attach a bus watch for error/EOS
    /// handling from the app.
    pub fn pipeline(&self) -> &gst::Pipeline {
        &self.pipeline
    }

    /// The pipeline bus, for watching error/EOS messages.
    pub fn bus(&self) -> Option<gst::Bus> {
        self.pipeline.bus()
    }

    /// Start playback.
    pub fn start(&self) -> Result<(), MediaError> {
        self.pipeline.set_state(gst::State::Playing)?;
        Ok(())
    }

    /// Pause playback.
    pub fn pause(&self) -> Result<(), MediaError> {
        self.pipeline.set_state(gst::State::Paused)?;
        Ok(())
    }

    /// Stop and tear down the pipeline.
    pub fn stop(&self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }

    /// Spawn a background thread that polls the pipeline bus and logs errors,
    /// warnings, and EOS. Unlike [`install_bus_logger`](Self::install_bus_logger)
    /// this needs no glib main loop, so it fits the winit/wgpu app whose main
    /// thread runs the render loop. The thread lives for the process.
    pub fn spawn_bus_logger(&self) {
        let Some(bus) = self.pipeline.bus() else {
            return;
        };
        let _ = std::thread::Builder::new()
            .name("couchcast-gst-bus".into())
            .spawn(move || {
                use gst::MessageView;
                loop {
                    let Some(msg) = bus.timed_pop(gst::ClockTime::from_seconds(1)) else {
                        continue;
                    };
                    match msg.view() {
                        MessageView::Error(e) => tracing::error!(
                            "pipeline error from {:?}: {} ({:?})",
                            e.src().map(|s| s.path_string()),
                            e.error(),
                            e.debug()
                        ),
                        MessageView::Warning(w) => {
                            tracing::warn!("pipeline warning: {} ({:?})", w.error(), w.debug())
                        }
                        MessageView::Eos(_) => tracing::info!("pipeline reached end of stream"),
                        _ => {}
                    }
                }
            });
    }

    /// Install a bus watch that logs pipeline errors, warnings, and EOS. Must be
    /// called from the thread running a glib main loop (the dedicated bus
    /// thread). The watch lives until the pipeline is dropped.
    pub fn install_bus_logger(&mut self) -> Result<(), MediaError> {
        let Some(bus) = self.pipeline.bus() else {
            return Ok(());
        };
        let guard = bus.add_watch_local(move |_bus, msg| {
            use gst::MessageView;
            match msg.view() {
                MessageView::Error(e) => {
                    tracing::error!(
                        "pipeline error from {:?}: {} ({:?})",
                        e.src().map(|s| s.path_string()),
                        e.error(),
                        e.debug()
                    );
                }
                MessageView::Warning(w) => {
                    tracing::warn!("pipeline warning: {} ({:?})", w.error(), w.debug());
                }
                MessageView::Eos(_) => tracing::info!("pipeline reached end of stream"),
                _ => {}
            }
            gst::glib::ControlFlow::Continue
        })?;
        self.bus_guard = Some(guard);
        Ok(())
    }
}

impl Drop for CapturePipeline {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Assemble the `gst-launch`-style pipeline description.
///
/// Latency knobs matter here: `decodebin` auto-plugs whatever cheap dongles
/// expose (MJPEG / H.264 / raw YUYV); a short `leaky=downstream` queue plus the
/// `appsink`'s `max-buffers=1 drop=true` collapse GStreamer's default buffering
/// toward one-frame latency. `sync=true` releases each buffer at its PTS against
/// the shared pipeline clock, keeping video aligned to the clock-synced audio.
///
/// The video branch converts to NV12 in system memory for the sysmem upload
/// path. `TODO`: a DMABUF branch (no `videoconvert`, VA decoder) for zero-copy;
/// select the audio source node explicitly instead of `pipewiresrc`'s default.
/// Append the `,width=…,height=…,framerate=…/1` caps fragments present in `config`.
fn append_dims(desc: &mut String, config: &PipelineConfig) {
    if let Some(w) = config.width {
        desc.push_str(&format!(",width={w}"));
    }
    if let Some(h) = config.height {
        desc.push_str(&format!(",height={h}"));
    }
    if let Some(fps) = config.framerate {
        desc.push_str(&format!(",framerate={fps}/1"));
    }
}

fn build_description(config: &PipelineConfig, with_audio: bool) -> String {
    let mut desc = String::new();

    // --- Video branch ---
    // Dev hook: COUCHCAST_TEST_SOURCE swaps the capture device for a synthetic
    // test pattern, so the appsink → upload → render path can be exercised on a
    // machine with no capture hardware. The synthetic source has no real device
    // caps, so codec selection does not apply to it.
    let is_test_source = std::env::var_os("COUCHCAST_TEST_SOURCE").is_some();
    let codec = if is_test_source { None } else { config.codec };

    if is_test_source {
        desc.push_str("videotestsrc is-live=true ! videoconvert ! ");
    } else {
        desc.push_str(&format!(
            "v4l2src device={} do-timestamp=true ! ",
            config.device_node
        ));
        // With a codec chosen, pin the capture mode (codec + resolution +
        // framerate) directly on the device caps so v4l2src negotiates it at the
        // source; decodebin then decodes that exact stream.
        if let Some(codec) = codec {
            desc.push_str(codec.source_caps());
            append_dims(&mut desc, config);
            desc.push_str(" ! ");
        }
        desc.push_str("decodebin ! videoconvert ! ");
    }

    // Without a source-pinned codec, apply any resolution/framerate override by
    // scaling after decode (the historical path, also used by the test source).
    if codec.is_none()
        && (config.width.is_some() || config.height.is_some() || config.framerate.is_some())
    {
        desc.push_str("videoscale ! videorate ! ");
    }

    desc.push_str(&format!(
        "video/x-raw,format={}",
        crate::frame::NEGOTIATED_FORMAT
    ));
    if codec.is_none() {
        append_dims(&mut desc, config);
    }

    desc.push_str(
        " ! queue leaky=downstream max-size-buffers=3 max-size-time=0 max-size-bytes=0 ! \
         appsink name=videosink sync=true max-buffers=1 drop=true",
    );

    // --- Audio branch (independent chain in the same pipeline / clock) ---
    // The synthetic test source is video-only (no PipeWire node to attach to).
    if with_audio && std::env::var_os("COUCHCAST_TEST_SOURCE").is_none() {
        desc.push_str(
            "  pipewiresrc ! queue leaky=downstream max-size-time=20000000 ! \
             audioconvert ! audioresample ! autoaudiosink",
        );
    }

    desc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_includes_device_and_low_latency_appsink() {
        let cfg = PipelineConfig::new("/dev/video0");
        let desc = build_description(&cfg, false);
        assert!(desc.contains("v4l2src device=/dev/video0"));
        assert!(desc.contains("appsink name=videosink sync=true max-buffers=1 drop=true"));
        assert!(desc.contains("format=NV12"));
        assert!(!desc.contains("gtk4paintablesink"));
        assert!(!desc.contains("pipewiresrc"));
    }

    #[test]
    fn audio_branch_is_added_when_requested() {
        let cfg = PipelineConfig::new("/dev/video0");
        let desc = build_description(&cfg, true);
        assert!(desc.contains("pipewiresrc"));
        assert!(desc.contains("autoaudiosink"));
    }

    #[test]
    fn format_overrides_add_caps_filter() {
        let cfg = PipelineConfig {
            device_node: "/dev/video2".into(),
            codec: None,
            width: Some(1920),
            height: Some(1080),
            framerate: Some(60),
            audio: false,
        };
        let desc = build_description(&cfg, false);
        assert!(desc.contains("width=1920"));
        assert!(desc.contains("height=1080"));
        assert!(desc.contains("framerate=60/1"));
        assert!(desc.contains("videoscale"));
    }

    #[test]
    fn codec_pins_source_caps_and_skips_scaling() {
        let cfg = PipelineConfig {
            device_node: "/dev/video0".into(),
            codec: Some(CaptureCodec::Mjpeg),
            width: Some(1920),
            height: Some(1080),
            framerate: Some(60),
            audio: false,
        };
        let desc = build_description(&cfg, false);
        // Source-side caps filter in front of decodebin carries the mode.
        assert!(desc.contains("image/jpeg,width=1920,height=1080,framerate=60/1 ! decodebin"));
        // No post-decode rescale when the source is pinned.
        assert!(!desc.contains("videoscale"));
        // Output still negotiates NV12 (without dimensions).
        assert!(desc.contains("video/x-raw,format=NV12 !"));
    }

    #[test]
    fn raw_codec_pins_pixel_format_at_source() {
        // 10-bit P010 pinned at the source, converted down to NV12 for the renderer.
        let cfg = PipelineConfig {
            device_node: "/dev/video0".into(),
            codec: Some(CaptureCodec::P010),
            width: Some(1920),
            height: Some(1080),
            framerate: Some(60),
            audio: false,
        };
        let desc = build_description(&cfg, false);
        assert!(desc.contains(
            "video/x-raw,format=P010_10LE,width=1920,height=1080,framerate=60/1 ! decodebin"
        ));
        assert!(desc.contains("videoconvert ! video/x-raw,format=NV12 !"));
    }
}
