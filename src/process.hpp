/* © 2024 GSI Helmholtz Centre for Heavy Ion Research GmbH and other
 * JAM project contributors. See the top-level COPYRIGHT.md file for details.
 * © 2024 Ole Christian Eidheim <eidheim@gmail.com> and other
 * tiny-process-library project contributors
 *
 * Based on the awesome https://gitlab.com/eidheim/tiny-process-library !! <3
 * Original SPDX-License-Identifier: MIT
 *
 * JAM project modification summary:
 * - strip platform independence, we only care for GNU/Linux
 * - rename and adjust to JAM project coding style
 * - strip threaded async_read implementation and asio-fy via `asio::*_pipe`
 *
 * SPDX-License-Identifier: GPL-3.0-only */

#pragma once

#include <algorithm>
#include <asio.hpp>
#include <asio/any_io_executor.hpp>
#include <asio/awaitable.hpp>
#include <asio/use_awaitable.hpp>
#include <bitset>
#include <cassert>
#include <cstdlib>
#include <errno.h>
#include <fcntl.h>
#include <format>
#include <functional>
#include <limits.h>
#include <memory>
#include <mutex>
#include <print>
#include <signal.h>
#include <stdexcept>
#include <string.h>
#include <string>
#include <sys/wait.h>
#include <thread>
#include <unistd.h>
#include <unordered_map>
#include <vector>

namespace jam {

// FIXME: write tests
// NOTE: for now no allocator support, hardcoded to global
template <typename Executor = asio::any_io_executor> struct basic_process {
  using executor_type = Executor;
  using string_type = std::string;
  using environment_type = std::unordered_map<string_type, string_type>;
  using arguments_type = std::vector<string_type>;

  basic_process(executor_type const &ex, arguments_type const &arguments,
           string_type const &path = string_type{},
           environment_type const &environment = environment_type{})
      : stdin_{ex}, stdout_{ex}, stderr_{ex},
        sigchld_{ex, SIGCHLD} {
    asio::readable_pipe stdin_child{ex};
    asio::writable_pipe stdout_child{ex};
    asio::writable_pipe stderr_child{ex};

    asio::connect_pipe(stdin_child, stdin_);
    asio::connect_pipe(stdout_, stdout_child);
    asio::connect_pipe(stderr_, stderr_child);

    ex.context().notify_fork(asio::io_context::fork_prepare);

    if (auto const pid = ::fork(); pid == 0) {
      ex.context().notify_fork(asio::io_context::fork_child);

      stdin_.close();
      stdout_.close();
      stderr_.close();

      ::dup2(stdin_child.native_handle(), 0);
      ::dup2(stdout_child.native_handle(), 1);
      ::dup2(stdout_child.native_handle(), 2);

      // close inherited fds
      //
      // Optimization on some systems: using 8 * 1024 (Debian's default
      // _SC_OPEN_MAX) as fd_max limit
      auto fd_max = std::min(
          8192, static_cast<int>(sysconf(_SC_OPEN_MAX))); // Truncation is safe
      if (fd_max < 0)
        fd_max = 8192;
      for (auto fd = 3; fd < fd_max; fd++) {
        close(fd);
      }

      // setpgid(0, 0); // TODO: Understand this

      if (arguments.empty())
        exit(127);

      // TODO: Search $PATH if executable is relative

      std::vector<const char *> argv_ptrs;

      argv_ptrs.reserve(arguments.size() + 1);

      for (auto &argument : arguments) {
        argv_ptrs.emplace_back(argument.c_str());
      }
      argv_ptrs.emplace_back(nullptr);

      if (!path.empty()) {
        if (chdir(path.c_str()) != 0) {
          exit(1);
        };
      }

      // TODO: Clean this case if not needed
      // if (!environment)
      //   execvp(argv_ptrs[0], const_cast<char *const *>(argv_ptrs.data()));
      // else {
      // TODO: rework
      std::vector<std::string> env_strs;
      std::vector<const char *> env_ptrs;
      env_strs.reserve(environment.size());
      env_ptrs.reserve(environment.size() + 1);
      for (const auto &e : environment) {
        env_strs.emplace_back(std::format("{}={}", e.first, e.second));
        env_ptrs.emplace_back(env_strs.back().c_str());
      }
      env_ptrs.emplace_back(nullptr);
      ::execvpe(argv_ptrs[0], const_cast<char *const *>(argv_ptrs.data()),
                const_cast<char *const *>(env_ptrs.data()));
    } else {
      ex.context().notify_fork(asio::io_context::fork_parent);

      stdin_child.close();
      stdout_child.close();
      stderr_child.close();

      child_pid_ = pid;
    }
  }

  /// Get the process id of the started process.
  auto get_pid() const noexcept { return child_pid_; }

private:
  // TODO: rework
  auto try_get_exit_status() noexcept -> bool {
    auto exit_status{-1};
    if (child_pid_ <= 0) {
      child_exit_status_ = exit_status;
      return true;
    }

    auto const pid{::waitpid(child_pid_, &exit_status, WNOHANG)};
    if (pid < 0 && errno == ECHILD) {
      // PID doesn't exist anymore, set previously sampled exit status (or -1)
      // exit_status = child_exit_status;
      return true;
    } else if (pid <= 0) {
      // Process still running (p==0) or error
      return false;
    } else {
      // store exit status for future calls
      if (exit_status >= 256)
        exit_status = exit_status >> 8;
      child_exit_status_ = exit_status;
      exited_ = true;
    }

    return true;
  }

public:
  // TODO: rewrite as more general asio async function via
  // asio::experimental::co_composed
  auto async_wait() -> asio::awaitable<int> {
    if (exited_) {
      co_return child_exit_status_;
    }

    for (;;) {
      co_await sigchld_.async_wait(asio::use_awaitable);
      if (try_get_exit_status()) {
        co_return child_exit_status_;
      }
    }
  }

  auto &get_stdin() { return stdin_; }
  auto &get_stdout() { return stdout_; }
  auto &get_stderr() { return stderr_; }

private:
  asio::writable_pipe stdin_;
  asio::readable_pipe stdout_, stderr_;
  pid_t child_pid_{};
  int child_exit_status_{};
  bool exited_{false};
  asio::signal_set sigchld_;
};

using process = basic_process<>;

} // namespace jam
