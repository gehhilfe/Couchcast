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
 │ Fire TV / │ ────────▶ │ HDMI capture  │ ───────▶ │  v4l2src → decode → appsink → Vulkan texture │
 │ streaming │           │ dongle (V4L2) │          │  pipewiresrc → autoaudiosink   (one         │
 │  stick    │ ◀───────┐ └───────────────┘          │                                 pipeline)   │
 └───────────┘         │                            │  SDL3 pad ─▶ button map ─▶ Transport ──┐     │
        ▲              │  ADB over Wi-Fi/USB         └────────────────────────────────────────┼────┘
        └──────────────┴────────────────────────────────────────────────────────────────────┘
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
- [x] Fullscreen low-latency video (GStreamer `appsink` → Vulkan texture; DMABUF zero-copy path planned)
- [x] Audio passthrough kept in A/V sync (shared GStreamer pipeline/clock)

Input forwarding
- [x] Read the controller via SDL3's gamepad API (the Steam Virtual Gamepad under Gaming Mode)
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

Couchcast is a modern **C++20** app (`CMakeLists.txt`, `src/`, `shaders/`), built
with CMake. Each subsystem uses a focused C/C++ library; the load-bearing design
decisions are documented in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

| Concern | Library |
| --- | --- |
| Language | C++20 (CMake) |
| Video + audio | GStreamer (C API) — one pipeline, one clock |
| Window | SDL3 |
| GPU | Vulkan directly (YUV→RGB, SDR + scRGB HDR) |
| UI | Dear ImGui (controller-first 10-foot menu) |
| Controller input | SDL3 gamepad (the Steam Virtual Gamepad) |
| Transport worker | ASIO (standalone) |
| Config | `toml++` |
| Input forwarding | `Transport` interface; ADB backend |
| Packaging | Flatpak → Flathub → SteamOS Gaming Mode |

## Build from source

You need a C++20 compiler, CMake + Ninja, the GStreamer development libraries, a
Vulkan loader/driver, and `glslc` (from shaderc). Dear ImGui is vendored under
`third_party/imgui`.

```sh
# Arch / SteamOS build dependencies
sudo pacman -S --needed cmake ninja gcc \
  sdl3 vulkan-headers vulkan-icd-loader shaderc \
  gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad \
  glib2 pkgconf asio tomlplusplus v4l-utils

# Debian / Ubuntu build dependencies
sudo apt install -y cmake ninja-build g++ pkg-config \
  libsdl3-dev libvulkan-dev vulkan-headers glslc \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
  libglib2.0-dev libasio-dev libtomlplusplus-dev v4l-utils

# Vendor Dear ImGui (once)
git clone --depth 1 --branch v1.92.1 https://github.com/ocornut/imgui.git third_party/imgui

# Configure, build, test, run
cmake -S . -B build -G Ninja
cmake --build build
ctest --test-dir build            # runs the ported unit tests
./build/couchcast
```

Useful env toggles: `COUCHCAST_WINDOWED=1`
(windowed 1280×720 instead of fullscreen), `COUCHCAST_TEST_SOURCE=1` (synthetic
`videotestsrc` instead of a capture device), `COUCHCAST_DEBUG=1` (start with the
debug overlay visible), `COUCHCAST_LOG=debug` (verbosity), and
`COUCHCAST_VK_VALIDATION=1` (enable Vulkan validation layers).

For input forwarding to a Fire TV during development you also need `adb` on your
`PATH`. Turn on **Settings → My Fire TV → Developer Options → ADB Debugging** on
the device, note its IP, then set it in Couchcast's settings screen.

## Using it

- Open the settings overlay any time with **Start + Select** on your controller.
- Pick your capture device and enter the target's IP, then **Connect**.
- Close the overlay; your controller now drives the streaming stick. Remap
  buttons in the settings screen, or edit `~/.config/couchcast/config.toml`.

Verbose logs: `COUCHCAST_LOG=debug ./build/couchcast`.

Toggle an on-screen **debug overlay** any time with **F3** (or click **L3 + R3**
on the controller) — no rebuild needed. It shows render/capture FPS, the live
frame resolution and pixel format, the selected device and requested capture
mode, the GPU adapter, the transport target, and the controller diagnostics from
the old input HUD (connected pads, held buttons, and stick position) — handy for
confirming what Steam Input actually forwards, including the Guide/Steam button
under Gaming Mode / Big Picture. Start it already visible with
`COUCHCAST_DEBUG=1`.

## Install on SteamOS (Flatpak)

Once released on Flathub this will be a one-click install; until then you can
build the Flatpak locally — see [`docs/PACKAGING.md`](docs/PACKAGING.md). In
short: build with `flatpak run org.flatpak.Builder --user --install --force-clean
build-dir flatpak/io.github.gehhilfe.Couchcast.yml`, then in Desktop Mode add it
via **Steam → Add a Non-Steam Game**, and give the shortcut a Gamepad/Keyboard
controller layout so the UI is navigable in Gaming Mode.

## Project layout

```
src/
  main.cpp app.cpp worker.cpp  app entry, wiring, and the ASIO transport worker
  render/                      Vulkan renderer (YUV→RGB, SDR + scRGB HDR)
  media/                       capture + audio + frame export (one GStreamer pipeline → appsink)
  input/                       SDL3 gamepad reading → device-agnostic events
  transport/                   Transport interface + RemoteAction model + ADB backend
  config/                      TOML config: device, video prefs, target, button map
  ui/                          controller-first ImGui menu + debug overlay
shaders/                       GLSL video shaders (compiled to SPIR-V by glslc)
third_party/imgui/             vendored Dear ImGui (cloned separately, see build steps)
tests/                         unit tests (run with ctest)
data/                          .desktop, AppStream MetaInfo, icon
flatpak/                       Flatpak manifest
docs/                          architecture, packaging, roadmap
```

## Contributing

Contributions welcome — see [`CONTRIBUTING.md`](CONTRIBUTING.md). Build and run
the tests (`cmake --build build && ctest --test-dir build`) before pushing. Good
first areas are listed in the [roadmap](docs/ROADMAP.md): a new transport
backend, keyboard forwarding, or capture-format probing.

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in the work by you shall be dual-licensed
as above, without any additional terms or conditions.
