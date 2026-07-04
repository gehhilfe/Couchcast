#include "transport/event.hpp"

#include <array>
#include <cstdio>
#include <utility>

namespace couchcast::transport {

const char* to_string(Direction d) {
    switch (d) {
        case Direction::Up: return "up";
        case Direction::Down: return "down";
        case Direction::Left: return "left";
        case Direction::Right: return "right";
    }
    return "up";
}

const char* to_string(PadButton b) {
    switch (b) {
        case PadButton::South: return "south";
        case PadButton::East: return "east";
        case PadButton::North: return "north";
        case PadButton::West: return "west";
        case PadButton::LeftBumper: return "left_bumper";
        case PadButton::RightBumper: return "right_bumper";
        case PadButton::LeftTrigger: return "left_trigger";
        case PadButton::RightTrigger: return "right_trigger";
        case PadButton::Select: return "select";
        case PadButton::Start: return "start";
        case PadButton::Guide: return "guide";
        case PadButton::LeftStick: return "left_stick";
        case PadButton::RightStick: return "right_stick";
        case PadButton::DPadUp: return "dpad_up";
        case PadButton::DPadDown: return "dpad_down";
        case PadButton::DPadLeft: return "dpad_left";
        case PadButton::DPadRight: return "dpad_right";
    }
    return "south";
}

const char* to_string(PadAxis a) {
    switch (a) {
        case PadAxis::LeftStickX: return "left_stick_x";
        case PadAxis::LeftStickY: return "left_stick_y";
        case PadAxis::RightStickX: return "right_stick_x";
        case PadAxis::RightStickY: return "right_stick_y";
        case PadAxis::LeftTrigger: return "left_trigger";
        case PadAxis::RightTrigger: return "right_trigger";
    }
    return "left_stick_x";
}

bool parse_pad_button(const std::string& s, PadButton& out) {
    static constexpr std::array<PadButton, 17> all = {
        PadButton::South,       PadButton::East,        PadButton::North,
        PadButton::West,        PadButton::LeftBumper,  PadButton::RightBumper,
        PadButton::LeftTrigger, PadButton::RightTrigger, PadButton::Select,
        PadButton::Start,       PadButton::Guide,       PadButton::LeftStick,
        PadButton::RightStick,  PadButton::DPadUp,      PadButton::DPadDown,
        PadButton::DPadLeft,    PadButton::DPadRight};
    for (auto b : all) {
        if (s == to_string(b)) {
            out = b;
            return true;
        }
    }
    return false;
}

bool parse_direction(const std::string& s, Direction& out) {
    for (auto d : {Direction::Up, Direction::Down, Direction::Left, Direction::Right}) {
        if (s == to_string(d)) {
            out = d;
            return true;
        }
    }
    return false;
}

namespace {
const char* dir_camel(Direction d) {
    switch (d) {
        case Direction::Up: return "Up";
        case Direction::Down: return "Down";
        case Direction::Left: return "Left";
        case Direction::Right: return "Right";
    }
    return "Up";
}
}  // namespace

std::string RemoteAction::label() const {
    char buf[128];
    switch (kind) {
        case Kind::Navigate:
            std::snprintf(buf, sizeof(buf), "Navigate %s", dir_camel(direction));
            return buf;
        case Kind::Select: return "Select";
        case Kind::Back: return "Back";
        case Kind::Home: return "Home";
        case Kind::Menu: return "Menu";
        case Kind::PlayPause: return "PlayPause";
        case Kind::Play: return "Play";
        case Kind::Pause: return "Pause";
        case Kind::Stop: return "Stop";
        case Kind::Rewind: return "Rewind";
        case Kind::FastForward: return "FastForward";
        case Kind::Next: return "Next";
        case Kind::Previous: return "Previous";
        case Kind::VolumeUp: return "VolumeUp";
        case Kind::VolumeDown: return "VolumeDown";
        case Kind::Mute: return "Mute";
        case Kind::Power: return "Power";
        case Kind::Text:
            std::snprintf(buf, sizeof(buf), "Text(%s)", text.c_str());
            return buf;
        case Kind::GamepadButton:
            std::snprintf(buf, sizeof(buf), "%s %s", to_string(button),
                          pressed ? "down" : "up");
            return buf;
        case Kind::Analog:
            std::snprintf(buf, sizeof(buf), "%s = %.2f", to_string(axis), value);
            return buf;
    }
    return "?";
}

}  // namespace couchcast::transport
