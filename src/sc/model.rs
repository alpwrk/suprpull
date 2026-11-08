//! Serde models for the (unofficial) SoundCloud api-v2 responses.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct User {
    pub id: u64,
    #[serde(default)]
    pub username: String,
    pub avatar_url: Option<String>,
    pub permalink_url: Option<String>,
}

/// A track. Inside playlists, tracks beyond the first few are "stubs"
/// that only carry `id` (everything else is `None`) and must be re-fetched
/// via `/tracks?ids=...`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Track {
    pub id: u64,
    pub title: Option<String>,
    pub user: Option<User>,
    /// Playable duration in ms (30s for snipped Go+ previews).
    pub duration: Option<u64>,
    pub full_duration: Option<u64>,
    /// "ALLOW" | "MONETIZE" | "SNIP" | "BLOCK"
    pub policy: Option<String>,
    pub artwork_url: Option<String>,
    pub permalink_url: Option<String>,
    pub track_authorization: Option<String>,
    pub media: Option<Media>,
}

impl Track {
    /// True if this is a playlist stub without metadata/media.
    pub fn is_stub(&self) -> bool {
        self.media.is_none() || self.title.is_none()
    }

    pub fn title_or_default(&self) -> &str {
        self.title.as_deref().unwrap_or("Unknown title")
    }

    pub fn artist_or_default(&self) -> &str {
        self.user
            .as_ref()
            .map(|u| u.username.as_str())
            .unwrap_or("Unknown artist")
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Media {
    #[serde(default)]
    pub transcodings: Vec<Transcoding>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Transcoding {
    /// Must be called with `client_id` + `track_authorization`; returns `{"url": ...}`.
    pub url: String,
    /// e.g. "mp3_1_0", "opus_0_0", "aac_160k"
    #[serde(default)]
    pub preset: String,
    #[serde(default)]
    pub snipped: bool,
    /// "sq" | "hq"
    #[serde(default)]
    pub quality: String,
    pub format: TranscodingFormat,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TranscodingFormat {
    /// "progressive" | "hls" | "ctr-encrypted-hls" | "cbc-encrypted-hls" | ...
    pub protocol: String,
    /// e.g. "audio/mpeg", "audio/ogg; codecs=\"opus\"", "audio/mp4; codecs=\"mp4a.40.2\""
    pub mime_type: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Playlist {
    pub id: u64,
    pub title: Option<String>,
    pub user: Option<User>,
    pub artwork_url: Option<String>,
    pub permalink_url: Option<String>,
    pub track_count: Option<u32>,
    #[serde(default)]
    pub tracks: Vec<Track>,
}

/// Response of `/resolve`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind")]
pub enum Resolved {
    #[serde(rename = "track")]
    Track(Box<Track>),
    #[serde(rename = "playlist")]
    Playlist(Box<Playlist>),
    /// users, system-playlists etc. - unsupported
    #[serde(other)]
    Unsupported,
}

/// Response of a transcoding URL.
#[derive(Debug, Clone, Deserialize)]
pub struct StreamUrlResponse {
    pub url: String,
}
