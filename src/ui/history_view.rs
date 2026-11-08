//! History tab: finished downloads, newest first.

use chrono::Local;
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};

use super::app::App;
use super::theme;
use crate::types::HistoryEntry;

pub fn render(app: &App, f: &mut Frame, area: Rect, focused: bool) {
    if app.history.is_empty() {
        let p = Paragraph::new(Line::styled("No downloads yet.", theme::dim())).centered();
        f.render_widget(p, area);
        return;
    }
    let show_time = area.width >= 60;
    let show_path = area.width >= 90;
    let rows = app.history.iter().map(|e| row(e, show_time, show_path));
    let mut widths = Vec::with_capacity(4);
    if show_time {
        widths.push(Constraint::Length(16));
    }
    widths.push(Constraint::Fill(3));
    widths.push(Constraint::Length(4));
    if show_path {
        widths.push(Constraint::Fill(2));
    }
    let table = Table::new(rows, widths)
        .column_spacing(2)
        .row_highlight_style(theme::selected(focused));
    let mut state = TableState::default().with_selected(Some(app.history_sel));
    f.render_stateful_widget(table, area, &mut state);
}

fn row(e: &HistoryEntry, show_time: bool, show_path: bool) -> Row<'static> {
    let mut cells = Vec::with_capacity(4);
    if show_time {
        let t = e
            .time
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
            .to_string();
        cells.push(Cell::from(Span::styled(t, theme::dim())));
    }
    cells.push(Cell::from(format!("{} - {}", e.artist, e.title)));
    cells.push(Cell::from(Span::styled(e.format.clone(), theme::accent())));
    if show_path {
        cells.push(Cell::from(Span::styled(
            e.path.display().to_string(),
            theme::dim(),
        )));
    }
    Row::new(cells)
}
