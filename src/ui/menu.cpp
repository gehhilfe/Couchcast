#include "ui/menu.hpp"

#include <imgui.h>

#include <algorithm>
#include <cstdio>
#include <cstdlib>

namespace couchcast::ui {

const std::array<TransportChoice, 2> TRANSPORT_CHOICES = {{
    {"ADB - Fire TV / Android TV", TransportKind::Adb},
    {"Log (debug, no device)", TransportKind::Log},
}};

namespace {
// The rows, in vertical order. `selected_` indexes into this list.
enum class Row {
    Device,
    Codec,
    Resolution,
    Framerate,
    Transport,
    Address,
    Audio,
    Hdr,
    Connect,
    Close,
};
constexpr std::array<Row, 10> ROWS = {Row::Device,    Row::Codec,     Row::Resolution,
                                      Row::Framerate, Row::Transport, Row::Address,
                                      Row::Audio,     Row::Hdr,       Row::Connect,
                                      Row::Close};

size_t wrap(size_t idx, int delta, size_t len) {
    if (len == 0) return 0;
    int n = static_cast<int>(len);
    return static_cast<size_t>((((static_cast<int>(idx) + delta) % n) + n) % n);
}
}  // namespace

Menu::Menu(size_t device_idx, size_t transport_idx, std::string address, bool audio,
           bool hdr_output)
    : address(std::move(address)),
      device_idx_(device_idx),
      transport_idx_(transport_idx),
      audio_(audio),
      hdr_output_(hdr_output) {
    // Dev hook: start with the menu open to exercise its drawing.
    open = std::getenv("COUCHCAST_MENU_OPEN") != nullptr;
}

void Menu::set_formats(std::vector<CaptureFormat> formats,
                       std::optional<CaptureCodec> codec, std::optional<uint32_t> width,
                       std::optional<uint32_t> height,
                       std::optional<uint32_t> framerate) {
    formats_ = std::move(formats);
    codec_opts_.clear();
    codec_opts_.push_back(std::nullopt);
    for (auto c : media::codecs(formats_)) codec_opts_.push_back(c);
    codec_idx_ = 0;
    if (codec) {
        for (size_t i = 0; i < codec_opts_.size(); ++i)
            if (codec_opts_[i] == codec) {
                codec_idx_ = i;
                break;
            }
    }

    recompute_res_opts();
    res_idx_ = 0;
    if (width && height) {
        auto want = std::make_pair(*width, *height);
        for (size_t i = 0; i < res_opts_.size(); ++i)
            if (res_opts_[i] == want) {
                res_idx_ = i;
                break;
            }
    }

    recompute_fps_opts();
    fps_idx_ = 0;
    if (framerate) {
        for (size_t i = 0; i < fps_opts_.size(); ++i)
            if (fps_opts_[i] == framerate) {
                fps_idx_ = i;
                break;
            }
    }
}

void Menu::set_hdr(bool available, bool active) {
    hdr_available_ = available;
    hdr_output_ = active;
}

std::optional<CaptureCodec> Menu::current_codec() const {
    return codec_idx_ < codec_opts_.size() ? codec_opts_[codec_idx_] : std::nullopt;
}
std::optional<std::pair<uint32_t, uint32_t>> Menu::current_res() const {
    return res_idx_ < res_opts_.size() ? res_opts_[res_idx_] : std::nullopt;
}
std::optional<uint32_t> Menu::current_fps() const {
    return fps_idx_ < fps_opts_.size() ? fps_opts_[fps_idx_] : std::nullopt;
}

void Menu::recompute_res_opts() {
    res_opts_.clear();
    res_opts_.push_back(std::nullopt);
    if (auto c = current_codec()) {
        for (auto r : media::resolutions(formats_, *c)) res_opts_.push_back(r);
    }
    if (res_idx_ >= res_opts_.size()) res_idx_ = 0;
}

void Menu::recompute_fps_opts() {
    fps_opts_.clear();
    fps_opts_.push_back(std::nullopt);
    auto c = current_codec();
    auto r = current_res();
    if (c && r) {
        for (auto f : media::framerates(formats_, *c, *r)) fps_opts_.push_back(f);
    }
    if (fps_idx_ >= fps_opts_.size()) fps_idx_ = 0;
}

void Menu::on_codec_changed() {
    auto prev = current_res();
    recompute_res_opts();
    res_idx_ = 0;
    if (prev) {
        for (size_t i = 0; i < res_opts_.size(); ++i)
            if (res_opts_[i] == prev) {
                res_idx_ = i;
                break;
            }
    }
    on_res_changed();
}

void Menu::on_res_changed() {
    auto prev = current_fps();
    recompute_fps_opts();
    fps_idx_ = 0;
    if (prev) {
        for (size_t i = 0; i < fps_opts_.size(); ++i)
            if (fps_opts_[i] == prev) {
                fps_idx_ = i;
                break;
            }
    }
}

std::tuple<std::optional<CaptureCodec>, std::optional<uint32_t>,
           std::optional<uint32_t>, std::optional<uint32_t>>
Menu::capture_selection() const {
    std::optional<uint32_t> w, h;
    if (auto r = current_res()) {
        w = r->first;
        h = r->second;
    }
    return {current_codec(), w, h, current_fps()};
}

MenuAction Menu::capture_action() const {
    auto [codec, w, h, fps] = capture_selection();
    MenuAction a;
    a.kind = MenuAction::Kind::SetCapture;
    a.codec = codec;
    a.width = w;
    a.height = h;
    a.framerate = fps;
    return a;
}

TransportKind Menu::selected_transport() const {
    if (transport_idx_ < TRANSPORT_CHOICES.size())
        return TRANSPORT_CHOICES[transport_idx_].kind;
    return TransportKind::Adb;
}

void Menu::toggle_open() {
    open = !open;
    if (!open) editing_address = false;
}

MenuAction Menu::nav(NavDir dir, size_t device_count) {
    switch (dir) {
        case NavDir::Up:
            selected_ = (selected_ + ROWS.size() - 1) % ROWS.size();
            return MenuAction::none();
        case NavDir::Down:
            selected_ = (selected_ + 1) % ROWS.size();
            return MenuAction::none();
        case NavDir::Left:
            return cycle(-1, device_count);
        case NavDir::Right:
            return cycle(1, device_count);
    }
    return MenuAction::none();
}

MenuAction Menu::cycle(int delta, size_t device_count) {
    switch (ROWS[selected_]) {
        case Row::Device:
            if (device_count > 0) {
                device_idx_ = wrap(device_idx_, delta, device_count);
                MenuAction a;
                a.kind = MenuAction::Kind::SelectDevice;
                a.device_index = device_idx_;
                return a;
            }
            return MenuAction::none();
        case Row::Codec:
            codec_idx_ = wrap(codec_idx_, delta, codec_opts_.size());
            on_codec_changed();
            return capture_action();
        case Row::Resolution:
            res_idx_ = wrap(res_idx_, delta, res_opts_.size());
            on_res_changed();
            return capture_action();
        case Row::Framerate:
            fps_idx_ = wrap(fps_idx_, delta, fps_opts_.size());
            return capture_action();
        case Row::Transport:
            transport_idx_ = wrap(transport_idx_, delta, TRANSPORT_CHOICES.size());
            return MenuAction::none();
        case Row::Audio: {
            audio_ = !audio_;
            MenuAction a;
            a.kind = MenuAction::Kind::SetAudio;
            a.on = audio_;
            return a;
        }
        case Row::Hdr:
            if (hdr_available_) {
                hdr_output_ = !hdr_output_;
                MenuAction a;
                a.kind = MenuAction::Kind::SetHdrOutput;
                a.on = hdr_output_;
                return a;
            }
            return MenuAction::none();
        default:
            return MenuAction::none();
    }
}

MenuAction Menu::activate() {
    switch (ROWS[selected_]) {
        case Row::Address:
            editing_address = true;
            focus_address_ = true;
            return MenuAction::none();
        case Row::Connect: {
            MenuAction a;
            a.kind = MenuAction::Kind::Connect;
            return a;
        }
        case Row::Close: {
            MenuAction a;
            a.kind = MenuAction::Kind::Close;
            return a;
        }
        default:
            return MenuAction::none();
    }
}

MenuAction Menu::back() {
    if (editing_address) {
        editing_address = false;
        return MenuAction::none();
    }
    MenuAction a;
    a.kind = MenuAction::Kind::Close;
    return a;
}

void Menu::draw(const std::vector<CaptureDevice>& devices, const std::string& status) {
    const ImU32 accent = IM_COL32(0x3a, 0x8e, 0xff, 255);
    ImGuiIO& io = ImGui::GetIO();
    ImVec2 screen = io.DisplaySize;

    float panel_w = std::min(720.0f, screen.x - 96.0f);
    ImGui::SetNextWindowPos(ImVec2(screen.x * 0.5f, screen.y * 0.12f),
                            ImGuiCond_Always, ImVec2(0.5f, 0.0f));
    ImGui::SetNextWindowSize(ImVec2(panel_w + 56.0f, 0.0f), ImGuiCond_Always);
    ImGui::PushStyleColor(ImGuiCol_WindowBg, IM_COL32(0, 0, 0, 210));
    ImGui::PushStyleVar(ImGuiStyleVar_WindowRounding, 16.0f);
    ImGui::PushStyleVar(ImGuiStyleVar_WindowPadding, ImVec2(28, 28));

    ImGuiWindowFlags flags = ImGuiWindowFlags_NoTitleBar | ImGuiWindowFlags_NoResize |
                             ImGuiWindowFlags_NoMove | ImGuiWindowFlags_NoCollapse |
                             ImGuiWindowFlags_NoScrollbar |
                             ImGuiWindowFlags_AlwaysAutoResize;
    ImGui::Begin("##couchcast-menu", nullptr, flags);

    // Title.
    {
        const char* title = "Couchcast";
        float tw = ImGui::CalcTextSize(title).x * 1.6f;
        ImGui::SetCursorPosX((ImGui::GetWindowSize().x - tw) * 0.5f);
        ImGui::SetWindowFontScale(1.6f);
        ImGui::TextUnformatted(title);
        ImGui::SetWindowFontScale(1.0f);
    }
    ImGui::Dummy(ImVec2(0, 12));

    const char* device_name = device_idx_ < devices.size()
                                  ? devices[device_idx_].name.c_str()
                                  : "(no capture device)";
    const char* transport_label = transport_idx_ < TRANSPORT_CHOICES.size()
                                      ? TRANSPORT_CHOICES[transport_idx_].label
                                      : "ADB";

    char codec_val[32], res_val[32], fps_val[32];
    if (auto c = current_codec())
        std::snprintf(codec_val, sizeof(codec_val), "%s", media::codec_label(*c));
    else
        std::snprintf(codec_val, sizeof(codec_val), "Auto");
    if (auto r = current_res())
        std::snprintf(res_val, sizeof(res_val), "%ux%u", r->first, r->second);
    else
        std::snprintf(res_val, sizeof(res_val), "Auto");
    if (auto f = current_fps())
        std::snprintf(fps_val, sizeof(fps_val), "%u fps", *f);
    else
        std::snprintf(fps_val, sizeof(fps_val), "Auto");

    const char* hdr_val = !hdr_available_ ? "Unavailable" : (hdr_output_ ? "On" : "Off");

    auto value_row = [&](const char* label, const char* value, bool selected) {
        ImDrawList* dl = ImGui::GetWindowDrawList();
        ImVec2 p0 = ImGui::GetCursorScreenPos();
        float row_w = ImGui::GetContentRegionAvail().x;
        float row_h = ImGui::GetTextLineHeight() + 20.0f;
        if (selected) {
            dl->AddRectFilled(p0, ImVec2(p0.x + row_w, p0.y + row_h),
                              IM_COL32(0x3a, 0x8e, 0xff, 76), 10.0f);
            dl->AddRect(p0, ImVec2(p0.x + row_w, p0.y + row_h), accent, 10.0f, 0, 2.5f);
        }
        ImGui::Dummy(ImVec2(0, 6));
        ImGui::SameLine(16.0f);
        ImGui::TextUnformatted(label);
        char shown[64];
        std::snprintf(shown, sizeof(shown), "<  %s  >", value);
        float vw = ImGui::CalcTextSize(shown).x;
        ImGui::SameLine(row_w - vw - 8.0f);
        ImGui::TextUnformatted(shown);
        ImGui::Dummy(ImVec2(0, 8));
    };

    auto action_row = [&](const char* label, bool selected) {
        ImDrawList* dl = ImGui::GetWindowDrawList();
        ImVec2 p0 = ImGui::GetCursorScreenPos();
        float row_w = ImGui::GetContentRegionAvail().x;
        float row_h = ImGui::GetTextLineHeight() + 20.0f;
        if (selected) {
            dl->AddRectFilled(p0, ImVec2(p0.x + row_w, p0.y + row_h),
                              IM_COL32(0x3a, 0x8e, 0xff, 76), 10.0f);
            dl->AddRect(p0, ImVec2(p0.x + row_w, p0.y + row_h), accent, 10.0f, 0, 2.5f);
        }
        ImGui::Dummy(ImVec2(0, 6));
        float tw = ImGui::CalcTextSize(label).x;
        ImGui::SetCursorPosX((ImGui::GetWindowSize().x - tw) * 0.5f);
        ImGui::TextUnformatted(label);
        ImGui::Dummy(ImVec2(0, 8));
    };

    for (size_t i = 0; i < ROWS.size(); ++i) {
        bool selected = (i == selected_);
        switch (ROWS[i]) {
            case Row::Device: value_row("Capture device", device_name, selected); break;
            case Row::Codec: value_row("Format", codec_val, selected); break;
            case Row::Resolution: value_row("Resolution", res_val, selected); break;
            case Row::Framerate: value_row("Framerate", fps_val, selected); break;
            case Row::Transport:
                value_row("Forward input to", transport_label, selected);
                break;
            case Row::Address: {
                ImDrawList* dl = ImGui::GetWindowDrawList();
                ImVec2 p0 = ImGui::GetCursorScreenPos();
                float row_w = ImGui::GetContentRegionAvail().x;
                float row_h = ImGui::GetTextLineHeight() + 20.0f;
                if (selected) {
                    dl->AddRectFilled(p0, ImVec2(p0.x + row_w, p0.y + row_h),
                                      IM_COL32(0x3a, 0x8e, 0xff, 76), 10.0f);
                    dl->AddRect(p0, ImVec2(p0.x + row_w, p0.y + row_h), accent, 10.0f, 0,
                                2.5f);
                }
                ImGui::Dummy(ImVec2(0, 6));
                ImGui::SameLine(16.0f);
                ImGui::TextUnformatted("Target address");
                ImGui::SameLine(row_w - 280.0f);
                if (editing_address) {
                    ImGui::SetNextItemWidth(260.0f);
                    char buf[256];
                    std::snprintf(buf, sizeof(buf), "%s", address.c_str());
                    if (focus_address_) {
                        ImGui::SetKeyboardFocusHere();
                        focus_address_ = false;
                    }
                    if (ImGui::InputText("##addr", buf, sizeof(buf))) address = buf;
                } else {
                    ImGui::TextUnformatted(address.empty()
                                               ? "[ press A to edit ]"
                                               : address.c_str());
                }
                ImGui::Dummy(ImVec2(0, 8));
                break;
            }
            case Row::Audio:
                value_row("Audio passthrough", audio_ ? "On" : "Off", selected);
                break;
            case Row::Hdr: value_row("HDR output", hdr_val, selected); break;
            case Row::Connect: action_row("Connect", selected); break;
            case Row::Close: action_row("Close", selected); break;
        }
    }

    ImGui::Dummy(ImVec2(0, 8));
    ImGui::TextDisabled("%s", status.c_str());
    ImGui::Dummy(ImVec2(0, 4));
    ImGui::TextColored(ImVec4(0.78f, 0.78f, 0.78f, 1.0f),
                       "(A) Select   (B) Back   <  > Change   Steam+X to type");

    ImGui::End();
    ImGui::PopStyleVar(2);
    ImGui::PopStyleColor();
}

}  // namespace couchcast::ui
