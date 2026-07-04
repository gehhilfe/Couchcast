# Contributing to Couchcast

Thanks for your interest in improving Couchcast! This document covers how to get
a development environment running and what we expect from contributions.

## Ground rules

- Be respectful — see [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md).
- Discuss non-trivial changes in an issue before opening a large PR.
- By contributing, you agree that your contributions are dual-licensed under
  [MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE), matching the project.

## Development environment

Couchcast is a C++20 application built on GStreamer, with an SDL3 / Vulkan / Dear
ImGui render layer. You need a C++20 compiler, CMake + Ninja, `glslc` (from
shaderc), the native development libraries, and a Vulkan loader/driver. Dear
ImGui is vendored under `third_party/imgui`.

### System dependencies

On an Arch-based system (including SteamOS's dev tooling):

```sh
sudo pacman -S --needed cmake ninja gcc \
  sdl3 vulkan-headers vulkan-icd-loader shaderc \
  gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad \
  glib2 pkgconf asio tomlplusplus
```

On Debian/Ubuntu:

```sh
sudo apt install -y cmake ninja-build g++ pkg-config \
  libsdl3-dev libvulkan-dev vulkan-headers glslc \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
  libglib2.0-dev libasio-dev libtomlplusplus-dev
```

For input forwarding to a Fire TV / Android TV target during development you also
need the Android platform tools (`adb`).

### Build & run

```sh
git clone --depth 1 --branch v1.92.1 https://github.com/ocornut/imgui.git third_party/imgui
cmake -S . -B build -G Ninja
cmake --build build
./build/couchcast
```

### Before you push

```sh
cmake --build build
ctest --test-dir build --output-on-failure
```

CI runs the same build and tests; PRs must be green.

## Project layout

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for a tour of the source tree
and how video, audio, input reading and input forwarding fit together.

## Commit style

- Keep commits focused and reversible.
- Write imperative, present-tense commit subjects ("Add V4L2 device enumeration").
- Reference issues where relevant.
