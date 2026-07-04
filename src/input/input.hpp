#pragma once
//! Controller input reading for Couchcast, ported from `couchcast-input`.
//!
//! The Rust version read the Steam Virtual Gamepad via `gilrs`; here we use
//! SDL3's gamepad subsystem, which likewise presents the normalized Xbox-style
//! virtual pad Steam Input exposes under Gaming Mode. SDL delivers gamepad events
//! through the main event queue, so `InputManager` translates each SDL_Event into
//! zero or more toolkit-free `PadEvent`s (the app owns the SDL loop).

#include <chrono>
#include <optional>
#include <string>
#include <vector>

#include "transport/event.hpp"

union SDL_Event;

namespace couchcast::input {

using transport::PadAxis;
using transport::PadButton;

using Clock = std::chrono::steady_clock;
using Instant = Clock::time_point;

/// A normalized controller event, free of any SDL types.
struct PadEvent {
    enum class Kind { Button, Axis, Connected, Disconnected };
    Kind kind;
    PadButton button = PadButton::South;  // Button
    bool pressed = false;                 // Button
    PadAxis axis = PadAxis::LeftStickX;   // Axis
    float value = 0.0f;                   // Axis
    std::string name;                     // Connected / Disconnected
};

/// A pure directional intent (no Activate/Back), used by the menu's cursor.
enum class NavDir { Up, Down, Left, Right };

/// Left-stick deflection past this magnitude counts as a directional press.
constexpr float LEFT_STICK_DEADZONE = 0.5f;

/// Convert a left-stick position to a directional intent, or nullopt inside the
/// dead zone. The dominant axis wins. Y is +up (already negated from SDL).
std::optional<NavDir> stick_to_nav(float x, float y);

/// Reads controllers via SDL and yields normalized PadEvents.
class InputManager {
   public:
    InputManager() = default;
    ~InputManager();

    /// Initialize the SDL gamepad subsystem. Returns false on failure.
    bool init();

    /// Translate one SDL_Event into zero or more PadEvents. Non-gamepad events
    /// yield nothing.
    std::vector<PadEvent> handle_event(const SDL_Event& event);

    /// Names of the controllers currently connected.
    std::vector<std::string> connected_names() const;

   private:
    bool owns_subsystem_ = false;
    // Synthesised digital state for the analog triggers (SDL reports them as axes
    // only; the gamepad-passthrough path wants press/release edges).
    bool left_trigger_down_ = false;
    bool right_trigger_down_ = false;
};

/// Turns a *held* direction into a stream of discrete nav steps: one immediately
/// on press, then — after an initial delay — repeats at a steady rate while held.
class NavRepeater {
   public:
    /// Advance the repeater. `desired` is the direction currently held, or
    /// nullopt if neutral. Returns the direction on ticks a step should apply.
    std::optional<NavDir> tick(Instant now, std::optional<NavDir> desired);

   private:
    std::optional<NavDir> dir_;
    std::optional<Instant> next_fire_;
    std::chrono::milliseconds initial_delay_{400};
    std::chrono::milliseconds interval_{90};
};

/// Autorepeat for a *held remote action* forwarded to the target (e.g. holding a
/// direction to keep navigating the device's own UI). Unlike NavRepeater it does
/// NOT emit on the initial press — the caller forwards that immediately to keep
/// latency low — and only produces the repeats after an initial delay while the
/// same action stays held.
class ActionRepeater {
   public:
    /// `held` is the repeatable action currently held (or nullopt if none).
    /// Returns the action on ticks where a repeat should be forwarded.
    std::optional<transport::RemoteAction> tick(
        Instant now, std::optional<transport::RemoteAction> held);

   private:
    std::optional<transport::RemoteAction> action_;
    std::optional<Instant> next_fire_;
    std::chrono::milliseconds initial_delay_{400};
    std::chrono::milliseconds interval_{90};
};

}  // namespace couchcast::input
