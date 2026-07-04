//! The GTK4 / libadwaita application: a single fullscreen window with the live
//! capture as its background and a controller-navigable settings overlay on top.
//!
//! ## Input routing (the core interaction model)
//!
//! Controllers are polled from the glib main loop via `gilrs`. A pressed
//! **Start + Select** chord toggles the settings overlay. Then:
//!
//! * **Overlay open** → pad events become GTK focus moves (D-pad → focus
//!   direction, A activates, B closes) so the whole UI is usable with no mouse.
//! * **Overlay closed** ("capture mode") → each button is looked up in the
//!   editable button map and the resulting action is forwarded to the target
//!   device through the transport worker.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use gtk4 as gtk;

use couchcast_config::{Config, DeviceRef, TargetConfig, TransportKind};
use couchcast_input::{InputManager, NavEvent, PadEvent, nav_from_pad};
use couchcast_media::{CaptureDevice, CapturePipeline, PipelineConfig, list_devices};
use couchcast_transport::{PadButton, TargetAddr};

use crate::worker::TransportWorker;

/// Reverse-DNS application id, matching the GitHub repository `gehhilfe/Couchcast`
/// and every packaging filename under `data/` and `flatpak/`.
pub const APP_ID: &str = "io.github.gehhilfe.Couchcast";

/// How often the controller is polled (~120 Hz). Cheap and non-blocking.
const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(8);

/// Shared, main-thread-only application state.
struct AppState {
    config: Config,
    devices: Vec<CaptureDevice>,
    input: InputManager,
    worker: TransportWorker,
    window: adw::ApplicationWindow,
    picture: gtk::Picture,
    status: gtk::Label,
    settings_container: gtk::Widget,
    first_control: gtk::Widget,
    pipeline: Option<CapturePipeline>,
    overlay_visible: bool,
    pressed: HashSet<PadButton>,
    chord_active: bool,
}

/// Build the libadwaita application and hook up activation.
pub fn build_application() -> adw::Application {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app
}

fn build_ui(app: &adw::Application) {
    let config = Config::load_or_default();

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Couchcast")
        .default_width(1280)
        .default_height(720)
        .build();
    window.fullscreen();

    // Live video fills the window; the settings panel floats on top.
    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Contain);

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&picture));

    let devices = list_devices().unwrap_or_else(|e| {
        tracing::error!("device enumeration failed: {e}");
        Vec::new()
    });

    let panel = build_settings_panel(&devices);
    panel.container.set_visible(false);
    overlay.add_overlay(&panel.container);

    window.set_content(Some(&overlay));

    let input = match InputManager::new() {
        Ok(input) => input,
        Err(e) => {
            // Without a controller subsystem the app still shows video; log and
            // carry on with an empty input source is not possible, so surface it.
            tracing::error!("failed to start controller input: {e}");
            panel
                .status
                .set_text(&format!("Controller input unavailable: {e}"));
            window.present();
            return;
        }
    };

    let first_control: gtk::Widget = if devices.is_empty() {
        panel.transport_dropdown.clone().upcast()
    } else {
        panel.device_dropdown.clone().upcast()
    };

    let state = Rc::new(RefCell::new(AppState {
        config,
        devices,
        input,
        worker: TransportWorker::spawn(),
        window: window.clone(),
        picture: picture.clone(),
        status: panel.status.clone(),
        settings_container: panel.container.clone().upcast(),
        first_control,
        pipeline: None,
        overlay_visible: false,
        pressed: HashSet::new(),
        chord_active: false,
    }));

    // Start the pipeline on the remembered device, else the first found device.
    let initial_node = {
        let s = state.borrow();
        s.config
            .last_device
            .as_ref()
            .map(|d| d.node.clone())
            .or_else(|| s.devices.first().map(|d| d.node.clone()))
    };
    match initial_node {
        Some(node) => set_device(&state, node),
        None => state
            .borrow()
            .status
            .set_text("No capture device found. Connect an HDMI capture device."),
    }

    // Auto-connect the transport: to the saved target if present, otherwise to
    // the logging transport so forwarding is observable during development.
    {
        let s = state.borrow();
        match &s.config.target {
            Some(target) => s.worker.connect(target.transport, target.to_target_addr()),
            None => s
                .worker
                .connect(TransportKind::Log, TargetAddr::network("unset")),
        }
    }

    // Cleanly disconnect the transport when the window closes.
    let close_state = state.clone();
    window.connect_close_request(move |_| {
        close_state.borrow().worker.disconnect();
        glib::Propagation::Proceed
    });

    wire_settings(&state, &panel);

    // Poll the controller from the main loop.
    let tick_state = state.clone();
    glib::timeout_add_local(INPUT_POLL_INTERVAL, move || {
        on_tick(&tick_state);
        glib::ControlFlow::Continue
    });

    window.present();
}

// --------------------------------------------------------------------------
// Input handling
// --------------------------------------------------------------------------

fn on_tick(state: &Rc<RefCell<AppState>>) {
    let events = state.borrow_mut().input.poll();
    for event in events {
        handle_pad_event(state, event);
    }
}

fn handle_pad_event(state: &Rc<RefCell<AppState>>, event: PadEvent) {
    // Track pressed buttons and detect the Start+Select overlay-toggle chord.
    let toggle = {
        let mut s = state.borrow_mut();
        if let PadEvent::Button { button, pressed } = &event {
            if *pressed {
                s.pressed.insert(*button);
            } else {
                s.pressed.remove(button);
            }
        }
        let chord = s.pressed.contains(&PadButton::Start) && s.pressed.contains(&PadButton::Select);
        if chord && !s.chord_active {
            s.chord_active = true;
            true
        } else {
            if !chord {
                s.chord_active = false;
            }
            false
        }
    };
    if toggle {
        toggle_overlay(state);
        return;
    }

    let overlay_visible = state.borrow().overlay_visible;
    if overlay_visible {
        if let Some(nav) = nav_from_pad(&event) {
            apply_nav(state, nav);
        }
    } else if let PadEvent::Button {
        button,
        pressed: true,
    } = event
    {
        // Capture mode: map the button and forward the action.
        let action = state.borrow().config.mapping.action_for(button).cloned();
        if let Some(action) = action {
            state.borrow().worker.send(action);
        }
    }
}

fn apply_nav(state: &Rc<RefCell<AppState>>, nav: NavEvent) {
    let window = state.borrow().window.clone();
    match nav {
        NavEvent::Up => {
            window.child_focus(gtk::DirectionType::Up);
        }
        NavEvent::Down => {
            window.child_focus(gtk::DirectionType::Down);
        }
        NavEvent::Left => {
            window.child_focus(gtk::DirectionType::Left);
        }
        NavEvent::Right => {
            window.child_focus(gtk::DirectionType::Right);
        }
        NavEvent::Activate => {
            if let Some(widget) = gtk::prelude::GtkWindowExt::focus(&window) {
                widget.activate();
            }
        }
        NavEvent::Back => toggle_overlay(state),
    }
}

fn toggle_overlay(state: &Rc<RefCell<AppState>>) {
    let (visible, first_control) = {
        let mut s = state.borrow_mut();
        s.overlay_visible = !s.overlay_visible;
        s.settings_container.set_visible(s.overlay_visible);
        (s.overlay_visible, s.first_control.clone())
    };
    if visible {
        // Grab focus so the first D-pad press is not swallowed.
        first_control.grab_focus();
    }
    tracing::debug!(visible, "settings overlay toggled");
}

// --------------------------------------------------------------------------
// Device / pipeline management
// --------------------------------------------------------------------------

fn set_device(state: &Rc<RefCell<AppState>>, node: String) {
    let (media, device_name) = {
        let s = state.borrow();
        let name = s
            .devices
            .iter()
            .find(|d| d.node == node)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| node.clone());
        (s.config.media.clone(), name)
    };

    let cfg = PipelineConfig {
        device_node: node.clone(),
        width: media.width,
        height: media.height,
        framerate: media.framerate,
        audio: media.audio,
    };

    match CapturePipeline::new(&cfg) {
        Ok(mut pipeline) => {
            if let Err(e) = pipeline.install_bus_logger() {
                tracing::warn!("could not install pipeline bus logger: {e}");
            }
            let mut s = state.borrow_mut();
            s.picture.set_paintable(Some(pipeline.paintable()));
            if let Err(e) = pipeline.start() {
                tracing::error!("failed to start pipeline: {e}");
                s.status
                    .set_text(&format!("Failed to start {device_name}: {e}"));
            } else {
                s.status.set_text(&format!("Playing {device_name}"));
            }
            s.pipeline = Some(pipeline);
            s.config.last_device = Some(DeviceRef {
                name: device_name,
                node,
            });
            save_config(&s.config);
        }
        Err(e) => {
            tracing::error!("failed to build pipeline for {node}: {e}");
            state
                .borrow()
                .status
                .set_text(&format!("Failed to open {device_name}: {e}"));
        }
    }
}

fn save_config(config: &Config) {
    if let Err(e) = config.save() {
        tracing::warn!("failed to save config: {e}");
    }
}

// --------------------------------------------------------------------------
// Settings panel
// --------------------------------------------------------------------------

/// Widget handles for the settings overlay.
struct SettingsPanel {
    container: gtk::Box,
    device_dropdown: gtk::DropDown,
    transport_dropdown: gtk::DropDown,
    address_entry: gtk::Entry,
    audio_switch: gtk::Switch,
    connect_button: gtk::Button,
    close_button: gtk::Button,
    status: gtk::Label,
}

/// The order of entries in the transport dropdown, mapped to [`TransportKind`].
const TRANSPORT_CHOICES: &[(&str, TransportKind)] = &[
    ("ADB — Fire TV / Android TV", TransportKind::Adb),
    ("Log (debug, no device)", TransportKind::Log),
];

fn build_settings_panel(devices: &[CaptureDevice]) -> SettingsPanel {
    let container = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .width_request(480)
        .build();
    container.add_css_class("background");
    container.add_css_class("card");
    container.set_margin_top(24);
    container.set_margin_bottom(24);
    container.set_margin_start(24);
    container.set_margin_end(24);

    let title = gtk::Label::new(Some("Couchcast Settings"));
    title.add_css_class("title-2");
    container.append(&title);

    // Capture device.
    container.append(&row_label("Capture device"));
    let device_names: Vec<&str> = devices.iter().map(|d| d.name.as_str()).collect();
    let device_dropdown = gtk::DropDown::from_strings(&device_names);
    container.append(&device_dropdown);

    // Target transport + address.
    container.append(&row_label("Forward input to"));
    let transport_labels: Vec<&str> = TRANSPORT_CHOICES.iter().map(|(label, _)| *label).collect();
    let transport_dropdown = gtk::DropDown::from_strings(&transport_labels);
    container.append(&transport_dropdown);

    let address_entry = gtk::Entry::builder()
        .placeholder_text("Device IP, e.g. 192.168.1.42")
        .build();
    container.append(&address_entry);

    // Audio toggle.
    let audio_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    audio_row.append(&row_label("Audio passthrough"));
    let audio_switch = gtk::Switch::builder()
        .active(true)
        .halign(gtk::Align::End)
        .hexpand(true)
        .build();
    audio_row.append(&audio_switch);
    container.append(&audio_row);

    // Actions.
    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    button_row.set_halign(gtk::Align::End);
    let connect_button = gtk::Button::with_label("Connect");
    connect_button.add_css_class("suggested-action");
    let close_button = gtk::Button::with_label("Close");
    button_row.append(&close_button);
    button_row.append(&connect_button);
    container.append(&button_row);

    let status = gtk::Label::new(Some("Press Start + Select to open this menu."));
    status.add_css_class("dim-label");
    status.set_wrap(true);
    container.append(&status);

    SettingsPanel {
        container,
        device_dropdown,
        transport_dropdown,
        address_entry,
        audio_switch,
        connect_button,
        close_button,
        status,
    }
}

fn row_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_halign(gtk::Align::Start);
    label.add_css_class("heading");
    label
}

fn wire_settings(state: &Rc<RefCell<AppState>>, panel: &SettingsPanel) {
    // Pre-fill the address and audio toggle from config.
    {
        let s = state.borrow();
        if let Some(target) = &s.config.target {
            panel.address_entry.set_text(&target.address);
            if let Some(idx) = TRANSPORT_CHOICES
                .iter()
                .position(|(_, kind)| *kind == target.transport)
            {
                panel.transport_dropdown.set_selected(idx as u32);
            }
        }
        panel.audio_switch.set_active(s.config.media.audio);
    }

    // Device selection rebuilds the pipeline.
    let st = state.clone();
    panel.device_dropdown.connect_selected_notify(move |dd| {
        let idx = dd.selected() as usize;
        let node = st.borrow().devices.get(idx).map(|d| d.node.clone());
        if let Some(node) = node {
            set_device(&st, node);
        }
    });

    // Audio toggle updates config (applied on next pipeline rebuild).
    let st = state.clone();
    panel.audio_switch.connect_active_notify(move |sw| {
        let mut s = st.borrow_mut();
        s.config.media.audio = sw.is_active();
        save_config(&s.config);
    });

    // Connect: persist the target and (re)connect the transport worker.
    let st = state.clone();
    let transport_dropdown = panel.transport_dropdown.clone();
    let address_entry = panel.address_entry.clone();
    panel.connect_button.connect_clicked(move |_| {
        let kind = TRANSPORT_CHOICES
            .get(transport_dropdown.selected() as usize)
            .map(|(_, kind)| *kind)
            .unwrap_or(TransportKind::Adb);
        let address = address_entry.text().to_string();

        let mut s = st.borrow_mut();
        s.config.target = Some(TargetConfig {
            transport: kind,
            address: address.clone(),
        });
        save_config(&s.config);
        s.worker.connect(kind, TargetAddr::network(address.clone()));
        s.status.set_text(&format!("Connecting to {address}…"));
    });

    // Close hides the overlay.
    let st = state.clone();
    panel.close_button.connect_clicked(move |_| {
        if st.borrow().overlay_visible {
            toggle_overlay(&st);
        }
    });
}
