# Roadmap

Couchcast is an early scaffold: the structure and every subsystem compile, but it
has not been validated end-to-end on real capture hardware. This is the ordered
list of what turns it into a working, shippable app.

## Milestone 1 — first end-to-end vertical slice

The smallest slice that exercises every load-bearing subsystem on real hardware:

- [ ] Play live video **and** PCM audio from an HDMI capture dongle, A/V-synced,
      through the single GStreamer pipeline.
- [ ] Read the Steam Virtual Gamepad via SDL3 and forward D-pad + Select/Back
      to a Fire TV over ADB-over-TCP using the persistent shell.
- [ ] Confirm the settings overlay is fully navigable with a controller.

This proves capture, audio, input-read, input-forward, the `Transport`
abstraction, and the Flatpak sandbox all work together.

## Media

- [ ] **Runtime caps probing** — cheap dongles misreport formats/framerates;
      enumerate real caps and pick a working one instead of trusting defaults.
- [ ] **Hardware decode** — prefer VA-API (`vajpegdec`/`vah264dec`/`vapostproc`)
      over `decodebin`'s CPU path to keep per-frame decode off the hot path.
- [ ] **Explicit audio node selection** — target the capture card's PipeWire node
      by name/serial instead of relying on `pipewiresrc`'s default source.
- [ ] **DMABUF zero-copy validation** — verify graphics-offload on AMD (Steam
      Deck) and the GL-texture fallback on NVIDIA.
- [ ] Measure and, if needed, add per-branch latency compensation for A/V lock.
- [x] **True HDR output** — P010 presents to a scRGB (`Rgba16Float`,
      extended-sRGB-linear) swapchain when the surface advertises one, passing HDR
      through instead of tone-mapping; falls back to the SDR tone-map otherwise.
      Toggle in the settings menu (`media.hdr_output`).
- [ ] **HDR output polish** — remaining items on the HDR path:
      - Drive the SDR tone-map peak from the stream's HDR10 mastering / `MaxCLL`
        metadata instead of the fixed 1000-nit assumption.
      - Pass BT.2020 wide gamut through in HDR mode (currently the BT.2020→BT.709
        conversion clamps out-of-709 colours) once a target-gamut probe exists.
      - The ImGui overlay renders at the scRGB reference white (~80 nits) in HDR
        mode; composite it at a configurable paper-white if it reads too dim.
      - On-device validation under gamescope HDR (SteamOS) and a Wayland
        `color-management` compositor.

## Input

- [ ] **ADB `sendevent` fast path** — `getevent -pl` once to find the evdev node,
      then stream raw packets (~10–30 ms) instead of `input keyevent` (JVM cost).
- [ ] **Analog stick forwarding** — requires the `sendevent` path (no `input`
      analog equivalent).
- [ ] **Keyboard forwarding** — the SDL3 gamepad path ignores keyboards; read via `evdev`.
- [ ] **Deterministic pad selection** — match by VID/PID/name, tolerate
      "Steam Virtual Gamepad 0/1" duplicates and Steam's controller reorder.

## Transport backends

The `Transport` interface exists; these are planned backends today.

- [ ] **Bluetooth-HID** — host advertises as a BT keyboard/gamepad; works on
      Fire TV / Android TV / Apple TV with no dev mode. Real work: BlueZ classic
      HID profile is incomplete, so expect hand-rolled SDP + L2CAP
      (PSM 0x11/0x13) or a D-Bus `Profile1`. Needs `--system-talk-name=org.bluez`.
- [ ] **Roku ECP** — trivial HTTP (`POST /keypress/<Key>`) + SSDP discovery;
      near-free once wired.
- [ ] **HDMI-CEC** — via `libcec`; needs a CEC-capable adapter (most capture
      cards don't expose the CEC line). Navigation/media only.
- [ ] **Android TV Remote v2 / Apple TV** — no off-the-shelf C/C++ client; would
      mean implementing the reverse-engineered protocols or running a sidecar.
      Defer.
- [ ] Talk the ADB wire protocol directly instead of shelling out to the system
      `adb` binary, so nothing extra needs bundling in the Flatpak.
- [ ] Device discovery (mDNS/SSDP) + reconnect/backoff + a per-device capability
      map.

## UX / settings

- [ ] Full button-remapping UI with live "press a button" capture.
- [ ] Resolution/framerate picker driven by probed caps.
- [ ] Focus traps in dropdowns/popovers (a controller must not get stuck), and a
      guaranteed initial focus on overlay open.
- [ ] First-run wizard: pick device → pick target → pair/connect.

## Packaging / distribution

- [ ] Real screenshots for the AppStream MetaInfo (Flathub requires public https
      URLs).
- [ ] Pin/verify the bundled-dependency module versions (SDL3, shaderc, toml++,
      ASIO, Dear ImGui) for a reproducible offline Flathub build.
- [ ] Flathub submission (reverse-DNS id `io.github.gehhilfe.Couchcast`).
- [ ] Document the `--device=all` justification for Flathub review, and evaluate
      the Camera portal alternative.
- [ ] On-device validation in SteamOS Gaming Mode (per-shortcut Steam Input
      layout, gamescope composition).

## Nice to have

- [ ] Multiple saved target profiles (switch between a Fire TV and, later, an
      Apple TV as a data change, not a code path).
- [ ] Optional on-screen latency/FPS overlay for tuning.
