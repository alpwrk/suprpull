//! api-v2 calls: resolve, playlist hydration, stream URLs.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{StatusCode, Url};
use serde::de::DeserializeOwned;
use tokio::sync::RwLock;

use crate::sc::client_id::{USER_AGENT, get_client_id};
use crate::sc::model::{Resolved, StreamUrlResponse, Track, Transcoding};
use crate::types::{Codec, FormatChoice, FormatOption, ResolvedItem};

const API_BASE: &str = "https://api-v2.soundcloud.com";
const HYDRATE_CHUNK: usize = 50;
/// Codecs in "best first" order.
const CODEC_RANK: [Codec; 3] = [Codec::Aac, Codec::Opus, Codec::Mp3];

/// Final stream to hand to ffmpeg.
#[derive(Debug, Clone)]
pub struct StreamInfo {
    /// Signed CDN URL (direct mp3 or .m3u8 playlist).
    pub url: String,
    pub codec: Codec,
}

pub struct ScClient {
    http: reqwest::Client,
    client_id: RwLock<Option<String>>,
}

impl ScClient {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::limited(10))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .context("could not build HTTP client")?;
        Ok(Self {
            http,
            client_id: RwLock::new(None),
        })
    }

    /// Cached client_id, fetched on first use.
    async fn client_id(&self) -> Result<String> {
        if let Some(id) = self.client_id.read().await.as_ref() {
            return Ok(id.clone());
        }
        let mut guard = self.client_id.write().await;
        if let Some(id) = guard.as_ref() {
            return Ok(id.clone());
        }
        let id = get_client_id(&self.http, false).await?;
        *guard = Some(id.clone());
        Ok(id)
    }

    /// Force a new client_id unless another task already replaced `stale`.
    async fn refresh_client_id(&self, stale: &str) -> Result<String> {
        let mut guard = self.client_id.write().await;
        if let Some(id) = guard.as_ref()
            && id != stale
        {
            return Ok(id.clone());
        }
        let id = get_client_id(&self.http, true).await?;
        *guard = Some(id.clone());
        Ok(id)
    }

    /// GET `url` with `query` + `client_id`; on 401/403 refresh the client_id once and retry.
    async fn get_json<T: DeserializeOwned>(&self, url: &str, query: &[(&str, &str)]) -> Result<T> {
        let mut id = self.client_id().await?;
        let mut retried = false;
        loop {
            let resp = self
                .http
                .get(url)
                .query(query)
                .query(&[("client_id", id.as_str())])
                .send()
                .await
                .with_context(|| format!("request to {} failed", strip_query(url)))?;
            let status = resp.status();
            if (status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN) && !retried {
                retried = true;
                id = self.refresh_client_id(&id).await?;
                continue;
            }
            if status == StatusCode::NOT_FOUND {
                bail!("not found (HTTP 404) - deleted, private or mistyped link?");
            }
            if !status.is_success() {
                bail!("HTTP {} from {}", status, strip_query(url));
            }
            return resp
                .json::<T>()
                .await
                .with_context(|| format!("unexpected response from {}", strip_query(url)));
        }
    }

    /// Resolve any soundcloud link (track, set, on.soundcloud.com short link, private link).
    pub async fn resolve(&self, url: &str) -> Result<ResolvedItem> {
        let mut normalized = normalize_url(url);
        let host = host_of(&normalized)?;
        if host == "on.soundcloud.com" {
            let resp = self
                .http
                .get(&normalized)
                .send()
                .await
                .context("could not follow the short link")?
                .error_for_status()
                .context("could not follow the short link")?;
            normalized = normalize_url(resp.url().as_str());
            if host_of(&normalized)? != "soundcloud.com" {
                bail!("short link did not lead to a soundcloud.com page");
            }
        } else if host != "soundcloud.com" {
            bail!("not a SoundCloud link: {host}");
        }

        let resolved: Resolved = self
            .get_json(&format!("{API_BASE}/resolve"), &[("url", &normalized)])
            .await
            .context("could not resolve link")?;

        match resolved {
            Resolved::Track(track) => {
                let track = *track;
                if track.is_stub() {
                    bail!("track has no stream information");
                }
                check_playable(&track)
                    .map_err(|e| anyhow!("cannot download \"{}\": {e}", track.title_or_default()))?;
                Ok(ResolvedItem {
                    source_url: normalized,
                    title: track.title_or_default().to_string(),
                    artist: track.artist_or_default().to_string(),
                    set_title: None,
                    format_options: format_options(std::slice::from_ref(&track)),
                    tracks: vec![track],
                    skipped: Vec::new(),
                })
            }
            Resolved::Playlist(pl) => {
                let pl = *pl;
                let stub_ids: Vec<u64> =
                    pl.tracks.iter().filter(|t| t.is_stub()).map(|t| t.id).collect();
                let mut fetched = Vec::new();
                for chunk in stub_ids.chunks(HYDRATE_CHUNK) {
                    let ids = chunk.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
                    let mut part: Vec<Track> = self
                        .get_json(&format!("{API_BASE}/tracks"), &[("ids", &ids)])
                        .await
                        .context("could not load playlist tracks")?;
                    fetched.append(&mut part);
                }
                let (tracks, skipped) = hydrate_and_split(pl.tracks, fetched);
                let set_title = pl.title.clone().unwrap_or_else(|| "Unknown set".to_string());
                if tracks.is_empty() {
                    bail!("no downloadable tracks in \"{set_title}\" ({} skipped)", skipped.len());
                }
                let artist = pl
                    .user
                    .as_ref()
                    .map(|u| u.username.clone())
                    .unwrap_or_else(|| tracks[0].artist_or_default().to_string());
                Ok(ResolvedItem {
                    source_url: normalized,
                    title: set_title.clone(),
                    artist,
                    set_title: Some(set_title),
                    format_options: format_options(&tracks),
                    tracks,
                    skipped,
                })
            }
            Resolved::Unsupported => bail!("unsupported link (only tracks and sets)"),
        }
    }

    /// Pick a transcoding for `choice` and fetch its signed stream URL.
    pub async fn stream_url(&self, track: &Track, choice: FormatChoice) -> Result<StreamInfo> {
        check_playable(track)?;
        let tc = pick_transcoding(track, choice).ok_or_else(|| anyhow!("no downloadable stream"))?;
        let codec = Codec::from_mime(&tc.format.mime_type)
            .ok_or_else(|| anyhow!("unknown stream codec {}", tc.format.mime_type))?;
        let mut query: Vec<(&str, &str)> = Vec::new();
        if let Some(auth) = track.track_authorization.as_deref() {
            query.push(("track_authorization", auth));
        }
        let resp: StreamUrlResponse = self
            .get_json(&tc.url, &query)
            .await
            .context("could not get stream URL")?;
        // ffmpeg handles HLS playlists and progressive files alike.
        Ok(StreamInfo {
            url: resp.url,
            codec,
        })
    }
}

fn strip_query(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

fn host_of(url: &str) -> Result<String> {
    let parsed = Url::parse(url).map_err(|_| anyhow!("not a valid link: {url}"))?;
    parsed
        .host_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("not a valid link: {url}"))
}

/// Replace playlist stubs with fetched tracks (keeping playlist order) and split
/// into playable tracks and skipped (title, reason) pairs.
fn hydrate_and_split(
    playlist_tracks: Vec<Track>,
    fetched: Vec<Track>,
) -> (Vec<Track>, Vec<(String, String)>) {
    let mut by_id: HashMap<u64, Track> = fetched.into_iter().map(|t| (t.id, t)).collect();
    let mut tracks = Vec::new();
    let mut skipped = Vec::new();
    for t in playlist_tracks {
        let t = if t.is_stub() {
            match by_id.remove(&t.id) {
                Some(full) => full,
                None => {
                    skipped.push((format!("Track {}", t.id), "unavailable".to_string()));
                    continue;
                }
            }
        } else {
            t
        };
        match check_playable(&t) {
            Ok(()) => tracks.push(t),
            Err(e) => skipped.push((t.title_or_default().to_string(), e.to_string())),
        }
    }
    (tracks, skipped)
}

/// m.soundcloud.com -> soundcloud.com, strip query params except secret tokens, trim.
///
/// Secret tokens of private links usually live in the path (`/s-XXXX`); a
/// `secret_token` query parameter is kept as well. Everything else in the query
/// string and the fragment is dropped. Non-URL input is returned trimmed.
pub fn normalize_url(url: &str) -> String {
    let trimmed = url.trim();
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let Ok(parsed) = Url::parse(&with_scheme) else {
        return trimmed.to_string();
    };
    let Some(host) = parsed.host_str() else {
        return trimmed.to_string();
    };
    let host = host.to_ascii_lowercase();
    let host = match host.as_str() {
        "m.soundcloud.com" | "www.soundcloud.com" => "soundcloud.com",
        h => h,
    };
    let path = parsed.path().trim_end_matches('/');
    match parsed
        .query_pairs()
        .find(|(k, v)| k == "secret_token" && !v.is_empty())
    {
        Some((_, token)) => format!("https://{host}{path}?secret_token={token}"),
        None => format!("https://{host}{path}"),
    }
}

/// Err with a human readable reason if the track is not downloadable (BLOCK / SNIP / no usable stream).
pub fn check_playable(track: &Track) -> Result<()> {
    match track.policy.as_deref() {
        Some("BLOCK") => bail!("blocked in your region"),
        Some("SNIP") => bail!("preview only (Go+ track)"),
        _ => {}
    }
    let transcodings = track
        .media
        .as_ref()
        .map(|m| m.transcodings.as_slice())
        .unwrap_or_default();
    if !transcodings.is_empty() && transcodings.iter().all(|t| t.snipped) {
        bail!("preview only (Go+ track)");
    }
    if pick_transcoding(track, FormatChoice::Best).is_none() {
        bail!("no downloadable stream");
    }
    Ok(())
}

fn is_usable(t: &Transcoding) -> bool {
    !t.snipped
        && !t.format.protocol.contains("encrypted")
        && matches!(t.format.protocol.as_str(), "hls" | "progressive")
        && Codec::from_mime(&t.format.mime_type).is_some()
}

/// Best usable transcoding of `codec`: hq over sq, then progressive over hls.
fn best_of_codec(track: &Track, codec: Codec) -> Option<&Transcoding> {
    let mut best: Option<(&Transcoding, (bool, bool))> = None;
    for t in track.media.as_ref()?.transcodings.iter() {
        if !is_usable(t) || Codec::from_mime(&t.format.mime_type) != Some(codec) {
            continue;
        }
        let score = (t.quality == "hq", t.format.protocol == "progressive");
        if best.is_none_or(|(_, s)| score > s) {
            best = Some((t, score));
        }
    }
    best.map(|(t, _)| t)
}

/// Choose a non-encrypted, non-snipped transcoding. Preferred codec first, else best
/// (AAC > Opus > MP3, HLS-or-progressive both fine).
pub fn pick_transcoding(track: &Track, choice: FormatChoice) -> Option<&Transcoding> {
    if let Some(codec) = choice.preferred_codec()
        && let Some(t) = best_of_codec(track, codec)
    {
        return Some(t);
    }
    CODEC_RANK.iter().find_map(|&c| best_of_codec(track, c))
}

fn codec_name(c: Codec) -> &'static str {
    match c {
        Codec::Aac => "AAC",
        Codec::Opus => "Opus",
        Codec::Mp3 => "MP3",
    }
}

/// Bitrate in kbps from a preset like "aac_160k"; known defaults for mp3/opus presets.
fn preset_kbps(preset: &str) -> Option<u32> {
    if let Some(n) = preset
        .split('_')
        .find_map(|part| part.strip_suffix('k').and_then(|d| d.parse::<u32>().ok()))
    {
        return Some(n);
    }
    if preset.starts_with("mp3_") {
        Some(128)
    } else if preset.starts_with("opus_") {
        Some(64)
    } else {
        None
    }
}

/// e.g. "AAC 160 kbps"
fn quality_label(codec: Codec, t: &Transcoding) -> String {
    match preset_kbps(&t.preset) {
        Some(k) => format!("{} {k} kbps", codec_name(codec)),
        None => codec_name(codec).to_string(),
    }
}

fn protocol_label(t: &Transcoding) -> &'static str {
    if t.format.protocol == "progressive" {
        "progressive"
    } else {
        "HLS"
    }
}

/// Options for the format popup across all tracks (Best + available codecs + ConvertMp3).
pub fn format_options(tracks: &[Track]) -> Vec<FormatOption> {
    let n = tracks.len();
    let mut out = Vec::new();

    // Best
    let bests: Vec<(Codec, &Transcoding)> = tracks
        .iter()
        .filter_map(|t| pick_transcoding(t, FormatChoice::Best))
        .filter_map(|tc| Codec::from_mime(&tc.format.mime_type).map(|c| (c, tc)))
        .collect();
    let best_label = match bests.first() {
        Some(&(c, tc)) if bests.iter().all(|(c2, _)| *c2 == c) => {
            format!("Best ({}) -> .{}", quality_label(c, tc), c.extension())
        }
        Some(_) => "Best (per track: AAC > Opus > MP3)".to_string(),
        None => "Best".to_string(),
    };
    out.push(FormatOption {
        choice: FormatChoice::Best,
        label: best_label,
    });

    // One entry per available codec.
    for codec in CODEC_RANK {
        let found: Vec<&Transcoding> =
            tracks.iter().filter_map(|t| best_of_codec(t, codec)).collect();
        let Some(first) = found.first() else {
            continue;
        };
        let mut label = format!(
            "{} ({}) -> .{}",
            quality_label(codec, first),
            protocol_label(first),
            codec.extension()
        );
        if found.len() < n {
            label.push_str(&format!(" ({}/{} tracks)", found.len(), n));
        }
        let choice = match codec {
            Codec::Aac => FormatChoice::Aac,
            Codec::Opus => FormatChoice::Opus,
            Codec::Mp3 => FormatChoice::Mp3,
        };
        out.push(FormatOption { choice, label });
    }

    out.push(FormatOption {
        choice: FormatChoice::ConvertMp3,
        label: "Convert to MP3 (.mp3)".to_string(),
    });
    out
}

/// Cover URL in 500x500 (artwork, else user avatar).
pub fn cover_url(track: &Track) -> Option<String> {
    track
        .artwork_url
        .clone()
        .or_else(|| track.user.as_ref().and_then(|u| u.avatar_url.clone()))
        .map(|u| u.replace("-large", "-t500x500"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sc::model::Playlist;

    const TRACK: &str = include_str!("../../tests/fixtures/sc/track.json");
    const SNIP: &str = include_str!("../../tests/fixtures/sc/track_snip.json");
    const BLOCK: &str = include_str!("../../tests/fixtures/sc/track_block.json");
    const PLAYLIST: &str = include_str!("../../tests/fixtures/sc/playlist.json");
    const USER: &str = include_str!("../../tests/fixtures/sc/user.json");
    const HYDRATED: &str = include_str!("../../tests/fixtures/sc/tracks_hydrated.json");

    fn track(json: &str) -> Track {
        serde_json::from_str(json).expect("track fixture")
    }

    fn playlist() -> Playlist {
        match serde_json::from_str::<Resolved>(PLAYLIST).expect("playlist fixture") {
            Resolved::Playlist(p) => *p,
            other => panic!("expected playlist, got {other:?}"),
        }
    }

    #[test]
    fn normalize() {
        let cases = [
            ("  https://soundcloud.com/a/b  ", "https://soundcloud.com/a/b"),
            ("soundcloud.com/a/b", "https://soundcloud.com/a/b"),
            ("https://m.soundcloud.com/a/b", "https://soundcloud.com/a/b"),
            ("http://www.soundcloud.com/a/b/", "https://soundcloud.com/a/b"),
            (
                "https://soundcloud.com/a/b?si=123&utm_source=clipboard#t=1:00",
                "https://soundcloud.com/a/b",
            ),
            (
                "https://soundcloud.com/a/b?si=1&secret_token=s-AbC12",
                "https://soundcloud.com/a/b?secret_token=s-AbC12",
            ),
            (
                "https://soundcloud.com/a/b/s-AbCdE?si=x",
                "https://soundcloud.com/a/b/s-AbCdE",
            ),
            ("https://on.soundcloud.com/XyZ12", "https://on.soundcloud.com/XyZ12"),
            ("https://Soundcloud.COM/a/sets/c", "https://soundcloud.com/a/sets/c"),
            ("https://example.com/x", "https://example.com/x"),
        ];
        for (input, want) in cases {
            assert_eq!(normalize_url(input), want, "input: {input}");
        }
    }

    #[test]
    fn deserialize_resolved_kinds() {
        assert!(matches!(
            serde_json::from_str::<Resolved>(TRACK).unwrap(),
            Resolved::Track(_)
        ));
        assert!(matches!(
            serde_json::from_str::<Resolved>(PLAYLIST).unwrap(),
            Resolved::Playlist(_)
        ));
        assert!(matches!(
            serde_json::from_str::<Resolved>(USER).unwrap(),
            Resolved::Unsupported
        ));
        let pl = playlist();
        assert_eq!(pl.tracks.len(), 4);
        assert_eq!(pl.tracks.iter().filter(|t| t.is_stub()).count(), 2);
    }

    #[test]
    fn pick_best_is_aac_hls() {
        let t = track(TRACK);
        let tc = pick_transcoding(&t, FormatChoice::Best).unwrap();
        assert_eq!(tc.preset, "aac_160k");
        assert_eq!(tc.format.protocol, "hls");
        // ConvertMp3 re-encodes the best stream.
        let tc = pick_transcoding(&t, FormatChoice::ConvertMp3).unwrap();
        assert_eq!(tc.preset, "aac_160k");
    }

    #[test]
    fn pick_preferred_codecs() {
        let t = track(TRACK);
        let mp3 = pick_transcoding(&t, FormatChoice::Mp3).unwrap();
        assert_eq!(mp3.format.protocol, "progressive");
        assert_eq!(mp3.format.mime_type, "audio/mpeg");
        let opus = pick_transcoding(&t, FormatChoice::Opus).unwrap();
        assert_eq!(opus.preset, "opus_0_0");
        let aac = pick_transcoding(&t, FormatChoice::Aac).unwrap();
        assert_eq!(aac.preset, "aac_160k");
    }

    #[test]
    fn pick_never_encrypted() {
        let mut t = track(TRACK);
        // Keep only the encrypted transcoding.
        t.media
            .as_mut()
            .unwrap()
            .transcodings
            .retain(|x| x.format.protocol.contains("encrypted"));
        assert!(pick_transcoding(&t, FormatChoice::Best).is_none());
        assert_eq!(check_playable(&t).unwrap_err().to_string(), "no downloadable stream");
    }

    #[test]
    fn pick_prefers_hq() {
        let mut t = track(TRACK);
        let mut hq = t.media.as_ref().unwrap().transcodings[0].clone();
        hq.preset = "aac_256k".into();
        hq.quality = "hq".into();
        t.media.as_mut().unwrap().transcodings.push(hq);
        assert_eq!(pick_transcoding(&t, FormatChoice::Best).unwrap().preset, "aac_256k");
    }

    #[test]
    fn pick_falls_back_when_preferred_missing() {
        let mut t = track(TRACK);
        t.media
            .as_mut()
            .unwrap()
            .transcodings
            .retain(|x| !x.format.mime_type.contains("mp4a"));
        let tc = pick_transcoding(&t, FormatChoice::Aac).unwrap();
        assert_eq!(tc.preset, "opus_0_0");
    }

    #[test]
    fn playable_checks() {
        assert!(check_playable(&track(TRACK)).is_ok());
        assert_eq!(
            check_playable(&track(SNIP)).unwrap_err().to_string(),
            "preview only (Go+ track)"
        );
        assert_eq!(
            check_playable(&track(BLOCK)).unwrap_err().to_string(),
            "blocked in your region"
        );
        // All snipped but policy not SNIP.
        let mut t = track(SNIP);
        t.policy = Some("ALLOW".into());
        assert_eq!(check_playable(&t).unwrap_err().to_string(), "preview only (Go+ track)");
    }

    #[test]
    fn options_single_track() {
        let opts = format_options(&[track(TRACK)]);
        let labels: Vec<_> = opts.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "Best (AAC 160 kbps) -> .m4a",
                "AAC 160 kbps (HLS) -> .m4a",
                "Opus 64 kbps (HLS) -> .opus",
                "MP3 128 kbps (progressive) -> .mp3",
                "Convert to MP3 (.mp3)",
            ]
        );
        let choices: Vec<_> = opts.iter().map(|o| o.choice).collect();
        assert_eq!(
            choices,
            vec![
                FormatChoice::Best,
                FormatChoice::Aac,
                FormatChoice::Opus,
                FormatChoice::Mp3,
                FormatChoice::ConvertMp3
            ]
        );
    }

    #[test]
    fn options_empty() {
        let opts = format_options(&[]);
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0].choice, FormatChoice::Best);
        assert_eq!(opts[1].choice, FormatChoice::ConvertMp3);
    }

    #[test]
    fn playlist_hydration_and_options() {
        let pl = playlist();
        let fetched: Vec<Track> = serde_json::from_str(HYDRATED).unwrap();
        let (tracks, skipped) = hydrate_and_split(pl.tracks, fetched);
        let titles: Vec<_> = tracks.iter().map(|t| t.title_or_default()).collect();
        assert_eq!(titles, vec!["Night Drive", "Second Song", "Third Song"]);
        assert_eq!(
            skipped,
            vec![("Hit Single".to_string(), "preview only (Go+ track)".to_string())]
        );

        let labels: Vec<_> = format_options(&tracks).into_iter().map(|o| o.label).collect();
        assert_eq!(
            labels,
            vec![
                "Best (per track: AAC > Opus > MP3)",
                "AAC 160 kbps (HLS) -> .m4a (2/3 tracks)",
                "Opus 64 kbps (HLS) -> .opus (1/3 tracks)",
                "MP3 128 kbps (progressive) -> .mp3",
                "Convert to MP3 (.mp3)",
            ]
        );
    }

    #[test]
    fn hydration_missing_track_is_skipped() {
        let pl = playlist();
        let (tracks, skipped) = hydrate_and_split(pl.tracks, Vec::new());
        assert_eq!(tracks.len(), 2);
        assert_eq!(skipped.len(), 2);
        assert_eq!(skipped[0], ("Track 1700000005".to_string(), "unavailable".to_string()));
    }

    #[test]
    fn preset_bitrates() {
        assert_eq!(preset_kbps("aac_160k"), Some(160));
        assert_eq!(preset_kbps("aac_256k"), Some(256));
        assert_eq!(preset_kbps("mp3_1_0"), Some(128));
        assert_eq!(preset_kbps("opus_0_0"), Some(64));
        assert_eq!(preset_kbps("abr_sq"), None);
    }

    #[test]
    fn covers() {
        assert_eq!(
            cover_url(&track(TRACK)).as_deref(),
            Some("https://i1.sndcdn.com/artworks-000123456789-abcdef-t500x500.jpg")
        );
        assert_eq!(
            cover_url(&track(SNIP)).as_deref(),
            Some("https://i1.sndcdn.com/avatars-000111-aaa-t500x500.jpg")
        );
        let mut t = track(BLOCK);
        t.artwork_url = None;
        assert_eq!(cover_url(&t), None);
    }

    #[test]
    fn host_check() {
        assert_eq!(host_of("https://soundcloud.com/a").unwrap(), "soundcloud.com");
        assert!(host_of("not a url").is_err());
    }
}
