#pragma once
//! A no-op transport that just logs every action. Handy for developing the UI
//! and input pipeline without a real device attached.

#include <optional>
#include <string>

#include "transport/transport.hpp"

namespace couchcast::transport::backends {

class LogTransport final : public Transport {
   public:
    const char* name() const override { return "log"; }
    DeviceCapabilities capabilities() const override {
        return DeviceCapabilities::android_tv();
    }
    bool is_connected() const override { return connected_; }
    bool connect(const TargetAddr& target) override;
    bool send(const RemoteAction& action) override;
    void disconnect() override;

   private:
    bool connected_ = false;
    std::string target_;
};

}  // namespace couchcast::transport::backends
