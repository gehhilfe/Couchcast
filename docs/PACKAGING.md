# Packaging & SteamOS integration

Couchcast ships as a Flatpak so it installs in one click and runs cleanly inside
gamescope on SteamOS. This document covers building the Flatpak, the permission
model, and adding it to Steam Gaming Mode.

## Files

| File | Purpose |
| --- | --- |
| `flatpak/io.github.gehhilfe.Couchcast.yml` | Flatpak manifest |
| `flatpak/cargo-sources.json` | Generated offline crate sources (not committed until first release) |
| `data/io.github.gehhilfe.Couchcast.desktop` | Desktop entry |
| `data/io.github.gehhilfe.Couchcast.metainfo.xml` | AppStream MetaInfo |
| `data/icons/io.github.gehhilfe.Couchcast.svg` | Scalable app icon |

Every filename, the AppStream `<id>`, and the app-id in code (`APP_ID`) must stay
in lockstep — Flathub's linter treats a mismatch as fatal.

## Runtime & SDK

- Runtime: `org.gnome.Platform//49` (bundles GLib, GStreamer, and Mesa — the
  Vulkan ICD wgpu needs; layered on the freedesktop base) + `org.gnome.Sdk//49`.
  Couchcast itself no longer uses GTK, but the GNOME runtime is a convenient
  source of GStreamer + Mesa; `--device=dri` grants the GPU/Vulkan access.
- Rust: `org.freedesktop.Sdk.Extension.rust-stable//25.08` — the branch **must**
  equal the runtime's freedesktop base version.
- Codecs: `org.freedesktop.Platform.ffmpeg-full//25.08` (add-extension) supplies
  H.264/H.265/AAC for dongles that emit compressed video.

> Verify these versions against what's on Flathub before building/submitting —
> `flatpak remote-info flathub org.gnome.Platform`.

GStreamer core and Mesa come from the runtime; **never bundle
`libgstreamer`/`libvulkan`/Mesa** (symbol clashes). The video path uses GStreamer's
stock `appsink` (no custom or GTK plugin); only custom Rust `gst` plugins, if any,
would go in `/app/lib/gstreamer-1.0`.

## Building locally

```sh
# One-time: install the runtime, SDK, extensions, and the builder
flatpak install flathub org.gnome.Platform//49 org.gnome.Sdk//49 \
  org.freedesktop.Sdk.Extension.rust-stable//25.08 \
  org.freedesktop.Platform.ffmpeg-full//25.08 org.flatpak.Builder

# Regenerate the offline crate list whenever Cargo.lock changes.
# Needs flatpak-cargo-generator.py from github.com/flatpak/flatpak-builder-tools
COUCHCAST_CARGO_GENERATOR=/path/to/flatpak-cargo-generator.py cargo xtask cargo-sources

# Build + install, then lint
cargo xtask flatpak-build
cargo xtask flatpak-lint
```

For iteration you can skip `cargo-sources.json` by swapping the manifest's `git`
source for a local `dir` source (commented in the manifest) — but a Flathub
submission must build offline from the generated sources.

## Permissions (`finish-args`)

```
--socket=wayland --socket=fallback-x11 --share=ipc --device=dri   # window + GPU
--socket=pulseaudio --socket=pipewire                              # audio + camera nodes
--device=all                                                       # V4L2 + controllers + USB
--share=network                                                    # ADB / adb-connect
--system-talk-name=org.bluez                                       # Bluetooth-HID (deferred)
```

### About `--device=all`

Reaching a raw V4L2 `/dev/video*` HDMI node from the sandbox requires
`--device=all` — there is still no granular `--device=v4l`/`camera` flag as of
2026. This draws Flathub reviewer scrutiny. Options:

1. **Ship with `--device=all`** and a written justification (this is a capture
   appliance whose entire purpose is reading an arbitrary HDMI capture node).
2. **Use the `org.freedesktop.portal.Camera` portal** via `pipewiresrc`, which
   would let us drop `--device=all` in favor of `--device=dri` + `--device=input`
   + `--socket=pipewire`. The catch: the portal restricts which nodes are
   pickable and may not surface an arbitrary HDMI-capture node. Under evaluation
   (see [ROADMAP](ROADMAP.md)).

`--device=input` (controllers/joysticks) and `--device=usb` (ADB-over-USB) are
subsumed by `--device=all`; if the Camera portal path works we'd list them
explicitly instead.

## AppStream / desktop validation

Flathub CI treats **warnings as fatal**. Before submitting:

```sh
appstreamcli validate --no-net data/io.github.gehhilfe.Couchcast.metainfo.xml
desktop-file-validate data/io.github.gehhilfe.Couchcast.desktop
```

Common blockers: the mandatory `<releases>` block, exact id/filename matching,
and screenshots that must be reachable public https URLs at submission time
(placeholders are in the MetaInfo — replace them with real screenshots).

## SteamOS Gaming Mode

1. In **Desktop Mode**, install the Flatpak.
2. **Steam → Games → Add a Non-Steam Game**, and point the shortcut at the
   exported app (or the `.desktop` under
   `~/.local/share/flatpak/exports/share/applications`).
3. Back in **Gaming Mode** it launches nested inside gamescope as a single
   fullscreen Wayland client.
4. Give the shortcut a **Gamepad with Keyboard/Mouse** (or Gamepad) controller
   layout — non-Steam apps get no automatic Desktop layout, so without this the
   D-pad won't drive the UI.

Notes:
- A plain freedesktop-runtime Flatpak composites cleanly; avoid nesting a
  pressure-vessel Steam Runtime, which can hang on the Steam splash.
- MangoApp/HUD is XWayland-only and its keybind is broken inside the gamescope
  Steam session — don't rely on it for a Wayland-native app.
- Design for exactly one top-level window; multiple top-levels can be
  mis-focused under gamescope.
