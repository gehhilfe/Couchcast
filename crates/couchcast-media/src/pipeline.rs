//! The single shared capture pipeline.

use gst::prelude::*;
use gstreamer as gst;
use gtk4::gdk;

use crate::error::MediaError;
use crate::init_video_sink;

/// Parameters for building the capture pipeline.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// The V4L2 device node to capture from, e.g. `/dev/video0`.
    pub device_node: String,
    /// Preferred output width (post-decode scaling); `None` = device default.
    pub width: Option<u32>,
    /// Preferred output height.
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
            width: None,
            height: None,
            framerate: None,
            audio: true,
        }
    }
}

/// A live capture pipeline. Rendering is exposed as a [`gdk::Paintable`] the UI
/// drops into a `gtk::Picture`; audio (if enabled) plays straight to the default
/// output. Stops itself on drop.
pub struct CapturePipeline {
    pipeline: gst::Pipeline,
    paintable: gdk::Paintable,
    has_audio: bool,
    /// Keeps the bus watch alive — dropping the guard removes the watch.
    bus_guard: Option<gst::bus::BusWatchGuard>,
}

impl CapturePipeline {
    /// Build the pipeline for `config`. If audio is requested but the audio
    /// branch cannot be constructed (e.g. no PipeWire), it falls back to a
    /// video-only pipeline rather than failing outright.
    pub fn new(config: &PipelineConfig) -> Result<Self, MediaError> {
        init_video_sink()?;
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
        let paintable = sink.property::<gdk::Paintable>("paintable");

        Ok(Self {
            pipeline,
            paintable,
            has_audio: with_audio,
            bus_guard: None,
        })
    }

    /// The paintable rendering the live video. Set it on a `gtk::Picture` with
    /// `picture.set_paintable(Some(pipeline.paintable()))`.
    pub fn paintable(&self) -> &gdk::Paintable {
        &self.paintable
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

    /// Install a bus watch that logs pipeline errors, warnings, and EOS. Must be
    /// called from the thread running the glib main loop (i.e. the GTK main
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
/// expose (MJPEG / H.264 / raw YUYV); a short `leaky=downstream` queue plus
/// `sync=false` on the sink collapse GStreamer's default ~200 ms buffering
/// toward one-frame latency. The audio branch keeps default sync so the shared
/// clock preserves A/V alignment.
///
/// `TODO`: probe the device's real caps and prefer a hardware VA-API decoder
/// (`vajpegdec`/`vah264dec`) over `decodebin`'s CPU fallback; select the audio
/// source node explicitly instead of relying on `pipewiresrc`'s default (the top
/// audio footgun — see `docs/ARCHITECTURE.md`).
fn build_description(config: &PipelineConfig, with_audio: bool) -> String {
    let mut desc = String::new();

    // --- Video branch ---
    desc.push_str(&format!(
        "v4l2src device={} do-timestamp=true ! decodebin ! videoconvert ! ",
        config.device_node
    ));

    if config.width.is_some() || config.height.is_some() || config.framerate.is_some() {
        desc.push_str("videoscale ! videorate ! video/x-raw");
        if let Some(w) = config.width {
            desc.push_str(&format!(",width={w}"));
        }
        if let Some(h) = config.height {
            desc.push_str(&format!(",height={h}"));
        }
        if let Some(fps) = config.framerate {
            desc.push_str(&format!(",framerate={fps}/1"));
        }
        desc.push_str(" ! ");
    }

    desc.push_str(
        "queue leaky=downstream max-size-buffers=3 max-size-time=0 max-size-bytes=0 ! \
         gtk4paintablesink name=videosink sync=false",
    );

    // --- Audio branch (independent chain in the same pipeline / clock) ---
    if with_audio {
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
    fn description_includes_device_and_low_latency_sink() {
        let cfg = PipelineConfig::new("/dev/video0");
        let desc = build_description(&cfg, false);
        assert!(desc.contains("v4l2src device=/dev/video0"));
        assert!(desc.contains("gtk4paintablesink name=videosink sync=false"));
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
            width: Some(1920),
            height: Some(1080),
            framerate: Some(60),
            audio: false,
        };
        let desc = build_description(&cfg, false);
        assert!(desc.contains("width=1920"));
        assert!(desc.contains("height=1080"));
        assert!(desc.contains("framerate=60/1"));
    }
}
