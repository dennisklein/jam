// SPDX-FileCopyrightText: 2026 GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
// SPDX-License-Identifier: LGPL-3.0-or-later

use std::io;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

mod app;
mod cgroup;
mod process;
mod ui;

use app::App;
use ui::render_ui;

#[derive(Parser, Debug)]
#[command(name = "jam")]
#[command(about = "Job Analysis and Monitoring - cgroupv2 and process tree visualizer")]
#[command(version)]
struct Args {
    /// Refresh interval in milliseconds
    #[arg(short, long, default_value = "1000")]
    refresh: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    run_tui(args)
}

fn run_tui(args: Args) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut app = App::new(args.refresh);

    // Initial data load
    app.refresh()?;

    // Main loop
    let result = run_event_loop(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    use std::time::Instant;

    let tick_rate = Duration::from_millis(100);
    let mut needs_redraw = true;
    let mut frame_count: u32 = 0;
    let mut fps_update_time = Instant::now();

    loop {
        // Only render when needed
        if needs_redraw {
            terminal.draw(|f| render_ui(f, app))?;
            needs_redraw = false;
            frame_count += 1;

            // Update FPS every second
            let now = Instant::now();
            let elapsed = now.duration_since(fps_update_time).as_secs_f32();
            if elapsed >= 1.0 {
                app.self_stats.fps = frame_count as f32 / elapsed;
                frame_count = 0;
                fps_update_time = now;
            }
        }

        // Handle input with timeout
        if event::poll(tick_rate)? {
            match event::read()? {
                Event::Key(key) => {
                    needs_redraw = true; // Redraw on any key input
                    match (key.code, key.modifiers) {
                        // Quit
                        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                            app.should_quit = true;
                        }
                        // Navigation
                        (KeyCode::Up, _) | (KeyCode::Char('k'), _) => app.select_prev(),
                        (KeyCode::Down, _) | (KeyCode::Char('j'), _) => app.select_next(),
                        (KeyCode::PageUp, _) | (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                            let height = terminal.size()?.height as usize;
                            app.page_up(height.saturating_sub(2));
                        }
                        (KeyCode::PageDown, _) | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                            let height = terminal.size()?.height as usize;
                            app.page_down(height.saturating_sub(2));
                        }
                        (KeyCode::Home, _) | (KeyCode::Char('g'), _) => app.select_first(),
                        (KeyCode::End, _) | (KeyCode::Char('G'), _) => app.select_last(),
                        // Manual refresh
                        (KeyCode::Char('r'), _) => {
                            let refresh_start = Instant::now();
                            let _ = app.refresh();
                            app.self_stats.refresh_time_ms = refresh_start.elapsed().as_secs_f32() * 1000.0;
                        }
                        // Sorting
                        (KeyCode::Char('>'), _) | (KeyCode::Char('.'), _) => {
                            app.sort_next_column();
                        }
                        (KeyCode::Char('<'), _) | (KeyCode::Char(','), _) => {
                            app.sort_prev_column();
                        }
                        (KeyCode::Char('I'), _) => {
                            app.toggle_sort_direction();
                        }
                        _ => {}
                    }
                }
                Event::Resize(_, _) => {
                    needs_redraw = true;
                }
                _ => {}
            }
        }

        // Check for quit
        if app.should_quit {
            break;
        }

        // Auto-refresh
        if app.needs_refresh() {
            let refresh_start = Instant::now();
            let _ = app.refresh();
            app.self_stats.refresh_time_ms = refresh_start.elapsed().as_secs_f32() * 1000.0;
            needs_redraw = true;
        }
    }

    Ok(())
}
