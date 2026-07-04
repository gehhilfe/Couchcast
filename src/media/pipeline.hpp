#pragma once
//! The single shared capture pipeline. Ported from `couchcast-media::pipeline`.

#include <gst/gst.h>

#include <functional>
#include <memory>
#include <optional>
#include <string>

#include "media/device.hpp"
#include "media/frame.hpp"

namespace couchcast::media {

/// Parameters for building the capture pipeline.
struct PipelineConfig {
    std::string device_node;
    std::optional<CaptureCodec> codec;
    std::optional<uint32_t> width;
    std::optional<uint32_t> height;
    std::optional<uint32_t> framerate;
    bool audio = true;
};

/// Assemble the gst-launch-style pipeline description. Exposed for tests.
std::string build_description(const PipelineConfig& config, bool with_audio);

/// A live capture pipeline. Video is delivered as VideoFrames through a callback;
/// audio (if enabled) plays straight to the default output. Stops on destruction.
class CapturePipeline {
   public:
    using FrameCallback = std::function<void(VideoFrame)>;

    ~CapturePipeline();
    CapturePipeline(const CapturePipeline&) = delete;
    CapturePipeline& operator=(const CapturePipeline&) = delete;

    /// Build the pipeline for `config`, falling back to video-only if the audio
    /// branch cannot be constructed. Returns nullptr on failure.
    static std::unique_ptr<CapturePipeline> create(const PipelineConfig& config);

    /// Register a callback fired on the GStreamer streaming thread per frame.
    void set_frame_callback(FrameCallback cb);

    bool has_audio() const { return has_audio_; }

    bool start();
    void pause();
    void stop();

    /// Spawn a background thread polling the bus, logging errors/warnings/EOS.
    void spawn_bus_logger();

   private:
    CapturePipeline() = default;
    static std::unique_ptr<CapturePipeline> build(const PipelineConfig& config,
                                                  bool with_audio);

    GstElement* pipeline_ = nullptr;
    GstElement* appsink_ = nullptr;
    bool has_audio_ = false;
    FrameCallback callback_;
};

}  // namespace couchcast::media
