#include "transport/backends/log_transport.hpp"

#include "log.hpp"

namespace couchcast::transport::backends {

bool LogTransport::connect(const TargetAddr& target) {
    target_ = target.debug();
    connected_ = true;
    CC_INFO("log transport connected: target=%s", target_.c_str());
    return true;
}

bool LogTransport::send(const RemoteAction& action) {
    CC_INFO("forward: target=%s action=%s", target_.c_str(),
            action.label().c_str());
    return true;
}

void LogTransport::disconnect() {
    connected_ = false;
    CC_INFO("log transport disconnected");
}

}  // namespace couchcast::transport::backends
