#include "log.hpp"

#include <pwd.h>
#include <sys/stat.h>
#include <unistd.h>

#include <cstdlib>
#include <cstring>
#include <ctime>
#include <mutex>

namespace couchcast::log {

namespace {
std::mutex g_mutex;

// Optional file sink, mirroring every message. nullptr until init_file_sink().
FILE* g_file = nullptr;

// Resolve the real user home from the passwd database, falling back to $HOME.
// Robust to a sandbox/gamescope session that remaps $HOME.
std::string real_home() {
    if (const struct passwd* pw = getpwuid(getuid()); pw && pw->pw_dir) {
        return pw->pw_dir;
    }
    if (const char* h = std::getenv("HOME"); h && h[0]) return h;
    return {};
}

// `mkdir -p` for the directory holding `file`.
void make_parent_dirs(const std::string& file) {
    for (size_t i = 1; i < file.size(); ++i) {
        if (file[i] == '/') mkdir(file.substr(0, i).c_str(), 0755);
    }
}

// The current wall-clock time as `YYYY-MM-DD HH:MM:SS`, for file lines.
std::string timestamp() {
    std::time_t t = std::time(nullptr);
    struct tm tm_buf;
    localtime_r(&t, &tm_buf);
    char buf[32];
    std::strftime(buf, sizeof(buf), "%Y-%m-%d %H:%M:%S", &tm_buf);
    return buf;
}

Level parse_env() {
    const char* v = std::getenv("COUCHCAST_LOG");
    if (!v) return Level::Info;
    if (std::strcmp(v, "error") == 0) return Level::Error;
    if (std::strcmp(v, "warn") == 0) return Level::Warn;
    if (std::strcmp(v, "info") == 0) return Level::Info;
    if (std::strcmp(v, "debug") == 0) return Level::Debug;
    if (std::strcmp(v, "trace") == 0) return Level::Trace;
    return Level::Info;
}

Level g_threshold = parse_env();

const char* label(Level l) {
    switch (l) {
        case Level::Error: return "ERROR";
        case Level::Warn: return "WARN ";
        case Level::Info: return "INFO ";
        case Level::Debug: return "DEBUG";
        case Level::Trace: return "TRACE";
    }
    return "?????";
}
}  // namespace

Level threshold() { return g_threshold; }

void set_threshold(Level level) { g_threshold = level; }

void emit(Level level, std::string_view msg) {
    std::lock_guard<std::mutex> lock(g_mutex);
    std::fprintf(stderr, "[%s couchcast] %.*s\n", label(level),
                 static_cast<int>(msg.size()), msg.data());
    std::fflush(stderr);
    if (g_file) {
        // File lines carry a timestamp so a captured session can be read cold.
        std::fprintf(g_file, "%s [%s] %.*s\n", timestamp().c_str(), label(level),
                     static_cast<int>(msg.size()), msg.data());
        std::fflush(g_file);
    }
}

std::string init_file_sink() {
    std::string path;
    if (const char* p = std::getenv("COUCHCAST_LOG_FILE"); p && p[0]) {
        path = p;
    } else {
        std::string base;
        if (const char* x = std::getenv("XDG_STATE_HOME"); x && x[0]) {
            base = x;
        } else {
            std::string home = real_home();
            if (home.empty()) return {};
            base = home + "/.local/state";
        }
        path = base + "/couchcast/couchcast.log";
    }

    make_parent_dirs(path);
    FILE* f = std::fopen(path.c_str(), "w");  // truncate per launch
    if (!f) {
        emit(Level::Warn, detail::fmt("logging: cannot open log file %s",
                                      path.c_str()));
        return {};
    }
    {
        std::lock_guard<std::mutex> lock(g_mutex);
        if (g_file) std::fclose(g_file);
        g_file = f;
    }
    emit(Level::Info, detail::fmt("logging to %s", path.c_str()));
    return path;
}

}  // namespace couchcast::log
