/* © 2024 GSI Helmholtz Centre for Heavy Ion Research GmbH and other
 * JAM project contributors. See the top-level COPYRIGHT.md file for details.
 *
 * SPDX-License-Identifier: GPL-3.0-only */

#pragma once

#include <process.hpp>

#include <asio.hpp>
#include <array>
#include <memory>
#include <simdjson.h>
#include <string>
#include <vector>

namespace jam {

struct host_info {
  std::string chassis{};
  std::string hostname{};
  std::string kernel{};
  std::string operating_system{};
};

struct data {
  asio::executor ex;
  std::array<std::string, 3> headers{"PID", "Comm", "CPU%"};
  std::vector<std::array<std::string, 3>> processes{
      {"42", "Firefox", "14"}, {"3035", "tmux", "2"}, {"32001", "jam", "5"}};
  std::size_t selected_pid = 0;
  host_info host;

  bool ssh_hop_enabled = true;
  std::string ssh_hop = "virgo3.hpc.gsi.de";
  std::unique_ptr<process> ssh;
};

} // namespace jam

namespace simdjson {

// clang-format off
// example json input (`hostnamectl --json=pretty`):
// {
//         "Hostname" : "delta",
//         "StaticHostname" : "delta",
//         "PrettyHostname" : null,
//         "DefaultHostname" : "fedora",
//         "HostnameSource" : "static",
//         "IconName" : "computer-laptop",
//         "Chassis" : "laptop",
//         "Deployment" : null,
//         "Location" : null,
//         "KernelName" : "Linux",
//         "KernelRelease" : "6.8.7-300.fc40.x86_64",
//         "KernelVersion" : "#1 SMP PREEMPT_DYNAMIC Wed Apr 17 19:21:08 UTC 2024",
//         "OperatingSystemPrettyName" : "Fedora Linux 40 (Workstation Edition)",
//         "OperatingSystemCPEName" : "cpe:/o:fedoraproject:fedora:40",
//         "OperatingSystemHomeURL" : "https://fedoraproject.org/",
//         "OperatingSystemSupportEnd" : 1747094400000000,
//         "HardwareVendor" : "ASUSTeK COMPUTER INC.",
//         "HardwareModel" : "ROG Zephyrus G14 GA401QC_GA401QC",
//         "HardwareSerial" : null,
//         "FirmwareVersion" : "GA401QC.406",
//         "FirmwareVendor" : "American Megatrends International, LLC.",
//         "FirmwareDate" : 1620864000000000,
//         "MachineID" : "5aeb9cde3e564c17ac334e8b781c1416",
//         "BootID" : "792f2d32967449aeabb7d66ee132af5b",
//         "ProductUUID" : null
// }
// clang-format on
template <>
inline auto simdjson::ondemand::document::get() & noexcept
    -> simdjson::simdjson_result<jam::host_info> {
  // TODO: Simplify this code
  ondemand::object obj;
  {
    auto error = get_object().get(obj);
    if (error) { return error; }
  }
  jam::host_info host;
  for (auto field : obj) {
    raw_json_string key;
    {
      auto const error = field.key().get(key);
      if (error) { return error; }
    }
    if (key == "Chassis") {
      {
        auto const error = field.value().get_string(host.chassis);
        if (error) { return error; }
      }
    } else if (key == "StaticHostname") {
      {
        auto const error = field.value().get_string(host.hostname);
        if (error) { return error; }
      }
    } else if (key == "OperatingSystemPrettyName") {
      {
        auto const error = field.value().get_string(host.operating_system);
        if (error) { return error; }
      }
    } else if (key == "KernelRelease") {
      {
        auto const error = field.value().get_string(host.kernel);
        if (error) { return error; }
      }
    }
  }
  return host;
}

} // namespace simdjson
