#pragma once
//! Minimal leveled logging, replacing Rust's `tracing`.
//!
//! Verbosity is controlled by the `COUCHCAST_LOG` env var (`error`, `warn`,
//! `info`, `debug`, `trace`); the default is `info`. Messages go to stderr, and
//! additionally to a log file once `init_file_sink()` has been called (useful
//! when stderr is swallowed, e.g. launched from the Steam gamescope session).

#include <cstdio>
#include <string>
#include <string_view>

namespace couchcast::log {

enum class Level { Error = 0, Warn, Info, Debug, Trace };

// The active threshold; messages at or below this level are printed.
Level threshold();

void set_threshold(Level level);

// Emit a preformatted line at `level` (thread-safe).
void emit(Level level, std::string_view msg);

// Open a log file and mirror every subsequent message to it (in addition to
// stderr). The path is `$COUCHCAST_LOG_FILE` if set, else
// `<state>/couchcast/couchcast.log` where <state> is `$XDG_STATE_HOME` or the
// passwd home's `.local/state` — resolved from the passwd database so it lands
// in the real home even when a sandbox/gamescope session remaps `$HOME`. The
// file is truncated per launch. Call once at startup, before threads spawn; the
// resolved path is itself logged. Returns the path opened, or "" on failure.
std::string init_file_sink();

namespace detail {
template <class... Args>
std::string fmt(const char* f, Args&&... args) {
    // Small printf-style formatter. Sizes the buffer then formats.
    int n = std::snprintf(nullptr, 0, f, args...);
    if (n <= 0) return {};
    std::string s(static_cast<size_t>(n) + 1, '\0');
    std::snprintf(s.data(), s.size(), f, args...);
    s.resize(static_cast<size_t>(n));
    return s;
}
}  // namespace detail

}  // namespace couchcast::log

// printf-style logging macros. The level check short-circuits before formatting.
#define CC_LOG(level, ...)                                                   \
    do {                                                                     \
        if (static_cast<int>(level) <= static_cast<int>(                     \
                ::couchcast::log::threshold())) {                            \
            ::couchcast::log::emit(level,                                    \
                ::couchcast::log::detail::fmt(__VA_ARGS__));                 \
        }                                                                    \
    } while (0)

#define CC_ERROR(...) CC_LOG(::couchcast::log::Level::Error, __VA_ARGS__)
#define CC_WARN(...) CC_LOG(::couchcast::log::Level::Warn, __VA_ARGS__)
#define CC_INFO(...) CC_LOG(::couchcast::log::Level::Info, __VA_ARGS__)
#define CC_DEBUG(...) CC_LOG(::couchcast::log::Level::Debug, __VA_ARGS__)
#define CC_TRACE(...) CC_LOG(::couchcast::log::Level::Trace, __VA_ARGS__)
