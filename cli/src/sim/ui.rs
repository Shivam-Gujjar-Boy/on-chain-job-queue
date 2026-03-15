use crate::sim::types::SharedState;
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};
use ratatui::Terminal;
use std::io::{self, Stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ── Palette ─────────────────────────────────────────────────────────────────
const LABEL:    Color = Color::Cyan;
const OK:       Color = Color::Green;
const WARN:     Color = Color::Yellow;
const ERR:      Color = Color::Red;
const ACCENT:   Color = Color::Magenta;
const DIM:      Color = Color::DarkGray;
const PENDING_C:Color = Color::LightBlue;
const ACTIVE_C: Color = Color::LightCyan;

fn bold(c: Color) -> Style {
    Style::default().fg(c).add_modifier(Modifier::BOLD)
}
fn fg(c: Color) -> Style {
    Style::default().fg(c)
}

/// Color a numeric count: zero → DIM, otherwise the given color.
fn count_cell(n: u64, nonzero_color: Color) -> Cell<'static> {
    let s = n.to_string();
    if n == 0 {
        Cell::from(s).style(fg(DIM))
    } else {
        Cell::from(s).style(fg(nonzero_color))
    }
}

/// Colorize a single log line by scanning for signal words.
fn color_log_line(line: &str) -> Line<'static> {
    let owned = line.to_owned();
    let lower = owned.to_lowercase();
    let color = if lower.contains('✓') || lower.contains("complete") || lower.contains("ok") {
        OK
    } else if lower.contains("timeout") || lower.contains("timed out") || lower.contains("stall") {
        WARN
    } else if lower.contains('✗')
        || lower.contains("fail")
        || lower.contains("error")
        || lower.contains("err:")
    {
        ERR
    } else if lower.contains("heartbeat") || lower.contains("claim") {
        ACCENT
    } else {
        Color::White
    };
    Line::from(Span::styled(owned, fg(color)))
}

// ── Entry points ─────────────────────────────────────────────────────────────

pub fn run_tui(state: SharedState, stop: Arc<AtomicBool>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let render_result = render_loop(&mut terminal, state, stop.clone());

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    render_result
}

fn render_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: SharedState,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    loop {
        let snapshot = state.lock().map(|guard| guard.clone()).ok();

        terminal.draw(|frame| {
            let area = frame.area();
            let sections = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(10),
                    Constraint::Length(10),
                    Constraint::Min(8),
                ])
                .split(area);

            let Some(snapshot) = snapshot.as_ref() else {
                let p = Paragraph::new("State lock unavailable")
                    .block(Block::default().title("jq-sim").borders(Borders::ALL));
                frame.render_widget(p, area);
                return;
            };

            // ── Header ───────────────────────────────────────────────────────
            let elapsed = snapshot.elapsed_seconds();
            let remaining = snapshot.duration_seconds.saturating_sub(elapsed);
            let (status_text, status_style) = if snapshot.done {
                ("FINISHED", bold(WARN))
            } else {
                ("RUNNING ", bold(OK))
            };
            let header_line = Line::from(vec![
                Span::styled(" Status: ", bold(LABEL)),
                Span::styled(status_text, status_style),
                Span::styled("  │  ", fg(DIM)),
                Span::styled("Elapsed: ", fg(LABEL)),
                Span::styled(format!("{}s", elapsed), fg(Color::White)),
                Span::styled("  │  ", fg(DIM)),
                Span::styled("Remaining: ", fg(LABEL)),
                Span::styled(format!("{}s", remaining), fg(Color::White)),
                Span::styled("  │  ", fg(DIM)),
                Span::styled("TX ", fg(LABEL)),
                Span::styled(snapshot.total_tx_success.to_string(), bold(OK)),
                Span::styled("/", fg(DIM)),
                Span::styled(snapshot.total_tx_fail.to_string(), bold(ERR)),
                Span::styled("  │  ", fg(DIM)),
                Span::styled("q / Esc = stop", fg(DIM)),
            ]);
            let header = Paragraph::new(header_line).block(
                Block::default()
                    .title(Span::styled(" jq-sim ", bold(LABEL)))
                    .borders(Borders::ALL)
                    .border_style(fg(LABEL)),
            );
            frame.render_widget(header, sections[0]);

            // ── Queue table ──────────────────────────────────────────────────
            let hdr_style = bold(WARN);
            let queue_header = Row::new(vec![
                Cell::from("#").style(hdr_style),
                Cell::from("Queue").style(hdr_style),
                Cell::from("Sub").style(hdr_style),
                Cell::from("Claim").style(hdr_style),
                Cell::from("Complete").style(hdr_style),
                Cell::from("Fail").style(hdr_style),
                Cell::from("Timeout").style(hdr_style),
                Cell::from("Pending").style(hdr_style),
                Cell::from("Active").style(hdr_style),
            ])
            .style(bold(WARN));

            let queue_rows: Vec<Row> = snapshot
                .queues
                .iter()
                .enumerate()
                .map(|(idx, queue)| {
                    let stats = snapshot.queue_stats.get(idx).cloned().unwrap_or_default();
                    Row::new(vec![
                        Cell::from(idx.to_string()).style(fg(DIM)),
                        Cell::from(queue.name.clone()).style(bold(Color::White)),
                        Cell::from(stats.submitted.to_string()).style(fg(Color::White)),
                        Cell::from(stats.claimed.to_string()).style(fg(Color::White)),
                        count_cell(stats.completed, OK),
                        count_cell(stats.failed, ERR),
                        count_cell(stats.timed_out, WARN),
                        count_cell(stats.onchain_pending, PENDING_C),
                        count_cell(stats.onchain_active, ACTIVE_C),
                    ])
                })
                .collect();

            let queue_table = Table::new(
                queue_rows,
                [
                    Constraint::Length(4),
                    Constraint::Length(16),
                    Constraint::Length(8),
                    Constraint::Length(8),
                    Constraint::Length(9),
                    Constraint::Length(7),
                    Constraint::Length(9),
                    Constraint::Length(8),
                    Constraint::Length(7),
                ],
            )
            .header(queue_header)
            .block(
                Block::default()
                    .title(Span::styled(" Queues ", bold(LABEL)))
                    .borders(Borders::ALL)
                    .border_style(fg(LABEL)),
            )
            .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));
            frame.render_widget(queue_table, sections[1]);

            // ── Worker table ─────────────────────────────────────────────────
            let worker_header = Row::new(vec![
                Cell::from("#").style(hdr_style),
                Cell::from("Q").style(hdr_style),
                Cell::from("Authority").style(hdr_style),
                Cell::from("Claim").style(hdr_style),
                Cell::from("Complete").style(hdr_style),
                Cell::from("Fail").style(hdr_style),
                Cell::from("Heartbeat").style(hdr_style),
                Cell::from("Stall").style(hdr_style),
                Cell::from("TxErr").style(hdr_style),
            ])
            .style(bold(WARN));

            let worker_rows: Vec<Row> = snapshot
                .workers
                .iter()
                .enumerate()
                .map(|(idx, worker)| {
                    let stats = snapshot.worker_stats.get(idx).cloned().unwrap_or_default();
                    Row::new(vec![
                        Cell::from(worker.worker_id.to_string()).style(fg(DIM)),
                        Cell::from(worker.queue_index.to_string()).style(fg(DIM)),
                        Cell::from(short_pubkey(worker.authority.to_string()))
                            .style(fg(Color::White)),
                        Cell::from(stats.claimed.to_string()).style(fg(Color::White)),
                        count_cell(stats.completed, OK),
                        count_cell(stats.failed, ERR),
                        count_cell(stats.heartbeats, LABEL),
                        count_cell(stats.stalls, WARN),
                        count_cell(stats.tx_errors, ERR),
                    ])
                })
                .collect();

            let worker_table = Table::new(
                worker_rows,
                [
                    Constraint::Length(4),
                    Constraint::Length(5),
                    Constraint::Length(16),
                    Constraint::Length(8),
                    Constraint::Length(9),
                    Constraint::Length(7),
                    Constraint::Length(10),
                    Constraint::Length(7),
                    Constraint::Length(8),
                ],
            )
            .header(worker_header)
            .block(
                Block::default()
                    .title(Span::styled(" Workers ", bold(ACCENT)))
                    .borders(Borders::ALL)
                    .border_style(fg(ACCENT)),
            )
            .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));
            frame.render_widget(worker_table, sections[2]);

            // ── Summary / Event Log ──────────────────────────────────────────
            let mut log_lines: Vec<Line> = vec![
                Line::from(vec![
                    Span::styled("scenario: ", fg(DIM)),
                    Span::styled(snapshot.scenario.clone(), fg(Color::White)),
                ]),
                Line::from(vec![
                    Span::styled("sub=", fg(LABEL)),
                    Span::styled(snapshot.total_job_submissions.to_string(), bold(Color::White)),
                    Span::styled("  compl=", fg(OK)),
                    Span::styled(snapshot.total_job_completions.to_string(), bold(OK)),
                    Span::styled("  fail=", fg(ERR)),
                    Span::styled(snapshot.total_job_failures.to_string(), bold(ERR)),
                    Span::styled("  timeout=", fg(WARN)),
                    Span::styled(snapshot.total_job_timeouts.to_string(), bold(WARN)),
                ]),
                Line::from(""),
            ];

            for entry in snapshot.recent_logs.iter().rev().take(14) {
                log_lines.push(color_log_line(entry));
            }

            let logs = Paragraph::new(Text::from(log_lines))
                .block(
                    Block::default()
                        .title(Span::styled(" Summary / Event Log ", bold(OK)))
                        .borders(Borders::ALL)
                        .border_style(fg(OK)),
                )
                .wrap(Wrap { trim: false });
            frame.render_widget(logs, sections[3]);
        })?;

        if let Ok(guard) = state.lock() {
            if guard.done {
                stop.store(true, Ordering::Relaxed);
                return Ok(());
            }
        }

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key_event) = event::read()? {
                if matches!(key_event.code, KeyCode::Char('q') | KeyCode::Esc) {
                    stop.store(true, Ordering::Relaxed);
                    return Ok(());
                }
            }
        }
    }
}

fn short_pubkey(pubkey: String) -> String {
    if pubkey.len() < 10 {
        return pubkey;
    }
    format!("{}...{}", &pubkey[..6], &pubkey[pubkey.len() - 4..])
}
