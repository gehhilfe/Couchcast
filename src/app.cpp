#include "app.hpp"

#include <SDL3/SDL.h>
#include <backends/imgui_impl_sdl3.h>
#include <imgui.h>

#include <chrono>
#include <cstdlib>
#include <utility>

#include "log.hpp"

namespace couchcast {

using config::CaptureCodec;
using input::NavDir;
using input::PadEvent;
using transport::PadAxis;
using transport::PadButton;
using transport::TargetAddr;
using ui::MenuAction;
using Clock = std::chrono::steady_clock;

namespace {

media::CaptureCodec to_media_codec(config::CaptureCodec c) {
    switch (c) {
        case config::CaptureCodec::Mjpeg: return media::CaptureCodec::Mjpeg;
        case config::CaptureCodec::H264: return media::CaptureCodec::H264;
        case config::CaptureCodec::Yuyv: return media::CaptureCodec::Yuyv;
        case config::CaptureCodec::Nv12: return media::CaptureCodec::Nv12;
        case config::CaptureCodec::I420: return media::CaptureCodec::I420;
        case config::CaptureCodec::P010: return media::CaptureCodec::P010;
        case config::CaptureCodec::Bgr: return media::CaptureCodec::Bgr;
    }
    return media::CaptureCodec::Nv12;
}

config::CaptureCodec to_config_codec(media::CaptureCodec c) {
    switch (c) {
        case media::CaptureCodec::Mjpeg: return config::CaptureCodec::Mjpeg;
        case media::CaptureCodec::H264: return config::CaptureCodec::H264;
        case media::CaptureCodec::Yuyv: return config::CaptureCodec::Yuyv;
        case media::CaptureCodec::Nv12: return config::CaptureCodec::Nv12;
        case media::CaptureCodec::I420: return config::CaptureCodec::I420;
        case media::CaptureCodec::P010: return config::CaptureCodec::P010;
        case media::CaptureCodec::Bgr: return config::CaptureCodec::Bgr;
    }
    return config::CaptureCodec::Nv12;
}

const char* debug_codec_label(config::CaptureCodec c) {
    switch (c) {
        case config::CaptureCodec::Mjpeg: return "MJPEG";
        case config::CaptureCodec::H264: return "H.264";
        case config::CaptureCodec::Yuyv: return "YUYV";
        case config::CaptureCodec::Nv12: return "NV12";
        case config::CaptureCodec::I420: return "I420";
        case config::CaptureCodec::P010: return "P010";
        case config::CaptureCodec::Bgr: return "BGR";
    }
    return "auto";
}

size_t compute_device_idx(const config::Config& cfg,
                          const std::vector<media::CaptureDevice>& devices) {
    if (cfg.last_device) {
        for (size_t i = 0; i < devices.size(); ++i)
            if (devices[i].node == cfg.last_device->node) return i;
    }
    return 0;
}

size_t compute_transport_idx(const config::Config& cfg) {
    if (cfg.target) {
        for (size_t i = 0; i < ui::TRANSPORT_CHOICES.size(); ++i)
            if (ui::TRANSPORT_CHOICES[i].kind == cfg.target->transport) return i;
    }
    return 0;
}

std::string compute_address(const config::Config& cfg) {
    return cfg.target ? cfg.target->address : std::string();
}

/// Best-effort request to show Steam's on-screen keyboard.
void open_steam_osk() {
    int rc = std::system("steam steam://open/keyboard >/dev/null 2>&1 &");
    if (rc != 0)
        (void)std::system("xdg-open steam://open/keyboard >/dev/null 2>&1 &");
    CC_DEBUG("requested Steam OSK (best-effort)");
}

/// The idle overlay shown when the menu is closed: a small status pill.
void status_overlay(const std::string& status) {
    ImGuiIO& io = ImGui::GetIO();
    ImGui::SetNextWindowPos(ImVec2(16.0f, io.DisplaySize.y - 16.0f), ImGuiCond_Always,
                            ImVec2(0.0f, 1.0f));
    ImGui::SetNextWindowBgAlpha(0.63f);
    ImGuiWindowFlags flags = ImGuiWindowFlags_NoTitleBar | ImGuiWindowFlags_NoResize |
                             ImGuiWindowFlags_NoMove | ImGuiWindowFlags_NoScrollbar |
                             ImGuiWindowFlags_AlwaysAutoResize |
                             ImGuiWindowFlags_NoNav | ImGuiWindowFlags_NoInputs;
    ImGui::Begin("##couchcast-status", nullptr, flags);
    ImGui::TextUnformatted(status.c_str());
    ImGui::End();
}

}  // namespace

App::App()
    : config_(config::Config::load_or_default()),
      devices_(media::list_devices()),
      menu_(compute_device_idx(config_, devices_), compute_transport_idx(config_),
            compute_address(config_), config_.media.audio, config_.media.hdr_output),
      debug_(std::getenv("COUCHCAST_DEBUG") != nullptr) {
    input_ok_ = input_.init();
    if (input_ok_) debug_.set_devices(input_.connected_names());
}

App::~App() = default;

// --------------------------------------------------------------------------
// Window / renderer
// --------------------------------------------------------------------------
bool App::init_window_and_renderer() {
    SDL_WindowFlags flags = SDL_WINDOW_VULKAN | SDL_WINDOW_HIGH_PIXEL_DENSITY;
    int w = 1280, h = 720;
    if (std::getenv("COUCHCAST_WINDOWED") == nullptr) {
        flags |= SDL_WINDOW_FULLSCREEN;
    } else {
        flags |= SDL_WINDOW_RESIZABLE;
    }

    window_ = SDL_CreateWindow("Couchcast", w, h, flags);
    if (!window_) {
        CC_ERROR("failed to create window: %s", SDL_GetError());
        return false;
    }

    // On Wayland the compositor must send an initial configure before a swapchain
    // can be created against the surface; pump a few event cycles so SDL commits
    // the window and receives it.
    SDL_ShowWindow(window_);
    SDL_SyncWindow(window_);
    for (int i = 0; i < 5; ++i) {
        SDL_PumpEvents();
        SDL_Delay(10);
    }

    renderer_ = render::Renderer::create(window_, config_.media.hdr_output);
    if (!renderer_) {
        CC_ERROR("failed to initialize GPU");
        return false;
    }

    // ImGui context + backends.
    IMGUI_CHECKVERSION();
    ImGui::CreateContext();
    ImGuiIO& io = ImGui::GetIO();
    io.IniFilename = nullptr;  // no imgui.ini
    io.FontGlobalScale = 1.4f;  // 10-foot readability
    ImGui::StyleColorsDark();
    renderer_->init_imgui();

    const char* hdr_status = renderer_->hdr_available() ? "HDR available" : "HDR unsupported";
    debug_.set_gpu(renderer_->adapter_info() + " . " + hdr_status);
    menu_.set_hdr(renderer_->hdr_available(), renderer_->hdr_output());
    return true;
}

// --------------------------------------------------------------------------
// Capture
// --------------------------------------------------------------------------
void App::start_capture() {
    std::optional<std::string> node;
    if (config_.last_device) {
        node = config_.last_device->node;
    } else if (!devices_.empty()) {
        node = devices_.front().node;
    }

    if (node) {
        set_device(*node);
    } else if (std::getenv("COUCHCAST_TEST_SOURCE") != nullptr) {
        set_device("/dev/null");
    } else {
        status_ = "No capture device found. Connect an HDMI capture device.";
        CC_WARN("%s", status_.c_str());
    }
}

void App::set_device(std::string node) {
    std::string name = device_name(node);
    config_.last_device = config::DeviceRef{name, node};
    refresh_menu_formats();
    build_and_store(node, name);
    save_config();
}

void App::rebuild_pipeline() {
    if (config_.last_device) {
        std::string node = config_.last_device->node;
        build_and_store(node, device_name(node));
    } else if (std::getenv("COUCHCAST_TEST_SOURCE") != nullptr) {
        build_and_store("/dev/null", "test source");
    }
}

std::string App::device_name(const std::string& node) const {
    for (const auto& d : devices_)
        if (d.node == node) return d.name;
    return node;
}

void App::refresh_menu_formats() {
    std::vector<media::CaptureFormat> formats;
    if (config_.last_device) {
        for (const auto& d : devices_)
            if (d.node == config_.last_device->node) {
                formats = d.formats;
                break;
            }
    }
    std::optional<media::CaptureCodec> codec;
    if (config_.media.codec) codec = to_media_codec(*config_.media.codec);
    menu_.set_formats(formats, codec, config_.media.width, config_.media.height,
                      config_.media.framerate);

    auto [c, w, h, fps] = menu_.capture_selection();
    config_.media.codec = c ? std::optional<config::CaptureCodec>(to_config_codec(*c))
                            : std::nullopt;
    config_.media.width = w;
    config_.media.height = h;
    config_.media.framerate = fps;
}

void App::build_and_store(std::string node, std::string name) {
    media::PipelineConfig cfg;
    cfg.device_node = node;
    cfg.codec = config_.media.codec
                    ? std::optional<media::CaptureCodec>(to_media_codec(*config_.media.codec))
                    : std::nullopt;
    cfg.width = config_.media.width;
    cfg.height = config_.media.height;
    cfg.framerate = config_.media.framerate;
    cfg.audio = config_.media.audio;

    logged_first_frame_ = false;

    auto pipeline = media::CapturePipeline::create(cfg);
    if (!pipeline) {
        CC_ERROR("failed to start pipeline for %s", node.c_str());
        status_ = "Failed to open " + name;
        return;
    }
    pipeline->set_frame_callback([this](media::VideoFrame frame) {
        std::lock_guard<std::mutex> lock(mailbox_mutex_);
        mailbox_ = std::move(frame);
    });
    pipeline->spawn_bus_logger();
    if (!pipeline->start()) {
        CC_ERROR("failed to start pipeline for %s", node.c_str());
        status_ = "Failed to open " + name;
        return;
    }
    status_ = "Playing " + name;
    pipeline_ = std::move(pipeline);
}

void App::save_config() {
    if (!config_.save()) CC_WARN("failed to save config");
}

void App::auto_connect_transport() {
    if (config_.target) {
        worker_.connect(config_.target->transport, config_.target->to_target_addr());
    } else {
        worker_.connect(config::TransportKind::Log, TargetAddr::network("unset"));
    }
}

void App::poll_transport_status() {
    TransportStatus st = worker_.status();
    if (st.phase == last_conn_phase_) return;
    last_conn_phase_ = st.phase;

    // The log transport is the no-target placeholder used when nothing is
    // configured; don't surface its lifecycle in the status pill (it would
    // clobber the initial hint and read as a bogus "Connected").
    if (st.backend == "log") return;

    switch (st.phase) {
        case ConnPhase::Connecting:
            status_ = "Connecting to " + st.target + "...";
            break;
        case ConnPhase::Connected:
            status_ = "Connected to " + st.target;
            break;
        case ConnPhase::Failed:
            status_ = st.target.empty()
                          ? "Transport error: " + st.detail
                          : "Connect to " + st.target + " failed: " + st.detail;
            break;
        case ConnPhase::Disconnected:
            break;
    }
}

void App::drain_frame() {
    std::optional<media::VideoFrame> frame;
    {
        std::lock_guard<std::mutex> lock(mailbox_mutex_);
        frame = std::move(mailbox_);
        mailbox_.reset();
    }
    if (!frame || !renderer_) return;

    if (!logged_first_frame_) {
        CC_INFO("first video frame uploaded: %ux%u", frame->width(), frame->height());
        logged_first_frame_ = true;
    }
    debug_.on_capture_frame(Clock::now(), frame->width(), frame->height(),
                            media::pixel_format_label(frame->format()));
    renderer_->upload_video(*frame);
}

// --------------------------------------------------------------------------
// Input
// --------------------------------------------------------------------------
void App::handle_sdl_event(const SDL_Event& ev) {
    switch (ev.type) {
        case SDL_EVENT_QUIT:
            worker_.disconnect();
            running_ = false;
            break;
        case SDL_EVENT_KEY_DOWN: {
            if (ev.key.repeat) break;
            switch (ev.key.key) {
                case SDLK_F1: menu_.toggle_open(); break;
                case SDLK_F3: debug_.toggle(); break;
                case SDLK_ESCAPE:
                    worker_.disconnect();
                    running_ = false;
                    break;
                default: break;
            }
            break;
        }
        case SDL_EVENT_GAMEPAD_ADDED:
        case SDL_EVENT_GAMEPAD_REMOVED:
        case SDL_EVENT_GAMEPAD_BUTTON_DOWN:
        case SDL_EVENT_GAMEPAD_BUTTON_UP:
        case SDL_EVENT_GAMEPAD_AXIS_MOTION: {
            if (!input_ok_) break;
            for (const auto& pe : input_.handle_event(ev)) handle_pad_event(pe);
            break;
        }
        default:
            break;
    }
}

void App::handle_pad_event(const PadEvent& ev) {
    switch (ev.kind) {
        case PadEvent::Kind::Button:
            if (ev.pressed)
                pressed_.insert(ev.button);
            else
                pressed_.erase(ev.button);
            debug_.update(pressed_);
            break;
        case PadEvent::Kind::Axis:
            if (ev.axis == PadAxis::LeftStickX) stick_.first = ev.value;
            else if (ev.axis == PadAxis::LeftStickY) stick_.second = ev.value;
            break;
        case PadEvent::Kind::Connected:
        case PadEvent::Kind::Disconnected:
            if (input_ok_) debug_.set_devices(input_.connected_names());
            break;
    }
    debug_.set_stick(stick_.first, stick_.second);

    // Start + Select toggles the menu (edge-triggered).
    bool chord = pressed_.count(PadButton::Start) && pressed_.count(PadButton::Select);
    if (chord && !chord_active_) {
        chord_active_ = true;
        menu_.toggle_open();
        return;
    }
    if (!chord) chord_active_ = false;

    // L3 + R3 toggles the debug overlay (edge-triggered).
    bool debug_chord =
        pressed_.count(PadButton::LeftStick) && pressed_.count(PadButton::RightStick);
    if (debug_chord && !debug_chord_active_) {
        debug_chord_active_ = true;
        debug_.toggle();
        return;
    }
    if (!debug_chord) debug_chord_active_ = false;

    // Edge-triggered actions on button press.
    if (ev.kind != PadEvent::Kind::Button || !ev.pressed) return;

    if (menu_.open) {
        if (ev.button == PadButton::South) {
            bool was_editing = menu_.editing_address;
            MenuAction action = menu_.activate();
            if (!was_editing && menu_.editing_address) open_steam_osk();
            apply_menu_action(action);
        } else if (ev.button == PadButton::East) {
            apply_menu_action(menu_.back());
        }
    } else if (const auto* action = config_.mapping.action_for(ev.button)) {
        worker_.send(*action);
    }
}

std::optional<NavDir> App::current_nav_dir() const {
    if (pressed_.count(PadButton::DPadUp)) return NavDir::Up;
    if (pressed_.count(PadButton::DPadDown)) return NavDir::Down;
    if (pressed_.count(PadButton::DPadLeft)) return NavDir::Left;
    if (pressed_.count(PadButton::DPadRight)) return NavDir::Right;
    return input::stick_to_nav(stick_.first, stick_.second);
}

void App::tick_nav_repeater() {
    std::optional<NavDir> desired;
    if (menu_.open && !menu_.editing_address) desired = current_nav_dir();
    if (auto dir = nav_repeater_.tick(Clock::now(), desired)) {
        apply_menu_action(menu_.nav(*dir, devices_.size()));
    }
}

namespace {
// Only directional and volume actions autorepeat when held — repeating Select,
// Back, Home, or a media/power key would fire it many times on a single hold.
bool is_repeatable(const transport::RemoteAction& a) {
    using K = transport::RemoteAction::Kind;
    switch (a.kind) {
        case K::Navigate:
        case K::VolumeUp:
        case K::VolumeDown:
            return true;
        default:
            return false;
    }
}
}  // namespace

std::optional<transport::RemoteAction> App::current_repeatable_action() const {
    // pressed_ iterates in PadButton enum order, so a held direction resolves
    // deterministically when several buttons are down.
    for (PadButton button : pressed_) {
        if (const auto* action = config_.mapping.action_for(button)) {
            if (is_repeatable(*action)) return *action;
        }
    }
    return std::nullopt;
}

void App::tick_remote_repeater() {
    // Menu-closed only: the menu has its own cursor repeater (tick_nav_repeater).
    // The initial press was already forwarded edge-triggered in handle_pad_event;
    // this adds the held repeats.
    std::optional<transport::RemoteAction> held;
    if (!menu_.open) held = current_repeatable_action();
    if (auto action = remote_repeater_.tick(Clock::now(), held)) {
        worker_.send(*action);
    }
}

void App::apply_menu_action(const ui::MenuAction& action) {
    using K = ui::MenuAction::Kind;
    switch (action.kind) {
        case K::None:
            break;
        case K::SelectDevice:
            if (action.device_index < devices_.size())
                set_device(devices_[action.device_index].node);
            break;
        case K::SetCapture:
            config_.media.codec =
                action.codec ? std::optional<config::CaptureCodec>(to_config_codec(*action.codec))
                             : std::nullopt;
            config_.media.width = action.width;
            config_.media.height = action.height;
            config_.media.framerate = action.framerate;
            save_config();
            rebuild_pipeline();
            break;
        case K::SetAudio:
            config_.media.audio = action.on;
            save_config();
            rebuild_pipeline();
            break;
        case K::SetHdrOutput:
            config_.media.hdr_output = action.on;
            save_config();
            if (renderer_) {
                renderer_->set_hdr_output(action.on);
                menu_.set_hdr(renderer_->hdr_available(), renderer_->hdr_output());
            }
            break;
        case K::Connect: {
            config::TransportKind kind = menu_.selected_transport();
            std::string address = menu_.address;
            config_.target = config::TargetConfig{kind, address};
            save_config();
            // The worker publishes Connecting/Connected/Failed; poll_transport_status()
            // reflects it into status_ each frame (no manual status here).
            worker_.connect(kind, TargetAddr::network(address));
            break;
        }
        case K::Close:
            menu_.open = false;
            break;
    }
}

// --------------------------------------------------------------------------
// Draw
// --------------------------------------------------------------------------
std::pair<std::string, std::string> App::debug_capture_context() const {
    std::string device_line = "(none)";
    if (config_.last_device)
        device_line = config_.last_device->name + " (" + config_.last_device->node + ")";

    const auto& m = config_.media;
    const char* codec = m.codec ? debug_codec_label(*m.codec) : "auto";
    auto dim = [](std::optional<uint32_t> v) {
        return v ? std::to_string(*v) : std::string("auto");
    };
    std::string fps = m.framerate ? ("@" + std::to_string(*m.framerate)) : std::string();
    std::string mode_line =
        std::string(codec) + " " + dim(m.width) + "x" + dim(m.height) + fps;
    return {device_line, mode_line};
}

std::string App::debug_transport_line() const {
    if (config_.target) {
        return std::string(config::to_string(config_.target->transport)) + " -> " +
               config_.target->address;
    }
    return "Log -> (unset)";
}

void App::draw() {
    auto now = Clock::now();
    poll_transport_status();
    debug_.tick_render(now);
    if (debug_.is_enabled()) {
        auto [device_line, mode_line] = debug_capture_context();
        debug_.set_capture_context(device_line, mode_line, config_.media.audio);
        debug_.set_transport(debug_transport_line());
    }

    renderer_->new_frame();
    if (menu_.open)
        menu_.draw(devices_, status_);
    else
        status_overlay(status_);
    debug_.draw(now, status_);
    ImGui::Render();
    renderer_->render();
}

// --------------------------------------------------------------------------
// Main loop
// --------------------------------------------------------------------------
int App::run() {
    if (!init_window_and_renderer()) return 1;

    start_capture();
    auto_connect_transport();

    // Test hook: exit cleanly after N rendered frames (for headless smoke tests).
    long max_frames = -1;
    if (const char* mf = std::getenv("COUCHCAST_MAX_FRAMES")) max_frames = std::atol(mf);
    long frame_count = 0;

    while (running_) {
        SDL_Event ev;
        while (SDL_PollEvent(&ev)) {
            ImGui_ImplSDL3_ProcessEvent(&ev);
            handle_sdl_event(ev);
        }
        if (!running_) break;

        tick_nav_repeater();
        tick_remote_repeater();

        if (SDL_GetWindowFlags(window_) & SDL_WINDOW_MINIMIZED) {
            SDL_Delay(16);
            continue;
        }

        drain_frame();
        draw();

        if (max_frames >= 0 && ++frame_count >= max_frames) {
            CC_INFO("reached COUCHCAST_MAX_FRAMES=%ld; exiting", max_frames);
            running_ = false;
        }
    }

    if (renderer_) renderer_.reset();  // shuts down the ImGui backends
    ImGui::DestroyContext();
    if (window_) SDL_DestroyWindow(window_);
    return 0;
}

}  // namespace couchcast
