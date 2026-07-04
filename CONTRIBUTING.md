# Contributing to Couchcast

Thanks for your interest in improving Couchcast! This document covers how to get
a development environment running and what we expect from contributions.

## Ground rules

- Be respectful — see [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md).
- Discuss non-trivial changes in an issue before opening a large PR.
- By contributing, you agree that your contributions are dual-licensed under
  [MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE), matching the project.

## Development environment

Couchcast is a Rust application built on GStreamer and GTK4. You need a recent
stable Rust toolchain (see [`rust-toolchain.toml`](rust-toolchain.toml)) plus the
native development libraries.

### System dependencies

On an Arch-based system (including SteamOS's dev tooling):

```sh
sudo pacman -S --needed \
  gtk4 libadwaita \
  gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad \
  glib2 pkgconf
```

On Debian/Ubuntu:

```sh
sudo apt install -y \
  libgtk-4-dev libadwaita-1-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
  libglib2.0-dev pkg-config
```

For input forwarding to a Fire TV / Android TV target during development you also
need the Android platform tools (`adb`).

### Build & run

```sh
cargo build
cargo run -p couchcast
```

### Before you push

```sh
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

CI runs the same checks; PRs must be green.

## Project layout

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for a tour of the crates and
how video, audio, input reading and input forwarding fit together.

## Commit style

- Keep commits focused and reversible.
- Write imperative, present-tense commit subjects ("Add V4L2 device enumeration").
- Reference issues where relevant.
