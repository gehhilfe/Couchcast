//! The controller-first settings menu: an owned selection model plus its egui
//! drawing. We deliberately bypass egui's focus system — with a handful of rows,
//! tracking our own `selected` index and rendering the highlight ourselves is
//! simpler and gives the crisp, game-like navigation a 10-foot UI needs.

use couchcast_config::TransportKind;
use couchcast_input::NavDir;
use couchcast_media::CaptureDevice;

/// Transport options offered in the menu, in display order, mapped to
/// [`TransportKind`].
pub const TRANSPORT_CHOICES: &[(&str, TransportKind)] = &[
    ("ADB — Fire TV / Android TV", TransportKind::Adb),
    ("Log (debug, no device)", TransportKind::Log),
];

/// The rows, in vertical order. `selected` indexes into this list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Device,
    Transport,
    Address,
    Audio,
    Connect,
    Close,
}

const ROWS: &[Row] = &[
    Row::Device,
    Row::Transport,
    Row::Address,
    Row::Audio,
    Row::Connect,
    Row::Close,
];

/// A side effect the app should perform in response to menu input.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    None,
    /// Cycle to a different capture device (index into the device list).
    SelectDevice(usize),
    /// Toggle audio passthrough.
    SetAudio(bool),
    /// Persist the target + (re)connect the transport.
    Connect,
    /// Close the menu.
    Close,
}

/// The mutable state of the settings menu.
pub struct Menu {
    pub open: bool,
    selected: usize,
    pub editing_address: bool,
    device_idx: usize,
    transport_idx: usize,
    pub address: String,
    audio: bool,
    /// Set for one frame after entering edit mode, so the draw code can grab
    /// egui keyboard focus for the text field exactly once.
    pub focus_address: bool,
}

impl Menu {
    pub fn new(device_idx: usize, transport_idx: usize, address: String, audio: bool) -> Self {
        Self {
            // Dev hook: start with the menu open to exercise its drawing without
            // a controller.
            open: std::env::var_os("COUCHCAST_MENU_OPEN").is_some(),
            selected: 0,
            editing_address: false,
            device_idx,
            transport_idx,
            address,
            audio,
            focus_address: false,
        }
    }

    pub fn selected_transport(&self) -> TransportKind {
        TRANSPORT_CHOICES
            .get(self.transport_idx)
            .map(|(_, k)| *k)
            .unwrap_or(TransportKind::Adb)
    }

    pub fn toggle_open(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.editing_address = false;
        }
    }

    /// A directional nav step (from the D-pad or stick). Up/Down move the cursor;
    /// Left/Right adjust the selected row's value.
    pub fn nav(&mut self, dir: NavDir, device_count: usize) -> MenuAction {
        match dir {
            NavDir::Up => {
                self.selected = (self.selected + ROWS.len() - 1) % ROWS.len();
                MenuAction::None
            }
            NavDir::Down => {
                self.selected = (self.selected + 1) % ROWS.len();
                MenuAction::None
            }
            NavDir::Left => self.cycle(-1, device_count),
            NavDir::Right => self.cycle(1, device_count),
        }
    }

    fn cycle(&mut self, delta: i32, device_count: usize) -> MenuAction {
        match ROWS[self.selected] {
            Row::Device if device_count > 0 => {
                self.device_idx = wrap(self.device_idx, delta, device_count);
                MenuAction::SelectDevice(self.device_idx)
            }
            Row::Transport => {
                self.transport_idx = wrap(self.transport_idx, delta, TRANSPORT_CHOICES.len());
                MenuAction::None
            }
            Row::Audio => {
                self.audio = !self.audio;
                MenuAction::SetAudio(self.audio)
            }
            _ => MenuAction::None,
        }
    }

    /// The A button: activate the selected row.
    pub fn activate(&mut self) -> MenuAction {
        match ROWS[self.selected] {
            Row::Address => {
                self.editing_address = true;
                self.focus_address = true;
                MenuAction::None
            }
            Row::Connect => MenuAction::Connect,
            Row::Close => MenuAction::Close,
            _ => MenuAction::None,
        }
    }

    /// The B button: leave edit mode if editing, else close the menu.
    pub fn back(&mut self) -> MenuAction {
        if self.editing_address {
            self.editing_address = false;
            MenuAction::None
        } else {
            MenuAction::Close
        }
    }

    /// Draw the menu into the (screen-filling) root `ui`. Returns whether the
    /// address edit field currently wants text (so the app can note the OSK hint).
    pub fn draw(&mut self, ui: &mut egui::Ui, devices: &[CaptureDevice], status: &str) {
        let accent = egui::Color32::from_rgb(0x3a, 0x8e, 0xff);
        let panel_w = 720.0_f32.min(ui.available_width() - 48.0);

        egui::Frame::new()
            .fill(egui::Color32::from_black_alpha(210))
            .inner_margin(egui::Margin::same(28))
            .corner_radius(16.0)
            .show(ui, |ui| {
                ui.set_width(panel_w);
                ui.vertical_centered(|ui| {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Couchcast").size(34.0).strong());
                    ui.add_space(16.0);
                });

                let device_name = devices
                    .get(self.device_idx)
                    .map(|d| d.name.as_str())
                    .unwrap_or("(no capture device)");
                let transport_label = TRANSPORT_CHOICES
                    .get(self.transport_idx)
                    .map(|(l, _)| *l)
                    .unwrap_or("ADB");

                for (i, row) in ROWS.iter().enumerate() {
                    let selected = i == self.selected;
                    row_frame(ui, panel_w, selected, accent, |ui| match row {
                        Row::Device => value_row(ui, "Capture device", &cyc(device_name)),
                        Row::Transport => value_row(ui, "Forward input to", &cyc(transport_label)),
                        Row::Address => self.address_row(ui),
                        Row::Audio => value_row(
                            ui,
                            "Audio passthrough",
                            &cyc(if self.audio { "On" } else { "Off" }),
                        ),
                        Row::Connect => action_row(ui, "Connect"),
                        Row::Close => action_row(ui, "Close"),
                    });
                }

                ui.add_space(12.0);
                ui.label(egui::RichText::new(status).size(15.0).weak());
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Ⓐ Select    Ⓑ Back    ◀▶ Change    Steam+X to type")
                        .size(15.0)
                        .color(egui::Color32::from_gray(200)),
                );
            });
    }

    fn address_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Target address").size(20.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.editing_address {
                    let edit = egui::TextEdit::singleline(&mut self.address)
                        .hint_text("192.168.1.42")
                        .desired_width(260.0)
                        .font(egui::TextStyle::Heading);
                    let resp = ui.add(edit);
                    if self.focus_address {
                        resp.request_focus();
                        self.focus_address = false;
                    }
                } else {
                    let shown = if self.address.is_empty() {
                        "[ press Ⓐ to edit ]".to_owned()
                    } else {
                        self.address.clone()
                    };
                    ui.label(egui::RichText::new(shown).size(20.0));
                }
            });
        });
    }
}

fn wrap(idx: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let n = len as i32;
    (((idx as i32 + delta) % n + n) % n) as usize
}

/// Wrap a selector value in the `‹ value ›` cycling affordance.
fn cyc(value: &str) -> String {
    format!("‹  {value}  ›")
}

/// A menu row background with the bold selection highlight.
fn row_frame(
    ui: &mut egui::Ui,
    width: f32,
    selected: bool,
    accent: egui::Color32,
    contents: impl FnOnce(&mut egui::Ui),
) {
    let fill = if selected {
        accent.gamma_multiply(0.30)
    } else {
        egui::Color32::TRANSPARENT
    };
    let mut frame = egui::Frame::new()
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(16, 12))
        .corner_radius(10.0);
    if selected {
        frame = frame.stroke(egui::Stroke::new(2.5, accent));
    }
    frame.show(ui, |ui| {
        ui.set_width(width - 32.0);
        contents(ui);
    });
    ui.add_space(6.0);
}

fn value_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(20.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).size(20.0).strong());
        });
    });
}

fn action_row(ui: &mut egui::Ui, label: &str) {
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(label).size(22.0).strong());
    });
}
