#include "media/pipeline.hpp"

#include <gst/app/gstappsink.h>

#include <cstdlib>
#include <thread>

#include "log.hpp"

namespace couchcast::media {

namespace {

void append_dims(std::string& desc, const PipelineConfig& config) {
    if (config.width) desc += ",width=" + std::to_string(*config.width);
    if (config.height) desc += ",height=" + std::to_string(*config.height);
    if (config.framerate) desc += ",framerate=" + std::to_string(*config.framerate) + "/1";
}

bool test_source_env() { return std::getenv("COUCHCAST_TEST_SOURCE") != nullptr; }

}  // namespace

std::string build_description(const PipelineConfig& config, bool with_audio) {
    std::string desc;

    // Dev hook: COUCHCAST_TEST_SOURCE swaps the capture device for a synthetic
    // test pattern so the appsink -> upload -> render path can be exercised
    // without capture hardware.
    bool is_test = test_source_env();
    std::optional<CaptureCodec> codec = is_test ? std::nullopt : config.codec;

    if (is_test) {
        desc += "videotestsrc is-live=true ! videoconvert ! ";
    } else {
        desc += "v4l2src device=" + config.device_node + " do-timestamp=true ! ";
        if (codec) {
            desc += source_caps(*codec);
            append_dims(desc, config);
            desc += " ! ";
        }
        desc += "decodebin ! videoconvert ! ";
    }

    if (!codec && (config.width || config.height || config.framerate)) {
        desc += "videoscale ! videorate ! ";
    }

    desc += "video/x-raw,format=";
    desc += negotiated_format(codec);
    if (!codec) append_dims(desc, config);

    // sync=false: this is a live "newest frame wins" viewer. We keep only the
    // latest buffer (max-buffers=1 drop=true) and present on the render loop's
    // own vsync, so the sink must NOT clock-synchronize to PTS. Clock-syncing
    // pulls in base-sink QoS, which drops "late" frames — and the plane copy on
    // the streaming thread reads as late, so QoS collapses the capture rate
    // (25->12.5, 60->8) the harder the requested framerate. sync=false disables
    // both the wait and the lateness dropping.
    desc +=
        " ! queue leaky=downstream max-size-buffers=3 max-size-time=0 "
        "max-size-bytes=0 ! appsink name=videosink sync=false max-buffers=1 drop=true";

    if (with_audio && !is_test) {
        desc +=
            "  pipewiresrc ! queue leaky=downstream max-size-time=20000000 ! "
            "audioconvert ! audioresample ! autoaudiosink";
    }

    return desc;
}

CapturePipeline::~CapturePipeline() {
    stop();
    if (appsink_) gst_object_unref(appsink_);
    if (pipeline_) gst_object_unref(pipeline_);
}

std::unique_ptr<CapturePipeline> CapturePipeline::build(const PipelineConfig& config,
                                                        bool with_audio) {
    std::string description = build_description(config, with_audio);
    CC_DEBUG("building capture pipeline: %s", description.c_str());

    GError* err = nullptr;
    GstElement* element = gst_parse_launch(description.c_str(), &err);
    if (!element || err) {
        if (err) {
            CC_ERROR("gst_parse_launch failed: %s", err->message);
            g_error_free(err);
        }
        if (element) gst_object_unref(element);
        return nullptr;
    }
    if (!GST_IS_PIPELINE(element)) {
        CC_ERROR("pipeline description did not produce a pipeline");
        gst_object_unref(element);
        return nullptr;
    }

    GstElement* sink = gst_bin_get_by_name(GST_BIN(element), "videosink");
    if (!sink) {
        CC_ERROR("pipeline is missing the videosink element");
        gst_object_unref(element);
        return nullptr;
    }

    auto pipeline = std::unique_ptr<CapturePipeline>(new CapturePipeline());
    pipeline->pipeline_ = element;
    pipeline->appsink_ = sink;  // owns the ref from gst_bin_get_by_name
    pipeline->has_audio_ = with_audio;
    return pipeline;
}

std::unique_ptr<CapturePipeline> CapturePipeline::create(const PipelineConfig& config) {
    if (!gst_is_initialized()) gst_init(nullptr, nullptr);
    auto p = build(config, config.audio);
    if (!p && config.audio) {
        CC_WARN("pipeline with audio failed; retrying video-only");
        p = build(config, false);
    }
    return p;
}

namespace {
// appsink new-sample callback: pull, wrap, and hand the frame to the app.
GstFlowReturn on_new_sample(GstAppSink* sink, gpointer user_data) {
    auto* cb = static_cast<CapturePipeline::FrameCallback*>(user_data);
    GstSample* sample = gst_app_sink_pull_sample(sink);
    if (!sample) return GST_FLOW_EOS;
    auto frame = VideoFrame::from_sample(sample);  // takes ownership of sample
    if (frame && *cb) {
        (*cb)(std::move(*frame));
    }
    return GST_FLOW_OK;
}
}  // namespace

void CapturePipeline::set_frame_callback(FrameCallback cb) {
    callback_ = std::move(cb);
    GstAppSinkCallbacks callbacks{};
    callbacks.new_sample = on_new_sample;
    gst_app_sink_set_callbacks(GST_APP_SINK(appsink_), &callbacks, &callback_, nullptr);
}

bool CapturePipeline::start() {
    return gst_element_set_state(pipeline_, GST_STATE_PLAYING) !=
           GST_STATE_CHANGE_FAILURE;
}

void CapturePipeline::pause() {
    gst_element_set_state(pipeline_, GST_STATE_PAUSED);
}

void CapturePipeline::stop() {
    if (pipeline_) gst_element_set_state(pipeline_, GST_STATE_NULL);
}

void CapturePipeline::spawn_bus_logger() {
    GstBus* bus = gst_element_get_bus(pipeline_);
    if (!bus) return;
    std::thread([bus]() {
        for (;;) {
            GstMessage* msg = gst_bus_timed_pop(bus, GST_SECOND);
            if (!msg) continue;
            switch (GST_MESSAGE_TYPE(msg)) {
                case GST_MESSAGE_ERROR: {
                    GError* e = nullptr;
                    gchar* dbg = nullptr;
                    gst_message_parse_error(msg, &e, &dbg);
                    CC_ERROR("pipeline error: %s (%s)", e ? e->message : "?",
                             dbg ? dbg : "");
                    if (e) g_error_free(e);
                    g_free(dbg);
                    break;
                }
                case GST_MESSAGE_WARNING: {
                    GError* e = nullptr;
                    gchar* dbg = nullptr;
                    gst_message_parse_warning(msg, &e, &dbg);
                    CC_WARN("pipeline warning: %s (%s)", e ? e->message : "?",
                            dbg ? dbg : "");
                    if (e) g_error_free(e);
                    g_free(dbg);
                    break;
                }
                case GST_MESSAGE_EOS:
                    CC_INFO("pipeline reached end of stream");
                    break;
                default:
                    break;
            }
            gst_message_unref(msg);
        }
    }).detach();
}

}  // namespace couchcast::media
