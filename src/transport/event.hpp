#pragma once
//! The device-agnostic input vocabulary shared across the app.
//!
//! Ported from `couchcast-transport::event`. `RemoteAction` is modelled as a
//! tagged struct (a closed set of variants) rather than a class hierarchy, so it
//! copies cheaply and maps directly onto the Rust enum.

#include <cstdint>
#include <string>

namespace couchcast::transport {

/// A directional navigation intent — the lowest common denominator every target
/// understands.
enum class Direction { Up, Down, Left, Right };

/// Physical gamepad buttons, normalized to an Xbox-style layout.
enum class PadButton {
    South,   // A (bottom face)
    East,    // B (right face)
    North,   // Y (top face)
    West,    // X (left face)
    LeftBumper,
    RightBumper,
    LeftTrigger,
    RightTrigger,
    Select,  // View / Back / minus
    Start,   // Menu / Start / plus
    Guide,   // Guide / Steam / Home
    LeftStick,
    RightStick,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
};

/// Analog axes, normalized to [-1.0, 1.0] (triggers use [0.0, 1.0]).
enum class PadAxis {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    LeftTrigger,
    RightTrigger,
};

/// A device-agnostic action to forward to the target. Backends downgrade
/// gracefully per their DeviceCapabilities.
struct RemoteAction {
    enum class Kind {
        Navigate,
        Select,
        Back,
        Home,
        Menu,
        PlayPause,
        Play,
        Pause,
        Stop,
        Rewind,
        FastForward,
        Next,
        Previous,
        VolumeUp,
        VolumeDown,
        Mute,
        Power,
        Text,
        GamepadButton,
        Analog,
    };

    Kind kind = Kind::Select;
    Direction direction = Direction::Up;  // for Navigate
    std::string text;                     // for Text
    PadButton button = PadButton::South;  // for GamepadButton
    bool pressed = false;                 // for GamepadButton
    PadAxis axis = PadAxis::LeftStickX;   // for Analog
    float value = 0.0f;                   // for Analog

    static RemoteAction navigate(Direction d) {
        RemoteAction a;
        a.kind = Kind::Navigate;
        a.direction = d;
        return a;
    }
    static RemoteAction simple(Kind k) {
        RemoteAction a;
        a.kind = k;
        return a;
    }
    static RemoteAction make_text(std::string t) {
        RemoteAction a;
        a.kind = Kind::Text;
        a.text = std::move(t);
        return a;
    }
    static RemoteAction gamepad(PadButton b, bool pressed) {
        RemoteAction a;
        a.kind = Kind::GamepadButton;
        a.button = b;
        a.pressed = pressed;
        return a;
    }
    static RemoteAction analog(PadAxis ax, float v) {
        RemoteAction a;
        a.kind = Kind::Analog;
        a.axis = ax;
        a.value = v;
        return a;
    }

    bool operator==(const RemoteAction&) const = default;

    /// A short, human-readable label for logs and the mapping UI.
    std::string label() const;
};

// String names (snake_case) for enum values, used by config serialization.
const char* to_string(Direction d);
const char* to_string(PadButton b);
const char* to_string(PadAxis a);

bool parse_pad_button(const std::string& s, PadButton& out);
bool parse_direction(const std::string& s, Direction& out);

}  // namespace couchcast::transport
