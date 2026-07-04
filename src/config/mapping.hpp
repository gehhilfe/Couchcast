#pragma once
//! The editable controller -> action button map. Ported from
//! `couchcast-config::mapping`.

#include <optional>
#include <vector>

#include "transport/event.hpp"

namespace couchcast::config {

using transport::PadButton;
using transport::RemoteAction;

struct Binding {
    PadButton button;
    RemoteAction action;
    bool operator==(const Binding&) const = default;
};

class ButtonMap {
   public:
    std::vector<Binding> bindings;

    /// The action currently bound to `button`, if any.
    const RemoteAction* action_for(PadButton button) const;

    /// Bind (or rebind) `button` to `action`.
    void set(PadButton button, RemoteAction action);

    /// Remove any binding for `button`.
    void clear(PadButton button);

    /// A sensible default mapping for driving an Android TV / Fire TV UI.
    static ButtonMap make_default();

    bool operator==(const ButtonMap&) const = default;
};

}  // namespace couchcast::config
