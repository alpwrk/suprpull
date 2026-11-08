//! Centered popup to pick the download format for a resolved link.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use super::theme;
use crate::types::{FormatChoice, ResolvedItem};

/// Max number of skipped tracks listed individually.
const MAX_SKIPPED_LINES: usize = 5;

#[derive(Debug)]
pub struct FormatPopup {
    pub item: ResolvedItem,
    pub selected: usize,
}

impl FormatPopup {
    /// Preselects the option matching `default`, else the first one.
    pub fn new(item: ResolvedItem, default: FormatChoice) -> Self {
        let selected = item
            .format_options
            .iter()
            .position(|o| o.choice == default)
            .unwrap_or(0);
        Self { item, selected }
    }

    pub fn move_sel(&mut self, delta: isize) {
        let len = self.item.format_options.len();
        if len == 0 {
            return;
        }
        self.selected = self.selected.saturating_add_signed(delta).min(len - 1);
    }

    pub fn selected_choice(&self) -> FormatChoice {
        self.item
            .format_options
            .get(self.selected)
            .map(|o| o.choice)
            .unwrap_or(FormatChoice::Best)
    }

    fn lines(&self) -> Vec<Line<'_>> {
        let it = &self.item;
        let mut lines = vec![
            Line::from(Span::styled(
                it.title.as_str(),
                Style::new().add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled("by ", theme::dim()),
                Span::raw(it.artist.as_str()),
            ]),
        ];
        let n = it.tracks.len();
        let mut info = vec![Span::styled(
            if it.set_title.is_some() {
                format!("Set: {n} track{}", if n == 1 { "" } else { "s" })
            } else {
                "Single track".to_string()
            },
            theme::dim(),
        )];
        if !it.skipped.is_empty() {
            info.push(Span::styled(
                format!(" · {} skipped", it.skipped.len()),
                Style::new().fg(theme::WARN),
            ));
        }
        lines.push(Line::from(info));
        for (title, reason) in it.skipped.iter().take(MAX_SKIPPED_LINES) {
            lines.push(Line::from(vec![
                Span::styled("  • ", Style::new().fg(theme::WARN)),
                Span::raw(title.as_str()),
                Span::styled(format!(" — {reason}"), theme::dim()),
            ]));
        }
        if it.skipped.len() > MAX_SKIPPED_LINES {
            lines.push(Line::styled(
                format!("  +{} more", it.skipped.len() - MAX_SKIPPED_LINES),
                theme::dim(),
            ));
        }
        lines.push(Line::default());
        for (i, opt) in it.format_options.iter().enumerate() {
            let line = if i == self.selected {
                Line::from(vec![
                    Span::styled(" › ", theme::accent_bold()),
                    Span::styled(opt.label.as_str(), theme::accent_bold()),
                ])
            } else {
                Line::from(vec![Span::raw("   "), Span::raw(opt.label.as_str())])
            };
            lines.push(line);
        }
        lines
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let lines = self.lines();
        // Index of the selected option's line (options come last).
        let sel_line = lines.len() - self.item.format_options.len() + self.selected;
        let content_w = lines.iter().map(Line::width).max().unwrap_or(0);
        let w = (content_w as u16 + 4).clamp(40, 80).min(area.width);
        let h = (lines.len() as u16 + 2).min(area.height);
        let rect = area.centered(Constraint::Length(w), Constraint::Length(h));
        f.render_widget(Clear, rect);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme::accent())
            .title(Line::styled(" Choose format ", theme::accent_bold()))
            .title_bottom(
                Line::from(vec![
                    Span::styled(" ↑↓", theme::accent()),
                    Span::raw(" select "),
                    Span::styled("Enter", theme::accent()),
                    Span::raw(" download "),
                    Span::styled("Esc", theme::accent()),
                    Span::raw(" cancel "),
                ])
                .right_aligned(),
            );
        // Scroll so the selected option stays visible on very small terminals.
        let inner_h = h.saturating_sub(2) as usize;
        let scroll = (sel_line + 1).saturating_sub(inner_h) as u16;
        f.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), rect);
    }
}
