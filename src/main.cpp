/* © 2024 GSI Helmholtz Centre for Heavy Ion Research GmbH and other
 * JAM project contributors. See the top-level COPYRIGHT.md file for details.
 *
 * SPDX-License-Identifier: GPL-3.0-only */

#include <jam.hpp>

#include <CLI/CLI.hpp>
#include <print>

auto main(int argc, char **argv) -> int {
  CLI::App cli{JAM_NAME " - " JAM_DESCRIPTION, JAM_EXECUTABLE};

  argv = cli.ensure_utf8(argv);
  try {
    cli.parse(argc, argv);
  } catch (CLI::ParseError const &e) {
    return cli.exit(e);
  }

  try {
    jam::run_tui(cli);
  } catch (std::exception const& e) {
    std::println("Unhandled exception! Report to https://github.com/dennisklein/jam/issues");
    std::println("{}", e.what());
    return 1;
  } catch (...) {
    std::println("Unhandled exception! Report to https://github.com/dennisklein/jam/issues");
    throw;
  }

  return 0;
}
