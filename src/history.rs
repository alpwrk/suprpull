//! Download history as JSON lines in `~/.local/share/suprpull/history.jsonl`.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};

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
        let dirs = directories::ProjectDirs::from("", "", "suprpull")
            .context("could not determine the data directory")?;
        Ok(dirs.data_dir().join("history.jsonl"))
    }

    /// All entries, newest first. Missing file -> empty; corrupt lines are skipped.
    pub fn load(&self) -> Result<Vec<HistoryEntry>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(e).with_context(|| format!("could not read {}", self.path.display()));
            }
        };
        let mut entries: Vec<HistoryEntry> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        entries.reverse();
        Ok(entries)
    }

    pub fn append(&self, entry: &HistoryEntry) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        let mut line = serde_json::to_string(entry).context("could not serialize history entry")?;
        line.push('\n');
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("could not open {}", self.path.display()))?;
        f.write_all(line.as_bytes())
            .with_context(|| format!("could not write {}", self.path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str) -> HistoryEntry {
        HistoryEntry {
            time: chrono::Utc::now(),
            title: title.to_string(),
            artist: "Artist".to_string(),
            path: PathBuf::from(format!("/music/{title}.m4a")),
            source_url: "https://soundcloud.com/a/b".to_string(),
            format: "aac".to_string(),
        }
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/b/test-tmp/history")
            .join(name);
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn missing_file_is_empty() {
        let h = History::new(tmpdir("missing").join("history.jsonl"));
        assert!(h.load().unwrap().is_empty());
    }

    #[test]
    fn append_and_load_newest_first_skipping_corrupt() {
        let path = tmpdir("append").join("nested/history.jsonl");
        let h = History::new(path.clone());
        h.append(&entry("one")).unwrap();
        h.append(&entry("two")).unwrap();
        // simulate a corrupt / truncated line
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(b"{\"title\": broken\n\n").unwrap();
        h.append(&entry("three")).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().filter(|l| !l.is_empty()).count(), 4);

        let titles: Vec<_> = h.load().unwrap().into_iter().map(|e| e.title).collect();
        assert_eq!(titles, ["three", "two", "one"]);
    }
}
