# © 2024 GSI Helmholtz Centre for Heavy Ion Research GmbH and other
# JAM project contributors. See the top-level COPYRIGHT.md file for details.
#
# SPDX-License-Identifier: GPL-3.0-only

cmake_minimum_required(VERSION 3.28 FATAL_ERROR)

project(JAM DESCRIPTION "Job Analysis and Monitoring tool" LANGUAGES CXX)
string(TOLOWER ${PROJECT_NAME} PROJECT_NAME_LOWER)

set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

include(CTest)
include(CMakeParseArguments)
include(GNUInstallDirs)

set(PROJECT_SOURCE_DIR src)
set(PROJECT_THIRD_PARTY_DIR third_party)
set(PROJECT_TESTS_DIR tests)

macro(project_require PKG)
  cmake_parse_arguments(ARGS "" "SOURCE_SUBDIR" "" ${ARGN})
  include(FetchContent)
  if(ARGS_SOURCE_SUBDIR)
    FetchContent_Declare(${PKG}
      SOURCE_DIR ${CMAKE_SOURCE_DIR}/${PROJECT_THIRD_PARTY_DIR}/${PKG}
      OVERRIDE_FIND_PACKAGE SYSTEM QUIET EXCLUDE_FROM_ALL
      SOURCE_SUBDIR ${ARGS_SOURCE_SUBDIR}
    )
  else()
    FetchContent_Declare(${PKG}
      SOURCE_DIR ${CMAKE_SOURCE_DIR}/${PROJECT_THIRD_PARTY_DIR}/${PKG}
      OVERRIDE_FIND_PACKAGE SYSTEM QUIET EXCLUDE_FROM_ALL
    )
  endif()
  find_package(${PKG} REQUIRED QUIET)
endmacro()

# Dependencies
project_require(asio)
project_require(CLI11)
set(gsl_CONFIG_DEFAULTS 1)
project_require(gsl-lite)
project_require(imtui)
# set(MP_UNITS_BUILD_CXX_MODULES ON)
set(MP_UNITS_API_STD_FORMAT TRUE)
set(CMAKE_CXX_STANDARD 23)
project_require(mp-units SOURCE_SUBDIR src)
# project_require(tracy)
project_require(simdjson)
if(BUILD_TESTING)
  project_require(Catch2)
  # Include `Catch` module for the `catch_discover_tests` command, see
  # https://github.com/catchorg/Catch2/blob/devel/docs/cmake-integration.md#customization
  list(PREPEND CMAKE_MODULE_PATH ${Catch2_SOURCE_DIR}/extras)
  include(Catch)
endif()

# Build Targets
add_subdirectory(${PROJECT_SOURCE_DIR})
if(BUILD_TESTING)
  add_subdirectory(${PROJECT_TESTS_DIR})
endif()
