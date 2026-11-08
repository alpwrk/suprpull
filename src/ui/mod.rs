//! ratatui front end. Owns the terminal; talks to the app only via `UiAction` / `UiMsg`.

use anyhow::Result;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::types::{Config, HistoryEntry, UiAction, UiMsg};

/// Set up the terminal, run the event loop until the user quits, restore the terminal.
pub async fn run(
    cfg: Config,
    history: Vec<HistoryEntry>,
    actions: UnboundedSender<UiAction>,
    msgs: UnboundedReceiver<UiMsg>,
) -> Result<()> {
    let _ = (cfg, history, actions, msgs);
    todo!()
}
