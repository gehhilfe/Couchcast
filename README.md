<div align="center">

# Couchcast

**Play a streaming stick on your Linux PC or Steam Deck — full-screen, low-latency, controller-first.**

Couchcast displays live video and audio from a V4L2 HDMI capture device (e.g. an
HDMI capture of a Fire TV or other streaming stick) in a fullscreen window, and
forwards your controller/keyboard input back to that device. It's packaged as a
Flatpak so it installs in one click on SteamOS and launches straight from Steam
Gaming Mode — making a streaming stick behave like a native Steam entry.

[![CI](https://github.com/gehhilfe/Couchcast/actions/workflows/ci.yml/badge.svg)](https://github.com/gehhilfe/Couchcast/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

</div>

> [!NOTE]
> **Status: early scaffold.** The project structure, build, and the core
> subsystems (capture pipeline, input reading, transport abstraction, settings
> UI, Flatpak packaging) are in place and compile. It has not yet been validated
> end-to-end on real capture hardware. See the [roadmap](docs/ROADMAP.md).

## How it works

```
 ┌───────────┐   HDMI    ┌───────────────┐   USB    ┌──────────────── Couchcast ─────────────────┐
 │ Fire TV / │ ────────▶ │ HDMI capture  │ ───────▶ │  v4l2src → decode → gtk4paintablesink       │
 │ streaming │           │ dongle (V4L2) │          │  pipewiresrc → autoaudiosink   (one         │
 │  stick    │ ◀───────┐ └───────────────┘          │                                 pipeline)   │
 └───────────┘         │                            │  gilrs ─▶ button map ─▶ Transport ──┐        │
        ▲              │  ADB over Wi-Fi/USB         └─────────────────────────────────────┼───────┘
        └──────────────┴─────────────────────────────────────────────────────────────────┘
```

The device is connected **only** through HDMI capture, so there is no return
channel "through" the video. Input is forwarded over a separate out-of-band
link — for Fire TV / Android TV that's **ADB over TCP**, held open as a single
persistent shell to avoid per-keypress latency. The transport is abstracted
behind a trait so Bluetooth-HID, HDMI-CEC, and Roku ECP backends can drop in
later. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Features

Capture & display
- [x] Enumerate and select a V4L2 capture device
- [x] Fullscreen low-latency video (zero-copy `gtk4paintablesink`, tuned for one-frame latency)
- [x] Audio passthrough kept in A/V sync (shared GStreamer pipeline/clock)

Input forwarding
- [x] Read the controller via `gilrs` (the Steam Virtual Gamepad under Gaming Mode)
- [x] Map & forward input to the target — ADB backend for Fire TV / Android TV
- [x] Editable button mapping
- [ ] Keyboard forwarding (planned, via `evdev`)

SteamOS integration
- [x] Flatpak manifest + Flathub metadata
- [x] Single fullscreen Wayland window (composites cleanly under gamescope)
- [x] Fully controller-navigable UI (D-pad focus, no mouse required)

Config
- [x] Remembers last-used device and mapping
- [x] Settings screen (device, resolution, transport, mapping)

## The stack

| Concern | Choice |
| --- | --- |
| Language | Rust (edition 2024) |
| Video + audio | GStreamer (`gstreamer-rs`) — one pipeline, `v4l2src` + `pipewiresrc` |
| Display | GTK4 + libadwaita, `gtk4paintablesink` into a `Gtk.Picture` |
| Controller input | `gilrs` |
| Input forwarding | `Transport` trait; ADB backend first (Fire TV / Android TV) |
| Packaging | Flatpak → Flathub → SteamOS Gaming Mode |

## Build from source

You need a recent stable Rust toolchain and the GTK4/GStreamer development
libraries.

```sh
# Arch / SteamOS dev tooling
sudo pacman -S --needed gtk4 libadwaita gstreamer gst-plugins-base \
  gst-plugins-good gst-plugins-bad glib2 pkgconf

# Debian / Ubuntu
sudo apt install -y libgtk-4-dev libadwaita-1-dev libgstreamer1.0-dev \
  libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad libglib2.0-dev libudev-dev pkg-config

# Build & run
cargo run -p couchcast
```

For input forwarding to a Fire TV during development you also need `adb` on your
`PATH`. Turn on **Settings → My Fire TV → Developer Options → ADB Debugging** on
the device, note its IP, then set it in Couchcast's settings screen.

## Using it

- Open the settings overlay any time with **Start + Select** on your controller.
- Pick your capture device and enter the target's IP, then **Connect**.
- Close the overlay; your controller now drives the streaming stick. Remap
  buttons in the settings screen, or edit `~/.config/couchcast/config.toml`.

Verbose logs: `RUST_LOG=couchcast=debug cargo run -p couchcast`.

## Install on SteamOS (Flatpak)

Once released on Flathub this will be a one-click install; until then you can
build the Flatpak locally — see [`docs/PACKAGING.md`](docs/PACKAGING.md). In
short: build with `cargo xtask flatpak-build`, then in Desktop Mode add it via
**Steam → Add a Non-Steam Game**, and give the shortcut a Gamepad/Keyboard
controller layout so the UI is navigable in Gaming Mode.

## Project layout

```
crates/
  couchcast/            app — GTK4/libadwaita UI, input routing, wiring (the binary)
  couchcast-media/      capture + display + audio (one GStreamer pipeline)
  couchcast-input/      gilrs controller reading → device-agnostic events
  couchcast-transport/  Transport trait + RemoteAction model + ADB backend
  couchcast-config/     TOML config: device, video prefs, target, button map
xtask/                  dev tasks: cargo-sources, flatpak build/lint, local CI
data/                   .desktop, AppStream MetaInfo, icon
flatpak/                Flatpak manifest
docs/                   architecture, packaging, roadmap
```

## Contributing

Contributions welcome — see [`CONTRIBUTING.md`](CONTRIBUTING.md). Run
`cargo xtask ci` before pushing (fmt, clippy, tests). Good first areas are
listed in the [roadmap](docs/ROADMAP.md): a new transport backend, keyboard
forwarding, or capture-format probing.

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in the work by you shall be dual-licensed
as above, without any additional terms or conditions.
