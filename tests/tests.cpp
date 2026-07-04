//! Unit tests ported from the Rust crates' `#[cfg(test)]` modules.
//!
//! A dependency-free harness (a couple of CHECK macros) exercises the
//! toolkit-agnostic logic: transport action mapping + capabilities, the config
//! round-trip, capture-device helpers, the pipeline description, controller
//! math, and the settings-menu selection model. Run via `ctest`.

#include <unistd.h>

#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <optional>
#include <string>

#include "config/config.hpp"
#include "input/input.hpp"
#include "transport/process.hpp"
#include "media/device.hpp"
#include "media/pipeline.hpp"
#include "transport/backends/adb.hpp"
#include "transport/capabilities.hpp"
#include "ui/menu.hpp"

static int g_fail = 0;
static int g_total = 0;

#define CHECK(cond)                                                       \
    do {                                                                  \
        ++g_total;                                                        \
        if (!(cond)) {                                                    \
            ++g_fail;                                                     \
            std::fprintf(stderr, "FAIL %s:%d  %s\n", __FILE__, __LINE__, #cond); \
        }                                                                 \
    } while (0)

using namespace couchcast;
using transport::Direction;
using transport::PadButton;
using transport::RemoteAction;
using RA = transport::RemoteAction;

// --------------------------------------------------------------------------
// transport: ADB backend
// --------------------------------------------------------------------------
static void test_adb() {
    using transport::backends::AdbTransport;
    auto up = AdbTransport::action_to_line(RA::navigate(Direction::Up));
    CHECK(up && *up == "input keyevent 19");
    auto sel = AdbTransport::action_to_line(RA::simple(RA::Kind::Select));
    CHECK(sel && *sel == "input keyevent 23");
    auto back = AdbTransport::action_to_line(RA::simple(RA::Kind::Back));
    CHECK(back && *back == "input keyevent 4");

    // Analog is dropped for now.
    CHECK(!AdbTransport::action_to_line(RA::analog(transport::PadAxis::LeftStickX, 0.9f)));

    // Text is escaped: spaces -> %s, shell metacharacters stripped.
    auto t1 = AdbTransport::action_to_line(RA::make_text("hi there"));
    CHECK(t1 && *t1 == "input text hi%sthere");
    auto t2 = AdbTransport::action_to_line(RA::make_text("a;rm -rf"));
    CHECK(t2 && *t2 == "input text arm%s-rf");

    // Network target gets the default port; an explicit port is kept.
    CHECK(AdbTransport::serial_for(transport::TargetAddr::network("10.0.0.5")) ==
          "10.0.0.5:5555");
    CHECK(AdbTransport::serial_for(transport::TargetAddr::network("10.0.0.5:5678")) ==
          "10.0.0.5:5678");

    // monkey fast path: `press <keycode>`, reusing the same Android keycodes as
    // the `input` path. Text/analog have no keycode; gamepad presses map, but a
    // (never-forwarded) gamepad release does not.
    auto m_up = AdbTransport::monkey_command(RA::navigate(Direction::Up));
    CHECK(m_up && *m_up == "press 19");
    auto m_sel = AdbTransport::monkey_command(RA::simple(RA::Kind::Select));
    CHECK(m_sel && *m_sel == "press 23");
    auto m_pad = AdbTransport::monkey_command(RA::gamepad(transport::PadButton::South, true));
    CHECK(m_pad && *m_pad == "press 96");
    CHECK(!AdbTransport::monkey_command(RA::gamepad(transport::PadButton::South, false)));
    CHECK(!AdbTransport::monkey_command(RA::make_text("hi")));
    CHECK(!AdbTransport::monkey_command(RA::analog(transport::PadAxis::LeftStickX, 0.9f)));

    // evdev fast path: a key tap is four little-endian input_events (down, SYN,
    // up, SYN), each with a zeroed timeval, octal-escaped into one `printf`.
    // KEY_ENTER (28 = 0x1c) on a 64-bit device (24-byte events), fd slot 3.
    //   16 zero timeval bytes, then type/code/value:
    //   EV_KEY(1) code 28 val 1 : \001\000 \034\000 \001\000\000\000
    //   EV_SYN(0) code 0  val 0 : \000\000 \000\000 \000\000\000\000
    //   EV_KEY(1) code 28 val 0 : \001\000 \034\000 \000\000\000\000
    //   EV_SYN(0) code 0  val 0 : \000\000 \000\000 \000\000\000\000
    const std::string z16 =
        "\\000\\000\\000\\000\\000\\000\\000\\000"
        "\\000\\000\\000\\000\\000\\000\\000\\000";
    std::string expect = "printf '";
    expect += z16 + "\\001\\000\\034\\000\\001\\000\\000\\000";  // key down
    expect += z16 + "\\000\\000\\000\\000\\000\\000\\000\\000";  // SYN
    expect += z16 + "\\001\\000\\034\\000\\000\\000\\000\\000";  // key up
    expect += z16 + "\\000\\000\\000\\000\\000\\000\\000\\000";  // SYN
    expect += "' >&3";
    CHECK(AdbTransport::evdev_line_for(28, 24, 3) == expect);

    // 32-bit devices use 16-byte events (8-byte timeval); no literal '%' anywhere.
    auto line32 = AdbTransport::evdev_line_for(28, 16, 5);
    CHECK(line32.rfind("printf '", 0) == 0);
    CHECK(line32.find('%') == std::string::npos);
    CHECK(line32.substr(line32.size() - 4) == " >&5");
}

// ensure_auth_key_env exports ANDROID_VENDOR_KEYS so adb offers the host's
// already-paired key from a sandbox that hides ~/.android (Flatpak/Steam).
static void test_adb_auth_key_env() {
    using transport::backends::AdbTransport;

    // 1. An explicit ANDROID_VENDOR_KEYS is never overridden.
    setenv("ANDROID_VENDOR_KEYS", "/custom/keys", 1);
    AdbTransport::ensure_auth_key_env();
    CHECK(std::string(std::getenv("ANDROID_VENDOR_KEYS")) == "/custom/keys");

    // 2. Otherwise it either stays unset (no key found) or points at a readable
    //    key file — never a bogus/unreadable path. (The home is resolved from the
    //    passwd db, which we can't fake here, so we assert the invariant.)
    unsetenv("ANDROID_VENDOR_KEYS");
    AdbTransport::ensure_auth_key_env();
    if (const char* v = std::getenv("ANDROID_VENDOR_KEYS"); v && v[0]) {
        CHECK(access(v, R_OK) == 0);
    }
    unsetenv("ANDROID_VENDOR_KEYS");
}

// run_capture captures stdout only — child stderr (e.g. Steam's LD_PRELOAD
// ld.so warnings) must not leak into the text we parse for adb results.
static void test_run_capture_ignores_stderr() {
    auto out = transport::run_capture({"sh", "-c", "echo OUT; echo ERRNOISE 1>&2"});
    CHECK(out.has_value());
    CHECK(out->stdout_text.find("OUT") != std::string::npos);
    CHECK(out->stdout_text.find("ERRNOISE") == std::string::npos);
}

// --------------------------------------------------------------------------
// transport: capabilities
// --------------------------------------------------------------------------
static void test_capabilities() {
    auto atv = transport::DeviceCapabilities::android_tv();
    CHECK(atv.supports(RA::navigate(Direction::Up)));
    CHECK(atv.supports(RA::make_text("hi")));
    CHECK(atv.supports(RA::gamepad(PadButton::South, true)));

    auto basic = transport::DeviceCapabilities::basic_remote();
    CHECK(basic.supports(RA::simple(RA::Kind::Select)));
    CHECK(!basic.supports(RA::make_text("hi")));
    CHECK(!basic.supports(RA::analog(transport::PadAxis::LeftStickX, 0.5f)));
}

// --------------------------------------------------------------------------
// config: mapping + round-trip
// --------------------------------------------------------------------------
static void test_config() {
    using namespace couchcast::config;

    // Default mapping covers dpad + face buttons.
    ButtonMap map = ButtonMap::make_default();
    const RemoteAction* up = map.action_for(PadButton::DPadUp);
    CHECK(up && *up == RA::navigate(Direction::Up));
    const RemoteAction* south = map.action_for(PadButton::South);
    CHECK(south && *south == RA::simple(RA::Kind::Select));

    // set() rebinds in place (no duplicate).
    map.set(PadButton::South, RA::simple(RA::Kind::Home));
    const RemoteAction* s2 = map.action_for(PadButton::South);
    CHECK(s2 && *s2 == RA::simple(RA::Kind::Home));
    int count = 0;
    for (const auto& b : map.bindings)
        if (b.button == PadButton::South) ++count;
    CHECK(count == 1);

    // Round-trip a populated config through TOML.
    Config cfg;
    cfg.last_device = DeviceRef{"USB Capture", "/dev/video0"};
    cfg.media.codec = CaptureCodec::Mjpeg;
    cfg.media.width = 1920;
    cfg.media.height = 1080;
    cfg.media.framerate = 60;
    cfg.media.audio = true;
    cfg.media.hdr_output = true;
    cfg.target = TargetConfig{TransportKind::Adb, "192.168.1.42"};

    std::string path = "couchcast_test_roundtrip.toml";
    CHECK(cfg.save_to(path));
    Config back = Config::load_from(path);
    CHECK(back.last_device == cfg.last_device);
    CHECK(back.target == cfg.target);
    CHECK(back.media == cfg.media);
    CHECK(back.mapping.action_for(PadButton::DPadUp) &&
          *back.mapping.action_for(PadButton::DPadUp) == RA::navigate(Direction::Up));
    std::remove(path.c_str());

    // A media table predating hdr_output must load with HDR on, not off.
    std::string hdr_path = "couchcast_test_hdr.toml";
    {
        std::ofstream out(hdr_path);
        out << "[media]\naudio = false\n";
    }
    Config predating = Config::load_from(hdr_path);
    CHECK(predating.media.hdr_output);
    CHECK(!predating.media.audio);
    std::remove(hdr_path.c_str());
}

// --------------------------------------------------------------------------
// media: device helpers
// --------------------------------------------------------------------------
static media::CaptureFormat fmt(media::CaptureCodec c, uint32_t w, uint32_t h,
                                std::vector<uint32_t> rates) {
    return media::CaptureFormat{c, w, h, std::move(rates)};
}

static void test_device() {
    using media::CaptureCodec;
    CHECK(media::codec_from_fourcc({'N', 'V', '1', '2'}) == CaptureCodec::Nv12);
    CHECK(media::codec_from_fourcc({'P', '0', '1', '0'}) == CaptureCodec::P010);
    CHECK(media::codec_from_fourcc({'Y', 'U', '1', '2'}) == CaptureCodec::I420);
    CHECK(media::codec_from_fourcc({'M', 'J', 'P', 'G'}) == CaptureCodec::Mjpeg);
    CHECK(!media::codec_from_fourcc({'X', 'X', 'X', 'X'}));

    CHECK(std::string(media::source_caps(CaptureCodec::Nv12)) == "video/x-raw,format=NV12");
    CHECK(std::string(media::source_caps(CaptureCodec::P010)) ==
          "video/x-raw,format=P010_10LE");
    CHECK(std::string(media::source_caps(CaptureCodec::Mjpeg)) == "image/jpeg");

    std::vector<media::CaptureFormat> formats = {
        fmt(CaptureCodec::Mjpeg, 1920, 1080, {60, 30}),
        fmt(CaptureCodec::Nv12, 1280, 720, {60, 30}),
        fmt(CaptureCodec::P010, 1920, 1080, {60, 30, 25}),
        fmt(CaptureCodec::P010, 1280, 720, {60}),
    };
    auto cs = media::codecs(formats);
    CHECK(cs.size() == 3 && cs[0] == CaptureCodec::Mjpeg && cs[1] == CaptureCodec::Nv12 &&
          cs[2] == CaptureCodec::P010);
    auto res = media::resolutions(formats, CaptureCodec::P010);
    CHECK(res.size() == 2 && res[0] == std::make_pair(1920u, 1080u) &&
          res[1] == std::make_pair(1280u, 720u));
    auto fr = media::framerates(formats, CaptureCodec::P010, {1920, 1080});
    CHECK(fr.size() == 3 && fr[0] == 60 && fr[1] == 30 && fr[2] == 25);
}

// --------------------------------------------------------------------------
// media: pipeline description
// --------------------------------------------------------------------------
static void test_pipeline() {
    using media::PipelineConfig;
    auto contains = [](const std::string& s, const std::string& sub) {
        return s.find(sub) != std::string::npos;
    };

    PipelineConfig base;
    base.device_node = "/dev/video0";
    base.audio = false;

    std::string desc = media::build_description(base, false);
    CHECK(contains(desc, "v4l2src device=/dev/video0"));
    CHECK(contains(desc, "appsink name=videosink sync=false max-buffers=1 drop=true"));
    CHECK(contains(desc, "format=NV12"));
    CHECK(!contains(desc, "pipewiresrc"));

    std::string with_audio = media::build_description(base, true);
    CHECK(contains(with_audio, "pipewiresrc"));
    CHECK(contains(with_audio, "autoaudiosink"));

    PipelineConfig over = base;
    over.width = 1920;
    over.height = 1080;
    over.framerate = 60;
    std::string od = media::build_description(over, false);
    CHECK(contains(od, "width=1920") && contains(od, "height=1080") &&
          contains(od, "framerate=60/1"));
    CHECK(contains(od, "videoscale"));

    PipelineConfig mjpeg = over;
    mjpeg.codec = media::CaptureCodec::Mjpeg;
    std::string md = media::build_description(mjpeg, false);
    CHECK(contains(md, "image/jpeg,width=1920,height=1080,framerate=60/1 ! decodebin"));
    CHECK(!contains(md, "videoscale"));
    CHECK(contains(md, "video/x-raw,format=NV12 !"));

    PipelineConfig p010 = over;
    p010.codec = media::CaptureCodec::P010;
    std::string pd = media::build_description(p010, false);
    CHECK(contains(pd,
                   "video/x-raw,format=P010_10LE,width=1920,height=1080,framerate=60/1 "
                   "! decodebin"));
    CHECK(contains(pd, "videoconvert ! video/x-raw,format=P010_10LE !"));
}

// --------------------------------------------------------------------------
// input: stick + nav repeater
// --------------------------------------------------------------------------
static void test_input() {
    using input::NavDir;
    using input::NavRepeater;
    using input::stick_to_nav;

    CHECK(!stick_to_nav(0.1f, -0.2f));
    CHECK(stick_to_nav(0.9f, 0.1f) == NavDir::Right);
    CHECK(stick_to_nav(-0.9f, 0.1f) == NavDir::Left);
    CHECK(stick_to_nav(0.1f, 0.9f) == NavDir::Up);
    CHECK(stick_to_nav(0.1f, -0.9f) == NavDir::Down);

    using namespace std::chrono;
    input::Instant t0{};
    NavRepeater r;
    CHECK(r.tick(t0, NavDir::Down) == NavDir::Down);              // fires once
    CHECK(!r.tick(t0 + milliseconds(100), NavDir::Down));         // before delay
    CHECK(r.tick(t0 + milliseconds(450), NavDir::Down) == NavDir::Down);  // repeat
    CHECK(!r.tick(t0 + milliseconds(500), std::nullopt));         // release clears

    NavRepeater r2;
    CHECK(r2.tick(t0, NavDir::Up) == NavDir::Up);
    CHECK(r2.tick(t0, NavDir::Left) == NavDir::Left);             // refire on change
}

// --------------------------------------------------------------------------
// ui: menu selection model (driven through the public nav API)
// --------------------------------------------------------------------------
static std::vector<media::CaptureFormat> sample_formats() {
    using media::CaptureCodec;
    return {
        fmt(CaptureCodec::Mjpeg, 1920, 1080, {60, 30}),
        fmt(CaptureCodec::Mjpeg, 1280, 720, {60, 30}),
        fmt(CaptureCodec::Nv12, 1280, 720, {10}),
    };
}

static void test_menu() {
    using input::NavDir;
    using media::CaptureCodec;

    // set_formats seeds the selection from the persisted prefs.
    {
        ui::Menu m(0, 0, "", true, true);
        m.set_formats(sample_formats(), CaptureCodec::Mjpeg, 1920u, 1080u, 60u);
        auto [c, w, h, f] = m.capture_selection();
        CHECK(c == CaptureCodec::Mjpeg && w == 1920u && h == 1080u && f == 60u);
    }

    // Changing codec to one lacking the resolution drops res + fps to Auto.
    {
        ui::Menu m(0, 0, "", true, true);
        m.set_formats(sample_formats(), CaptureCodec::Mjpeg, 1920u, 1080u, 60u);
        m.nav(NavDir::Down, 0);   // Device -> Codec
        m.nav(NavDir::Right, 0);  // Mjpeg -> Nv12
        auto [c, w, h, f] = m.capture_selection();
        CHECK(c == CaptureCodec::Nv12 && !w && !h && !f);
    }

    // A shared resolution survives the codec switch; an unshared fps falls to Auto.
    {
        ui::Menu m(0, 0, "", true, true);
        m.set_formats(sample_formats(), CaptureCodec::Mjpeg, 1280u, 720u, 60u);
        m.nav(NavDir::Down, 0);
        m.nav(NavDir::Right, 0);  // -> Nv12
        auto [c, w, h, f] = m.capture_selection();
        CHECK(c == CaptureCodec::Nv12 && w == 1280u && h == 720u && !f);
    }

    // The HDR toggle is inert until the renderer reports availability.
    {
        ui::Menu m(0, 0, "", true, true);
        for (int i = 0; i < 7; ++i) m.nav(NavDir::Down, 0);  // -> Hdr row
        CHECK(m.nav(NavDir::Right, 0).kind == ui::MenuAction::Kind::None);
        m.set_hdr(true, true);
        auto a1 = m.nav(NavDir::Right, 0);
        CHECK(a1.kind == ui::MenuAction::Kind::SetHdrOutput && a1.on == false);
        auto a2 = m.nav(NavDir::Right, 0);
        CHECK(a2.kind == ui::MenuAction::Kind::SetHdrOutput && a2.on == true);
    }

    // Auto codec forces Auto resolution and framerate.
    {
        ui::Menu m(0, 0, "", true, true);
        m.set_formats(sample_formats(), std::nullopt, std::nullopt, std::nullopt,
                      std::nullopt);
        auto [c, w, h, f] = m.capture_selection();
        CHECK(!c && !w && !h && !f);
    }
}

int main() {
    test_adb();
    test_adb_auth_key_env();
    test_run_capture_ignores_stderr();
    test_capabilities();
    test_config();
    test_device();
    test_pipeline();
    test_input();
    test_menu();

    std::printf("%d/%d checks passed\n", g_total - g_fail, g_total);
    return g_fail == 0 ? 0 : 1;
}
