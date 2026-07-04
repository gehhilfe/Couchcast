//! The controller-first settings menu: an owned selection model plus its egui
//! drawing. We deliberately bypass egui's focus system — with a handful of rows,
//! tracking our own `selected` index and rendering the highlight ourselves is
//! simpler and gives the crisp, game-like navigation a 10-foot UI needs.

use couchcast_config::TransportKind;
use couchcast_input::NavDir;
use couchcast_media::{CaptureCodec, CaptureDevice, CaptureFormat, codecs, framerates, resolutions};

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
    Codec,
    Resolution,
    Framerate,
    Transport,
    Address,
    Audio,
    Connect,
    Close,
}

const ROWS: &[Row] = &[
    Row::Device,
    Row::Codec,
    Row::Resolution,
    Row::Framerate,
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
    /// Change the capture format. Each field is `None` for "Auto" (device default
    /// / auto-negotiated).
    SetCapture {
        codec: Option<CaptureCodec>,
        width: Option<u32>,
        height: Option<u32>,
        framerate: Option<u32>,
    },
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

    // Capture-format selection, derived from the active device's advertised
    // formats. Each option list starts with `None` (Auto); the resolution and
    // framerate lists depend on the selected codec.
    formats: Vec<CaptureFormat>,
    codec_opts: Vec<Option<CaptureCodec>>,
    res_opts: Vec<Option<(u32, u32)>>,
    fps_opts: Vec<Option<u32>>,
    codec_idx: usize,
    res_idx: usize,
    fps_idx: usize,
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
            formats: Vec::new(),
            codec_opts: vec![None],
            res_opts: vec![None],
            fps_opts: vec![None],
            codec_idx: 0,
            res_idx: 0,
            fps_idx: 0,
        }
    }

    /// Rebuild the capture-format option lists for the active device's `formats`,
    /// seeding the current selection from the persisted preferences (falling back
    /// to Auto when a stored value isn't offered by this device). Called on
    /// startup and whenever the capture device changes.
    pub fn set_formats(
        &mut self,
        formats: Vec<CaptureFormat>,
        codec: Option<CaptureCodec>,
        width: Option<u32>,
        height: Option<u32>,
        framerate: Option<u32>,
    ) {
        self.formats = formats;
        self.codec_opts = std::iter::once(None)
            .chain(codecs(&self.formats).into_iter().map(Some))
            .collect();
        self.codec_idx = codec
            .and_then(|c| self.codec_opts.iter().position(|o| *o == Some(c)))
            .unwrap_or(0);

        self.recompute_res_opts();
        self.res_idx = match (width, height) {
            (Some(w), Some(h)) => self
                .res_opts
                .iter()
                .position(|o| *o == Some((w, h)))
                .unwrap_or(0),
            _ => 0,
        };

        self.recompute_fps_opts();
        self.fps_idx = framerate
            .and_then(|f| self.fps_opts.iter().position(|o| *o == Some(f)))
            .unwrap_or(0);
    }

    fn current_codec(&self) -> Option<CaptureCodec> {
        self.codec_opts.get(self.codec_idx).copied().flatten()
    }

    fn current_res(&self) -> Option<(u32, u32)> {
        self.res_opts.get(self.res_idx).copied().flatten()
    }

    fn current_fps(&self) -> Option<u32> {
        self.fps_opts.get(self.fps_idx).copied().flatten()
    }

    /// The resolutions valid for the selected codec (Auto codec ⇒ only Auto).
    fn recompute_res_opts(&mut self) {
        let mut opts = vec![None];
        if let Some(codec) = self.current_codec() {
            opts.extend(resolutions(&self.formats, codec).into_iter().map(Some));
        }
        self.res_opts = opts;
        if self.res_idx >= self.res_opts.len() {
            self.res_idx = 0;
        }
    }

    /// The framerates valid for the selected codec + resolution.
    fn recompute_fps_opts(&mut self) {
        let mut opts = vec![None];
        if let (Some(codec), Some(res)) = (self.current_codec(), self.current_res()) {
            opts.extend(framerates(&self.formats, codec, res).into_iter().map(Some));
        }
        self.fps_opts = opts;
        if self.fps_idx >= self.fps_opts.len() {
            self.fps_idx = 0;
        }
    }

    /// After a codec change: rebuild the resolution list, keeping the same
    /// resolution if the new codec still offers it (else Auto), then the framerates.
    fn on_codec_changed(&mut self) {
        let prev_res = self.current_res();
        self.recompute_res_opts();
        self.res_idx = prev_res
            .and_then(|r| self.res_opts.iter().position(|o| *o == Some(r)))
            .unwrap_or(0);
        self.on_res_changed();
    }

    /// After a resolution change: rebuild the framerate list, keeping the same
    /// framerate if still offered (else Auto).
    fn on_res_changed(&mut self) {
        let prev_fps = self.current_fps();
        self.recompute_fps_opts();
        self.fps_idx = prev_fps
            .and_then(|f| self.fps_opts.iter().position(|o| *o == Some(f)))
            .unwrap_or(0);
    }

    /// The current capture selection as `(codec, width, height, framerate)`, each
    /// `None` for Auto. The app reads this to persist the menu's clamped choice
    /// after a device switch.
    pub fn capture_selection(&self) -> (Option<CaptureCodec>, Option<u32>, Option<u32>, Option<u32>) {
        let (width, height) = match self.current_res() {
            Some((w, h)) => (Some(w), Some(h)),
            None => (None, None),
        };
        (self.current_codec(), width, height, self.current_fps())
    }

    /// The action carrying the current capture selection.
    fn capture_action(&self) -> MenuAction {
        let (codec, width, height, framerate) = self.capture_selection();
        MenuAction::SetCapture {
            codec,
            width,
            height,
            framerate,
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
            Row::Codec => {
                self.codec_idx = wrap(self.codec_idx, delta, self.codec_opts.len());
                self.on_codec_changed();
                self.capture_action()
            }
            Row::Resolution => {
                self.res_idx = wrap(self.res_idx, delta, self.res_opts.len());
                self.on_res_changed();
                self.capture_action()
            }
            Row::Framerate => {
                self.fps_idx = wrap(self.fps_idx, delta, self.fps_opts.len());
                self.capture_action()
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

                let codec_val = match self.current_codec() {
                    Some(c) => c.label().to_owned(),
                    None => "Auto".to_owned(),
                };
                let res_val = match self.current_res() {
                    Some((w, h)) => format!("{w}×{h}"),
                    None => "Auto".to_owned(),
                };
                let fps_val = match self.current_fps() {
                    Some(f) => format!("{f} fps"),
                    None => "Auto".to_owned(),
                };

                for (i, row) in ROWS.iter().enumerate() {
                    let selected = i == self.selected;
                    row_frame(ui, panel_w, selected, accent, |ui| match row {
                        Row::Device => value_row(ui, "Capture device", &cyc(device_name)),
                        Row::Codec => value_row(ui, "Format", &cyc(&codec_val)),
                        Row::Resolution => value_row(ui, "Resolution", &cyc(&res_val)),
                        Row::Framerate => value_row(ui, "Framerate", &cyc(&fps_val)),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(codec: CaptureCodec, width: u32, height: u32, framerates: Vec<u32>) -> CaptureFormat {
        CaptureFormat {
            codec,
            width,
            height,
            framerates,
        }
    }

    fn sample_formats() -> Vec<CaptureFormat> {
        vec![
            fmt(CaptureCodec::Mjpeg, 1920, 1080, vec![60, 30]),
            fmt(CaptureCodec::Mjpeg, 1280, 720, vec![60, 30]),
            fmt(CaptureCodec::Nv12, 1280, 720, vec![10]),
        ]
    }

    /// Position of `row` in `ROWS`, for driving `cycle` in tests.
    fn row_index(row: Row) -> usize {
        ROWS.iter().position(|r| *r == row).unwrap()
    }

    #[test]
    fn set_formats_seeds_from_prefs() {
        let mut menu = Menu::new(0, 0, String::new(), true);
        menu.set_formats(
            sample_formats(),
            Some(CaptureCodec::Mjpeg),
            Some(1920),
            Some(1080),
            Some(60),
        );
        assert_eq!(menu.current_codec(), Some(CaptureCodec::Mjpeg));
        assert_eq!(menu.current_res(), Some((1920, 1080)));
        assert_eq!(menu.current_fps(), Some(60));
    }

    #[test]
    fn changing_codec_drops_unsupported_resolution_to_auto() {
        let mut menu = Menu::new(0, 0, String::new(), true);
        menu.set_formats(
            sample_formats(),
            Some(CaptureCodec::Mjpeg),
            Some(1920),
            Some(1080),
            Some(60),
        );
        // Select the Format row and cycle from MJPEG to NV12 (which lacks 1920×1080).
        menu.selected = row_index(Row::Codec);
        let action = menu.cycle(1, 0);
        assert_eq!(menu.current_codec(), Some(CaptureCodec::Nv12));
        assert_eq!(menu.current_res(), None); // reset to Auto
        assert_eq!(menu.current_fps(), None);
        assert_eq!(
            action,
            MenuAction::SetCapture {
                codec: Some(CaptureCodec::Nv12),
                width: None,
                height: None,
                framerate: None,
            }
        );
    }

    #[test]
    fn changing_codec_keeps_shared_resolution() {
        let mut menu = Menu::new(0, 0, String::new(), true);
        menu.set_formats(
            sample_formats(),
            Some(CaptureCodec::Mjpeg),
            Some(1280),
            Some(720),
            Some(60),
        );
        // 1280×720 exists for NV12 too, so it survives the codec switch; but NV12
        // only offers 10 fps there, so the 60 fps selection falls back to Auto.
        menu.selected = row_index(Row::Codec);
        menu.cycle(1, 0);
        assert_eq!(menu.current_codec(), Some(CaptureCodec::Nv12));
        assert_eq!(menu.current_res(), Some((1280, 720)));
        assert_eq!(menu.current_fps(), None);
    }

    #[test]
    fn auto_codec_forces_auto_resolution_and_framerate() {
        let mut menu = Menu::new(0, 0, String::new(), true);
        menu.set_formats(sample_formats(), None, None, None, None);
        assert_eq!(menu.current_codec(), None);
        // With no codec pinned there are no specific device modes to offer.
        assert_eq!(menu.res_opts, vec![None]);
        assert_eq!(menu.fps_opts, vec![None]);
    }
}
