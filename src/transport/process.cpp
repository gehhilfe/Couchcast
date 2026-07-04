#include "transport/process.hpp"

#include <arpa/inet.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <signal.h>
#include <spawn.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include <cerrno>
#include <cstring>
#include <utility>

extern char** environ;

namespace couchcast::transport {

namespace {
// Build a NULL-terminated char* argv array from the string vector.
std::vector<char*> to_argv(const std::vector<std::string>& argv) {
    std::vector<char*> out;
    out.reserve(argv.size() + 1);
    for (const auto& s : argv) out.push_back(const_cast<char*>(s.c_str()));
    out.push_back(nullptr);
    return out;
}
}  // namespace

std::optional<CommandOutput> run_capture(const std::vector<std::string>& argv) {
    if (argv.empty()) return std::nullopt;

    int pipefd[2];
    if (pipe(pipefd) != 0) return std::nullopt;

    // Capture stdout only; send the child's stderr to /dev/null. adb's parseable
    // result goes to stdout, whereas stderr carries noise we must NOT parse —
    // notably, under Steam the injected 32-bit overlay (`gameoverlayrenderer.so`)
    // makes ld.so print "... cannot be preloaded ..." to every child's stderr.
    // Merging that in once made `adb connect`'s output match our "cannot"/"failed"
    // failure heuristic and tore down a working connection.
    int devnull = open("/dev/null", O_WRONLY);
    posix_spawn_file_actions_t actions;
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_adddup2(&actions, pipefd[1], STDOUT_FILENO);
    if (devnull >= 0) {
        posix_spawn_file_actions_adddup2(&actions, devnull, STDERR_FILENO);
        posix_spawn_file_actions_addclose(&actions, devnull);
    }
    posix_spawn_file_actions_addclose(&actions, pipefd[0]);
    posix_spawn_file_actions_addclose(&actions, pipefd[1]);

    auto cargv = to_argv(argv);
    pid_t pid = -1;
    int rc = posix_spawnp(&pid, cargv[0], &actions, nullptr, cargv.data(), environ);
    posix_spawn_file_actions_destroy(&actions);
    close(pipefd[1]);
    if (devnull >= 0) close(devnull);

    if (rc != 0) {
        close(pipefd[0]);
        return std::nullopt;
    }

    std::string out;
    char buf[4096];
    ssize_t n;
    while ((n = read(pipefd[0], buf, sizeof(buf))) > 0) {
        out.append(buf, static_cast<size_t>(n));
    }
    close(pipefd[0]);

    int status = 0;
    while (waitpid(pid, &status, 0) < 0 && errno == EINTR) {
    }
    CommandOutput result;
    result.stdout_text = std::move(out);
    result.exit_code = WIFEXITED(status) ? WEXITSTATUS(status) : -1;
    return result;
}

ChildProcess::~ChildProcess() { kill(); }

ChildProcess::ChildProcess(ChildProcess&& other) noexcept
    : pid_(other.pid_), stdin_fd_(other.stdin_fd_) {
    other.pid_ = -1;
    other.stdin_fd_ = -1;
}

ChildProcess& ChildProcess::operator=(ChildProcess&& other) noexcept {
    if (this != &other) {
        kill();
        pid_ = other.pid_;
        stdin_fd_ = other.stdin_fd_;
        other.pid_ = -1;
        other.stdin_fd_ = -1;
    }
    return *this;
}

bool ChildProcess::spawn(const std::vector<std::string>& argv) {
    if (argv.empty()) return false;
    kill();

    int pipefd[2];
    if (pipe(pipefd) != 0) return false;

    // The child reads from pipefd[0] as stdin; drop its stdout/stderr.
    int devnull = open("/dev/null", O_WRONLY);
    posix_spawn_file_actions_t actions;
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_adddup2(&actions, pipefd[0], STDIN_FILENO);
    if (devnull >= 0) {
        posix_spawn_file_actions_adddup2(&actions, devnull, STDOUT_FILENO);
        posix_spawn_file_actions_adddup2(&actions, devnull, STDERR_FILENO);
        posix_spawn_file_actions_addclose(&actions, devnull);
    }
    posix_spawn_file_actions_addclose(&actions, pipefd[0]);
    posix_spawn_file_actions_addclose(&actions, pipefd[1]);

    auto cargv = to_argv(argv);
    pid_t pid = -1;
    int rc = posix_spawnp(&pid, cargv[0], &actions, nullptr, cargv.data(), environ);
    posix_spawn_file_actions_destroy(&actions);
    close(pipefd[0]);
    if (devnull >= 0) close(devnull);

    if (rc != 0) {
        close(pipefd[1]);
        return false;
    }
    pid_ = pid;
    stdin_fd_ = pipefd[1];
    return true;
}

bool ChildProcess::write_line(const std::string& line) {
    if (stdin_fd_ < 0) return false;
    std::string buf = line;
    buf.push_back('\n');
    size_t written = 0;
    while (written < buf.size()) {
        ssize_t n = write(stdin_fd_, buf.data() + written, buf.size() - written);
        if (n < 0) {
            if (errno == EINTR) continue;
            close(stdin_fd_);
            stdin_fd_ = -1;
            return false;
        }
        written += static_cast<size_t>(n);
    }
    return true;
}

bool ChildProcess::alive() const {
    if (pid_ <= 0 || stdin_fd_ < 0) return false;
    // Non-blocking reap: if the child has already exited (e.g. `adb shell`
    // bailing out on an unauthorized/offline device), collect it and report
    // dead so callers stop treating a corpse as a live connection.
    int status = 0;
    pid_t r;
    while ((r = waitpid(pid_, &status, WNOHANG)) < 0 && errno == EINTR) {
    }
    if (r == pid_) {
        pid_ = -1;
        return false;
    }
    return true;
}

void ChildProcess::kill() {
    if (stdin_fd_ >= 0) {
        close(stdin_fd_);
        stdin_fd_ = -1;
    }
    if (pid_ > 0) {
        ::kill(pid_, SIGKILL);
        int status = 0;
        while (waitpid(pid_, &status, 0) < 0 && errno == EINTR) {
        }
        pid_ = -1;
    }
}

TcpLineClient::~TcpLineClient() { close(); }

TcpLineClient::TcpLineClient(TcpLineClient&& other) noexcept : fd_(other.fd_) {
    other.fd_ = -1;
}

TcpLineClient& TcpLineClient::operator=(TcpLineClient&& other) noexcept {
    if (this != &other) {
        close();
        fd_ = other.fd_;
        other.fd_ = -1;
    }
    return *this;
}

bool TcpLineClient::connect(uint16_t port, int attempts, int recv_timeout_ms) {
    close();
    for (int attempt = 0; attempt < attempts; ++attempt) {
        if (attempt > 0) {
            struct timespec ts{0, 300 * 1000 * 1000};  // 300 ms between tries
            nanosleep(&ts, nullptr);
        }
        int fd = socket(AF_INET, SOCK_STREAM, 0);
        if (fd < 0) continue;

        sockaddr_in addr{};
        addr.sin_family = AF_INET;
        addr.sin_port = htons(port);
        addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
        if (::connect(fd, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) != 0) {
            ::close(fd);
            continue;
        }
        // Bound each request/response so a wedged server can't stall the worker.
        struct timeval tv{recv_timeout_ms / 1000, (recv_timeout_ms % 1000) * 1000};
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
        int one = 1;
        setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof(one));  // no Nagle
        fd_ = fd;
        return true;
    }
    return false;
}

bool TcpLineClient::command(const std::string& line, std::string& reply) {
    reply.clear();
    if (fd_ < 0) return false;

    std::string buf = line;
    buf.push_back('\n');
    size_t written = 0;
    while (written < buf.size()) {
        ssize_t n = write(fd_, buf.data() + written, buf.size() - written);
        if (n < 0) {
            if (errno == EINTR) continue;
            close();
            return false;
        }
        written += static_cast<size_t>(n);
    }

    // Read until the first newline: monkey answers each command with one line.
    char c;
    for (;;) {
        ssize_t n = read(fd_, &c, 1);
        if (n < 0) {
            if (errno == EINTR) continue;
            close();
            return false;
        }
        if (n == 0) {  // peer closed
            close();
            return false;
        }
        if (c == '\n') break;
        if (c != '\r') reply.push_back(c);
    }
    return true;
}

void TcpLineClient::close() {
    if (fd_ >= 0) {
        ::close(fd_);
        fd_ = -1;
    }
}

}  // namespace couchcast::transport
