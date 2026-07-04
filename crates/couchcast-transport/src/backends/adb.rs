//! ADB-over-TCP transport for Fire TV / Android TV — the MVP backend.
//!
//! ## The latency trap this design avoids
//!
//! The naive way to send input over ADB is `adb shell input keyevent 23` per
//! button. That is unusably slow: each `adb` invocation opens a new connection,
//! and worse, Android's `input` binary is a Java wrapper that cold-starts an
//! `app_process` (JVM) every call — ~150-400 ms of latency per keypress.
//!
//! Instead this backend opens **one long-lived `adb shell`** at connect time and
//! streams command lines into its stdin. That removes the per-keypress `adb`
//! connection setup. Note that piping `input …` lines *still* pays the JVM
//! cold-start for each `input` call inside the shell; the genuinely low-latency
//! path is to locate the target's evdev node once (`getevent -pl`) and stream
//! raw `sendevent` packets, reaching ~10-30 ms. That optimization is tracked as
//! future work — see the `TODO(sendevent)` markers below — but the persistent
//! shell is the load-bearing architectural decision and is in place from day one.
//!
//! ## Requirements
//!
//! * ADB debugging enabled on the target (Fire TV: *Settings → My Fire TV →
//!   Developer Options → ADB Debugging*), one-time and survives reboot.
//! * The `adb` binary on `PATH`. For the Flatpak build we will migrate to the
//!   pure-Rust `adb_client` crate so no external binary needs bundling — tracked
//!   in `docs/ROADMAP.md`.

use std::process::Stdio;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin, Command};

use crate::capabilities::DeviceCapabilities;
use crate::event::{Direction, PadButton, RemoteAction};
use crate::transport::{Result, TargetAddr, Transport, TransportError};

/// Default ADB-over-TCP port exposed by Fire TV / Android TV.
const DEFAULT_ADB_PORT: u16 = 5555;

/// Forwards [`RemoteAction`]s to an Android TV device over ADB.
pub struct AdbTransport {
    adb_bin: String,
    /// The `-s` serial once connected (e.g. `"192.168.1.42:5555"`).
    serial: Option<String>,
    shell: Option<Child>,
    stdin: Option<ChildStdin>,
}

impl Default for AdbTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl AdbTransport {
    pub fn new() -> Self {
        Self {
            adb_bin: "adb".to_owned(),
            serial: None,
            shell: None,
            stdin: None,
        }
    }

    /// Override the `adb` executable (path or name).
    pub fn with_adb_binary(mut self, bin: impl Into<String>) -> Self {
        self.adb_bin = bin.into();
        self
    }

    /// Resolve a [`TargetAddr`] into the `-s <serial>` string ADB expects.
    fn serial_for(target: &TargetAddr) -> String {
        match target {
            TargetAddr::Network(host) => {
                if host.contains(':') {
                    host.clone()
                } else {
                    format!("{host}:{DEFAULT_ADB_PORT}")
                }
            }
            TargetAddr::UsbSerial(serial) => serial.clone(),
        }
    }

    /// Translate an action into a line to feed the persistent shell, or `None`
    /// if this backend cannot express it (dropped, not an error).
    fn action_to_line(action: &RemoteAction) -> Option<String> {
        // Android `KEYCODE_*` values (see android.view.KeyEvent).
        let keyevent = |code: u16| Some(format!("input keyevent {code}"));
        match action {
            RemoteAction::Navigate(Direction::Up) => keyevent(19),
            RemoteAction::Navigate(Direction::Down) => keyevent(20),
            RemoteAction::Navigate(Direction::Left) => keyevent(21),
            RemoteAction::Navigate(Direction::Right) => keyevent(22),
            RemoteAction::Select => keyevent(23), // DPAD_CENTER
            RemoteAction::Back => keyevent(4),
            RemoteAction::Home => keyevent(3),
            RemoteAction::Menu => keyevent(82),
            RemoteAction::PlayPause => keyevent(85),
            RemoteAction::Play => keyevent(126),
            RemoteAction::Pause => keyevent(127),
            RemoteAction::Stop => keyevent(86),
            RemoteAction::Rewind => keyevent(89),
            RemoteAction::FastForward => keyevent(90),
            RemoteAction::Next => keyevent(87),
            RemoteAction::Previous => keyevent(88),
            RemoteAction::VolumeUp => keyevent(24),
            RemoteAction::VolumeDown => keyevent(25),
            RemoteAction::Mute => keyevent(164),
            RemoteAction::Power => keyevent(26),
            RemoteAction::Text(text) => Some(format!("input text {}", escape_text(text))),
            // Raw gamepad passthrough → Android BUTTON_* keycodes. We only emit on
            // press (`input keyevent` is a discrete tap); holding/analog need the
            // sendevent path.
            RemoteAction::GamepadButton { button, pressed } if *pressed => {
                keyevent(gamepad_keycode(*button))
            }
            RemoteAction::GamepadButton { .. } => None,
            // TODO(sendevent): analog sticks require streaming raw evdev packets;
            // `input` has no analog equivalent, so these are dropped for now.
            RemoteAction::Analog { .. } => None,
        }
    }

    /// Write one line to the persistent shell, marking the transport
    /// disconnected if the pipe has broken.
    async fn write_line(&mut self, line: &str) -> Result<()> {
        let stdin = self.stdin.as_mut().ok_or(TransportError::NotConnected)?;
        if let Err(e) = async {
            stdin.write_all(line.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await
        }
        .await
        {
            self.stdin = None;
            return Err(TransportError::Io(e));
        }
        Ok(())
    }
}

#[async_trait]
impl Transport for AdbTransport {
    fn name(&self) -> &'static str {
        "adb"
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities::ANDROID_TV
    }

    fn is_connected(&self) -> bool {
        self.stdin.is_some()
    }

    async fn connect(&mut self, target: &TargetAddr) -> Result<()> {
        let serial = Self::serial_for(target);

        // For TCP targets, establish the ADB connection first.
        if let TargetAddr::Network(_) = target {
            let output = Command::new(&self.adb_bin)
                .arg("connect")
                .arg(&serial)
                .output()
                .await
                .map_err(|source| TransportError::Connect {
                    target: serial.clone(),
                    source,
                })?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let lower = stdout.to_ascii_lowercase();
            if lower.contains("cannot") || lower.contains("unable") || lower.contains("failed") {
                return Err(TransportError::Other(format!(
                    "adb connect {serial} failed: {}",
                    stdout.trim()
                )));
            }
            tracing::info!(%serial, "adb connect: {}", stdout.trim());
        }

        // Open the single persistent shell we stream events into.
        let mut child = Command::new(&self.adb_bin)
            .arg("-s")
            .arg(&serial)
            .arg("shell")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| TransportError::Connect {
                target: serial.clone(),
                source,
            })?;

        self.stdin = child.stdin.take();
        self.shell = Some(child);
        self.serial = Some(serial);
        tracing::info!(serial = ?self.serial, "adb persistent shell opened");
        Ok(())
    }

    async fn send(&mut self, action: RemoteAction) -> Result<()> {
        match Self::action_to_line(&action) {
            Some(line) => {
                tracing::debug!(%line, "adb send");
                self.write_line(&line).await
            }
            None => {
                tracing::trace!(action = %action.label(), "adb: action not mappable, dropped");
                Ok(())
            }
        }
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.stdin = None;
        if let Some(mut child) = self.shell.take() {
            let _ = child.kill().await;
        }
        self.serial = None;
        tracing::info!("adb transport disconnected");
        Ok(())
    }
}

/// Map a normalized pad button to an Android `KEYCODE_BUTTON_*` value.
fn gamepad_keycode(button: PadButton) -> u16 {
    match button {
        PadButton::South => 96,         // BUTTON_A
        PadButton::East => 97,          // BUTTON_B
        PadButton::West => 99,          // BUTTON_X
        PadButton::North => 100,        // BUTTON_Y
        PadButton::LeftBumper => 102,   // BUTTON_L1
        PadButton::RightBumper => 103,  // BUTTON_R1
        PadButton::LeftTrigger => 104,  // BUTTON_L2
        PadButton::RightTrigger => 105, // BUTTON_R2
        PadButton::Select => 109,       // BUTTON_SELECT
        PadButton::Start => 108,        // BUTTON_START
        PadButton::Guide => 110,        // BUTTON_MODE
        PadButton::LeftStick => 106,    // BUTTON_THUMBL
        PadButton::RightStick => 107,   // BUTTON_THUMBR
        PadButton::DPadUp => 19,
        PadButton::DPadDown => 20,
        PadButton::DPadLeft => 21,
        PadButton::DPadRight => 22,
    }
}

/// Escape a string for Android's `input text`, which treats spaces specially and
/// runs through the shell. Spaces become `%s`; a conservative allowlist keeps
/// everything else literal so we never inject shell metacharacters.
fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            ' ' => out.push_str("%s"),
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => out.push(ch),
            // Drop anything else rather than risk shell injection; real text
            // entry will move to the sendevent/keyboard path.
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_maps_to_dpad_keyevents() {
        assert_eq!(
            AdbTransport::action_to_line(&RemoteAction::Navigate(Direction::Up)).as_deref(),
            Some("input keyevent 19")
        );
        assert_eq!(
            AdbTransport::action_to_line(&RemoteAction::Select).as_deref(),
            Some("input keyevent 23")
        );
        assert_eq!(
            AdbTransport::action_to_line(&RemoteAction::Back).as_deref(),
            Some("input keyevent 4")
        );
    }

    #[test]
    fn analog_is_dropped_for_now() {
        assert!(
            AdbTransport::action_to_line(&RemoteAction::Analog {
                axis: crate::event::PadAxis::LeftStickX,
                value: 0.9,
            })
            .is_none()
        );
    }

    #[test]
    fn text_is_escaped() {
        assert_eq!(
            AdbTransport::action_to_line(&RemoteAction::Text("hi there".into())).as_deref(),
            Some("input text hi%sthere")
        );
        // Shell metacharacters are stripped; the space still becomes `%s`.
        assert_eq!(
            AdbTransport::action_to_line(&RemoteAction::Text("a;rm -rf".into())).as_deref(),
            Some("input text arm%s-rf")
        );
    }

    #[test]
    fn network_target_gets_default_port() {
        assert_eq!(
            AdbTransport::serial_for(&TargetAddr::Network("10.0.0.5".into())),
            "10.0.0.5:5555"
        );
        assert_eq!(
            AdbTransport::serial_for(&TargetAddr::Network("10.0.0.5:5678".into())),
            "10.0.0.5:5678"
        );
    }
}
