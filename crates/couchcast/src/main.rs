//! Couchcast — fullscreen HDMI-capture viewer with controller input forwarding.
//!
//! The render/UI layer is a game loop: winit owns the window and event loop,
//! wgpu the GPU, and egui draws the controller-first menu over the live video
//! texture. See [`app`] for the `ApplicationHandler` and [`render`] for wgpu.

mod app;
mod hud;
mod menu;
mod render;
mod worker;

use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> anyhow::Result<()> {
    init_tracing();
    prefer_x11_under_steam();

    let event_loop = EventLoop::with_user_event().build()?;
    // We drive input polling on a fixed cadence, so wait-with-timeout rather than
    // busy-poll; per-frame redraws are requested explicitly when state changes.
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = app::App::new(&event_loop);
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// Steam decides which app receives the controller by tracking the focused
/// window over **X11** (Steam itself runs under XWayland). A native-Wayland
/// window is invisible to that tracking — and a winit Wayland window can also
/// fail to map when launched by Steam inside gamescope — so on a Wayland session
/// we prefer winit's X11 backend when Steam launched us. This is the winit
/// equivalent of the old `GDK_BACKEND=x11` shim.
///
/// A plain terminal run stays native Wayland; an explicit `WINIT_UNIX_BACKEND`
/// is always respected.
fn prefer_x11_under_steam() {
    let launched_by_steam = std::env::var_os("SteamGameId").is_some()
        || std::env::var_os("SteamOverlayGameId").is_some()
        || std::env::var_os("SteamClientLaunch").is_some();
    let on_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let backend_already_set = std::env::var_os("WINIT_UNIX_BACKEND").is_some();

    if launched_by_steam && on_wayland && !backend_already_set {
        tracing::info!(
            "Steam launch on Wayland detected; setting WINIT_UNIX_BACKEND=x11 so Steam Input can \
             track window focus and route the controller here"
        );
        // SAFETY: runs at the very top of `main`, before the event loop / any
        // other thread exists, so there is no concurrent env access.
        unsafe { std::env::set_var("WINIT_UNIX_BACKEND", "x11") };
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
