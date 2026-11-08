//! suprpull — a terminal SoundCloud downloader.

mod config;
mod download;
mod history;
mod metadata;
mod queue;
mod sc;
mod types;
mod ui;

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::history::History;
use crate::sc::api::ScClient;
use crate::types::{JobId, QueueCmd, UiAction, UiMsg};

#[tokio::main]
async fn main() -> Result<()> {
    let cfg_path = config::default_path()?;
    let cfg = config::load_from(&cfg_path)
        .with_context(|| format!("failed to load config {}", cfg_path.display()))?;
    check_ffmpeg(&cfg.ffmpeg_path)?;
    let history = History::new(History::default_path()?);
    let past = history.load().unwrap_or_default();
    let sc = Arc::new(ScClient::new().context("failed to create SoundCloud client")?);

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<QueueCmd>();
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let (ui_tx, ui_rx) = mpsc::unbounded_channel::<UiMsg>();
    let (act_tx, mut act_rx) = mpsc::unbounded_channel::<UiAction>();

    let queue_task = queue::spawn(cfg.clone(), sc.clone(), history, cmd_rx, ev_tx);

    // Queue events -> UI.
    let fwd_tx = ui_tx.clone();
    let forward = tokio::spawn(async move {
        while let Some(ev) = ev_rx.recv().await {
            if fwd_tx.send(UiMsg::Queue(ev)).is_err() {
                break;
            }
        }
    });

    // UI actions -> resolver / queue.
    let dispatch_cfg = cfg.clone();
    let dispatcher = tokio::spawn(async move {
        let mut next_id: JobId = 1;
        while let Some(action) = act_rx.recv().await {
            match action {
                UiAction::Resolve(url) => {
                    let sc = sc.clone();
                    let tx = ui_tx.clone();
                    tokio::spawn(async move {
                        let res = sc.resolve(&url).await.map_err(|e| format!("{e:#}"));
                        let _ = tx.send(UiMsg::Resolved(res));
                    });
                }
                UiAction::Enqueue { item, format } => {
                    let jobs =
                        queue::jobs_from_resolved(&item, format, &dispatch_cfg, &mut next_id);
                    let _ = cmd_tx.send(QueueCmd::Add(jobs));
                }
                UiAction::Remove(id) => {
                    let _ = cmd_tx.send(QueueCmd::Remove(id));
                }
                UiAction::Retry(id) => {
                    let _ = cmd_tx.send(QueueCmd::Retry(id));
                }
            }
        }
        // Dropping cmd_tx here closes the queue, which aborts running downloads.
    });

    let result = ui::run(cfg, past, act_tx, ui_rx).await;

    // UI is gone: the dispatcher ends (act_tx dropped), which closes the queue.
    let _ = dispatcher.await;
    let _ = queue_task.await;
    forward.abort();
    result
}

/// Fail early (before the TUI takes over the terminal) if ffmpeg cannot be run.
fn check_ffmpeg(ffmpeg: &str) -> Result<()> {
    let ok = std::process::Command::new(ffmpeg)
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    anyhow::ensure!(
        ok,
        "ffmpeg not found (`{ffmpeg}`). Install ffmpeg or set `ffmpeg_path` in the config file."
    );
    Ok(())
}
