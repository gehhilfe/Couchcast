//! The winit application: a single fullscreen window with the live capture as
//! its background and a controller-navigable egui menu on top.
//!
//! ## Input routing (the core interaction model)
//!
//! Controllers are polled from the winit loop via `gilrs`. A pressed
//! **Start + Select** chord toggles the settings menu. Then:
//!
//! * **Menu open** → pad events drive an owned selection model (D-pad / left
//!   stick move the highlight with hold-to-repeat, Left/Right cycles a value,
//!   A activates, B closes).
//! * **Menu closed** ("capture mode") → each button is looked up in the editable
//!   button map and the resulting action is forwarded to the target device.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use couchcast_config::{Config, DeviceRef, TargetConfig, TransportKind};
use couchcast_input::{InputManager, NavDir, NavRepeater, PadEvent, stick_to_nav};
use couchcast_media::{CaptureDevice, CapturePipeline, PipelineConfig, VideoFrame, list_devices};
use couchcast_transport::{PadAxis, PadButton, TargetAddr};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Fullscreen, Window, WindowId};

use crate::hud::ButtonHud;
use crate::menu::{Menu, MenuAction, TRANSPORT_CHOICES};
use crate::render::Renderer;
use crate::worker::TransportWorker;

/// How often the controller is polled (~120 Hz). Cheap and non-blocking.
const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(8);

/// Events posted into the winit loop from other threads.
#[derive(Debug)]
pub enum UserEvent {
    /// A new video frame is waiting in the mailbox.
    FrameReady,
}

/// A single-slot mailbox: the GStreamer streaming thread overwrites it with the
/// freshest frame; the winit thread drains it once per redraw.
type FrameMailbox = Arc<Mutex<Option<VideoFrame>>>;

/// Per-window state, created on `resumed`.
struct Active {
    window: Arc<Window>,
    renderer: Renderer,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
}

/// The application. Owns everything on the winit (main) thread.
pub struct App {
    proxy: EventLoopProxy<UserEvent>,
    active: Option<Active>,
    config: Config,
    devices: Vec<CaptureDevice>,
    worker: TransportWorker,
    pipeline: Option<CapturePipeline>,
    mailbox: FrameMailbox,
    status: String,
    logged_first_frame: bool,

    input: Option<InputManager>,
    menu: Menu,
    hud: ButtonHud,
    nav_repeater: NavRepeater,
    pressed: HashSet<PadButton>,
    chord_active: bool,
    stick: (f32, f32),
}

impl App {
    pub fn new(event_loop: &EventLoop<UserEvent>) -> Self {
        let config = Config::load_or_default();
        let devices = list_devices().unwrap_or_else(|e| {
            tracing::error!("device enumeration failed: {e}");
            Vec::new()
        });

        let device_idx = config
            .last_device
            .as_ref()
            .and_then(|d| devices.iter().position(|c| c.node == d.node))
            .unwrap_or(0);
        let (transport_idx, address) = match &config.target {
            Some(t) => (
                TRANSPORT_CHOICES
                    .iter()
                    .position(|(_, k)| *k == t.transport)
                    .unwrap_or(0),
                t.address.clone(),
            ),
            None => (0, String::new()),
        };
        let menu = Menu::new(device_idx, transport_idx, address, config.media.audio);

        let input = match InputManager::new() {
            Ok(i) => Some(i),
            Err(e) => {
                tracing::error!("controller input unavailable: {e}");
                None
            }
        };
        let mut hud = ButtonHud::new();
        if let Some(input) = &input {
            hud.set_devices(&input.connected_names());
        }

        Self {
            proxy: event_loop.create_proxy(),
            active: None,
            config,
            devices,
            worker: TransportWorker::spawn(),
            pipeline: None,
            mailbox: Arc::new(Mutex::new(None)),
            status: "Press Start + Select (or F1) to open the menu.".to_owned(),
            logged_first_frame: false,
            input,
            menu,
            hud,
            nav_repeater: NavRepeater::default(),
            pressed: HashSet::new(),
            chord_active: false,
            stick: (0.0, 0.0),
        }
    }

    /// Start the capture pipeline on the remembered device, else the first found.
    fn start_capture(&mut self) {
        let node = self
            .config
            .last_device
            .as_ref()
            .map(|d| d.node.clone())
            .or_else(|| self.devices.first().map(|d| d.node.clone()));
        match node {
            Some(node) => self.set_device(node),
            None if std::env::var_os("COUCHCAST_TEST_SOURCE").is_some() => {
                // The test source ignores the device node.
                self.set_device("/dev/null".to_owned());
            }
            None => {
                self.status = "No capture device found. Connect an HDMI capture device.".to_owned();
                tracing::warn!("{}", self.status);
            }
        }
    }

    fn set_device(&mut self, node: String) {
        let name = self
            .devices
            .iter()
            .find(|d| d.node == node)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| node.clone());

        let cfg = PipelineConfig {
            device_node: node.clone(),
            width: self.config.media.width,
            height: self.config.media.height,
            framerate: self.config.media.framerate,
            audio: self.config.media.audio,
        };

        self.logged_first_frame = false;
        match self.build_started_pipeline(&cfg) {
            Ok(pipeline) => {
                self.status = format!("Playing {name}");
                self.pipeline = Some(pipeline);
                self.config.last_device = Some(DeviceRef { name, node });
                self.save_config();
            }
            Err(e) => {
                tracing::error!("failed to start pipeline for {node}: {e}");
                self.status = format!("Failed to open {name}: {e}");
            }
        }
    }

    /// Build a pipeline for `cfg`, wire the frame callback + bus logger, and
    /// start it. If starting fails while audio is enabled (e.g. no PipeWire node
    /// to attach to), retry once video-only — the video is what matters.
    fn build_started_pipeline(
        &self,
        cfg: &PipelineConfig,
    ) -> Result<CapturePipeline, couchcast_media::MediaError> {
        let build = |cfg: &PipelineConfig| -> Result<CapturePipeline, couchcast_media::MediaError> {
            let pipeline = CapturePipeline::new(cfg)?;
            let mailbox = self.mailbox.clone();
            let proxy = self.proxy.clone();
            pipeline.set_frame_callback(move |frame| {
                if let Ok(mut slot) = mailbox.lock() {
                    *slot = Some(frame);
                }
                let _ = proxy.send_event(UserEvent::FrameReady);
            });
            pipeline.spawn_bus_logger();
            pipeline.start()?;
            Ok(pipeline)
        };

        match build(cfg) {
            Ok(p) => Ok(p),
            Err(e) if cfg.audio => {
                tracing::warn!("pipeline start with audio failed ({e}); retrying video-only");
                build(&PipelineConfig {
                    audio: false,
                    ..cfg.clone()
                })
            }
            Err(e) => Err(e),
        }
    }

    fn save_config(&self) {
        if let Err(e) = self.config.save() {
            tracing::warn!("failed to save config: {e}");
        }
    }

    /// Auto-connect the transport: to the saved target if present, else the
    /// logging transport so forwarding is observable during development.
    fn auto_connect_transport(&self) {
        match &self.config.target {
            Some(target) => self
                .worker
                .connect(target.transport, target.to_target_addr()),
            None => self
                .worker
                .connect(TransportKind::Log, TargetAddr::network("unset")),
        }
    }

    /// Drain the freshest frame (if any) and upload it to the GPU.
    fn drain_frame(&mut self) {
        let frame = self.mailbox.lock().ok().and_then(|mut s| s.take());
        if let (Some(frame), Some(active)) = (frame, self.active.as_mut()) {
            if !self.logged_first_frame {
                tracing::info!(
                    width = frame.width(),
                    height = frame.height(),
                    "first video frame uploaded"
                );
                self.logged_first_frame = true;
            }
            active.renderer.upload_video(&frame);
        }
    }

    // ----------------------------------------------------------------------
    // Input
    // ----------------------------------------------------------------------

    fn poll_input(&mut self) {
        let events = match &mut self.input {
            Some(input) => input.poll(),
            None => return,
        };
        for event in events {
            self.handle_pad_event(event);
        }

        // Directional navigation is state/time-based (hold-to-repeat), driven off
        // the held D-pad or left stick — but only while the menu is open and not
        // in text-edit mode.
        let desired = if self.menu.open && !self.menu.editing_address {
            self.current_nav_dir()
        } else {
            None
        };
        if let Some(dir) = self.nav_repeater.tick(Instant::now(), desired) {
            let action = self.menu.nav(dir, self.devices.len());
            self.apply_menu_action(action);
        }
    }

    fn handle_pad_event(&mut self, event: PadEvent) {
        match &event {
            PadEvent::Button { button, pressed } => {
                if *pressed {
                    self.pressed.insert(*button);
                } else {
                    self.pressed.remove(button);
                }
                self.hud.update(&self.pressed);
            }
            PadEvent::Axis { axis, value } => match axis {
                PadAxis::LeftStickX => self.stick.0 = *value,
                PadAxis::LeftStickY => self.stick.1 = *value,
                _ => {}
            },
            PadEvent::Connected { .. } | PadEvent::Disconnected { .. } => {
                if let Some(input) = &self.input {
                    self.hud.set_devices(&input.connected_names());
                }
            }
        }

        // Start + Select toggles the menu (edge-triggered).
        let chord =
            self.pressed.contains(&PadButton::Start) && self.pressed.contains(&PadButton::Select);
        if chord && !self.chord_active {
            self.chord_active = true;
            self.menu.toggle_open();
            return;
        }
        if !chord {
            self.chord_active = false;
        }

        // Edge-triggered actions on button press.
        let PadEvent::Button {
            button,
            pressed: true,
        } = event
        else {
            return;
        };

        if self.menu.open {
            match button {
                PadButton::South => {
                    let was_editing = self.menu.editing_address;
                    let action = self.menu.activate();
                    if !was_editing && self.menu.editing_address {
                        open_steam_osk();
                    }
                    self.apply_menu_action(action);
                }
                PadButton::East => {
                    let action = self.menu.back();
                    self.apply_menu_action(action);
                }
                _ => {}
            }
        } else if let Some(action) = self.config.mapping.action_for(button).cloned() {
            // Capture mode: map the button and forward the action.
            self.worker.send(action);
        }
    }

    /// The direction currently held: D-pad takes priority, else the left stick.
    fn current_nav_dir(&self) -> Option<NavDir> {
        if self.pressed.contains(&PadButton::DPadUp) {
            Some(NavDir::Up)
        } else if self.pressed.contains(&PadButton::DPadDown) {
            Some(NavDir::Down)
        } else if self.pressed.contains(&PadButton::DPadLeft) {
            Some(NavDir::Left)
        } else if self.pressed.contains(&PadButton::DPadRight) {
            Some(NavDir::Right)
        } else {
            stick_to_nav(self.stick.0, self.stick.1)
        }
    }

    fn apply_menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::None => {}
            MenuAction::SelectDevice(idx) => {
                if let Some(node) = self.devices.get(idx).map(|d| d.node.clone()) {
                    self.set_device(node);
                }
            }
            MenuAction::SetAudio(on) => {
                self.config.media.audio = on;
                self.save_config();
            }
            MenuAction::Connect => {
                let kind = self.menu.selected_transport();
                let address = self.menu.address.clone();
                self.config.target = Some(TargetConfig {
                    transport: kind,
                    address: address.clone(),
                });
                self.save_config();
                self.worker
                    .connect(kind, TargetAddr::network(address.clone()));
                self.status = format!("Connecting to {address}…");
            }
            MenuAction::Close => self.menu.open = false,
        }
    }

    /// Run egui + present one frame.
    fn draw(&mut self) {
        let Self {
            active,
            menu,
            devices,
            hud,
            status,
            ..
        } = self;
        let Some(active) = active.as_mut() else {
            return;
        };
        let raw_input = active.egui_state.take_egui_input(&active.window);
        let output = active.egui_ctx.run_ui(raw_input, |ui| {
            if menu.open {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.10);
                    menu.draw(ui, devices, status);
                });
            } else {
                status_overlay(ui, status);
            }
            hud.draw(ui.ctx());
        });
        active
            .egui_state
            .handle_platform_output(&active.window, output.platform_output);
        let ppp = active.egui_ctx.pixels_per_point();
        let jobs = active.egui_ctx.tessellate(output.shapes, ppp);
        active.renderer.render(ppp, &jobs, &output.textures_delta);
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.active.is_some() {
            return;
        }

        let mut attrs = Window::default_attributes().with_title("Couchcast");
        if std::env::var_os("COUCHCAST_WINDOWED").is_none() {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        } else {
            attrs = attrs.with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        }

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!("failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };

        let renderer = match Renderer::new(window.clone()) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to initialize GPU: {e}");
                event_loop.exit();
                return;
            }
        };

        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &*window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );

        self.active = Some(Active {
            window,
            renderer,
            egui_ctx,
            egui_state,
        });

        self.start_capture();
        self.auto_connect_transport();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let consumed = if let Some(active) = self.active.as_mut() {
            active
                .egui_state
                .on_window_event(&active.window, &event)
                .consumed
        } else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => {
                self.worker.disconnect();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(active) = self.active.as_mut() {
                    active.renderer.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => {
                self.drain_frame();
                self.draw();
            }
            WindowEvent::KeyboardInput { event, .. } if !consumed => {
                use winit::keyboard::{Key, NamedKey};
                if !event.state.is_pressed() {
                    return;
                }
                match event.logical_key {
                    Key::Named(NamedKey::F1) => self.menu.toggle_open(),
                    Key::Named(NamedKey::Escape) => {
                        self.worker.disconnect();
                        event_loop.exit();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: UserEvent) {}

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.poll_input();
        if let Some(active) = self.active.as_ref() {
            active.window.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + INPUT_POLL_INTERVAL));
    }
}

/// The idle overlay shown when the menu is closed: a small status pill.
fn status_overlay(ui: &mut egui::Ui, status: &str) {
    egui::Area::new(egui::Id::new("couchcast-status"))
        .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(16.0, -16.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_black_alpha(160))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .corner_radius(8.0)
                .show(ui, |ui| {
                    ui.colored_label(egui::Color32::WHITE, status);
                });
        });
}

/// Best-effort request to show Steam's on-screen keyboard. This is unreliable
/// for a non-Steam app under gamescope (no Steamworks app-id), so it is not
/// depended on — the dependable path is the manual **Steam + X** gesture, which
/// the menu hints at. We simply nudge Steam and move on.
fn open_steam_osk() {
    let spawned = std::process::Command::new("steam")
        .arg("steam://open/keyboard")
        .spawn()
        .or_else(|_| {
            std::process::Command::new("xdg-open")
                .arg("steam://open/keyboard")
                .spawn()
        });
    match spawned {
        Ok(_) => tracing::debug!("requested Steam OSK (best-effort)"),
        Err(e) => tracing::debug!("could not request Steam OSK ({e}); use Steam+X"),
    }
}
