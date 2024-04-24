/* © 2024 GSI Helmholtz Centre for Heavy Ion Research GmbH and other
 * JAM project contributors. See the top-level COPYRIGHT.md file for details.
 *
 * SPDX-License-Identifier: GPL-3.0-only */

#pragma once

#include <process.hpp>

#include <asio.hpp>
#include <asio/experimental/awaitable_operators.hpp>
#include <ranges>
#include <regex>
#include <string>
#include <vector>

namespace jam {

using namespace asio::experimental::awaitable_operators;
using namespace std::chrono_literals;

inline auto rethrow(std::exception_ptr e) {
  if (e) {
    std::rethrow_exception(e);
  }
}

inline auto
exec(std::vector<std::string> const &cmd) -> asio::awaitable<std::string> {
  std::string out;
  process child{co_await asio::this_coro::executor,
                std::forward<decltype(cmd)>(cmd)};
  child.get_stdin().close();
  child.get_stderr().close();
  auto const [read_error, bytes_read, exit_status] =
      co_await (asio::async_read(child.get_stdout(), asio::dynamic_buffer(out),
                                 asio::transfer_all(),
                                 asio::as_tuple(asio::use_awaitable)) &&
                child.async_wait());
  if (read_error != asio::error::operation_aborted &&
      read_error != asio::error::eof) {
    throw std::runtime_error(std::format(
        "Failed reading stdout of command '{}': {} ({})",
        cmd | std::views::join_with(' ') | std::ranges::to<std::string>(),
        read_error.message(), read_error.value()));
    // TODO: include full command
  }
  if (exit_status != 0) {
    throw std::runtime_error(std::format(
        "Command '{}' failed with exit code {}",
        cmd | std::views::join_with(' ') | std::ranges::to<std::string>(),
        exit_status));
    // TODO: include full command
  }
  co_return out;
}

inline auto regex_search(std::string const &input, std::regex const &regex) {
  return std::ranges::subrange(
      std::sregex_iterator(input.cbegin(), input.cend(), regex),
      std::sregex_iterator());
}

auto read_host_info(auto &host) -> asio::awaitable<void> {
  // --json arg not available on rocky 8.9
  // std::string host_info_json{
  //     co_await exec({"/usr/bin/hostnamectl", "--json=short"})};
  // simdjson::ondemand::parser json_parser;
  // simdjson::ondemand::document doc{json_parser.iterate(host_info_json)};
  // host = host_info(doc);

  // TODO: replace std::regex with faster alternative
  auto const host_info_raw = co_await exec({"/usr/bin/hostnamectl"});
  auto const kv_regex = std::regex{" *([ _\\-\\w]+): (.+)\n"};
  for (auto const &key_value : regex_search(host_info_raw, kv_regex)) {
    auto const &key = key_value[1].str();
    auto const &value = key_value[2].str();
    if (key == "Static hostname") {
      host.hostname = value;
    }
    if (key == "Chassis") {
      host.chassis = value;
    }
    if (key == "Operating System") {
      host.operating_system = value;
    }
    if (key == "Kernel") {
      host.kernel = value;
    }
  }
}

auto query_jobs(auto &data) -> asio::awaitable<void> {
  auto ex = co_await asio::this_coro::executor;
  if (data.ssh_hop_enabled) {
    std::string line;
    data.ssh = std::make_unique<process>(ex, {"/usr/bin/ssh", data.ssh_hop});
    data.ssh.get_stderr().close();
    // FIXME: Add cancellation slot?
    asio::co_spawn(ex, data.ssh.async_wait(), asio::detached);

    // auto const [read_error, bytes_read, exit_status] = co_await (
    //     asio::async_read_until(ssh.get_stdout(), asio::dynamic_buffer(line),
    //                            '\n', asio::as_tuple(asio::use_awaitable)) &&
    //     child.async_wait());
    // if (read_error != asio::error::operation_aborted &&
    //     read_error != asio::error::eof) {
    //   throw std::runtime_error(std::format(
    //       "Failed reading stdout of command '{}': {} ({})",
    //       cmd | std::views::join_with(' ') | std::ranges::to<std::string>(),
    //       read_error.message(), read_error.value()));
    //   // TODO: include full command
    // }
    // if (exit_status != 0) {
    //   throw std::runtime_error(std::format(
    //       "Command '{}' failed with exit code {}",
    //       cmd | std::views::join_with(' ') | std::ranges::to<std::string>(),
    //       exit_status));
    //   // TODO: include full command
    // }
  } else {
    // FIXME:Not yet implemented
    throw std::runtime_error("Not yet implemented");
  }
}

}; // namespace jam
