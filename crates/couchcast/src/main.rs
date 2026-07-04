//! Couchcast — fullscreen HDMI-capture viewer with controller input forwarding.

mod app;
mod hud;
mod worker;

use gtk4::glib;
use gtk4::prelude::*;

fn main() -> glib::ExitCode {
    init_tracing();
    prefer_x11_under_steam();
    let application = app::build_application();
    application.run()
}

/// Steam decides which app receives the controller by tracking the focused
/// window over **X11** (Steam itself runs under XWayland). A native-Wayland
/// window is invisible to that tracking, so on a Wayland session (e.g.
/// Hyprland) Steam never hands Couchcast its (virtual) gamepad and controller
/// input stays in Big Picture. When we detect a Steam launch on Wayland, prefer
/// the XWayland GDK backend so Steam can see and route to our window.
///
/// A plain terminal run stays native Wayland; an explicit `GDK_BACKEND` is
/// always respected.
fn prefer_x11_under_steam() {
    let launched_by_steam = std::env::var_os("SteamGameId").is_some()
        || std::env::var_os("SteamOverlayGameId").is_some()
        || std::env::var_os("SteamClientLaunch").is_some();
    let on_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let backend_already_set = std::env::var_os("GDK_BACKEND").is_some();

    if launched_by_steam && on_wayland && !backend_already_set {
        tracing::info!(
            "Steam launch on Wayland detected; setting GDK_BACKEND=x11 so Steam Input can \
             track window focus and route the controller here"
        );
        // SAFETY: runs at the very top of `main`, before GTK/GLib/tokio start and
        // before any other thread exists, so there is no concurrent env access.
        unsafe { std::env::set_var("GDK_BACKEND", "x11") };
    }
}

/// Configure logging. Override verbosity with `RUST_LOG`, e.g.
/// `RUST_LOG=couchcast=trace`.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,couchcast=debug"));
    fmt().with_env_filter(filter).init();
}
