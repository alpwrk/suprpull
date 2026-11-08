//! Runs ffmpeg to fetch a stream (HLS or progressive) into a tagged audio file.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::types::Codec;

#[derive(Debug, Clone)]
pub struct DownloadSpec {
    pub input_url: String,
    /// Codec of the source stream.
    pub codec: Codec,
    /// Re-encode to mp3 instead of stream copy.
    pub convert_mp3: bool,
    /// Embedded as attached picture (skipped for opus).
    pub cover_url: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub track_no: Option<u32>,
    pub duration_ms: Option<u64>,
    /// Without extension.
    pub out_stem: PathBuf,
}

/// "m4a" | "opus" | "mp3"
pub fn output_extension(codec: Codec, convert_mp3: bool) -> &'static str {
    let _ = (codec, convert_mp3);
    todo!()
}

/// Full ffmpeg argument list (without the program name) writing to `out_path`.
pub fn build_ffmpeg_args(spec: &DownloadSpec, out_path: &Path) -> Vec<String> {
    let _ = (spec, out_path);
    todo!()
}

/// Parse one line of `-progress pipe:1` output; returns progress 0..=1 for `out_time_us=`/`out_time_ms=` lines.
pub fn parse_progress_line(line: &str, duration_ms: Option<u64>) -> Option<f32> {
    let _ = (line, duration_ms);
    todo!()
}

/// Run ffmpeg, report progress, return the final file path. Writes to a `.part` temp file first.
pub async fn run(
    spec: &DownloadSpec,
    ffmpeg: &str,
    progress: impl Fn(f32) + Send,
) -> Result<PathBuf> {
    let _ = (spec, ffmpeg, &progress);
    todo!()
}
