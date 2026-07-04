#include "ui/debug_overlay.hpp"

#include <imgui.h>

#include <array>

namespace couchcast::ui {

namespace {
// Fixed display order so the buttons line doesn't reshuffle.
constexpr std::array<PadButton, 17> DISPLAY_ORDER = {
    PadButton::DPadUp,      PadButton::DPadDown,    PadButton::DPadLeft,
    PadButton::DPadRight,   PadButton::North,       PadButton::South,
    PadButton::West,        PadButton::East,        PadButton::LeftBumper,
    PadButton::RightBumper, PadButton::LeftTrigger, PadButton::RightTrigger,
    PadButton::LeftStick,   PadButton::RightStick,  PadButton::Select,
    PadButton::Start,       PadButton::Guide};

const char* button_label(PadButton b) {
    switch (b) {
        case PadButton::South: return "A";
        case PadButton::East: return "B";
        case PadButton::North: return "Y";
        case PadButton::West: return "X";
        case PadButton::LeftBumper: return "LB";
        case PadButton::RightBumper: return "RB";
        case PadButton::LeftTrigger: return "LT";
        case PadButton::RightTrigger: return "RT";
        case PadButton::Select: return "Select";
        case PadButton::Start: return "Start";
        case PadButton::Guide: return "Guide";
        case PadButton::LeftStick: return "L3";
        case PadButton::RightStick: return "R3";
        case PadButton::DPadUp: return "Up";
        case PadButton::DPadDown: return "Down";
        case PadButton::DPadLeft: return "Left";
        case PadButton::DPadRight: return "Right";
    }
    return "?";
}

const char* dash(const std::string& s) { return s.empty() ? "-" : s.c_str(); }
}  // namespace

void FpsGauge::tick(Instant now) {
    if (last_) {
        float dt = std::chrono::duration<float>(now - *last_).count();
        if (dt > 0.0f) {
            float inst = 1.0f / dt;
            ema_ = (ema_ == 0.0f) ? inst : ema_ * 0.9f + inst * 0.1f;
        }
    }
    last_ = now;
}

float FpsGauge::get(Instant now) const {
    if (last_ && std::chrono::duration<float>(now - *last_).count() < 1.0f) return ema_;
    return 0.0f;
}

DebugOverlay::DebugOverlay(bool enabled) : enabled_(enabled) {}

void DebugOverlay::update(const std::set<PadButton>& pressed) {
    held_.clear();
    for (PadButton b : DISPLAY_ORDER) {
        if (pressed.count(b)) held_.push_back(button_label(b));
    }
}

void DebugOverlay::on_capture_frame(Instant now, uint32_t w, uint32_t h,
                                    const char* format) {
    capture_fps_.tick(now);
    frame_dims_ = std::make_pair(w, h);
    frame_format_ = format;
}

void DebugOverlay::set_capture_context(std::string device_line, std::string mode_line,
                                       bool audio) {
    device_line_ = std::move(device_line);
    mode_line_ = std::move(mode_line);
    audio_ = audio;
}

void DebugOverlay::draw(Instant now, const std::string& status) const {
    if (!enabled_) return;

    std::string pads = devices_.empty()
                           ? "(none - Steam is not presenting a gamepad)"
                           : std::string();
    for (size_t i = 0; i < devices_.size(); ++i) {
        pads += devices_[i];
        if (i + 1 < devices_.size()) pads += ", ";
    }
    std::string buttons;
    if (held_.empty()) {
        buttons = "-";
    } else {
        for (size_t i = 0; i < held_.size(); ++i) {
            buttons += held_[i];
            if (i + 1 < held_.size()) buttons += "  ";
        }
    }
    char frame[64];
    if (frame_dims_)
        std::snprintf(frame, sizeof(frame), "%ux%u %s", frame_dims_->first,
                      frame_dims_->second, frame_format_);
    else
        std::snprintf(frame, sizeof(frame), "-");

    ImGui::SetNextWindowPos(ImVec2(12, 12), ImGuiCond_Always);
    ImGui::SetNextWindowBgAlpha(0.72f);
    ImGuiWindowFlags flags = ImGuiWindowFlags_NoTitleBar | ImGuiWindowFlags_NoResize |
                             ImGuiWindowFlags_NoMove | ImGuiWindowFlags_NoScrollbar |
                             ImGuiWindowFlags_AlwaysAutoResize |
                             ImGuiWindowFlags_NoFocusOnAppearing |
                             ImGuiWindowFlags_NoNav | ImGuiWindowFlags_NoInputs;
    ImGui::Begin("##couchcast-debug", nullptr, flags);
    ImGui::TextColored(ImVec4(0.6f, 1.0f, 0.6f, 1.0f),
                       "Couchcast debug (F3 / L3+R3)");
    ImGui::Text("Render:    %5.1f fps   Capture: %5.1f fps", render_fps_.get(now),
                capture_fps_.get(now));
    ImGui::Text("Frame:     %s", frame);
    ImGui::Text("Device:    %s", dash(device_line_));
    ImGui::Text("Mode:      %s", dash(mode_line_));
    ImGui::Text("Audio:     %s", audio_ ? "on" : "off");
    ImGui::Text("GPU:       %s", dash(gpu_line_));
    ImGui::Text("Transport: %s", dash(transport_line_));
    ImGui::Text("Pads:      %s", pads.c_str());
    ImGui::Text("Buttons:   %s", buttons.c_str());
    ImGui::Text("Stick:     (%+.2f, %+.2f)", stick_.first, stick_.second);
    ImGui::Text("Status:    %s", status.c_str());
    ImGui::End();
}

}  // namespace couchcast::ui
