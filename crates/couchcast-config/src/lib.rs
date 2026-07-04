//! Persistent configuration for Couchcast.
//!
//! Stored as human-editable TOML under the XDG config directory
//! (`~/.config/couchcast/config.toml`). The app loads it at startup and rewrites
//! it whenever the user changes the device, video settings, target, or button
//! mapping — fulfilling the "remember last-used device and mapping" requirement.
//!
//! The config never bricks the app: a missing file yields [`Config::default`],
//! and a corrupt file is logged and replaced by defaults (see
//! [`Config::load_or_default`]).

mod mapping;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use couchcast_transport::TargetAddr;

pub use mapping::{Binding, ButtonMap};

/// Errors from loading or saving configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not determine a config directory for this platform")]
    NoConfigDir,
    #[error("reading {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("writing {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("serializing config: {0}")]
    Serialize(#[from] toml::ser::Error),
}

type Result<T> = std::result::Result<T, ConfigError>;

/// A capture device the user selected, remembered across runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRef {
    /// Human-readable name (e.g. "USB3 HDMI Capture").
    pub name: String,
    /// The V4L2 device node, e.g. `/dev/video0`.
    pub node: String,
}

/// Which transport backend to use for a target, matching the feature-gated
/// backends in `couchcast-transport`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransportKind {
    /// ADB-over-TCP to a Fire TV / Android TV (the MVP default).
    #[default]
    Adb,
    BluetoothHid,
    Cec,
    Roku,
    /// The logging no-op transport, for development without hardware.
    Log,
}

/// The device Couchcast forwards input to, and how to reach it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetConfig {
    pub transport: TransportKind,
    /// Host/IP (ADB, Roku) — the address the transport connects to.
    pub address: String,
}

impl TargetConfig {
    /// Build the [`TargetAddr`] the transport layer expects.
    pub fn to_target_addr(&self) -> TargetAddr {
        TargetAddr::Network(self.address.clone())
    }
}

/// The capture input format to request from the device (a V4L2 pixel format or
/// compressed codec). Mirrors `couchcast_media::CaptureCodec`; kept here (rather
/// than depending on the media crate) so config stays free of the heavy
/// `gstreamer`/`v4l` dependencies. The app maps between the two, as it already
/// does for the other media prefs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureCodec {
    Mjpeg,
    H264,
    Yuyv,
    Nv12,
    I420,
    P010,
    Bgr,
}

/// Video/audio preferences for the capture pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaPrefs {
    /// Preferred capture input format; `None` = let the pipeline auto-negotiate.
    pub codec: Option<CaptureCodec>,
    /// Preferred capture width in pixels; `None` = negotiate the device default.
    pub width: Option<u32>,
    /// Preferred capture height in pixels.
    pub height: Option<u32>,
    /// Preferred capture framerate in fps.
    pub framerate: Option<u32>,
    /// Whether to route the capture card's audio to the local output.
    pub audio: bool,
}

impl Default for MediaPrefs {
    fn default() -> Self {
        Self {
            codec: None,
            width: None,
            height: None,
            framerate: None,
            audio: true,
        }
    }
}

/// The full application configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The last capture device the user selected.
    pub last_device: Option<DeviceRef>,
    /// Video/audio preferences.
    pub media: MediaPrefs,
    /// The forwarding target (device + transport).
    pub target: Option<TargetConfig>,
    /// Controller → action mapping.
    pub mapping: ButtonMap,
}

impl Config {
    /// The path Couchcast reads and writes: `<config_dir>/config.toml`.
    pub fn path() -> Result<PathBuf> {
        Ok(config_dir()?.join("config.toml"))
    }

    /// Load from `path`, returning [`Config::default`] if the file is absent.
    pub fn load_from(path: &Path) -> Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(source) => Err(ConfigError::Read {
                path: path.to_owned(),
                source,
            }),
        }
    }

    /// Load from the standard path, returning defaults if absent.
    pub fn load() -> Result<Config> {
        Self::load_from(&Self::path()?)
    }

    /// Load from the standard path, falling back to defaults (with a logged
    /// warning) on any error, so a broken config never prevents startup.
    pub fn load_or_default() -> Config {
        match Self::load() {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::warn!("using default config: {e}");
                Config::default()
            }
        }
    }

    /// Write to `path`, creating parent directories as needed.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
                path: parent.to_owned(),
                source,
            })?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(path, text).map_err(|source| ConfigError::Write {
            path: path.to_owned(),
            source,
        })
    }

    /// Write to the standard path.
    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path()?)
    }
}

/// The Couchcast config directory (`~/.config/couchcast` on Linux).
pub fn config_dir() -> Result<PathBuf> {
    directories::ProjectDirs::from("io.github", "gehhilfe", "couchcast")
        .map(|dirs| dirs.config_dir().to_owned())
        .ok_or(ConfigError::NoConfigDir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use couchcast_transport::{Direction, PadButton, RemoteAction};

    #[test]
    fn default_config_round_trips_through_toml() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).expect("serialize");
        let parsed: Config = toml::from_str(&text).expect("deserialize");
        // The default mapping survives a round trip.
        assert_eq!(
            parsed.mapping.action_for(PadButton::DPadUp),
            Some(&RemoteAction::Navigate(Direction::Up))
        );
        assert!(parsed.media.audio);
    }

    #[test]
    fn populated_config_round_trips() {
        let cfg = Config {
            last_device: Some(DeviceRef {
                name: "USB Capture".into(),
                node: "/dev/video0".into(),
            }),
            media: MediaPrefs {
                codec: Some(CaptureCodec::Mjpeg),
                width: Some(1920),
                height: Some(1080),
                framerate: Some(60),
                audio: true,
            },
            target: Some(TargetConfig {
                transport: TransportKind::Adb,
                address: "192.168.1.42".into(),
            }),
            mapping: ButtonMap::default(),
        };
        let text = toml::to_string_pretty(&cfg).expect("serialize");
        let parsed: Config = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed.last_device, cfg.last_device);
        assert_eq!(parsed.target, cfg.target);
        assert_eq!(parsed.media, cfg.media);
    }
}
