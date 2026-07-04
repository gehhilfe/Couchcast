#pragma once
//! A runtime on-screen debug overlay. Ported from `couchcast::debug`.
//!
//! Toggled with F3 (or L3 + R3 on the controller), or started visible with
//! COUCHCAST_DEBUG=1. Surfaces render/capture FPS, live frame quality, the
//! selected device/mode, the GPU adapter, the transport, and controller
//! diagnostics (connected pads, held buttons, stick position).

#include <chrono>
#include <optional>
#include <set>
#include <string>
#include <vector>

#include "transport/event.hpp"

namespace couchcast::ui {

using transport::PadButton;
using Clock = std::chrono::steady_clock;
using Instant = Clock::time_point;

/// A smoothed frames-per-second gauge fed one timestamped tick per event.
class FpsGauge {
   public:
    void tick(Instant now);
    float get(Instant now) const;

   private:
    std::optional<Instant> last_;
    float ema_ = 0.0f;
};

class DebugOverlay {
   public:
    explicit DebugOverlay(bool enabled);

    void toggle() { enabled_ = !enabled_; }
    bool is_enabled() const { return enabled_; }

    void set_devices(const std::vector<std::string>& names) { devices_ = names; }
    void update(const std::set<PadButton>& pressed);
    void set_stick(float x, float y) { stick_ = {x, y}; }
    void tick_render(Instant now) { render_fps_.tick(now); }
    void on_capture_frame(Instant now, uint32_t w, uint32_t h, const char* format);
    void set_capture_context(std::string device_line, std::string mode_line, bool audio);
    void set_gpu(std::string gpu_line) { gpu_line_ = std::move(gpu_line); }
    void set_transport(std::string t) { transport_line_ = std::move(t); }

    void draw(Instant now, const std::string& status) const;

   private:
    bool enabled_ = false;
    std::vector<std::string> devices_;
    std::vector<const char*> held_;
    std::pair<float, float> stick_{0.0f, 0.0f};
    FpsGauge render_fps_;
    FpsGauge capture_fps_;
    std::optional<std::pair<uint32_t, uint32_t>> frame_dims_;
    const char* frame_format_ = "-";
    std::string device_line_;
    std::string mode_line_;
    bool audio_ = false;
    std::string gpu_line_;
    std::string transport_line_;
};

}  // namespace couchcast::ui
