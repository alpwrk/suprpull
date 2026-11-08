//! api-v2 calls: resolve, playlist hydration, stream URLs.

use anyhow::Result;

use crate::sc::model::{Track, Transcoding};
use crate::types::{Codec, FormatChoice, FormatOption, ResolvedItem};

/// Final stream to hand to ffmpeg.
#[derive(Debug, Clone)]
pub struct StreamInfo {
    /// Signed CDN URL (direct mp3 or .m3u8 playlist).
    pub url: String,
    pub codec: Codec,
    pub is_hls: bool,
}

pub struct ScClient {
    // http client + cached client_id (behind a tokio Mutex/RwLock)
}

impl ScClient {
    pub fn new() -> Result<Self> {
        todo!()
    }

    /// Resolve any soundcloud link (track, set, on.soundcloud.com short link, private link).
    pub async fn resolve(&self, url: &str) -> Result<ResolvedItem> {
        let _ = url;
        todo!()
    }

    /// Pick a transcoding for `choice` and fetch its signed stream URL.
    pub async fn stream_url(&self, track: &Track, choice: FormatChoice) -> Result<StreamInfo> {
        let _ = (track, choice);
        todo!()
    }
}

/// m.soundcloud.com -> soundcloud.com, strip query params except secret tokens, trim.
pub fn normalize_url(url: &str) -> String {
    let _ = url;
    todo!()
}

/// Err with a human readable reason if the track is not downloadable (BLOCK / SNIP / no usable stream).
pub fn check_playable(track: &Track) -> Result<()> {
    let _ = track;
    todo!()
}

/// Choose a non-encrypted, non-snipped transcoding. Preferred codec first, else best
/// (AAC > Opus > MP3, HLS-or-progressive both fine).
pub fn pick_transcoding(track: &Track, choice: FormatChoice) -> Option<&Transcoding> {
    let _ = (track, choice);
    todo!()
}

/// Options for the format popup across all tracks (Best + available codecs + ConvertMp3).
pub fn format_options(tracks: &[Track]) -> Vec<FormatOption> {
    let _ = tracks;
    todo!()
}

/// Cover URL in 500x500 (artwork, else user avatar).
pub fn cover_url(track: &Track) -> Option<String> {
    let _ = track;
    todo!()
}
