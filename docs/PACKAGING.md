# Packaging & SteamOS integration

Couchcast ships as a Flatpak so it installs in one click and runs cleanly inside
gamescope on SteamOS. This document covers building the Flatpak, the permission
model, and adding it to Steam Gaming Mode.

## Files

| File | Purpose |
| --- | --- |
| `flatpak/io.github.gehhilfe.Couchcast.yml` | Flatpak manifest (CMake build + bundled C/C++ deps) |
| `data/io.github.gehhilfe.Couchcast.desktop` | Desktop entry |
| `data/io.github.gehhilfe.Couchcast.metainfo.xml` | AppStream MetaInfo |
| `data/icons/io.github.gehhilfe.Couchcast.svg` | Scalable app icon |

Every filename, the AppStream `<id>`, and the app-id in code (`APP_ID`) must stay
in lockstep — Flathub's linter treats a mismatch as fatal.

## Runtime & SDK

- Runtime: `org.gnome.Platform//49` (bundles GLib, GStreamer, and Mesa — the
  Vulkan ICD the renderer needs; layered on the freedesktop base) + `org.gnome.Sdk//49`.
  Couchcast uses no GTK, but the GNOME runtime is a convenient source of GStreamer
  + Mesa; `--device=dri` grants the GPU/Vulkan access.
- C/C++ deps not in the runtime — **SDL3**, **shaderc** (`glslc`, build-time
  only), **toml++**, **ASIO**, and vendored **Dear ImGui** — are built as manifest
  modules from source. Verify each `tag`/version in the manifest before submitting.
- Codecs: `org.freedesktop.Platform.ffmpeg-full//25.08` (add-extension) supplies
  H.264/H.265/AAC for dongles that emit compressed video.

> Verify these versions against what's on Flathub before building/submitting —
> `flatpak remote-info flathub org.gnome.Platform`.

GStreamer core and Mesa come from the runtime; **never bundle
`libgstreamer`/`libvulkan`/Mesa** (symbol clashes). The video path uses GStreamer's
stock `appsink` (no custom or GTK plugin); only custom `gst` plugins, if any,
would go in `/app/lib/gstreamer-1.0`.

## Building locally

```sh
# One-time: install the runtime, SDK, codecs extension, and the builder
flatpak install flathub org.gnome.Platform//49 org.gnome.Sdk//49 \
  org.freedesktop.Platform.ffmpeg-full//25.08 org.flatpak.Builder

# Build + install
flatpak run org.flatpak.Builder --user --install --force-clean \
  build-dir flatpak/io.github.gehhilfe.Couchcast.yml

# Lint the manifest
flatpak run org.flatpak.Builder --user --lint manifest \
  flatpak/io.github.gehhilfe.Couchcast.yml
```

For iteration you can build from your working tree by swapping the manifest's
`git` source for a local `dir` source (commented in the manifest); a Flathub
submission builds from the tagged `git` source. Flathub requires offline builds,
so bundled-dependency modules must pin an exact commit/tag with a checksum.

## Permissions (`finish-args`)

```
--socket=wayland --socket=fallback-x11 --share=ipc --device=dri   # window + GPU
--socket=pulseaudio --socket=pipewire                              # audio + camera nodes
--device=all                                                       # V4L2 + controllers + USB
--share=network                                                    # ADB / adb-connect
--filesystem=~/.android:ro                                         # ADB auth key (read-only)
--system-talk-name=org.bluez                                       # Bluetooth-HID (deferred)
```

### About `--filesystem=~/.android:ro`

The target device (Fire TV / Android TV) authorizes one specific ADB RSA key —
the one your desktop `adb` generated at `~/.android/adbkey`. The Flatpak sandbox
remaps `$HOME`, so without this grant the bundled `adb` cannot read that key and
generates a fresh, *unauthorized* one; the device then rejects the connection
(most visibly when launched from the Steam gamescope session, where no host adb
server is running to fall back on). The grant exposes the host key read-only, and
the app exports `ANDROID_VENDOR_KEYS` at startup
(`AdbTransport::ensure_auth_key_env`) to point `adb` at it regardless of the
remapped `$HOME`. Read-only is sufficient: the key is only *offered* for auth;
the device stores the "always allow" decision on its side.

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
