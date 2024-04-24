# © 2024 GSI Helmholtz Centre for Heavy Ion Research GmbH and other
# JAM project contributors. See the top-level COPYRIGHT.md file for details.
#
# SPDX-License-Identifier: GPL-3.0-only

set(target ${PROJECT_NAME_LOWER})
add_executable(${target} main.cpp)
target_sources(${target}
  PRIVATE FILE_SET HEADERS FILES
  actions.hpp
  data.hpp
  jam.hpp
  imgui_stdlib.h
  process.hpp
  tui.hpp

  PRIVATE
  imgui_stdlib.cpp
)
target_compile_features(${target} PRIVATE cxx_std_23)
target_compile_definitions(${target}
  PRIVATE
  JAM_DESCRIPTION="${PROJECT_DESCRIPTION}"
  JAM_EXECUTABLE="${target}"
  JAM_NAME="${PROJECT_NAME}"
  gsl_CONFIG_DEFAULTS_VERSION=1
  # ASIO_ENABLE_HANDLER_TRACKING
)
target_link_libraries(${target}
  PRIVATE
  asio::asio
  CLI11::CLI11
  imtui-ncurses
  mp-units::mp-units
  simdjson::simdjson
)
install(TARGETS ${target}
  RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})
