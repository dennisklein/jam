// SPDX-FileCopyrightText: 2026 GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
// SPDX-License-Identifier: LGPL-3.0-or-later

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table, Cell},
    Frame,
};

use crate::app::{App, TreeItem, SortColumn, SortDirection};

/// Colors for the tree visualization
const CGROUP_BRANCH_COLOR: Color = Color::Cyan;
const PROCESS_BRANCH_COLOR: Color = Color::Yellow;
const SELECTED_BG: Color = Color::DarkGray;
const CGROUP_NAME_COLOR: Color = Color::White;
const PROCESS_NAME_COLOR: Color = Color::Green;
const HEADER_BG: Color = Color::Blue;
const HEADER_FG: Color = Color::White;

/// Column widths
const COL_PID: u16 = 8;
const COL_USER: u16 = 6;
const COL_STATE: u16 = 3;
const COL_CPU: u16 = 7;
const COL_MEM: u16 = 9;
const COL_TASKS: u16 = 6;
const COL_IO_R: u16 = 8;
const COL_IO_W: u16 = 8;

/// Render the entire UI
pub fn render_ui(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Title bar
            Constraint::Length(1), // Column headers
            Constraint::Min(0),    // Tree view
            Constraint::Length(1), // Footer/status
        ])
        .split(frame.area());

    render_title(frame, chunks[0], app);
    render_column_headers(frame, chunks[1], app);
    render_tree(frame, chunks[2], app);
    render_footer(frame, chunks[3], app);
}

// Static padding buffer to avoid allocation every frame
const PADDING_SPACES: &str = "                                                                                                                                                                                                                                                                ";

fn render_title(frame: &mut Frame, area: Rect, app: &App) {
    let title = " jam - cgroup/process tree ";

    // Format self stats
    let mem_str = format_bytes(app.self_stats.memory_bytes);
    let cpu_str = format!("{:.1}%", app.self_stats.cpu_percent);
    let hz = 1.0 / app.refresh_interval.as_secs_f64();
    let fps = app.self_stats.fps.round() as u32;
    let refresh_ms = format!("{:.1}ms", app.self_stats.refresh_time_ms);
    let self_stats = format!("{:.0}Hz {}fps {}  cpu:{} mem:{} ", hz, fps, refresh_ms, cpu_str, mem_str);

    // Calculate padding to right-align self stats
    let title_len = title.len();
    let stats_len = self_stats.len();
    let padding = (area.width as usize).saturating_sub(title_len + stats_len);
    let padding_str = &PADDING_SPACES[..padding.min(PADDING_SPACES.len())];

    let header_line = Line::from(vec![
        Span::styled(title, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(padding_str, Style::default()),
        Span::styled(self_stats, Style::default().fg(Color::Gray)),
    ]);

    let header = Paragraph::new(header_line)
        .style(Style::default().bg(Color::DarkGray));
    frame.render_widget(header, area);
}

fn render_column_headers(frame: &mut Frame, area: Rect, app: &App) {
    let header_style = Style::default().fg(HEADER_FG).bg(HEADER_BG).add_modifier(Modifier::BOLD);
    let sorted_style = Style::default().fg(Color::LightRed).bg(HEADER_BG).add_modifier(Modifier::BOLD);

    // Sort indicator
    let indicator = match app.sort_direction {
        SortDirection::Ascending => "▲",
        SortDirection::Descending => "▼",
    };

    // Calculate name column width (remaining space after other columns)
    let name_width = area.width.saturating_sub(COL_PID + COL_USER + COL_STATE + COL_CPU + COL_MEM + COL_TASKS + COL_IO_R + COL_IO_W + 8);

    // Helper to format header with sort indicator
    let fmt_header = |name: &str, col: SortColumn, width: usize, right_align: bool| {
        let is_sorted = app.sort_column == col;
        let text = if is_sorted {
            if right_align {
                format!("{}{:>width$}", indicator, name, width = width - 1)
            } else {
                format!("{}{:<width$}", indicator, name, width = width - 1)
            }
        } else if right_align {
            format!("{:>width$}", name, width = width)
        } else {
            format!("{:<width$}", name, width = width)
        };
        let style = if is_sorted { sorted_style } else { header_style };
        Cell::from(text).style(style)
    };

    let headers = Row::new(vec![
        fmt_header("PID", SortColumn::Pid, COL_PID as usize, true),
        fmt_header("USER", SortColumn::User, COL_USER as usize, false),
        fmt_header("S", SortColumn::State, COL_STATE as usize, false),
        fmt_header("CPU%", SortColumn::Cpu, COL_CPU as usize, true),
        fmt_header("MEM", SortColumn::Memory, COL_MEM as usize, true),
        fmt_header("TASKS", SortColumn::Pids, COL_TASKS as usize, true),
        fmt_header("IO-R/s", SortColumn::IoRead, COL_IO_R as usize, true),
        fmt_header("IO-W/s", SortColumn::IoWrite, COL_IO_W as usize, true),
        fmt_header("NAME", SortColumn::Name, name_width as usize, false),
    ]).style(header_style);

    let table = Table::new(
        vec![headers],
        [
            Constraint::Length(COL_PID),
            Constraint::Length(COL_USER),
            Constraint::Length(COL_STATE),
            Constraint::Length(COL_CPU),
            Constraint::Length(COL_MEM),
            Constraint::Length(COL_TASKS),
            Constraint::Length(COL_IO_R),
            Constraint::Length(COL_IO_W),
            Constraint::Min(name_width),
        ],
    );

    frame.render_widget(table, area);
}

fn render_tree(frame: &mut Frame, area: Rect, app: &App) {
    let visible_height = area.height as usize;

    // Calculate scroll to keep selection visible
    let scroll = if app.selected < app.scroll {
        app.selected
    } else if app.selected >= app.scroll + visible_height {
        app.selected - visible_height + 1
    } else {
        app.scroll
    };

    // Calculate name column width
    let name_width = area.width.saturating_sub(COL_PID + COL_USER + COL_STATE + COL_CPU + COL_MEM + COL_TASKS + COL_IO_R + COL_IO_W + 8) as usize;

    let rows: Vec<Row> = app
        .items
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(idx, item)| render_tree_row(item, idx == app.selected, name_width))
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(COL_PID),
            Constraint::Length(COL_USER),
            Constraint::Length(COL_STATE),
            Constraint::Length(COL_CPU),
            Constraint::Length(COL_MEM),
            Constraint::Length(COL_TASKS),
            Constraint::Length(COL_IO_R),
            Constraint::Length(COL_IO_W),
            Constraint::Min(name_width as u16),
        ],
    ).block(Block::default().borders(Borders::NONE));

    frame.render_widget(table, area);
}

fn render_tree_row(item: &TreeItem, selected: bool, _name_width: usize) -> Row<'static> {
    let row_style = if selected {
        Style::default().bg(SELECTED_BG)
    } else {
        Style::default()
    };

    match item {
        TreeItem::Cgroup {
            data,
            depth,
            is_last,
            prefix,
        } => {
            // Build the name with tree prefix
            let tree_prefix = if *depth > 0 {
                let branch = if *is_last { "└── " } else { "├── " };
                format!("{}{}", prefix, branch)
            } else {
                String::new()
            };

            let name_cell = Line::from(vec![
                Span::styled(tree_prefix, Style::default().fg(CGROUP_BRANCH_COLOR)),
                Span::styled(
                    data.name.clone(),
                    Style::default().fg(CGROUP_NAME_COLOR).add_modifier(Modifier::BOLD),
                ),
            ]);

            // Memory
            let mem_str = data.memory_current
                .map(format_bytes)
                .unwrap_or_default();

            // PIDs count
            let pids_str = data.pids_current
                .map(|p| p.to_string())
                .unwrap_or_default();

            // CPU%
            let cpu_str = format!("{:.1}", data.cpu_percent);

            // I/O rates
            let io_r_str = format_rate(data.io_read_rate);
            let io_w_str = format_rate(data.io_write_rate);

            Row::new(vec![
                Cell::from(format!("{:>width$}", "-", width = COL_PID as usize))
                    .style(Style::default().fg(Color::DarkGray)),
                Cell::from(format!("{:<width$}", "-", width = COL_USER as usize))
                    .style(Style::default().fg(Color::DarkGray)),
                Cell::from(format!("{:^width$}", "-", width = COL_STATE as usize))
                    .style(Style::default().fg(Color::DarkGray)),
                Cell::from(format!("{:>width$}", cpu_str, width = COL_CPU as usize))
                    .style(Style::default().fg(Color::Green)),
                Cell::from(format!("{:>width$}", mem_str, width = COL_MEM as usize))
                    .style(Style::default().fg(Color::Cyan)),
                Cell::from(format!("{:>width$}", pids_str, width = COL_TASKS as usize))
                    .style(Style::default().fg(Color::Blue)),
                Cell::from(format!("{:>width$}", io_r_str, width = COL_IO_R as usize))
                    .style(Style::default().fg(if io_r_str == "-" { Color::DarkGray } else { Color::LightBlue })),
                Cell::from(format!("{:>width$}", io_w_str, width = COL_IO_W as usize))
                    .style(Style::default().fg(if io_w_str == "-" { Color::DarkGray } else { Color::LightRed })),
                Cell::from(name_cell),
            ]).style(row_style)
        }
        TreeItem::Process {
            data,
            is_last,
            cgroup_prefix,
            process_prefix,
        } => {
            // Build the name with tree prefix
            let branch = if *is_last { "└─ " } else { "├─ " };

            let name_cell = Line::from(vec![
                Span::styled(cgroup_prefix.clone(), Style::default().fg(CGROUP_BRANCH_COLOR)),
                Span::styled(process_prefix.clone(), Style::default().fg(PROCESS_BRANCH_COLOR)),
                Span::styled(branch, Style::default().fg(PROCESS_BRANCH_COLOR)),
                Span::styled(
                    data.name.clone(),
                    Style::default().fg(PROCESS_NAME_COLOR),
                ),
            ]);

            // User (truncate if too long)
            let user_str: &str = if data.user.len() > COL_USER as usize {
                &data.user[..COL_USER as usize - 1]
            } else {
                &data.user
            };
            let user_display = if data.user.len() > COL_USER as usize {
                format!("{}+", user_str)
            } else {
                data.user.clone()
            };

            // State
            let state_char = data.state.as_char();
            let state_color = match data.state {
                crate::process::ProcessState::Running => Color::Green,
                crate::process::ProcessState::Sleeping => Color::Gray,
                crate::process::ProcessState::DiskSleep => Color::Yellow,
                crate::process::ProcessState::Stopped => Color::Red,
                crate::process::ProcessState::Zombie => Color::Magenta,
                _ => Color::Gray,
            };

            // CPU%
            let cpu_str = format!("{:.1}", data.cpu_percent);

            // Memory (RSS)
            let mem_str = format_bytes(data.rss);

            // I/O rates
            let io_r_str = format_rate(data.io_read_rate);
            let io_w_str = format_rate(data.io_write_rate);

            Row::new(vec![
                Cell::from(format!("{:>width$}", data.pid, width = COL_PID as usize))
                    .style(Style::default().fg(Color::Yellow)),
                Cell::from(format!("{:<width$}", user_display, width = COL_USER as usize))
                    .style(Style::default().fg(Color::Magenta)),
                Cell::from(format!("{:^width$}", state_char, width = COL_STATE as usize))
                    .style(Style::default().fg(state_color)),
                Cell::from(format!("{:>width$}", cpu_str, width = COL_CPU as usize))
                    .style(Style::default().fg(Color::Green)),
                Cell::from(format!("{:>width$}", mem_str, width = COL_MEM as usize))
                    .style(Style::default().fg(Color::Cyan)),
                Cell::from(format!("{:>width$}", data.num_threads, width = COL_TASKS as usize))
                    .style(Style::default().fg(Color::Blue)),
                Cell::from(format!("{:>width$}", io_r_str, width = COL_IO_R as usize))
                    .style(Style::default().fg(if io_r_str == "-" { Color::DarkGray } else { Color::LightBlue })),
                Cell::from(format!("{:>width$}", io_w_str, width = COL_IO_W as usize))
                    .style(Style::default().fg(if io_w_str == "-" { Color::DarkGray } else { Color::LightRed })),
                Cell::from(name_cell),
            ]).style(row_style)
        }
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;

    if bytes >= GIB {
        format!("{:.1}Gi", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1}Mi", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.0}Ki", bytes as f64 / KIB as f64)
    } else {
        format!("{}B", bytes)
    }
}

/// Format I/O rate (bytes/sec) with human-readable suffix
fn format_rate(rate: f32) -> String {
    const KIB: f32 = 1024.0;
    const MIB: f32 = KIB * 1024.0;
    const GIB: f32 = MIB * 1024.0;

    if rate < 0.1 {
        "-".to_string()  // Show dash for near-zero rates
    } else if rate >= GIB {
        format!("{:.1}Gi", rate / GIB)
    } else if rate >= MIB {
        format!("{:.1}Mi", rate / MIB)
    } else if rate >= KIB {
        format!("{:.0}Ki", rate / KIB)
    } else {
        format!("{:.0}B", rate)
    }
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    let text = if let Some(ref err) = app.error {
        Span::styled(
            format!(" Error: {} ", err),
            Style::default().fg(Color::Red),
        )
    } else {
        let count = app.items.len();
        let pos = if count > 0 { app.selected + 1 } else { 0 };
        Span::styled(
            format!(" {}/{} | q:quit  j/k:nav  </> :sort  I:reverse  r:refresh ", pos, count),
            Style::default().fg(Color::Gray),
        )
    };

    let footer = Paragraph::new(Line::from(text));
    frame.render_widget(footer, area);
}
