#pragma once
//! The controller-first settings menu: an owned selection model plus its ImGui
//! drawing. Ported from `couchcast::menu`. We deliberately bypass ImGui's own
//! focus/navigation — tracking our own `selected` index and rendering the
//! highlight ourselves gives the crisp, game-like navigation a 10-foot UI needs.

#include <array>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include "config/config.hpp"
#include "input/input.hpp"
#include "media/device.hpp"

namespace couchcast::ui {

using config::TransportKind;
using input::NavDir;
using media::CaptureCodec;
using media::CaptureDevice;
using media::CaptureFormat;

/// Transport options offered in the menu, in display order.
struct TransportChoice {
    const char* label;
    TransportKind kind;
};
extern const std::array<TransportChoice, 2> TRANSPORT_CHOICES;

/// A side effect the app should perform in response to menu input.
struct MenuAction {
    enum class Kind { None, SelectDevice, SetCapture, SetAudio, SetHdrOutput, Connect, Close };
    Kind kind = Kind::None;
    size_t device_index = 0;                  // SelectDevice
    std::optional<CaptureCodec> codec;        // SetCapture
    std::optional<uint32_t> width, height, framerate;
    bool on = false;                          // SetAudio / SetHdrOutput

    static MenuAction none() { return {}; }
};

class Menu {
   public:
    bool open = false;
    bool editing_address = false;
    std::string address;

    Menu(size_t device_idx, size_t transport_idx, std::string address, bool audio,
         bool hdr_output);

    void set_formats(std::vector<CaptureFormat> formats,
                     std::optional<CaptureCodec> codec, std::optional<uint32_t> width,
                     std::optional<uint32_t> height, std::optional<uint32_t> framerate);
    void set_hdr(bool available, bool active);

    /// The current capture selection as (codec, width, height, framerate).
    std::tuple<std::optional<CaptureCodec>, std::optional<uint32_t>,
               std::optional<uint32_t>, std::optional<uint32_t>>
    capture_selection() const;

    TransportKind selected_transport() const;
    void toggle_open();

    /// A directional nav step. Up/Down move the cursor; Left/Right adjust value.
    MenuAction nav(NavDir dir, size_t device_count);

    /// The A button: activate the selected row.
    MenuAction activate();

    /// The B button: leave edit mode if editing, else close the menu.
    MenuAction back();

    /// Draw the menu with ImGui, centered over the video.
    void draw(const std::vector<CaptureDevice>& devices, const std::string& status);

   private:
    std::optional<CaptureCodec> current_codec() const;
    std::optional<std::pair<uint32_t, uint32_t>> current_res() const;
    std::optional<uint32_t> current_fps() const;
    void recompute_res_opts();
    void recompute_fps_opts();
    void on_codec_changed();
    void on_res_changed();
    MenuAction capture_action() const;
    MenuAction cycle(int delta, size_t device_count);

    size_t selected_ = 0;
    size_t device_idx_ = 0;
    size_t transport_idx_ = 0;
    bool audio_ = true;
    bool hdr_output_ = true;
    bool hdr_available_ = false;
    bool focus_address_ = false;

    std::vector<CaptureFormat> formats_;
    std::vector<std::optional<CaptureCodec>> codec_opts_{std::nullopt};
    std::vector<std::optional<std::pair<uint32_t, uint32_t>>> res_opts_{std::nullopt};
    std::vector<std::optional<uint32_t>> fps_opts_{std::nullopt};
    size_t codec_idx_ = 0;
    size_t res_idx_ = 0;
    size_t fps_idx_ = 0;
};

}  // namespace couchcast::ui
