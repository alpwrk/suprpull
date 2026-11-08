//! Config file `~/.config/suprpull/config.toml`.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::types::Config;

impl Default for Config {
    /// output_dir = ~/Music/suprpull, default_format = best, concurrency = 3, ffmpeg_path = "ffmpeg"
    fn default() -> Self {
        todo!()
    }
}

pub fn default_path() -> Result<PathBuf> {
    todo!()
}

/// Load the config; write the default file if it does not exist. Missing keys fall back to defaults.
pub fn load_from(path: &Path) -> Result<Config> {
    let _ = path;
    todo!()
}
