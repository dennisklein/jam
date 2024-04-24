/* © 2024 GSI Helmholtz Centre for Heavy Ion Research GmbH and other
 * JAM project contributors. See the top-level COPYRIGHT.md file for details.
 *
 * SPDX-License-Identifier: GPL-3.0-only */

#pragma once

#include <actions.hpp>
#include <data.hpp>

#include <asio.hpp>
#include <cassert>
#include <experimental/scope>
#include <format>
#include <imgui/imgui.h>
#include <imgui/misc/cpp/imgui_stdlib.h>
#include <imtui/imtui-impl-ncurses.h>
#include <imtui/imtui.h>
#include <print>
#include <ranges>
#include <simdjson.h>
#include <string>

namespace jam {

inline auto &get_main_viewport() {
  auto const *mvp{ImGui::GetMainViewport()};
  assert(mvp);
  return *mvp;
}

auto frame(std::invocable auto &&content, ImTui::TScreen &screen) {
  std::experimental::scope_exit defer{[&] {
    ImGui::EndFrame();
    ImGui::Render();
    ImTui_ImplText_RenderDrawData(ImGui::GetDrawData(), &screen);
    ImTui_ImplNcurses_DrawScreen();
  }};
  ImTui_ImplNcurses_NewFrame();
  ImTui_ImplText_NewFrame();
  ImGui::NewFrame();
  content();
};

auto window(std::invocable auto &&content, auto &&...args) {
  std::experimental::scope_exit defer{[&] { ImGui::End(); }};
  if (ImGui::Begin(std::forward<decltype(args)>(args)...)) {
    content();
  }
};

auto table(std::invocable auto &&content, auto &&...args) {
  std::experimental::scope_exit defer{[&] { ImGui::EndTable(); }};
  if (ImGui::BeginTable(std::forward<decltype(args)>(args)...)) {
    content();
  }
};

auto main_window(std::invocable auto &&content) {
  auto main_viewport{get_main_viewport()};
  ImGui::SetNextWindowPos(main_viewport.WorkPos);
  ImGui::SetNextWindowSize(main_viewport.WorkSize);
  auto const title{
      std::format("{} {}###main", JAM_NAME,
                  main_viewport.WorkSize.x > 50 ? "- " JAM_DESCRIPTION : "")};
  constexpr auto fullscreen{
      ImGuiWindowFlags_NoMove | ImGuiWindowFlags_NoCollapse |
      ImGuiWindowFlags_NoBringToFrontOnFocus | ImGuiWindowFlags_NoResize};
  window([&] { content(); }, title.c_str(), nullptr, fullscreen);
}

auto process_table(auto &data) {
  table(
      [&] {
        ImGui::TableSetupScrollFreeze(0, 1);
        for (auto const &header : data.headers) {
          ImGui::TableSetupColumn(header.c_str());
        }
        ImGui::TableHeadersRow();

        for (auto const [process_index, process] :
             data.processes | std::views::enumerate) {
          ImGui::TableNextRow();
          for (auto const [col_index, col_value] :
               process | std::views::enumerate) {
            ImGui::TableSetColumnIndex(col_index);
            auto const selectable_label{
                std::format("###process{}", process_index)};
            if (data.headers[col_index] == "PID") {
              std::size_t const pid{std::stoul(col_value)};
              if (ImGui::Selectable(selectable_label.c_str(),
                                    pid == data.selected_pid,
                                    ImGuiSelectableFlags_SpanAllColumns)) {
                data.selected_pid = data.selected_pid == pid ? 0 : pid;
              }
              ImGui::SameLine();
            }
            ImGui::TextUnformatted(col_value.c_str());
          }
        }
      },
      "processes", data.headers.size(),
      ImGuiTableFlags_Resizable | ImGuiTableFlags_Reorderable |
          ImGuiTableFlags_Sortable | ImGuiTableFlags_RowBg);
}

auto tab_bar(std::invocable auto &&content, auto &&...args) {
  if (ImGui::BeginTabBar(std::forward<decltype(args)>(args)...)) {
    content();
    ImGui::EndTabBar();
  }
};

auto tab_item(std::invocable auto &&content, auto &&...args) {
  if (ImGui::BeginTabItem(std::forward<decltype(args)>(args)...)) {
    content();
    ImGui::EndTabItem();
  }
};

auto localhost_tab(auto &data) {
  tab_item(
      [&] {
        std::string const host_info{
            data.host.chassis.empty()
                ? "loading ..."
                : std::format("{} '{}' - {} ({})", data.host.chassis,
                              data.host.hostname, data.host.operating_system,
                              data.host.kernel)};
        ImGui::TextUnformatted(host_info.c_str());
        ImGui::NewLine();

        process_table(data);
      },
      "localhost");
}

auto jobs_tab(auto &data) {
  tab_item(
      [&] {
        ImGui::NewLine();
        ImGui::Checkbox("SSH:", &data.ssh_hop_enabled);
        ImGui::SameLine();
        ImGui::InputTextWithHint("###ssh_hop", "virgo3.hpc.gsi.de",
                                 &data.ssh_hop);
        ImGui::NewLine();
        if(ImGui::Button("Query Jobs")) {
          asio::co_spawn(data.ex, query_jobs(data), rethrow);
        }
      },
      "Jobs");
}

class tui {
  static auto make_screen() -> ImTui::TScreen & {
    IMGUI_CHECKVERSION();
    ImGui::CreateContext();
    ImTui_ImplText_Init();
    auto *screen{ImTui_ImplNcurses_Init(true)};
    assert(screen);
    return *screen;
  }

  ImTui::TScreen &screen{make_screen()};
  ImGuiIO &io{ImGui::GetIO()};

public:
  tui() { io.ConfigFlags |= ImGuiConfigFlags_NavEnableKeyboard; }

  ~tui() {
    ImTui_ImplText_Shutdown();
    ImTui_ImplNcurses_Shutdown();
  }

  auto render_frame(auto &data, auto &shutdown) {
    frame(
        [&] {
          main_window([&] {
            tab_bar(
                [&] {
                  jobs_tab(data);
                  localhost_tab(data);
                },
                "tabs", ImGuiTabBarFlags_None);
          });
        },
        screen);
    // FIXME:
    if (ImGui::IsKeyPressed(ImGuiKey_Z)) {
      shutdown();
    }
  }
};

} // namespace jam
