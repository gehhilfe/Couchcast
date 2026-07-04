#include "input/input.hpp"

#include <SDL3/SDL.h>

#include <cmath>

#include "log.hpp"

namespace couchcast::input {

namespace {

std::optional<PadButton> map_button(SDL_GamepadButton b) {
    switch (b) {
        case SDL_GAMEPAD_BUTTON_SOUTH: return PadButton::South;
        case SDL_GAMEPAD_BUTTON_EAST: return PadButton::East;
        case SDL_GAMEPAD_BUTTON_NORTH: return PadButton::North;
        case SDL_GAMEPAD_BUTTON_WEST: return PadButton::West;
        case SDL_GAMEPAD_BUTTON_LEFT_SHOULDER: return PadButton::LeftBumper;
        case SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER: return PadButton::RightBumper;
        case SDL_GAMEPAD_BUTTON_BACK: return PadButton::Select;
        case SDL_GAMEPAD_BUTTON_START: return PadButton::Start;
        case SDL_GAMEPAD_BUTTON_GUIDE: return PadButton::Guide;
        case SDL_GAMEPAD_BUTTON_LEFT_STICK: return PadButton::LeftStick;
        case SDL_GAMEPAD_BUTTON_RIGHT_STICK: return PadButton::RightStick;
        case SDL_GAMEPAD_BUTTON_DPAD_UP: return PadButton::DPadUp;
        case SDL_GAMEPAD_BUTTON_DPAD_DOWN: return PadButton::DPadDown;
        case SDL_GAMEPAD_BUTTON_DPAD_LEFT: return PadButton::DPadLeft;
        case SDL_GAMEPAD_BUTTON_DPAD_RIGHT: return PadButton::DPadRight;
        default: return std::nullopt;
    }
}

}  // namespace

std::optional<NavDir> stick_to_nav(float x, float y) {
    if (std::fabs(x) < LEFT_STICK_DEADZONE && std::fabs(y) < LEFT_STICK_DEADZONE)
        return std::nullopt;
    if (std::fabs(x) >= std::fabs(y)) {
        return x > 0.0f ? NavDir::Right : NavDir::Left;
    }
    return y > 0.0f ? NavDir::Up : NavDir::Down;
}

InputManager::~InputManager() {
    if (owns_subsystem_) SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
}

bool InputManager::init() {
    if (!SDL_InitSubSystem(SDL_INIT_GAMEPAD)) {
        CC_ERROR("failed to initialize gamepad input: %s", SDL_GetError());
        return false;
    }
    owns_subsystem_ = true;
    return true;
}

std::vector<PadEvent> InputManager::handle_event(const SDL_Event& event) {
    std::vector<PadEvent> out;
    switch (event.type) {
        case SDL_EVENT_GAMEPAD_ADDED: {
            SDL_JoystickID id = event.gdevice.which;
            SDL_Gamepad* pad = SDL_OpenGamepad(id);
            std::string name = pad ? (SDL_GetGamepadName(pad) ? SDL_GetGamepadName(pad) : "")
                                   : "";
            CC_INFO("controller connected: %s", name.c_str());
            PadEvent e;
            e.kind = PadEvent::Kind::Connected;
            e.name = name;
            out.push_back(std::move(e));
            break;
        }
        case SDL_EVENT_GAMEPAD_REMOVED: {
            SDL_JoystickID id = event.gdevice.which;
            SDL_Gamepad* pad = SDL_GetGamepadFromID(id);
            std::string name = pad && SDL_GetGamepadName(pad) ? SDL_GetGamepadName(pad) : "";
            if (pad) SDL_CloseGamepad(pad);
            CC_INFO("controller disconnected: %s", name.c_str());
            PadEvent e;
            e.kind = PadEvent::Kind::Disconnected;
            e.name = name;
            out.push_back(std::move(e));
            break;
        }
        case SDL_EVENT_GAMEPAD_BUTTON_DOWN:
        case SDL_EVENT_GAMEPAD_BUTTON_UP: {
            auto b = map_button(static_cast<SDL_GamepadButton>(event.gbutton.button));
            if (b) {
                PadEvent e;
                e.kind = PadEvent::Kind::Button;
                e.button = *b;
                e.pressed = event.gbutton.down;
                out.push_back(std::move(e));
            }
            break;
        }
        case SDL_EVENT_GAMEPAD_AXIS_MOTION: {
            auto axis = static_cast<SDL_GamepadAxis>(event.gaxis.axis);
            float raw = static_cast<float>(event.gaxis.value) / 32767.0f;
            if (raw > 1.0f) raw = 1.0f;
            if (raw < -1.0f) raw = -1.0f;
            std::optional<PadAxis> mapped;
            float value = raw;
            switch (axis) {
                case SDL_GAMEPAD_AXIS_LEFTX: mapped = PadAxis::LeftStickX; break;
                // SDL reports +Y down; our vocabulary (like gilrs) is +Y up.
                case SDL_GAMEPAD_AXIS_LEFTY: mapped = PadAxis::LeftStickY; value = -raw; break;
                case SDL_GAMEPAD_AXIS_RIGHTX: mapped = PadAxis::RightStickX; break;
                case SDL_GAMEPAD_AXIS_RIGHTY: mapped = PadAxis::RightStickY; value = -raw; break;
                case SDL_GAMEPAD_AXIS_LEFT_TRIGGER: mapped = PadAxis::LeftTrigger; break;
                case SDL_GAMEPAD_AXIS_RIGHT_TRIGGER: mapped = PadAxis::RightTrigger; break;
                default: break;
            }
            if (mapped) {
                PadEvent e;
                e.kind = PadEvent::Kind::Axis;
                e.axis = *mapped;
                e.value = value;
                out.push_back(std::move(e));

                // Synthesise digital press/release for the analog triggers so the
                // gamepad-passthrough path (which taps a keyevent on press) works.
                auto synth = [&](bool& state, PadButton btn) {
                    bool now = value > 0.5f;
                    if (now != state) {
                        state = now;
                        PadEvent be;
                        be.kind = PadEvent::Kind::Button;
                        be.button = btn;
                        be.pressed = now;
                        out.push_back(std::move(be));
                    }
                };
                if (*mapped == PadAxis::LeftTrigger)
                    synth(left_trigger_down_, PadButton::LeftTrigger);
                else if (*mapped == PadAxis::RightTrigger)
                    synth(right_trigger_down_, PadButton::RightTrigger);
            }
            break;
        }
        default:
            break;
    }
    return out;
}

std::vector<std::string> InputManager::connected_names() const {
    std::vector<std::string> names;
    int count = 0;
    SDL_JoystickID* ids = SDL_GetGamepads(&count);
    if (ids) {
        for (int i = 0; i < count; ++i) {
            const char* n = SDL_GetGamepadNameForID(ids[i]);
            names.push_back(n ? n : "Gamepad");
        }
        SDL_free(ids);
    }
    return names;
}

std::optional<NavDir> NavRepeater::tick(Instant now, std::optional<NavDir> desired) {
    if (!desired) {
        dir_.reset();
        next_fire_.reset();
        return std::nullopt;
    }
    if (dir_ != desired) {
        // New direction -> fire once and arm the initial delay.
        dir_ = desired;
        next_fire_ = now + initial_delay_;
        return desired;
    }
    if (next_fire_ && now >= *next_fire_) {
        next_fire_ = now + interval_;
        return desired;
    }
    return std::nullopt;
}

std::optional<transport::RemoteAction> ActionRepeater::tick(
    Instant now, std::optional<transport::RemoteAction> held) {
    if (!held) {
        action_.reset();
        next_fire_.reset();
        return std::nullopt;
    }
    if (!action_ || !(*action_ == *held)) {
        // New hold began. The caller already forwarded the initial press, so wait
        // a full initial delay before the first repeat (no immediate fire here).
        action_ = held;
        next_fire_ = now + initial_delay_;
        return std::nullopt;
    }
    if (next_fire_ && now >= *next_fire_) {
        next_fire_ = now + interval_;
        return action_;
    }
    return std::nullopt;
}

}  // namespace couchcast::input
