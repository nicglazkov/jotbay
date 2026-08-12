//! Live dashboard: `jotbay dash`.
//!
//! Redraws from local state on a short cadence and only touches the network on
//! an explicit refresh or sync, so the UI never stalls waiting on git.

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table, Wrap};
use std::time::{Duration, Instant};
use jotbay_core::{NodeHealth, Jotbay, JotbayStatus, SYNC_INTERVAL_SECS};

use crate::render::human_age;

const ACCENT: Color = Color::Rgb(122, 162, 247);
const DIM: Color = Color::Rgb(120, 128, 145);
const OK: Color = Color::Rgb(126, 200, 140);
const INFO: Color = Color::Rgb(108, 178, 224);
const WARN: Color = Color::Rgb(224, 175, 104);
const BAD: Color = Color::Rgb(224, 108, 118);

pub fn run(jotbay: &Jotbay) -> std::io::Result<()> {
    // `ratatui::init()` panics if it cannot take over the terminal, so a
    // full-screen dashboard asked for from a cron job, a pipe, or a CI log
    // answered with a Rust panic and a backtrace. Ask first and say the plain
    // thing instead.
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return Err(std::io::Error::other(
            "the dashboard needs a terminal. Try `jotbay` or `jotbay activity` instead.",
        ));
    }

    let mut terminal = ratatui::init();
    let result = event_loop(jotbay, &mut terminal);
    ratatui::restore();
    result
}

struct App {
    status: JotbayStatus,
    message: String,
    busy: bool,
    last_refresh: Instant,
}

fn event_loop(jotbay: &Jotbay, terminal: &mut ratatui::DefaultTerminal) -> std::io::Result<()> {
    let mut app = App {
        status: jotbay.status(true).unwrap_or_else(|_| empty_status()),
        message: "s sync · r refresh · q quit".into(),
        busy: false,
        last_refresh: Instant::now(),
    };

    loop {
        terminal.draw(|f| draw(f, &app))?;

        if event::poll(Duration::from_millis(500))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('s') => {
                        app.busy = true;
                        app.message = "syncing".into();
                        terminal.draw(|f| draw(f, &app))?;

                        app.message = match jotbay.sync() {
                            Ok(r) if r.skipped_locked => "another sync is running".into(),
                            Ok(r) if r.did_nothing() => "already in sync".into(),
                            Ok(r) => {
                                let mut parts = Vec::new();
                                if r.committed {
                                    parts.push("committed".to_string());
                                }
                                if r.pulled > 0 {
                                    parts.push(format!("pulled {}", r.pulled));
                                }
                                if !r.conflicts.is_empty() {
                                    parts.push(format!("{} conflict(s) kept both sides", r.conflicts.len()));
                                }
                                if r.pushed {
                                    parts.push("pushed".to_string());
                                }
                                parts.join(" · ")
                            }
                            Err(e) => format!("error: {e}"),
                        };
                        app.busy = false;
                        app.status = jotbay.status(false).unwrap_or_else(|_| empty_status());
                        app.last_refresh = Instant::now();
                    }
                    KeyCode::Char('r') => {
                        app.busy = true;
                        terminal.draw(|f| draw(f, &app))?;
                        app.status = jotbay.status(true).unwrap_or_else(|_| empty_status());
                        app.busy = false;
                        app.message = "refreshed".into();
                        app.last_refresh = Instant::now();
                    }
                    _ => {}
                }
            }
        }

        // Cheap local repaint keeps ages and dirty counts honest without
        // hitting the network.
        if app.last_refresh.elapsed() > Duration::from_secs(5) {
            app.status = jotbay.status(false).unwrap_or_else(|_| empty_status());
            app.last_refresh = Instant::now();
        }
    }

    Ok(())
}

fn empty_status() -> JotbayStatus {
    JotbayStatus {
        root: String::new(),
        notes: String::new(),
        branch: String::new(),
        head: String::new(),
        head_short: String::new(),
        ahead: 0,
        behind: 0,
        dirty_files: Vec::new(),
        rebase_in_progress: false,
        conflicts: Vec::new(),
        data_files: 0,
        update_available: None,
        warnings: Vec::new(),
        nodes: Vec::new(),
    }
}

fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(3),
    ])
    .split(frame.area());

    draw_header(frame, chunks[0], app);

    let body = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(chunks[1]);
    draw_nodes(frame, body[0], app);
    draw_local(frame, body[1], app);

    draw_footer(frame, chunks[2], app);
}

fn panel(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(DIM))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let s = &app.status;
    let line = Line::from(vec![
        Span::styled("  JOTBAY  ", Style::default().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(s.root.clone(), Style::default().fg(Color::White)),
        Span::styled(
            format!("   {} · {} · {} files", s.branch, s.head_short, s.data_files),
            Style::default().fg(DIM),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(DIM))),
        area,
    );
}

fn draw_nodes(frame: &mut Frame, area: Rect, app: &App) {
    let local_head = &app.status.head;

    let rows: Vec<Row> = app
        .status
        .nodes
        .iter()
        .map(|n| {
            let health = n.health(SYNC_INTERVAL_SECS, local_head);
            let color = match health {
                NodeHealth::Healthy => OK,
                NodeHealth::Behind => INFO,
                NodeHealth::Diverged => WARN,
                NodeHealth::Stale => DIM,
                NodeHealth::Offline => DIM,
                NodeHealth::Error => BAD,
            };

            let mut state = health.label().to_string();
            if n.behind > 0 {
                state.push_str(&format!(" ({} behind)", n.behind));
            } else if n.ahead > 0 {
                state.push_str(&format!(" ({} ahead)", n.ahead));
            }
            if n.dirty > 0 {
                state.push_str(&format!(" · {} dirty", n.dirty));
            }

            Row::new(vec![
                Cell::from(health.glyph()).style(Style::default().fg(color)),
                Cell::from(n.hostname.clone()).style(Style::default().fg(Color::White)),
                Cell::from(n.os.clone()).style(Style::default().fg(DIM)),
                Cell::from(human_age(n.age_secs())).style(Style::default().fg(DIM)),
                Cell::from(state).style(Style::default().fg(color)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(2),
        Constraint::Min(14),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Min(16),
    ];

    let table = Table::new(rows, widths)
        .header(
            Row::new(vec!["", "NODE", "OS", "LAST SYNC", "STATE"])
                .style(Style::default().fg(DIM).add_modifier(Modifier::BOLD)),
        )
        .block(panel("nodes"))
        .column_spacing(2);

    frame.render_widget(table, area);
}

fn draw_local(frame: &mut Frame, area: Rect, app: &App) {
    let s = &app.status;
    let mut lines: Vec<Line> = Vec::new();

    if s.rebase_in_progress {
        lines.push(Line::styled(
            format!("{} conflicted file(s)", s.conflicts.len()),
            Style::default().fg(BAD).add_modifier(Modifier::BOLD),
        ));
        for f in s.conflicts.iter().take(8) {
            lines.push(Line::styled(format!("  {f}"), Style::default().fg(BAD)));
        }
    } else if s.is_clean() {
        lines.push(Line::styled("everything in sync", Style::default().fg(OK)));
    } else {
        if s.ahead > 0 {
            lines.push(Line::styled(format!("{} commit(s) to push", s.ahead), Style::default().fg(WARN)));
        }
        if s.behind > 0 {
            lines.push(Line::styled(format!("{} commit(s) to pull", s.behind), Style::default().fg(WARN)));
        }
        if !s.dirty_files.is_empty() {
            lines.push(Line::styled(
                format!("{} uncommitted change(s)", s.dirty_files.len()),
                Style::default().fg(WARN),
            ));
            for f in s.dirty_files.iter().take(8) {
                lines.push(Line::styled(format!("  {f}"), Style::default().fg(DIM)));
            }
        }
    }

    frame.render_widget(
        Paragraph::new(lines).block(panel("this machine")).wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let style = if app.busy {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else if app.message.starts_with("error") {
        Style::default().fg(BAD)
    } else {
        Style::default().fg(DIM)
    };

    let line = Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(app.message.clone(), style),
    ]);

    frame.render_widget(
        Paragraph::new(line)
            .block(Block::default().borders(Borders::TOP).border_style(Style::default().fg(DIM))),
        area,
    );
}
