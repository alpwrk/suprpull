//! Download history as JSON lines in `~/.local/share/suprpull/history.jsonl`.

use std::path::PathBuf;

use anyhow::Result;

use crate::types::HistoryEntry;

#[derive(Debug, Clone)]
pub struct History {
    pub path: PathBuf,
}

impl History {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> Result<PathBuf> {
        todo!()
    }

    /// All entries, newest first. Missing file -> empty; corrupt lines are skipped.
    pub fn load(&self) -> Result<Vec<HistoryEntry>> {
        todo!()
    }

    pub fn append(&self, entry: &HistoryEntry) -> Result<()> {
        let _ = entry;
        todo!()
    }
}
