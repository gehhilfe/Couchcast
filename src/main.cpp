//! Couchcast — fullscreen HDMI-capture viewer with controller input forwarding.
//!
//! The render/UI layer is a game loop: SDL3 owns the window and event loop,
//! Vulkan the GPU, and Dear ImGui draws the controller-first menu over the live
//! video texture. Ported from the Rust winit/wgpu/egui application.

#include <SDL3/SDL.h>
#include <gst/gst.h>

#include <cstdlib>

#include "app.hpp"
#include "log.hpp"
#include "transport/backends/adb.hpp"

namespace {

/// Steam decides which app receives the controller by tracking the focused
/// window over X11 (Steam itself runs under XWayland). A native-Wayland window is
/// invisible to that tracking, so on a Wayland session we prefer SDL's X11 video
/// driver when Steam launched us. This is the SDL equivalent of the old
/// GDK_BACKEND=x11 / WINIT_UNIX_BACKEND=x11 shim.
void prefer_x11_under_steam() {
    bool launched_by_steam = std::getenv("SteamGameId") ||
                             std::getenv("SteamOverlayGameId") ||
                             std::getenv("SteamClientLaunch");
    bool on_wayland = std::getenv("WAYLAND_DISPLAY") != nullptr;
    bool driver_already_set = std::getenv("SDL_VIDEO_DRIVER") != nullptr;

    if (launched_by_steam && on_wayland && !driver_already_set) {
        CC_INFO(
            "Steam launch on Wayland detected; forcing SDL_VIDEO_DRIVER=x11 so Steam "
            "Input can track window focus and route the controller here");
        SDL_SetHint(SDL_HINT_VIDEO_DRIVER, "x11");
    }
}

}  // namespace

int main(int argc, char** argv) {
    // Mirror logs to a file first, so everything below is captured even when
    // stderr is swallowed (e.g. launched from the Steam gamescope session).
    couchcast::log::init_file_sink();

    prefer_x11_under_steam();

    // Export ANDROID_VENDOR_KEYS before anything spawns adb (and while still
    // single-threaded), so the ADB transport authenticates with the user's
    // already-paired key even when launched from the Flatpak/Steam sandbox that
    // hides ~/.android. See AdbTransport::ensure_auth_key_env.
    couchcast::transport::backends::AdbTransport::ensure_auth_key_env();

    gst_init(&argc, &argv);

    if (!SDL_Init(SDL_INIT_VIDEO)) {
        CC_ERROR("SDL_Init failed: %s", SDL_GetError());
        return 1;
    }

    int rc = 0;
    {
        couchcast::App app;
        rc = app.run();
    }

    SDL_Quit();
    return rc;
}
