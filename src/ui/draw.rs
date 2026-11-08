//! Frame layout: header, main list, input box, status line, footer, popup.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use super::app::{App, Focus, StatusKind, Tab};
use super::{history_view, queue_view, theme};

pub fn draw(app: &App, f: &mut Frame) {
    let [header, main, input, status, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    draw_header(app, f, header);
    draw_main(app, f, main);
    draw_input(app, f, input);
    draw_status(app, f, status);
    draw_footer(app, f, footer);

    if let Some(popup) = &app.popup {
        popup.render(f, f.area());
    }
}

fn draw_header(app: &App, f: &mut Frame, area: Rect) {
    let tab = |label: &'static str, active: bool| {
        if active {
            Span::styled(label, theme::accent_bold().add_modifier(Modifier::REVERSED))
        } else {
            Span::styled(label, theme::dim())
        }
    };
    let left = Line::from(vec![
        Span::styled(" suprpull ", theme::accent_bold()),
        Span::raw(" "),
        tab(" [1] Queue ", app.tab == Tab::Queue),
        Span::raw(" "),
        tab(" [2] History ", app.tab == Tab::History),
    ]);

    let (running, queued, done, failed) = app.stats();
    let mut stats = vec![
        Span::styled(format!("↓{running} "), theme::accent()),
        Span::styled(format!("⏳{queued} "), theme::dim()),
        Span::styled(format!("✔{done} "), Style::new().fg(theme::OK)),
    ];
    if failed > 0 {
        stats.push(Span::styled(
            format!("✖{failed} "),
            Style::new().fg(theme::ERR),
        ));
    }
    let stats = Line::from(stats);

    let stats_w = stats.width() as u16;
    if area.width > left.width() as u16 + stats_w {
        let [l, r] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(stats_w)]).areas(area);
        f.render_widget(left, l);
        f.render_widget(stats.right_aligned(), r);
    } else {
        f.render_widget(left, area);
    }
}

fn draw_main(app: &App, f: &mut Frame, area: Rect) {
    let focused = app.focus == Focus::List && app.popup.is_none();
    let (title, count) = match app.tab {
        Tab::Queue => ("Queue", app.jobs.len()),
        Tab::History => ("History", app.history.len()),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::border(focused))
        .title(Line::from(vec![
            Span::styled(
                format!(" {title} "),
                if focused {
                    theme::accent_bold()
                } else {
                    Style::new()
                },
            ),
            Span::styled(format!("({count}) "), theme::dim()),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    match app.tab {
        Tab::Queue => queue_view::render(app, f, inner, focused),
        Tab::History => history_view::render(app, f, inner, focused),
    }
}

fn draw_input(app: &App, f: &mut Frame, area: Rect) {
    let focused = app.focus == Focus::Input && app.popup.is_none();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::border(focused))
        .title(Span::styled(
            " SoundCloud link — Enter to resolve ",
            if focused {
                theme::accent_bold()
            } else {
                theme::dim()
            },
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    let width = inner.width as usize;
    if app.input.is_empty() {
        f.render_widget(
            Line::styled("https://soundcloud.com/artist/track-or-set", theme::dim()),
            inner,
        );
        if focused {
            f.set_cursor_position(Position::new(inner.x, inner.y));
        }
        return;
    }
    let (text, cursor) = app.input.visible(width);
    f.render_widget(Line::raw(text), inner);
    if focused {
        let x = inner.x + (cursor as u16).min(inner.width.saturating_sub(1));
        f.set_cursor_position(Position::new(x, inner.y));
    }
}

fn draw_status(app: &App, f: &mut Frame, area: Rect) {
    let line = if app.resolving > 0 {
        Line::from(vec![
            Span::styled(format!(" {} ", theme::spinner(app.tick)), theme::accent()),
            Span::raw("Resolving…"),
        ])
    } else if let Some(s) = &app.status {
        let style = match s.kind {
            StatusKind::Info => Style::new(),
            StatusKind::Success => Style::new().fg(theme::OK),
            StatusKind::Error => Style::new().fg(theme::ERR),
        };
        let prefix = match s.kind {
            StatusKind::Error => " ✖ ",
            StatusKind::Success => " ✔ ",
            StatusKind::Info => "   ",
        };
        Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(s.text.as_str(), style),
        ])
    } else {
        Line::default()
    };
    f.render_widget(Paragraph::new(line), area);
}

fn draw_footer(app: &App, f: &mut Frame, area: Rect) {
    let hints: &[(&str, &str)] = if app.popup.is_some() {
        &[("↑↓", "select"), ("Enter", "download"), ("Esc", "cancel")]
    } else if app.focus == Focus::Input {
        &[
            ("Enter", "resolve"),
            ("Tab", "list"),
            ("F1/F2", "tabs"),
            ("q", "quit"),
        ]
    } else if app.tab == Tab::Queue {
        &[
            ("↑↓", "move"),
            ("d", "remove"),
            ("r", "retry"),
            ("o", "open folder"),
            ("1/2", "tabs"),
            ("Tab", "input"),
            ("q", "quit"),
        ]
    } else {
        &[
            ("↑↓", "move"),
            ("o", "open folder"),
            ("1/2", "tabs"),
            ("Tab", "input"),
            ("q", "quit"),
        ]
    };
    let mut spans = vec![Span::raw(" ")];
    for (k, desc) in hints {
        spans.push(Span::styled(*k, theme::accent()));
        spans.push(Span::styled(format!(" {desc}  "), theme::dim()));
    }
    f.render_widget(Line::from(spans), area);
}

#[cfg(test)]
mod tests {
    use super::super::app::App;
    use super::super::app::tests::{app_with_jobs, cfg, entry, item};
    use super::*;
    use crate::types::{QueueEvent, UiMsg};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(app: &App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(app, f)).unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn queue_rows_show_title_and_percent() {
        let mut app = app_with_jobs(3);
        app.handle_msg(UiMsg::Queue(QueueEvent::Started(1)));
        app.handle_msg(UiMsg::Queue(QueueEvent::Progress(1, 0.42)));
        app.handle_msg(UiMsg::Queue(QueueEvent::Failed(
            2,
            "HTTP 403 Forbidden".into(),
        )));
        let out = render(&app, 100, 20);
        assert!(out.contains("suprpull"), "{out}");
        assert!(out.contains("[1] Queue"), "{out}");
        assert!(out.contains("Artist - Song 1"), "{out}");
        assert!(out.contains("42%"), "{out}");
        assert!(out.contains("HTTP 403 Forbidden"), "{out}");
        assert!(out.contains("queued"), "{out}");
        assert!(out.contains("SoundCloud link"), "{out}");
        assert!(out.contains("Enter resolve"), "{out}");
    }

    #[test]
    fn history_tab_lists_entries() {
        let mut app = App::new(cfg(), vec![entry("Old One")]);
        app.tab = Tab::History;
        let out = render(&app, 120, 12);
        assert!(out.contains("Artist - Old One"), "{out}");
        assert!(out.contains("aac"), "{out}");
        assert!(out.contains("/music/x/Artist - Old One.m4a"), "{out}");
    }

    #[test]
    fn popup_visible_with_options_and_skipped() {
        let mut app = App::new(cfg(), vec![]);
        app.handle_msg(UiMsg::Resolved(Ok(item(12, 7))));
        let out = render(&app, 100, 30);
        assert!(out.contains("Choose format"), "{out}");
        assert!(out.contains("Set: 12 tracks"), "{out}");
        assert!(out.contains("7 skipped"), "{out}");
        assert!(out.contains("+2 more"), "{out}");
        assert!(out.contains("› Opus 64 kbps"), "{out}");
        assert!(out.contains("Convert to MP3"), "{out}");
    }

    #[test]
    fn resolving_and_error_status() {
        let mut app = App::new(cfg(), vec![]);
        app.resolving = 1;
        assert!(render(&app, 60, 10).contains("Resolving…"));
        app.handle_msg(UiMsg::Resolved(Err("Link not found".into())));
        assert!(render(&app, 60, 10).contains("Link not found"));
    }

    #[test]
    fn input_scrolls_long_text() {
        let mut app = App::new(cfg(), vec![]);
        app.input
            .insert_str("https://soundcloud.com/some-artist/a-very-long-track-name-END");
        let out = render(&app, 30, 10);
        assert!(out.contains("name-END"), "{out}");
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        let mut app = app_with_jobs(3);
        app.handle_msg(UiMsg::Queue(QueueEvent::Started(1)));
        app.handle_msg(UiMsg::Queue(QueueEvent::Failed(2, "x".repeat(300))));
        app.handle_msg(UiMsg::Queue(QueueEvent::Done(3, entry(&"y".repeat(300)))));
        app.input.insert_str(&"z".repeat(200));
        for (w, h) in [(20, 5), (1, 1), (0, 0), (5, 20), (40, 8), (20, 3)] {
            render(&app, w, h);
            app.tab = Tab::History;
            render(&app, w, h);
            app.tab = Tab::Queue;
        }
        app.handle_msg(UiMsg::Resolved(Ok(item(3, 9))));
        for (w, h) in [(20, 5), (1, 1), (0, 0), (30, 6)] {
            render(&app, w, h);
        }
    }

    #[test]
    fn popup_keeps_selected_option_visible_when_small() {
        let mut app = App::new(cfg(), vec![]);
        app.handle_msg(UiMsg::Resolved(Ok(item(3, 9))));
        let out = render(&app, 60, 8);
        assert!(out.contains("Opus 64 kbps"), "{out}");
    }
}
