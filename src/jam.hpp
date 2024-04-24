/* © 2024 GSI Helmholtz Centre for Heavy Ion Research GmbH and other
 * JAM project contributors. See the top-level COPYRIGHT.md file for details.
 *
 * SPDX-License-Identifier: GPL-3.0-only */

#pragma once

#include <actions.hpp>
#include <data.hpp>
#include <process.hpp>
#include <tui.hpp>

#include <asio.hpp>
#include <asio/experimental/awaitable_operators.hpp>
#include <chrono>
#include <exception>
#include <format>
#include <simdjson.h>

namespace jam {

using namespace asio::experimental::awaitable_operators;
using namespace std::chrono_literals;

auto sample(std::chrono::milliseconds interval,
            std::invocable auto &&frame) -> asio::awaitable<void> {
  auto cs{co_await asio::this_coro::cancellation_state};
  asio::steady_timer timer(co_await asio::this_coro::executor);

  while (!cs.cancelled()) {
    timer.expires_from_now(interval);
    frame();
    try {
      co_await timer.async_wait(asio::use_awaitable);
    } catch (std::system_error &e) {
      if (e.code() == asio::error::operation_aborted) {
        continue; // cancellation is expected
      }
      throw;
    }
  }
}

auto ui_loop(auto &data, auto &shutdown) -> asio::awaitable<void> {
  jam::tui tui;
  co_await sample(17ms, [&] { tui.render_frame(data, shutdown); });
}

auto data_loop(auto &data) -> asio::awaitable<void> {
  asio::co_spawn(co_await asio::this_coro::executor, read_host_info(data.host),
                 jam::rethrow);
  co_await sample(500ms, [&, i{0}] mutable {
    data.processes[0][2] = std::format("{}", i * 2);
    ++i;
  });
}

auto async_run_tui(auto &data, auto &shutdown) -> asio::awaitable<void> {
  // FIXME: Implement proper input processing
  data.ex = co_await asio::this_coro::executor;
  co_await (ui_loop(data, shutdown) || data_loop(data));
  shutdown();
}

inline auto run_tui(auto & /* opts */) {
  jam::data data;

  asio::io_context io;
  asio::signal_set signals(io, SIGINT, SIGTERM);
  asio::cancellation_signal cancel;
  signals.async_wait([&](const std::error_code &ec, int) {
    if (!ec) {
      cancel.emit(asio::cancellation_type::terminal);
    }
  });
  auto shutdown{[&] {
    cancel.emit(asio::cancellation_type::terminal);
    signals.cancel();
  }};
  asio::co_spawn(io, jam::async_run_tui(data, shutdown),
                 asio::bind_cancellation_slot(cancel.slot(), jam::rethrow));
  try {
    io.run();
  } catch (asio::multiple_exceptions const &e) {
    // TODO: understand this
  }
}

} // namespace jam
