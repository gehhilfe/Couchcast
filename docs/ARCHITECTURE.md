# Architecture

Couchcast is a small Rust workspace. This document explains how the pieces fit
together and — more importantly — *why* the load-bearing decisions were made, so
future changes don't accidentally regress latency or A/V sync.

## Crates

| Crate | Kind | Responsibility |
| --- | --- | --- |
| `couchcast` | bin | GTK4/libadwaita app: window, settings overlay, input routing, and wiring everything together. Contains the `worker` (tokio transport thread) and the UI. |
| `couchcast-media` | lib | The single GStreamer pipeline (video + audio), V4L2 device enumeration, and the `gdk::Paintable` handed to the UI. |
| `couchcast-input` | lib | `gilrs` controller reading, normalized to a GTK-free `PadEvent` stream plus UI `NavEvent`s. |
| `couchcast-transport` | lib | The `Transport` trait, the device-agnostic `RemoteAction` vocabulary, `DeviceCapabilities`, and the pluggable backends (ADB built; Bluetooth/CEC/Roku feature-gated placeholders). |
| `couchcast-config` | lib | TOML config (device, video prefs, target, editable button map) under XDG. |
| `xtask` | bin | Dev tooling (`cargo xtask …`): regenerate `cargo-sources.json`, Flatpak build/lint, local CI. |

Dependency direction is a DAG: `transport` is the leaf; `config` and `input`
depend on it for the shared vocabulary; `media` depends on GStreamer/GTK; the
`couchcast` binary depends on everything.

## Data flow

```
                        ┌──────────────── couchcast (GTK main thread) ────────────────┐
 capture dongle ──▶ v4l2src ─▶ decode ─▶ gtk4paintablesink ─▶ gdk::Paintable ─▶ Picture│
                    pipewiresrc ─▶ … ─▶ autoaudiosink   (same GstPipeline / clock)      │
                                                                                        │
 controller ──▶ gilrs ─▶ PadEvent ─┬─(overlay open)─▶ NavEvent ─▶ GTK focus move        │
                                    └─(capture mode)─▶ ButtonMap ─▶ RemoteAction ──┐     │
                        └───────────────────────────────────────────────────────── │ ───┘
                                                                                    ▼  (tokio::mpsc)
                        ┌──────────── worker (tokio thread) ──────────────────────────────┐
                        │ Box<dyn Transport>::send(RemoteAction).await → persistent adb    │
                        └─────────────────────────────────────────────────────────────────┘
```

## Two execution contexts, one process

- **GTK / glib main thread** drives the UI, video rendering, and controller
  polling. `gilrs` is polled from a `glib::timeout_add_local` (~120 Hz); it never
  blocks.
- **tokio worker thread** owns all transport I/O (the persistent ADB shell,
  network, reconnect). The main thread hands it `RemoteAction`s over a bounded
  `tokio::mpsc` channel with a non-blocking `try_send`, so **UI never blocks on
  transport I/O** and input is dropped under backpressure rather than stalling.

Shared app state lives in an `Rc<RefCell<AppState>>` (single-threaded, main
thread only). Nothing about the UI needs to be `Send`.

## Video + audio: one pipeline, deliberately

Both branches live in a **single `GstPipeline`**:

```
v4l2src device=… do-timestamp=true ! decodebin ! videoconvert ! queue leaky=downstream ! gtk4paintablesink sync=false
pipewiresrc ! queue leaky=downstream max-size-time=20ms ! audioconvert ! audioresample ! autoaudiosink
```

One pipeline means one clock and one running-time base, which is what actually
produces A/V sync — GStreamer's PTS scheduling handles it with no hand-rolled
drift correction. This is the decisive reason to render video through GStreamer
(rather than a bare `waylandsink`/`wgpu` surface): audio then comes almost for
free, and `gtk4paintablesink` still lets the settings UI composite on top via
`Gtk.Overlay`.

### Latency knobs (do not remove without measuring)

GStreamer's default buffering (~200 ms) would silently defeat the low-latency
goal. The video sink runs `sync=false` behind a short `leaky=downstream` queue to
approach one-frame latency. The audio sink keeps default sync so the shared clock
preserves A/V alignment; `audioresample` absorbs the capture-card-vs-output clock
skew.

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

- **Overlay open** → `nav_from_pad` → GTK focus moves (`child_focus(direction)`),
  A activates the focused widget, B closes.
- **Overlay closed (capture mode)** → the editable `ButtonMap` turns a `PadButton`
  into a `RemoteAction`, forwarded to the worker.
- **Start + Select chord** toggles the overlay. (A chord Steam reliably passes
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

- GStreamer core and GTK come from the `org.gnome.Platform` runtime — never
  bundle them (symbol clashes). Only custom Rust `gst` plugins would go in
  `/app/lib/gstreamer-1.0`; `gtk4paintablesink` is registered in-process from the
  `gst-plugin-gtk4` crate.
- The app is a single fullscreen Wayland client; gamescope owns
  vsync/fullscreen. Design for exactly one top-level window to avoid focus
  mis-routing.
- `finish-args` and the `--device=all` justification live in
  [`PACKAGING.md`](PACKAGING.md).

## Where the design came from

The stack and these trade-offs were chosen from a focused research pass across
six areas (capture/display, audio, controller input, forwarding transports,
Flatpak/SteamOS, and controller-navigable UI). The conclusions are folded into
this document and the roadmap.
