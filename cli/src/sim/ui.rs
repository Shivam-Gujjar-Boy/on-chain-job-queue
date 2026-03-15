use crate::sim::types::SharedState;
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};
use ratatui::Terminal;
use std::io::{self, Stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

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

            let elapsed = snapshot.elapsed_seconds();
            let remaining = snapshot.duration_seconds.saturating_sub(elapsed);
            let status = if snapshot.done { "FINISHED" } else { "RUNNING" };
            let header = Paragraph::new(Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::Cyan)),
                Span::raw(status),
                Span::raw("   "),
                Span::styled("Elapsed: ", Style::default().fg(Color::Cyan)),
                Span::raw(format!("{}s", elapsed)),
                Span::raw("   "),
                Span::styled("Remaining: ", Style::default().fg(Color::Cyan)),
                Span::raw(format!("{}s", remaining)),
                Span::raw("   "),
                Span::styled("TX ok/fail: ", Style::default().fg(Color::Cyan)),
                Span::raw(format!("{}/{}", snapshot.total_tx_success, snapshot.total_tx_fail)),
                Span::raw("   q = stop"),
            ]))
            .block(Block::default().title("Simulation").borders(Borders::ALL));
            frame.render_widget(header, sections[0]);

            let queue_rows: Vec<Row> = snapshot
                .queues
                .iter()
                .enumerate()
                .map(|(idx, queue)| {
                    let stats = snapshot.queue_stats.get(idx).cloned().unwrap_or_default();
                    Row::new(vec![
                        Cell::from(idx.to_string()),
                        Cell::from(queue.name.clone()),
                        Cell::from(stats.submitted.to_string()),
                        Cell::from(stats.claimed.to_string()),
                        Cell::from(stats.completed.to_string()),
                        Cell::from(stats.failed.to_string()),
                        Cell::from(stats.timed_out.to_string()),
                        Cell::from(stats.onchain_pending.to_string()),
                        Cell::from(stats.onchain_active.to_string()),
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
            .header(Row::new(vec![
                "#", "Queue", "Sub", "Claim", "Complete", "Fail", "Timeout", "Pending", "Active",
            ]))
            .block(Block::default().title("Queues").borders(Borders::ALL));
            frame.render_widget(queue_table, sections[1]);

            let worker_rows: Vec<Row> = snapshot
                .workers
                .iter()
                .enumerate()
                .map(|(idx, worker)| {
                    let stats = snapshot.worker_stats.get(idx).cloned().unwrap_or_default();
                    Row::new(vec![
                        Cell::from(worker.worker_id.to_string()),
                        Cell::from(worker.queue_index.to_string()),
                        Cell::from(short_pubkey(worker.authority.to_string())),
                        Cell::from(stats.claimed.to_string()),
                        Cell::from(stats.completed.to_string()),
                        Cell::from(stats.failed.to_string()),
                        Cell::from(stats.heartbeats.to_string()),
                        Cell::from(stats.stalls.to_string()),
                        Cell::from(stats.tx_errors.to_string()),
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
            .header(Row::new(vec![
                "#", "Q", "Authority", "Claim", "Complete", "Fail", "Heartbeat", "Stall", "TxErr",
            ]))
            .block(Block::default().title("Workers").borders(Borders::ALL));
            frame.render_widget(worker_table, sections[2]);

            let log_text = snapshot
                .recent_logs
                .iter()
                .rev()
                .take(14)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");

            let totals = format!(
                "scenario: {}\nsubmissions={} completions={} failures={} timeouts={}\n\n{}",
                snapshot.scenario,
                snapshot.total_job_submissions,
                snapshot.total_job_completions,
                snapshot.total_job_failures,
                snapshot.total_job_timeouts,
                log_text
            );

            let logs = Paragraph::new(totals)
                .block(Block::default().title("Summary / Event Log").borders(Borders::ALL))
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
