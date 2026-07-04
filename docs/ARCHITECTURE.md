# Architecture

Couchcast is a small C++20 application built with CMake. This document explains
how the pieces fit together and — more importantly — *why* the load-bearing
decisions were made, so future changes don't accidentally regress latency or A/V
sync.

## Source layout

A single `couchcast_core` static library holds the toolkit-agnostic subsystems;
the `couchcast` executable adds the window/render/worker/main layer on top. Both
are declared in [`CMakeLists.txt`](../CMakeLists.txt).

| Path | Responsibility |
| --- | --- |
| `src/main.cpp`, `src/app.cpp` | App bootstrap, the SDL3 window + event loop, controller-first menu, input routing, and wiring everything together. |
| `src/worker.cpp` | The transport worker: an ASIO `io_context` thread that owns all transport I/O (persistent ADB shell, reconnect). |
| `src/render/` | The Vulkan renderer: uploads each `VideoFrame` to a texture (YUV→RGB), composites the ImGui menu on top, presents SDR or scRGB HDR. |
| `src/media/` | The single GStreamer pipeline (video + audio), V4L2 device enumeration, and the `VideoFrame`s handed to the app via an `appsink` callback. Renderer-agnostic. |
| `src/input/` | SDL3 gamepad reading, normalized to a toolkit-free `PadEvent` stream, plus the `NavDir`/`NavRepeater` menu-cursor helpers. |
| `src/transport/` | The `Transport` interface, the device-agnostic `RemoteAction` vocabulary, `DeviceCapabilities`, and the pluggable backends (ADB built; log backend for development). |
| `src/config/` | `toml++` config (device, video prefs, target, editable button map) under XDG. |
| `src/ui/` | The Dear ImGui menu and the debug overlay. |
| `tests/tests.cpp` | Unit tests for the core subsystems (run with `ctest`). |

Dependency direction is a DAG: `transport` is the leaf; `config` and `input`
depend on it for the shared vocabulary; `media` depends on GStreamer (but not on
the GPU/UI stack); the `couchcast` executable depends on everything and owns the
SDL3/Vulkan/ImGui layer.

## Data flow

```
   gst streaming thread                 ┌──────── couchcast (SDL3 main thread) ─────────┐
 capture ─▶ v4l2src ─▶ decode ─▶ appsink ─▶ VideoFrame ─(mailbox + wake)─▶ Vulkan texture │
           pipewiresrc ─▶ … ─▶ autoaudiosink   (same GstPipeline / clock)                 │
                                          │  render: video quad → ImGui menu (load, no clear)│
 controller ──▶ SDL3 ─▶ PadEvent ─┬─(menu open)──▶ NavDir/NavRepeater ─▶ menu cursor       │
                                  └─(capture mode)─▶ ButtonMap ─▶ RemoteAction ──┐         │
                        └──────────────────────────────────────────────────────── │ ───────┘
                                                                                   ▼  (worker queue)
                        ┌──────────── worker (ASIO io_context thread) ────────────────────┐
                        │ Transport::send(RemoteAction) → persistent adb shell            │
                        └─────────────────────────────────────────────────────────────────┘
```

## Execution contexts, one process

- **SDL3 main thread** drives the UI, the Vulkan render loop, and controller
  polling. Gamepad state is pumped from SDL's event queue each frame; it never
  blocks. ImGui is immediate-mode over an owned `App` struct — no shared
  ownership; nothing about the UI needs to be thread-safe.
- **GStreamer streaming thread** runs the pipeline and fires the `appsink`
  callback per decoded frame. The callback builds a `VideoFrame`, drops it into a
  single-slot mailbox, and wakes the main loop (an SDL user event); the main
  thread drains + uploads it on the next redraw. A small **bus thread** polls the
  pipeline bus for error/EOS logging (no glib main loop needed).
- **ASIO worker thread** owns all transport I/O (the persistent ADB shell,
  network, reconnect). The main thread posts `RemoteAction`s to the worker's
  `io_context` with a non-blocking hand-off, so **UI never blocks on transport
  I/O** and input is dropped under backpressure rather than stalling.

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
terminates in an `appsink`; the app uploads each `VideoFrame` to a Vulkan texture
and composites the ImGui menu on top in the same render pass (loading, not
clearing, the color attachment).

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
only with a VA hardware decoder in the chain — is a planned follow-up; the Vulkan
backend is the enabler.

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
`28DE:11FF`). Couchcast reads **that** normalized pad via SDL3's gamepad API.
Reading the raw physical evdev node instead would fight Steam's remap and produce
double/ghost input.

`src/input/` maps SDL3 buttons/axes to a small `PadEvent` enum expressed in the
transport's vocabulary. The app routes events by mode:

- **Menu open** → a `NavDir` (from the D-pad or left stick, with `NavRepeater`
  hold-to-repeat) moves an owned `selected` cursor; Left/Right cycles the focused
  row's value, A activates, B closes. The menu is drawn immediately in ImGui with
  our own highlight — ImGui's built-in nav focus is bypassed.
- **Menu closed (capture mode)** → the editable `ButtonMap` turns a `PadButton`
  into a `RemoteAction`, forwarded to the worker.
- **Start + Select chord** toggles the menu. (A chord Steam reliably passes
  through, unlike the Guide/Steam button which Steam intercepts.)

## Transport: the central abstraction

```cpp
class Transport {
   public:
    virtual ~Transport() = default;
    virtual const char* name() const = 0;
    virtual DeviceCapabilities capabilities() const = 0;
    virtual bool is_connected() const = 0;
    virtual bool connect(const TargetAddr& target) = 0;
    virtual bool send(const RemoteAction& action) = 0;
    virtual void disconnect() = 0;
};
```

The methods are ordinary blocking calls, invoked only from the ASIO worker thread
(never from the UI thread). `RemoteAction` is device-agnostic (navigation, media
keys, volume/power, text, and raw gamepad passthrough). Each backend advertises
`DeviceCapabilities` and drops actions it cannot express, so a controller is
always usable regardless of the target. The active backend is a
`std::unique_ptr<Transport>` swapped at runtime.

### ADB latency is designed in, not bolted on

The naive `adb shell input keyevent N` per button is unusably slow: each `adb`
invocation opens a fresh connection, and Android's `input` binary cold-starts a
JVM (`app_process`) — ~150–400 ms **per keypress**. `AdbTransport` therefore
opens **one long-lived `adb shell`** at connect time and streams command lines
into its stdin, removing the per-keypress connection setup.

Piping `input …` lines still pays the JVM cost per call (~½–1½ s on a low-end
stick, and calls serialize behind the shell's stdin read, so bursts stack into
multi-second lag). The fast path avoids `input` entirely: at connect
`setup_evdev()` runs `getevent -pl` once to learn which `/dev/input/eventN` node
advertises each **Linux** key code we need (note: these are `KEY_*` evdev codes,
*not* the Android `KEYCODE_*` values the `input` path uses), confirms the shell
uid can write them, holds them open with `exec N>node`, and per press streams a
raw `struct input_event` tap (down, SYN, up, SYN) straight to the kernel via one
`printf`. Latency drops to roughly network RTT.

The routing is discovered, not assumed, because a node only accepts codes it
declares — nav/media keys usually land on the remote/CEC node, but gamepad
`BTN_*` codes need a controller node present (or a rooted uinput device), so
gamepad, text, and any unroutable key fall back to the `input` path. The
persistent shell remains the load-bearing decision underneath both paths.

## Flatpak / gamescope

- GStreamer core and Mesa (the Vulkan ICD the renderer needs) come from the
  `org.gnome.Platform` runtime — never bundle them (symbol clashes). The C/C++
  deps the runtime lacks (SDL3, shaderc, toml++, ASIO, Dear ImGui) are built as
  manifest modules. `--device=dri` grants GPU/Vulkan access. There is no GTK
  dependency.
- The app is a single fullscreen window; gamescope owns vsync/fullscreen. It runs
  under XWayland when launched by Steam (SDL forced to the X11 video driver) so
  Steam Input can focus-track and route the controller. Design for exactly one
  top-level window to avoid focus mis-routing.
- `finish-args` and the `--device=all` justification live in
  [`PACKAGING.md`](PACKAGING.md).

## Where the design came from

The stack and these trade-offs were chosen from a focused research pass across
six areas (capture/display, audio, controller input, forwarding transports,
Flatpak/SteamOS, and controller-navigable UI). The conclusions are folded into
this document and the roadmap.
