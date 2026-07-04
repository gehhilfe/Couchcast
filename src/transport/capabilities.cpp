#include "transport/capabilities.hpp"

namespace couchcast::transport {

DeviceCapabilities DeviceCapabilities::none() { return DeviceCapabilities{}; }

DeviceCapabilities DeviceCapabilities::android_tv() {
    return DeviceCapabilities{true, true, true, true, true, true, true};
}

DeviceCapabilities DeviceCapabilities::basic_remote() {
    return DeviceCapabilities{true, true, true, true, false, false, false};
}

bool DeviceCapabilities::supports(const RemoteAction& action) const {
    using K = RemoteAction::Kind;
    switch (action.kind) {
        case K::Navigate:
        case K::Select:
        case K::Back:
        case K::Home:
        case K::Menu:
            return navigation;
        case K::PlayPause:
        case K::Play:
        case K::Pause:
        case K::Stop:
        case K::Rewind:
        case K::FastForward:
        case K::Next:
        case K::Previous:
            return media_keys;
        case K::VolumeUp:
        case K::VolumeDown:
        case K::Mute:
            return volume;
        case K::Power:
            return power;
        case K::Text:
            return text_input;
        case K::Analog:
            return analog;
        case K::GamepadButton:
            return raw_gamepad;
    }
    return false;
}

}  // namespace couchcast::transport
