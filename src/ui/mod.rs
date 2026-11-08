//! ratatui front end. Owns the terminal; talks to the app only via `UiAction` / `UiMsg`.

mod app;
mod draw;
mod format_popup;
mod history_view;
mod input;
mod queue_view;
mod theme;

use std::io::stdout;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, EventStream};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::MissedTickBehavior;

use self::app::{App, Effect};
use crate::types::{Config, HistoryEntry, UiAction, UiMsg};

/// Set up the terminal, run the event loop until the user quits, restore the terminal.
pub async fn run(
    cfg: Config,
    history: Vec<HistoryEntry>,
    actions: UnboundedSender<UiAction>,
    msgs: UnboundedReceiver<UiMsg>,
) -> Result<()> {
    // `ratatui::init` enters raw mode + alternate screen and installs a panic
    // hook that restores the terminal. Chain ours to also undo bracketed paste.
    let mut terminal = ratatui::init();
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(stdout(), DisableBracketedPaste);
        prev_hook(info);
    }));
    let _ = crossterm::execute!(stdout(), EnableBracketedPaste);

    let app = App::new(cfg, history);
    let res = event_loop(&mut terminal, app, actions, msgs).await;

    let _ = crossterm::execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();
    res
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    mut app: App,
    actions: UnboundedSender<UiAction>,
    mut msgs: UnboundedReceiver<UiMsg>,
) -> Result<()> {
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        terminal.draw(|f| draw::draw(&app, f))?;

        let effect = tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(ev)) => app.handle_event(ev),
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(()),
            },
            msg = msgs.recv() => match msg {
                Some(m) => { app.handle_msg(m); None }
                // The app side is gone; nothing more will happen.
                None => return Ok(()),
            },
            _ = tick.tick() => { app.on_tick(); None }
        };

        match effect {
            Some(Effect::Action(a)) => {
                if actions.send(a).is_err() {
                    return Ok(());
                }
            }
            Some(Effect::Open(dir)) => open_dir(&dir),
            None => {}
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

/// Open a directory in the desktop file manager, detached. Errors are ignored.
fn open_dir(dir: &Path) {
    let child = Command::new("xdg-open")
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = child {
        // Reap the process so it doesn't linger as a zombie.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}
