# Architecture

Couchcast is a small Rust workspace. This document explains how the pieces fit
together and — more importantly — *why* the load-bearing decisions were made, so
future changes don't accidentally regress latency or A/V sync.

## Crates

| Crate | Kind | Responsibility |
| --- | --- | --- |
| `couchcast` | bin | winit/wgpu/egui app: window, render loop, controller-first menu, input routing, and wiring everything together. Contains the `worker` (tokio transport thread), the `render`er, and the `menu`. |
| `couchcast-media` | lib | The single GStreamer pipeline (video + audio), V4L2 device enumeration, and the `VideoFrame`s handed to the app via an `appsink` callback. Renderer-agnostic (no GPU dependency). |
| `couchcast-input` | lib | `gilrs` controller reading, normalized to a toolkit-free `PadEvent` stream, plus the `NavDir`/`NavRepeater` menu-cursor helpers. |
| `couchcast-transport` | lib | The `Transport` trait, the device-agnostic `RemoteAction` vocabulary, `DeviceCapabilities`, and the pluggable backends (ADB built; Bluetooth/CEC/Roku feature-gated placeholders). |
| `couchcast-config` | lib | TOML config (device, video prefs, target, editable button map) under XDG. |
| `xtask` | bin | Dev tooling (`cargo xtask …`): regenerate `cargo-sources.json`, Flatpak build/lint, local CI. |

Dependency direction is a DAG: `transport` is the leaf; `config` and `input`
depend on it for the shared vocabulary; `media` depends on GStreamer (but not on
the GPU/UI stack); the `couchcast` binary depends on everything and owns the
winit/wgpu/egui layer.

## Data flow

```
   gst streaming thread                 ┌──────── couchcast (winit main thread) ────────┐
 capture ─▶ v4l2src ─▶ decode ─▶ appsink ─▶ VideoFrame ─(mailbox + wake)─▶ wgpu texture  │
           pipewiresrc ─▶ … ─▶ autoaudiosink   (same GstPipeline / clock)                │
                                          │  render: video quad → egui menu (LoadOp::Load)│
 controller ──▶ gilrs ─▶ PadEvent ─┬─(menu open)──▶ NavDir/NavRepeater ─▶ menu cursor     │
                                   └─(capture mode)─▶ ButtonMap ─▶ RemoteAction ──┐       │
                        └──────────────────────────────────────────────────────── │ ─────┘
                                                                                   ▼  (tokio::mpsc)
                        ┌──────────── worker (tokio thread) ──────────────────────────────┐
                        │ Box<dyn Transport>::send(RemoteAction).await → persistent adb    │
                        └─────────────────────────────────────────────────────────────────┘
```

## Execution contexts, one process

- **winit main thread** drives the UI, the wgpu render loop, and controller
  polling. `gilrs` is polled from `about_to_wait` on an 8 ms `WaitUntil` cadence
  (~120 Hz); it never blocks. egui is immediate-mode over an owned `App` struct —
  no `Rc<RefCell>`; nothing about the UI needs to be `Send`.
- **GStreamer streaming thread** runs the pipeline and fires the `appsink`
  callback per decoded frame. The callback builds a `VideoFrame`, drops it into a
  single-slot mailbox, and wakes the winit loop via `EventLoopProxy`; the main
  thread drains + uploads it on the next redraw. A small **bus thread** polls the
  pipeline bus for error/EOS logging (no glib main loop needed).
- **tokio worker thread** owns all transport I/O (the persistent ADB shell,
  network, reconnect). The main thread hands it `RemoteAction`s over a bounded
  `tokio::mpsc` channel with a non-blocking `try_send`, so **UI never blocks on
  transport I/O** and input is dropped under backpressure rather than stalling.

## Video + audio: one pipeline, deliberately

Both branches live in a **single `GstPipeline`**:

```
v4l2src device=… do-timestamp=true ! decodebin ! videoconvert ! video/x-raw,format=NV12 ! queue leaky=downstream max-size-buffers=3 ! appsink sync=true max-buffers=1 drop=true
pipewiresrc ! queue leaky=downstream max-size-time=20ms ! audioconvert ! audioresample ! autoaudiosink
```

One pipeline means one clock and one running-time base, which is what actually
produces A/V sync — GStreamer's PTS scheduling handles it with no hand-rolled
drift correction. This is the decisive reason to keep capture on GStreamer
(rather than a bare V4L2 read): audio comes almost for free. The video branch
terminates in an `appsink`; the app uploads each `VideoFrame` to a wgpu texture
and composites the egui menu on top in the same render pass (`LoadOp::Load`).

### Latency knobs (do not remove without measuring)

GStreamer's default buffering (~200 ms) would silently defeat the low-latency
goal. The video `appsink` runs `sync=true` (release each buffer at its PTS
against the shared clock, keeping video aligned to the clock-synced audio) with
`max-buffers=1 drop=true` behind a short `leaky=downstream` queue, so only the
freshest frame is kept — ~one-frame latency for a live source. The audio sink
keeps default sync; `audioresample` absorbs the capture-card-vs-output clock
skew. (`sync=false` is the fallback if measurement shows added latency.)

The current upload path is a single system-memory copy (physically optimal for a
USB dongle, whose frames already land in RAM). The DMABUF zero-copy path — genuine
only with a VA hardware decoder in the chain — is a planned follow-up; wgpu's
Vulkan backend is the enabler.

### Known follow-ups (see ROADMAP)

- Prefer a hardware VA-API decoder (`vajpegdec`/`vah264dec`) over `decodebin`'s
  CPU path — cheap dongles usually emit MJPEG at high resolution.
- Select the PipeWire audio **node** explicitly. `pipewiresrc` with no target
  captures the system default source — the top audio footgun. Today the audio
  branch is best-effort with a video-only fallback.
- Probe the device's real caps at runtime (dongles misreport formats/framerates).

## Input: read the *virtual* pad

Under SteamOS Gaming Mode, Steam Input grabs the physical controller and
re-presents it as a virtual Xbox-style pad (the "Steam Virtual Gamepad", Valve
`28DE:11FF`). Couchcast reads **that** normalized pad via `gilrs`. Reading the raw
physical evdev node instead would fight Steam's remap and produce double/ghost
input.

`couchcast-input` maps `gilrs` buttons/axes to a small `PadEvent` enum expressed
in `couchcast-transport`'s vocabulary. The app routes events by mode:

- **Menu open** → a `NavDir` (from the D-pad or left stick, with `NavRepeater`
  hold-to-repeat) moves an owned `selected` cursor; Left/Right cycles the focused
  row's value, A activates, B closes. The menu is drawn immediately in egui with
  our own highlight — egui's focus system is bypassed.
- **Menu closed (capture mode)** → the editable `ButtonMap` turns a `PadButton`
  into a `RemoteAction`, forwarded to the worker.
- **Start + Select chord** toggles the menu. (A chord Steam reliably passes
  through, unlike the Guide/Steam button which Steam intercepts.)

## Transport: the central abstraction

```rust
#[async_trait]
pub trait Transport: Send {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> DeviceCapabilities;
    fn is_connected(&self) -> bool;
    async fn connect(&mut self, target: &TargetAddr) -> Result<()>;
    async fn send(&mut self, action: RemoteAction) -> Result<()>;
    async fn disconnect(&mut self) -> Result<()>;
}
```

`RemoteAction` is device-agnostic (navigation, media keys, volume/power, text,
and raw gamepad passthrough). Each backend advertises `DeviceCapabilities` and
drops actions it cannot express, so a controller is always usable regardless of
the target. The active backend is a `Box<dyn Transport>` swapped at runtime.

### ADB latency is designed in, not bolted on

The naive `adb shell input keyevent N` per button is unusably slow: each `adb`
invocation opens a fresh connection, and Android's `input` binary cold-starts a
JVM (`app_process`) — ~150–400 ms **per keypress**. `AdbTransport` therefore
opens **one long-lived `adb shell`** at connect time and streams command lines
into its stdin, removing the per-keypress connection setup.

Piping `input …` lines still pays the JVM cost per call; the genuinely
low-latency path is to locate the target's evdev node once (`getevent -pl`) and
stream raw `sendevent` packets (~10–30 ms). That is the next optimization (marked
`TODO(sendevent)` in the code), but the persistent shell is the load-bearing
decision and is in place from day one.

## Flatpak / gamescope

- GStreamer core and Mesa (the Vulkan ICD wgpu needs) come from the
  `org.gnome.Platform` runtime — never bundle them (symbol clashes). Only custom
  Rust `gst` plugins would go in `/app/lib/gstreamer-1.0`. `--device=dri` grants
  GPU/Vulkan access. There is no GTK/`gst-plugin-gtk4` dependency any more.
- The app is a single fullscreen window; gamescope owns vsync/fullscreen. It runs
  under XWayland when launched by Steam (winit forced to the X11 backend) so Steam
  Input can focus-track and route the controller. Design for exactly one top-level
  window to avoid focus mis-routing.
- `finish-args` and the `--device=all` justification live in
  [`PACKAGING.md`](PACKAGING.md).

## Where the design came from

The stack and these trade-offs were chosen from a focused research pass across
six areas (capture/display, audio, controller input, forwarding transports,
Flatpak/SteamOS, and controller-navigable UI). The conclusions are folded into
this document and the roadmap.
