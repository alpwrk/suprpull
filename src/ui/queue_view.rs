//! Queue tab: one row per job with status icon, name and progress.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};

use super::app::App;
use super::theme;
use crate::types::{Job, JobStatus};

/// Width of the text progress bar (without the percentage).
const BAR_WIDTH: usize = 12;

pub fn render(app: &App, f: &mut Frame, area: Rect, focused: bool) {
    if app.jobs.is_empty() {
        let p = Paragraph::new(Line::styled(
            "Queue is empty — paste a SoundCloud link below.",
            theme::dim(),
        ))
        .centered();
        f.render_widget(p, area);
        return;
    }
    let wide = area.width >= 50;
    let rows = app.jobs.iter().map(|j| row(j, app.tick, wide));
    let progress_w = if wide { BAR_WIDTH as u16 + 6 } else { 6 };
    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Fill(1),
            Constraint::Length(progress_w),
        ],
    )
    .column_spacing(1)
    .row_highlight_style(theme::selected(focused));
    let mut state = TableState::default().with_selected(Some(app.queue_sel));
    f.render_stateful_widget(table, area, &mut state);
}

fn row(job: &Job, tick: usize, wide: bool) -> Row<'static> {
    let (icon, icon_style) = match &job.status {
        JobStatus::Queued => ("⏳", theme::dim()),
        JobStatus::Downloading(_) => (theme::spinner(tick), theme::accent()),
        JobStatus::Done(_) => ("✔", Style::new().fg(theme::OK)),
        JobStatus::Failed(_) => ("✖", Style::new().fg(theme::ERR)),
    };

    let mut name = vec![Span::raw(format!(
        "{} - {}",
        job.track.artist_or_default(),
        job.track.title_or_default()
    ))];
    if let Some(album) = &job.album {
        name.push(Span::styled(format!("  {album}"), theme::dim()));
    }
    let mut name_text = Text::from(Line::from(name));
    let mut height = 1;
    if let JobStatus::Failed(err) = &job.status {
        name_text.push_line(Line::styled(
            err.lines().next().unwrap_or_default().to_string(),
            Style::new().fg(theme::ERR),
        ));
        height = 2;
    }

    let progress = match &job.status {
        JobStatus::Queued => Line::styled("queued", theme::dim()),
        JobStatus::Downloading(p) => progress_line(*p, wide),
        JobStatus::Done(_) => Line::styled("done", Style::new().fg(theme::OK)),
        JobStatus::Failed(_) => Line::styled("failed", Style::new().fg(theme::ERR)),
    };

    Row::new([
        Cell::from(Span::styled(icon, icon_style)),
        Cell::from(name_text),
        Cell::from(progress.right_aligned()),
    ])
    .height(height)
}

fn progress_line(p: f32, wide: bool) -> Line<'static> {
    let p = p.clamp(0.0, 1.0);
    let pct = Span::styled(
        format!("{:>4}", format!("{}%", (p * 100.0).round() as u32)),
        theme::accent(),
    );
    if !wide {
        return Line::from(pct);
    }
    let filled = ((p * BAR_WIDTH as f32).round() as usize).min(BAR_WIDTH);
    Line::from(vec![
        Span::styled("━".repeat(filled), theme::accent()),
        Span::styled("─".repeat(BAR_WIDTH - filled), theme::dim()),
        Span::raw(" "),
        pct,
    ])
}
