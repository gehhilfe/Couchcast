//! A runtime on-screen debug overlay.
//!
//! This began life as a compile-time "button HUD" (the `debug-input-hud` Cargo
//! feature) that only listed the controller buttons currently held. It is now a
//! always-compiled, **runtime-toggled** diagnostics panel: press **F3** (or click
//! **L3 + R3** on the controller) to show/hide it, or start it visible with
//! `COUCHCAST_DEBUG=1`.
//!
//! Beyond the original input diagnostics it surfaces everything useful for
//! debugging a capture session at a glance:
//!
//! * **Render / Capture FPS** — the two rates that matter for latency: how fast
//!   the window redraws vs. how fast decoded frames actually arrive. A capture
//!   rate that reads `0` (stalled) or well below the configured framerate points
//!   straight at the pipeline.
//! * **Frame** — the live decoded resolution and pixel format (post-negotiation),
//!   i.e. the actual picture quality reaching the GPU.
//! * **Device / Mode** — the selected capture device and the codec/resolution/
//!   framerate the pipeline was asked to negotiate.
//! * **GPU** — the wgpu adapter driving the compositor.
//! * **Transport** — the input-forwarding backend and target.
//! * **Pads / Buttons / Stick** — the Steam Input diagnostics from the old HUD.
//!
//! Keeping it a plain runtime struct (no `#[cfg]`) means the same binary ships
//! with the overlay available everywhere — invaluable when debugging on a Steam
//! Deck in Gaming Mode where a rebuild-with-features loop is impractical.

use std::collections::HashSet;
use std::time::Instant;

use couchcast_transport::PadButton;

/// Fixed display order so the buttons line doesn't reshuffle as the (unordered)
/// pressed set changes. Lists every [`PadButton`] variant.
const DISPLAY_ORDER: &[PadButton] = &[
    PadButton::DPadUp,
    PadButton::DPadDown,
    PadButton::DPadLeft,
    PadButton::DPadRight,
    PadButton::North,
    PadButton::South,
    PadButton::West,
    PadButton::East,
    PadButton::LeftBumper,
    PadButton::RightBumper,
    PadButton::LeftTrigger,
    PadButton::RightTrigger,
    PadButton::LeftStick,
    PadButton::RightStick,
    PadButton::Select,
    PadButton::Start,
    PadButton::Guide,
];

/// A smoothed frames-per-second gauge fed one timestamped tick per event.
///
/// Uses an exponential moving average so the reading is steady rather than
/// jittering frame-to-frame, and reports `0` once ticks stop arriving so a
/// stalled stream reads as stalled instead of freezing at its last rate.
#[derive(Default)]
struct FpsGauge {
    last: Option<Instant>,
    ema: f32,
}

impl FpsGauge {
    fn tick(&mut self, now: Instant) {
        if let Some(last) = self.last {
            let dt = now.duration_since(last).as_secs_f32();
            if dt > 0.0 {
                let inst = 1.0 / dt;
                self.ema = if self.ema == 0.0 {
                    inst
                } else {
                    self.ema * 0.9 + inst * 0.1
                };
            }
        }
        self.last = Some(now);
    }

    /// The current rate, or `0.0` if no tick has arrived in the last second.
    fn get(&self, now: Instant) -> f32 {
        match self.last {
            Some(last) if now.duration_since(last).as_secs_f32() < 1.0 => self.ema,
            _ => 0.0,
        }
    }
}

/// The runtime debug overlay: fed state from the app, drawn as a translucent
/// panel in the top-left via egui when [`enabled`](Self::is_enabled).
#[derive(Default)]
pub struct DebugOverlay {
    enabled: bool,

    // Input diagnostics.
    devices: Vec<String>,
    held: Vec<&'static str>,
    stick: (f32, f32),

    // Rates.
    render_fps: FpsGauge,
    capture_fps: FpsGauge,

    // Live frame quality.
    frame_dims: Option<(u32, u32)>,
    frame_format: &'static str,

    // Context, refreshed when the corresponding state changes.
    device_line: String,
    mode_line: String,
    audio: bool,
    gpu_line: String,
    transport_line: String,
}

impl DebugOverlay {
    /// Create an overlay, initially visible if `enabled` (e.g. `COUCHCAST_DEBUG`).
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            frame_format: "—",
            ..Self::default()
        }
    }

    /// Toggle visibility (F3 / L3 + R3).
    pub fn toggle(&mut self) {
        self.enabled = !self.enabled;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Refresh the connected-controllers line (call on connect/disconnect).
    pub fn set_devices(&mut self, names: &[String]) {
        self.devices = names.to_vec();
    }

    /// Refresh the held-button line from the current set of held buttons.
    pub fn update(&mut self, pressed: &HashSet<PadButton>) {
        self.held = DISPLAY_ORDER
            .iter()
            .filter(|b| pressed.contains(b))
            .map(|b| button_label(*b))
            .collect();
    }

    /// Record the current left-stick position.
    pub fn set_stick(&mut self, x: f32, y: f32) {
        self.stick = (x, y);
    }

    /// Count one redrawn frame (call once per presented frame).
    pub fn tick_render(&mut self, now: Instant) {
        self.render_fps.tick(now);
    }

    /// Record one decoded capture frame and its dimensions.
    pub fn on_capture_frame(&mut self, now: Instant, width: u32, height: u32, format: &'static str) {
        self.capture_fps.tick(now);
        self.frame_dims = Some((width, height));
        self.frame_format = format;
    }

    /// Set the selected-device and requested-mode context lines.
    pub fn set_capture_context(&mut self, device_line: String, mode_line: String, audio: bool) {
        self.device_line = device_line;
        self.mode_line = mode_line;
        self.audio = audio;
    }

    /// Set the GPU adapter summary (once, after the renderer initializes).
    pub fn set_gpu(&mut self, gpu_line: String) {
        self.gpu_line = gpu_line;
    }

    /// Set the transport (input-forwarding) summary line.
    pub fn set_transport(&mut self, transport_line: String) {
        self.transport_line = transport_line;
    }

    /// Draw the overlay if enabled. `now` drives the FPS gauges; `status` is the
    /// app's current status string.
    pub fn draw(&self, ctx: &egui::Context, now: Instant, status: &str) {
        if !self.enabled {
            return;
        }

        let pads = if self.devices.is_empty() {
            "(none — Steam is not presenting a gamepad)".to_owned()
        } else {
            self.devices.join(", ")
        };
        let buttons = if self.held.is_empty() {
            "—".to_owned()
        } else {
            self.held.join("  ")
        };
        let frame = match self.frame_dims {
            Some((w, h)) => format!("{w}×{h} {}", self.frame_format),
            None => "—".to_owned(),
        };
        let dash = |s: &str| if s.is_empty() { "—" } else { s }.to_owned();

        egui::Area::new(egui::Id::new("couchcast-debug-overlay"))
            .anchor(egui::Align2::LEFT_TOP, egui::vec2(12.0, 12.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(184))
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .corner_radius(8.0)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.colored_label(egui::Color32::LIGHT_GREEN, "Couchcast debug (F3 / L3+R3)");
                        ui.monospace(format!(
                            "Render:    {:>5.1} fps   Capture: {:>5.1} fps",
                            self.render_fps.get(now),
                            self.capture_fps.get(now),
                        ));
                        ui.monospace(format!("Frame:     {frame}"));
                        ui.monospace(format!("Device:    {}", dash(&self.device_line)));
                        ui.monospace(format!("Mode:      {}", dash(&self.mode_line)));
                        ui.monospace(format!("Audio:     {}", if self.audio { "on" } else { "off" }));
                        ui.monospace(format!("GPU:       {}", dash(&self.gpu_line)));
                        ui.monospace(format!("Transport: {}", dash(&self.transport_line)));
                        ui.monospace(format!("Pads:      {pads}"));
                        ui.monospace(format!("Buttons:   {buttons}"));
                        ui.monospace(format!(
                            "Stick:     ({:+.2}, {:+.2})",
                            self.stick.0, self.stick.1
                        ));
                        ui.monospace(format!("Status:    {status}"));
                    });
            });
    }
}

/// A short, controller-agnostic name for the buttons line. Face buttons use their
/// Xbox letters (least obvious from the enum name); everything else is a compact
/// abbreviation.
fn button_label(button: PadButton) -> &'static str {
    match button {
        PadButton::South => "A",
        PadButton::East => "B",
        PadButton::North => "Y",
        PadButton::West => "X",
        PadButton::LeftBumper => "LB",
        PadButton::RightBumper => "RB",
        PadButton::LeftTrigger => "LT",
        PadButton::RightTrigger => "RT",
        PadButton::Select => "Select",
        PadButton::Start => "Start",
        PadButton::Guide => "Guide",
        PadButton::LeftStick => "L3",
        PadButton::RightStick => "R3",
        PadButton::DPadUp => "Up",
        PadButton::DPadDown => "Down",
        PadButton::DPadLeft => "Left",
        PadButton::DPadRight => "Right",
    }
}
