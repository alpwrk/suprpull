//! Shared contracts between the SoundCloud client, the download queue and the UI.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::sc::model::Track;

/// Audio codec of a SoundCloud transcoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    Aac,
    Opus,
    Mp3,
}

impl Codec {
    /// Derive the codec from a transcoding mime type.
    pub fn from_mime(mime: &str) -> Option<Codec> {
        let m = mime.to_ascii_lowercase();
        if m.contains("opus") {
            Some(Codec::Opus)
        } else if m.contains("mp4a") || m.contains("audio/mp4") || m.contains("aac") {
            Some(Codec::Aac)
        } else if m.contains("mpeg") || m.contains("mp3") {
            Some(Codec::Mp3)
        } else {
            None
        }
    }

    /// File extension when the stream is copied without re-encoding.
    pub fn extension(self) -> &'static str {
        match self {
            Codec::Aac => "m4a",
            Codec::Opus => "opus",
            Codec::Mp3 => "mp3",
        }
    }
}

/// What the user wants to get. Also the `default_format` in the config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormatChoice {
    /// Best available transcoding, stream-copied (AAC > Opus > MP3).
    Best,
    /// Prefer this codec (stream copy); falls back to `Best` per track if missing.
    Aac,
    Opus,
    Mp3,
    /// Take the best stream and re-encode it to MP3 (libmp3lame, VBR q0).
    ConvertMp3,
}

impl FormatChoice {
    pub fn preferred_codec(self) -> Option<Codec> {
        match self {
            FormatChoice::Aac => Some(Codec::Aac),
            FormatChoice::Opus => Some(Codec::Opus),
            FormatChoice::Mp3 => Some(Codec::Mp3),
            FormatChoice::Best | FormatChoice::ConvertMp3 => None,
        }
    }
}

/// One entry of the format popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOption {
    pub choice: FormatChoice,
    /// Human readable, e.g. "AAC 160 kbps (HLS) -> .m4a"
    pub label: String,
}

/// Result of resolving a user-supplied link.
#[derive(Debug, Clone)]
pub struct ResolvedItem {
    pub source_url: String,
    /// Track title or set title.
    pub title: String,
    pub artist: String,
    /// `Some(set title)` if the link was a playlist/set.
    pub set_title: Option<String>,
    /// Fully hydrated, playable tracks (stubs re-fetched).
    pub tracks: Vec<Track>,
    /// Tracks that were skipped (blocked / preview-only / no usable stream): (title, reason).
    pub skipped: Vec<(String, String)>,
    /// Options for the format popup; always contains `Best` and `ConvertMp3`.
    pub format_options: Vec<FormatOption>,
}

pub type JobId = u64;

#[derive(Debug, Clone, PartialEq)]
pub enum JobStatus {
    Queued,
    /// Progress 0.0..=1.0
    Downloading(f32),
    Done(PathBuf),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: JobId,
    pub track: Track,
    pub format: FormatChoice,
    /// Output path WITHOUT extension; the extension is chosen at download time.
    pub out_stem: PathBuf,
    /// Set title, written as album tag.
    pub album: Option<String>,
    /// Position inside the set (1-based).
    pub track_no: Option<u32>,
    /// Number of tracks in the set.
    pub track_total: Option<u32>,
    pub source_url: String,
    pub status: JobStatus,
}

/// Commands into the queue task.
#[derive(Debug)]
pub enum QueueCmd {
    Add(Vec<Job>),
    /// Cancel if running, drop if queued.
    Remove(JobId),
    /// Re-queue a failed job.
    Retry(JobId),
}

/// Events emitted by the queue task.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // `Added` is rare; boxing would only add noise
pub enum QueueEvent {
    Added(Job),
    Started(JobId),
    Progress(JobId, f32),
    Done(JobId, HistoryEntry),
    Failed(JobId, String),
    Removed(JobId),
}

/// Actions the UI asks the app to perform.
#[derive(Debug)]
pub enum UiAction {
    Resolve(String),
    Enqueue {
        item: ResolvedItem,
        format: FormatChoice,
    },
    Remove(JobId),
    Retry(JobId),
}

/// Messages delivered to the UI.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // see QueueEvent
pub enum UiMsg {
    Queue(QueueEvent),
    Resolved(Result<ResolvedItem, String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub output_dir: PathBuf,
    pub default_format: FormatChoice,
    /// Max parallel downloads.
    pub concurrency: usize,
    pub ffmpeg_path: String,
    /// Look up release metadata (album, date, track number, cover) on MusicBrainz.
    pub metadata_lookup: bool,
    /// Contact (URL or e-mail) sent in the MusicBrainz User-Agent, as their API etiquette asks.
    pub musicbrainz_contact: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub time: DateTime<Utc>,
    pub title: String,
    pub artist: String,
    pub path: PathBuf,
    pub source_url: String,
    /// e.g. "aac", "opus", "mp3"
    pub format: String,
}
