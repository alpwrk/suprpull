//! Download queue: limits concurrency, runs jobs, writes history, emits events.

use std::sync::Arc;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use crate::history::History;
use crate::sc::api::ScClient;
use crate::types::{Config, FormatChoice, Job, JobId, QueueCmd, QueueEvent, ResolvedItem};

/// Turn a resolved item into jobs with output paths:
/// single: `<out>/<Artist> - <Title>`; set: `<out>/<Set>/<NN> - <Artist> - <Title>` (sanitized).
pub fn jobs_from_resolved(
    item: &ResolvedItem,
    format: FormatChoice,
    cfg: &Config,
    next_id: &mut JobId,
) -> Vec<Job> {
    let _ = (item, format, cfg, next_id);
    todo!()
}

/// Spawn the queue task. Emits `Added` for every job received via `QueueCmd::Add`.
pub fn spawn(
    cfg: Config,
    sc: Arc<ScClient>,
    history: History,
    cmd_rx: UnboundedReceiver<QueueCmd>,
    ev_tx: UnboundedSender<QueueEvent>,
) -> JoinHandle<()> {
    let _ = (cfg, sc, history, cmd_rx, ev_tx);
    todo!()
}
