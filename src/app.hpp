#pragma once
//! The SDL3 application: a single fullscreen window with the live capture as its
//! background and a controller-navigable ImGui menu on top. Ported from
//! `couchcast::app` (which used winit's ApplicationHandler).
//!
//! ## Input routing (the core interaction model)
//!
//! Controllers are read via SDL's gamepad events. A pressed Start + Select chord
//! toggles the settings menu. Then:
//!  * Menu open  -> pad events drive an owned selection model.
//!  * Menu closed -> each button is looked up in the editable button map and the
//!    resulting action is forwarded to the target device.

#include <memory>
#include <mutex>
#include <optional>
#include <set>
#include <string>
#include <vector>

#include "config/config.hpp"
#include "input/input.hpp"
#include "media/device.hpp"
#include "media/frame.hpp"
#include "media/pipeline.hpp"
#include "render/renderer.hpp"
#include "ui/debug_overlay.hpp"
#include "ui/menu.hpp"
#include "worker.hpp"

struct SDL_Window;
union SDL_Event;

namespace couchcast {

class App {
   public:
    App();
    ~App();

    /// Create the window/renderer, start capture, and run the main loop until quit.
    int run();

   private:
    // Window/renderer lifecycle.
    bool init_window_and_renderer();

    // Capture.
    void start_capture();
    void set_device(std::string node);
    void rebuild_pipeline();
    std::string device_name(const std::string& node) const;
    void refresh_menu_formats();
    void build_and_store(std::string node, std::string name);
    void save_config();
    void auto_connect_transport();
    void poll_transport_status();
    void drain_frame();

    // Input.
    void handle_sdl_event(const SDL_Event& ev);
    void handle_pad_event(const input::PadEvent& ev);
    std::optional<input::NavDir> current_nav_dir() const;
    void tick_nav_repeater();
    // The repeatable action (navigation / volume) currently held with the menu
    // closed, used to autorepeat held buttons to the target. nullopt otherwise.
    std::optional<transport::RemoteAction> current_repeatable_action() const;
    void tick_remote_repeater();
    void apply_menu_action(const ui::MenuAction& action);

    // Draw.
    void draw();
    std::pair<std::string, std::string> debug_capture_context() const;
    std::string debug_transport_line() const;

    // State.
    config::Config config_;
    std::vector<media::CaptureDevice> devices_;
    TransportWorker worker_;
    std::unique_ptr<media::CapturePipeline> pipeline_;

    std::mutex mailbox_mutex_;
    std::optional<media::VideoFrame> mailbox_;

    std::string status_ = "Press Start + Select (or F1) to open the menu.";
    bool logged_first_frame_ = false;
    // Last connection phase reflected into status_, so we only overwrite the
    // status pill on an actual transition (and never clobber capture messages).
    ConnPhase last_conn_phase_ = ConnPhase::Disconnected;

    input::InputManager input_;
    bool input_ok_ = false;
    ui::Menu menu_;
    ui::DebugOverlay debug_;
    input::NavRepeater nav_repeater_;
    input::ActionRepeater remote_repeater_;
    std::set<transport::PadButton> pressed_;
    bool chord_active_ = false;
    bool debug_chord_active_ = false;
    std::pair<float, float> stick_{0.0f, 0.0f};

    SDL_Window* window_ = nullptr;
    std::unique_ptr<render::Renderer> renderer_;
    bool running_ = true;
};

}  // namespace couchcast
