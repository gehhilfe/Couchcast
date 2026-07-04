#pragma once
//! What a given target can actually do, so unsupported actions are dropped
//! rather than erroring. Ported from `couchcast-transport::capabilities`.

#include "transport/event.hpp"

namespace couchcast::transport {

struct DeviceCapabilities {
    bool navigation = false;
    bool media_keys = false;
    bool volume = false;
    bool power = false;
    bool text_input = false;
    bool analog = false;
    bool raw_gamepad = false;

    static DeviceCapabilities none();
    static DeviceCapabilities android_tv();
    static DeviceCapabilities basic_remote();

    /// Whether this target can express `action`.
    bool supports(const RemoteAction& action) const;
};

}  // namespace couchcast::transport
