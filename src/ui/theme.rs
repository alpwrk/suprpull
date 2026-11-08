//! Colors and shared styles. Deliberately restrained: one accent, a few states.

use ratatui::style::{Color, Modifier, Style};

/// SoundCloud orange.
pub const ACCENT: Color = Color::Rgb(255, 85, 0);
pub const OK: Color = Color::Green;
pub const ERR: Color = Color::Red;
pub const WARN: Color = Color::Yellow;
pub const DIM: Color = Color::DarkGray;

/// Braille spinner frames, advanced on every UI tick.
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(tick: usize) -> &'static str {
    SPINNER[tick % SPINNER.len()]
}

pub fn accent() -> Style {
    Style::new().fg(ACCENT)
}

pub fn accent_bold() -> Style {
    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn dim() -> Style {
    Style::new().fg(DIM)
}

pub fn border(focused: bool) -> Style {
    if focused { accent() } else { dim() }
}

/// Row highlight for the selected list entry.
pub fn selected(focused: bool) -> Style {
    if focused {
        Style::new()
            .bg(Color::Rgb(60, 30, 15))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    }
}
