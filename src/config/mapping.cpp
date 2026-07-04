#include "config/mapping.hpp"

namespace couchcast::config {

using transport::Direction;

const RemoteAction* ButtonMap::action_for(PadButton button) const {
    for (const auto& b : bindings) {
        if (b.button == button) return &b.action;
    }
    return nullptr;
}

void ButtonMap::set(PadButton button, RemoteAction action) {
    for (auto& b : bindings) {
        if (b.button == button) {
            b.action = std::move(action);
            return;
        }
    }
    bindings.push_back(Binding{button, std::move(action)});
}

void ButtonMap::clear(PadButton button) {
    std::erase_if(bindings, [&](const Binding& b) { return b.button == button; });
}

ButtonMap ButtonMap::make_default() {
    using K = RemoteAction::Kind;
    ButtonMap m;
    m.bindings = {
        {PadButton::DPadUp, RemoteAction::navigate(Direction::Up)},
        {PadButton::DPadDown, RemoteAction::navigate(Direction::Down)},
        {PadButton::DPadLeft, RemoteAction::navigate(Direction::Left)},
        {PadButton::DPadRight, RemoteAction::navigate(Direction::Right)},
        {PadButton::South, RemoteAction::simple(K::Select)},
        {PadButton::East, RemoteAction::simple(K::Back)},
        {PadButton::North, RemoteAction::simple(K::Menu)},
        {PadButton::West, RemoteAction::simple(K::PlayPause)},
        {PadButton::Start, RemoteAction::simple(K::Home)},
        {PadButton::LeftBumper, RemoteAction::simple(K::Rewind)},
        {PadButton::RightBumper, RemoteAction::simple(K::FastForward)},
    };
    return m;
}

}  // namespace couchcast::config
