# © 2024 GSI Helmholtz Centre for Heavy Ion Research GmbH and other
# JAM project contributors. See the top-level COPYRIGHT.md file for details.
#
# SPDX-License-Identifier: GPL-3.0-only

set(exe tests)
add_executable(${exe})
target_sources(${exe} PRIVATE dummy.cpp)
target_compile_features(${exe} PRIVATE cxx_std_23)
target_link_libraries(${exe} PRIVATE Catch2::Catch2WithMain)
catch_discover_tests(${exe})
