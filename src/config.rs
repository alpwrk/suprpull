//! Config file `~/.config/suprpull/config.toml`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::types::{Config, FormatChoice};

const MIN_CONCURRENCY: usize = 1;
const MAX_CONCURRENCY: usize = 8;

const HEADER: &str = "\
# suprpull configuration
#
# output_dir:     where downloads are stored (a leading \"~/\" is expanded)
# default_format: one of best, aac, opus, mp3, convert_mp3
#   best        - best available stream, no re-encoding (AAC > Opus > MP3)
#   aac/opus/mp3 - prefer this codec, falls back to best if unavailable
#   convert_mp3 - best stream re-encoded to MP3 (VBR V0)
# concurrency:    parallel downloads (1-8)
# ffmpeg_path:    ffmpeg binary (name in $PATH or absolute path)

";

impl Default for Config {
    /// output_dir = ~/Music/suprpull, default_format = best, concurrency = 3, ffmpeg_path = "ffmpeg"
    fn default() -> Self {
        let music = directories::UserDirs::new()
            .map(|u| {
                u.audio_dir()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| u.home_dir().join("Music"))
            })
            .unwrap_or_else(|| PathBuf::from("Music"));
        Config {
            output_dir: music.join("suprpull"),
            default_format: FormatChoice::Best,
            concurrency: 3,
            ffmpeg_path: "ffmpeg".to_string(),
        }
    }
}

pub fn default_path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "suprpull")
        .context("could not determine the config directory")?;
    Ok(dirs.config_dir().join("config.toml"))
}

/// Config file contents where every key is optional.
#[derive(Debug, Default, Deserialize)]
struct PartialConfig {
    output_dir: Option<PathBuf>,
    default_format: Option<FormatChoice>,
    concurrency: Option<usize>,
    ffmpeg_path: Option<String>,
}

impl PartialConfig {
    fn merge(self, d: Config) -> Config {
        Config {
            output_dir: self.output_dir.unwrap_or(d.output_dir),
            default_format: self.default_format.unwrap_or(d.default_format),
            concurrency: self.concurrency.unwrap_or(d.concurrency),
            ffmpeg_path: self.ffmpeg_path.unwrap_or(d.ffmpeg_path),
        }
    }
}

fn expand_tilde(p: &Path) -> PathBuf {
    let Ok(rest) = p.strip_prefix("~") else {
        return p.to_path_buf();
    };
    match directories::UserDirs::new() {
        Some(u) => u.home_dir().join(rest),
        None => p.to_path_buf(),
    }
}

fn normalize(mut cfg: Config) -> Config {
    cfg.output_dir = expand_tilde(&cfg.output_dir);
    cfg.concurrency = cfg.concurrency.clamp(MIN_CONCURRENCY, MAX_CONCURRENCY);
    if cfg.ffmpeg_path.trim().is_empty() {
        cfg.ffmpeg_path = "ffmpeg".to_string();
    }
    cfg
}

/// Serialize a config as TOML with the explanatory comment header.
fn to_file_contents(cfg: &Config) -> Result<String> {
    let body = toml::to_string_pretty(cfg).context("could not serialize config")?;
    Ok(format!("{HEADER}{body}"))
}

/// Load the config; write the default file if it does not exist. Missing keys fall back to defaults.
pub fn load_from(path: &Path) -> Result<Config> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let partial: PartialConfig = toml::from_str(&text)
                .with_context(|| format!("invalid config file {}", path.display()))?;
            Ok(normalize(partial.merge(Config::default())))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let cfg = Config::default();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("could not create {}", parent.display()))?;
            }
            std::fs::write(path, to_file_contents(&cfg)?)
                .with_context(|| format!("could not write {}", path.display()))?;
            Ok(normalize(cfg))
        }
        Err(e) => Err(e).with_context(|| format!("could not read {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/b/test-tmp/config")
            .join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn defaults() {
        let c = Config::default();
        assert!(c.output_dir.ends_with("suprpull"));
        assert_eq!(c.default_format, FormatChoice::Best);
        assert_eq!(c.concurrency, 3);
        assert_eq!(c.ffmpeg_path, "ffmpeg");
    }

    #[test]
    fn missing_file_is_created_and_roundtrips() {
        let path = tmpdir("missing").join("sub/dir/config.toml");
        let c = load_from(&path).unwrap();
        assert!(path.exists());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# suprpull"));
        assert!(text.contains("convert_mp3"));
        let again = load_from(&path).unwrap();
        assert_eq!(again.output_dir, c.output_dir);
        assert_eq!(again.default_format, c.default_format);
        assert_eq!(again.concurrency, c.concurrency);
        assert_eq!(again.ffmpeg_path, c.ffmpeg_path);
    }

    #[test]
    fn partial_file_uses_defaults_and_normalizes() {
        let path = tmpdir("partial").join("config.toml");
        std::fs::write(
            &path,
            "default_format = \"convert_mp3\"\nconcurrency = 99\noutput_dir = \"~/foo\"\n",
        )
        .unwrap();
        let c = load_from(&path).unwrap();
        assert_eq!(c.default_format, FormatChoice::ConvertMp3);
        assert_eq!(c.concurrency, 8);
        assert_eq!(c.ffmpeg_path, "ffmpeg");
        assert!(c.output_dir.ends_with("foo"));
        assert!(!c.output_dir.starts_with("~"));

        std::fs::write(&path, "concurrency = 0\n").unwrap();
        let c = load_from(&path).unwrap();
        assert_eq!(c.concurrency, 1);
        assert_eq!(c.output_dir, Config::default().output_dir);
    }

    #[test]
    fn invalid_format_is_an_error() {
        let path = tmpdir("invalid").join("config.toml");
        std::fs::write(&path, "default_format = \"flac\"\n").unwrap();
        assert!(load_from(&path).is_err());
    }
}
