//! Track metadata: built from SoundCloud's own fields and, when the match is
//! certain, enriched from MusicBrainz (+ cover from the Cover Art Archive).

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::sc::model::Track;

const MB_API: &str = "https://musicbrainz.org/ws/2";
const CAA_API: &str = "https://coverartarchive.org";
/// MusicBrainz allows one request per second per client.
const MB_INTERVAL: Duration = Duration::from_millis(1100);
/// Max length difference between the SoundCloud track and a MusicBrainz recording.
const MAX_LENGTH_DIFF_MS: u64 = 3_000;
/// Minimum MusicBrainz search score for a text match.
const MIN_SCORE: u32 = 90;
/// Max text searches per track (ISRC lookups come on top).
const MAX_TEXT_QUERIES: usize = 2;

/// All tags written into the output file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackMeta {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_no: Option<u32>,
    pub track_total: Option<u32>,
    /// `YYYY-MM-DD`, `YYYY-MM` or `YYYY`.
    pub date: Option<String>,
    pub genre: Option<String>,
    pub isrc: Option<String>,
    pub label: Option<String>,
    pub composer: Option<String>,
    pub copyright: Option<String>,
    pub cover_url: Option<String>,
    pub mb_recording_id: Option<String>,
    pub mb_release_id: Option<String>,
    pub mb_release_group_id: Option<String>,
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Position of a track inside a SoundCloud set that is downloaded as a whole.
#[derive(Debug, Clone, Copy)]
pub struct SetPosition<'a> {
    pub title: &'a str,
    pub number: u32,
    pub total: u32,
}

impl TrackMeta {
    /// Tags from SoundCloud alone. With `set`, the set becomes the album.
    pub fn from_soundcloud(track: &Track, set: Option<SetPosition<'_>>) -> Self {
        let pm = track.publisher_metadata.clone().unwrap_or_default();
        let artist = non_empty(pm.artist.as_deref())
            .unwrap_or_else(|| track.artist_or_default().to_string());
        let (album, track_no, track_total) = match set {
            Some(s) => (Some(s.title.to_string()), Some(s.number), Some(s.total)),
            None => (
                non_empty(pm.album_title.as_deref())
                    .or_else(|| non_empty(pm.release_title.as_deref())),
                None,
                None,
            ),
        };
        let date = non_empty(track.release_date.as_deref())
            .or_else(|| non_empty(track.created_at.as_deref()))
            .map(|d| d.chars().take(10).collect());
        let isrc = non_empty(pm.isrc.as_deref())
            .map(|i| i.replace('-', "").to_ascii_uppercase())
            .filter(|i| i.len() == 12);
        Self {
            title: track.title_or_default().trim().to_string(),
            artist,
            album,
            album_artist: None,
            track_no,
            track_total,
            date,
            genre: non_empty(track.genre.as_deref()),
            isrc,
            label: non_empty(track.label_name.as_deref()),
            composer: non_empty(pm.writer_composer.as_deref()),
            copyright: non_empty(pm.p_line_for_display.as_deref())
                .or_else(|| non_empty(pm.c_line_for_display.as_deref())),
            cover_url: crate::sc::api::cover_url(track),
            mb_recording_id: None,
            mb_release_id: None,
            mb_release_group_id: None,
        }
    }

    /// ffmpeg `-metadata` key/value pairs for an output extension ("m4a" | "opus" | "mp3").
    pub fn ffmpeg_tags(&self, ext: &str) -> Vec<(String, String)> {
        let mut t: Vec<(String, String)> = Vec::new();
        let mut push = |k: &str, v: Option<&str>| {
            if let Some(v) = v.filter(|v| !v.is_empty()) {
                t.push((k.to_string(), v.to_string()));
            }
        };
        push("title", Some(&self.title));
        push("artist", Some(&self.artist));
        push("album", self.album.as_deref());
        push("album_artist", self.album_artist.as_deref());
        let track = match (self.track_no, self.track_total) {
            (Some(n), Some(total)) => Some(format!("{n}/{total}")),
            (Some(n), None) => Some(n.to_string()),
            _ => None,
        };
        push("track", track.as_deref());
        push("date", self.date.as_deref());
        push("genre", self.genre.as_deref());
        push("composer", self.composer.as_deref());
        push("copyright", self.copyright.as_deref());
        // Container-specific keys. The mp4 muxer only writes known iTunes atoms,
        // so ISRC / label / MusicBrainz ids are skipped there.
        match ext {
            "opus" => {
                push("LABEL", self.label.as_deref());
                push("ISRC", self.isrc.as_deref());
                push("MUSICBRAINZ_TRACKID", self.mb_recording_id.as_deref());
                push("MUSICBRAINZ_ALBUMID", self.mb_release_id.as_deref());
                push(
                    "MUSICBRAINZ_RELEASEGROUPID",
                    self.mb_release_group_id.as_deref(),
                );
            }
            "mp3" => {
                push("publisher", self.label.as_deref()); // TPUB
                push("TSRC", self.isrc.as_deref());
                push("MusicBrainz Track Id", self.mb_recording_id.as_deref());
                push("MusicBrainz Album Id", self.mb_release_id.as_deref());
                push(
                    "MusicBrainz Release Group Id",
                    self.mb_release_group_id.as_deref(),
                );
            }
            _ => {}
        }
        t
    }
}

// ---------------------------------------------------------------- title cleanup

static PROMO_BRACKETS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\s*[\(\[\{][^\)\]\}]*\b(free\s*(dl|download)|out\s*now|premiere|buy|original\s*mix|exclusive|preview|clip|official\s*(audio|video))\b[^\)\]\}]*[\)\]\}]",
    )
    .unwrap()
});
static PROMO_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(premiere|exclusive|free\s*(dl|download))\s*[:|\-]\s*").unwrap()
});
static PROMO_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*(\||//|\*\*).*$|\s+free\s*(dl|download)\b.*$").unwrap());
static FEAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*[\(\[]?\s*\b(feat|ft|featuring)\b\.?\s+[^\)\]]*[\)\]]?").unwrap()
});
static ARTIST_SPLIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*(,|&|\bx\b|\band\b|\bvs\.?)\s*").unwrap());

/// Strip promo noise like "[FREE DL]", "(Original Mix)", "Premiere: ", "| out now".
pub fn clean_title(title: &str) -> String {
    let t = PROMO_PREFIX.replace(title, "");
    let t = PROMO_BRACKETS.replace_all(&t, "");
    let t = PROMO_SUFFIX.replace(&t, "");
    t.trim().to_string()
}

/// "Artist - Title" (also with en/em dashes) -> (artist, title).
pub fn split_artist_title(title: &str) -> Option<(String, String)> {
    ["  - ", " - ", " – ", " — "].iter().find_map(|sep| {
        let (a, t) = title.split_once(sep)?;
        let (a, t) = (a.trim(), t.trim());
        (!a.is_empty() && !t.is_empty()).then(|| (a.to_string(), t.to_string()))
    })
}

/// Lowercase, without "feat." parts, punctuation collapsed to single spaces.
fn norm(s: &str) -> String {
    let s = FEAT.replace_all(s, " ");
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn artists_match(query: &str, credit: &[Credit]) -> bool {
    let q = norm(query);
    let full = norm(&join_credit(credit));
    if q.is_empty() || full.is_empty() {
        return false;
    }
    if q == full || full.contains(&q) || q.contains(&full) {
        return true;
    }
    // "A & B" on SoundCloud vs. credit "A feat. B" etc.: every query artist must be credited.
    let names: Vec<String> = credit.iter().map(|c| norm(&c.name)).collect();
    ARTIST_SPLIT
        .split(query)
        .map(norm)
        .filter(|p| !p.is_empty())
        .all(|p| names.contains(&p))
}

/// Search queries (artist, title) to try, most specific first.
fn text_queries(meta: &TrackMeta, track: &Track) -> Vec<(String, String)> {
    let cleaned = clean_title(track.title_or_default());
    let uploader = track.artist_or_default();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut add = |a: &str, t: &str| {
        let pair = (a.trim().to_string(), clean_title(t));
        if !pair.0.is_empty() && !pair.1.is_empty() && !out.contains(&pair) {
            out.push(pair);
        }
    };
    let split = split_artist_title(&cleaned);
    // publisher_metadata artist is the most reliable source.
    if meta.artist != uploader {
        match &split {
            Some((a, t)) if norm(a) == norm(&meta.artist) => add(&meta.artist, t),
            _ => add(&meta.artist, &cleaned),
        }
    }
    if let Some((a, t)) = &split {
        add(a, t);
    }
    add(uploader, &cleaned);
    out.truncate(MAX_TEXT_QUERIES);
    out
}

// ---------------------------------------------------------------- MusicBrainz models

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    recordings: Vec<Recording>,
}

#[derive(Debug, Clone, Deserialize)]
struct Recording {
    id: String,
    #[serde(default)]
    score: u32,
    title: String,
    length: Option<u64>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<Credit>,
    #[serde(rename = "first-release-date")]
    first_release_date: Option<String>,
    #[serde(default)]
    releases: Vec<Release>,
}

#[derive(Debug, Clone, Deserialize)]
struct Credit {
    name: String,
    #[serde(default)]
    joinphrase: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Release {
    id: String,
    title: String,
    status: Option<String>,
    date: Option<String>,
    #[serde(rename = "artist-credit")]
    artist_credit: Option<Vec<Credit>>,
    #[serde(rename = "release-group")]
    release_group: Option<ReleaseGroup>,
    #[serde(default)]
    media: Vec<Medium>,
}

#[derive(Debug, Clone, Deserialize)]
struct ReleaseGroup {
    id: String,
    #[serde(rename = "primary-type")]
    primary_type: Option<String>,
    #[serde(rename = "secondary-types", default)]
    secondary_types: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Medium {
    #[serde(default)]
    track: Vec<MediumTrack>,
    #[serde(rename = "track-count")]
    track_count: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
struct MediumTrack {
    number: Option<String>,
}

fn join_credit(credit: &[Credit]) -> String {
    credit
        .iter()
        .map(|c| format!("{}{}", c.name, c.joinphrase))
        .collect::<String>()
        .trim()
        .to_string()
}

/// Best release for tagging: official, plain album/EP/single (no compilation or
/// soundtrack), by the same artist as the recording, earliest date.
fn pick_release(rec: &Recording) -> Option<&Release> {
    let rec_artist = norm(&join_credit(&rec.artist_credit));
    rec.releases.iter().min_by_key(|r| {
        let rg = r.release_group.as_ref();
        let type_rank = match rg.and_then(|g| g.primary_type.as_deref()) {
            Some("Album") => 0,
            Some("EP") => 1,
            Some("Single") => 2,
            _ => 3,
        };
        let other_artist = r
            .artist_credit
            .as_ref()
            .is_some_and(|c| norm(&join_credit(c)) != rec_artist);
        (
            r.status.as_deref() != Some("Official"),
            rg.is_some_and(|g| !g.secondary_types.is_empty()),
            other_artist,
            type_rank,
            r.date.is_none(),
            r.date.clone().unwrap_or_default(),
        )
    })
}

/// Pick the best acceptable recording. `strict` = text search (title/artist/score must match);
/// non-strict = ISRC lookup (only the length is checked).
fn pick_recording<'a>(
    recs: &'a [Recording],
    query: Option<(&str, &str)>,
    duration_ms: u64,
) -> Option<&'a Recording> {
    recs.iter()
        .filter(|r| {
            let Some(len) = r.length else { return false };
            if len.abs_diff(duration_ms) > MAX_LENGTH_DIFF_MS {
                return false;
            }
            match query {
                None => true,
                Some((artist, title)) => {
                    r.score >= MIN_SCORE
                        && norm(&r.title) == norm(title)
                        && artists_match(artist, &r.artist_credit)
                }
            }
        })
        .min_by_key(|r| {
            (
                std::cmp::Reverse(r.score),
                r.length.unwrap_or(0).abs_diff(duration_ms),
            )
        })
}

/// Merge a MusicBrainz match into SoundCloud metadata.
fn apply_match(mut meta: TrackMeta, rec: &Recording, in_set: bool) -> TrackMeta {
    meta.title = rec.title.clone();
    let artist = join_credit(&rec.artist_credit);
    if !artist.is_empty() {
        meta.artist = artist;
    }
    meta.mb_recording_id = Some(rec.id.clone());
    if let Some(rel) = pick_release(rec) {
        meta.mb_release_id = Some(rel.id.clone());
        meta.mb_release_group_id = rel.release_group.as_ref().map(|g| g.id.clone());
        if !in_set {
            meta.album = Some(rel.title.clone());
            meta.album_artist = rel
                .artist_credit
                .as_deref()
                .map(join_credit)
                .or_else(|| Some(meta.artist.clone()));
            if let Some(m) = rel.media.first() {
                meta.track_no = m
                    .track
                    .first()
                    .and_then(|t| t.number.as_deref())
                    .and_then(|n| n.parse().ok());
                meta.track_total = m.track_count;
            }
        }
        if let Some(d) = non_empty(rel.date.as_deref()) {
            meta.date = Some(d);
        }
    }
    if let Some(d) = non_empty(rec.first_release_date.as_deref()) {
        // The original release date beats a later re-release.
        if meta.date.as_deref().is_none_or(|cur| d.as_str() < cur) {
            meta.date = Some(d);
        }
    }
    meta
}

/// Escape a value for a quoted Lucene phrase.
fn lucene_phrase(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

// ---------------------------------------------------------------- client

/// Rate-limited MusicBrainz + Cover Art Archive client. Share one instance.
pub struct MusicBrainz {
    http: reqwest::Client,
    /// Time of the last request; held across a request so calls are serialized.
    last: Mutex<Option<Instant>>,
}

impl MusicBrainz {
    pub fn new(contact: Option<&str>) -> Result<Self> {
        let ua = match contact.map(str::trim).filter(|c| !c.is_empty()) {
            Some(c) => format!("suprpull/{} ( {c} )", env!("CARGO_PKG_VERSION")),
            None => format!("suprpull/{}", env!("CARGO_PKG_VERSION")),
        };
        let http = reqwest::Client::builder()
            .user_agent(ua)
            .timeout(Duration::from_secs(15))
            .build()
            .context("could not build MusicBrainz HTTP client")?;
        Ok(Self {
            http,
            last: Mutex::new(None),
        })
    }

    async fn search(&self, query: &str) -> Result<Vec<Recording>> {
        let mut last = self.last.lock().await;
        for attempt in 0..3 {
            if let Some(t) = *last {
                let wait = MB_INTERVAL.saturating_sub(t.elapsed());
                tokio::time::sleep(wait).await;
            }
            *last = Some(Instant::now());
            let resp = self
                .http
                .get(format!("{MB_API}/recording"))
                .query(&[("query", query), ("fmt", "json"), ("limit", "10")])
                .send()
                .await
                .context("MusicBrainz request failed")?;
            let status = resp.status();
            if status == reqwest::StatusCode::SERVICE_UNAVAILABLE && attempt < 2 {
                // "server is busy" / rate limited: back off and retry.
                tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
                continue;
            }
            if !status.is_success() {
                bail!("MusicBrainz returned HTTP {status}");
            }
            let body: SearchResponse = resp.json().await.context("invalid MusicBrainz response")?;
            return Ok(body.recordings);
        }
        bail!("MusicBrainz is busy")
    }

    /// Final URL of the 500px front cover of a release group, if one exists.
    async fn cover(&self, release_group_id: &str) -> Option<String> {
        let resp = self
            .http
            .head(format!(
                "{CAA_API}/release-group/{release_group_id}/front-500"
            ))
            .send()
            .await
            .ok()?;
        resp.status().is_success().then(|| resp.url().to_string())
    }

    /// Enrich SoundCloud metadata. Returns the input unchanged if nothing certain was found
    /// or MusicBrainz is unreachable. The bool tells whether a match was applied.
    pub async fn enrich(&self, meta: TrackMeta, track: &Track, in_set: bool) -> (TrackMeta, bool) {
        let Some(duration) = track.full_duration.or(track.duration) else {
            return (meta, false);
        };
        let mut found: Option<Recording> = None;

        if let Some(isrc) = &meta.isrc
            && let Ok(recs) = self.search(&format!("isrc:{isrc}")).await
        {
            found = pick_recording(&recs, None, duration).cloned();
        }
        if found.is_none() {
            for (artist, title) in text_queries(&meta, track) {
                let q = format!(
                    "recording:{} AND artist:{}",
                    lucene_phrase(&title),
                    lucene_phrase(&artist)
                );
                let Ok(recs) = self.search(&q).await else {
                    break;
                };
                if let Some(r) = pick_recording(&recs, Some((&artist, &title)), duration) {
                    found = Some(r.clone());
                    break;
                }
            }
        }
        let Some(rec) = found else {
            return (meta, false);
        };
        let mut meta = apply_match(meta, &rec, in_set);
        // Sets keep the SoundCloud set artwork so all files share one cover.
        if !in_set
            && let Some(rg) = meta.mb_release_group_id.clone()
            && let Some(url) = self.cover(&rg).await
        {
            meta.cover_url = Some(url);
        }
        (meta, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc_track(v: serde_json::Value) -> Track {
        serde_json::from_value(v).unwrap()
    }

    fn base_track() -> Track {
        sc_track(serde_json::json!({
            "id": 1,
            "title": "Flickermood",
            "user": {"id": 2, "username": "Forss"},
            "duration": 213000,
            "full_duration": 213000,
            "genre": "Electronic",
            "created_at": "2008-03-14T12:00:00Z",
            "artwork_url": "https://i1.sndcdn.com/artworks-1-large.jpg",
        }))
    }

    fn fixture() -> Vec<Recording> {
        let s: SearchResponse =
            serde_json::from_str(include_str!("../tests/fixtures/mb/search_flickermood.json"))
                .unwrap();
        s.recordings
    }

    #[test]
    fn from_soundcloud_basic() {
        let m = TrackMeta::from_soundcloud(&base_track(), None);
        assert_eq!(m.title, "Flickermood");
        assert_eq!(m.artist, "Forss");
        assert_eq!(m.genre.as_deref(), Some("Electronic"));
        assert_eq!(m.date.as_deref(), Some("2008-03-14"));
        assert_eq!(m.album, None);
        assert_eq!(
            m.cover_url.as_deref(),
            Some("https://i1.sndcdn.com/artworks-1-t500x500.jpg")
        );
    }

    #[test]
    fn from_soundcloud_publisher_metadata_and_set() {
        let t = sc_track(serde_json::json!({
            "id": 1, "title": "Song", "user": {"id": 2, "username": "Some Label"},
            "release_date": "2024-05-01T00:00:00Z", "label_name": "Some Label",
            "publisher_metadata": {
                "artist": "Real Artist", "album_title": "The EP", "isrc": "US-AB1-24-00001",
                "writer_composer": "Writer", "p_line_for_display": "℗ 2024 Some Label",
                "release_title": ""
            }
        }));
        let m = TrackMeta::from_soundcloud(&t, None);
        assert_eq!(m.artist, "Real Artist");
        assert_eq!(m.album.as_deref(), Some("The EP"));
        assert_eq!(m.isrc.as_deref(), Some("USAB12400001"));
        assert_eq!(m.date.as_deref(), Some("2024-05-01"));
        assert_eq!(m.label.as_deref(), Some("Some Label"));
        assert_eq!(m.composer.as_deref(), Some("Writer"));
        assert_eq!(m.copyright.as_deref(), Some("℗ 2024 Some Label"));

        let set = SetPosition {
            title: "My Set",
            number: 3,
            total: 12,
        };
        let m = TrackMeta::from_soundcloud(&t, Some(set));
        assert_eq!(m.album.as_deref(), Some("My Set"));
        assert_eq!((m.track_no, m.track_total), (Some(3), Some(12)));
    }

    #[test]
    fn title_cleanup() {
        assert_eq!(clean_title("Song [FREE DL]"), "Song");
        assert_eq!(
            clean_title("Premiere: Artist - Song (Original Mix)"),
            "Artist - Song"
        );
        assert_eq!(
            clean_title("Song (Someone Remix) | OUT NOW"),
            "Song (Someone Remix)"
        );
        assert_eq!(clean_title("Song **FREE DOWNLOAD**"), "Song");
        assert_eq!(clean_title("Song free download"), "Song");
        assert_eq!(clean_title("Buyers Club"), "Buyers Club");
        assert_eq!(
            split_artist_title("Artist A & B – Song"),
            Some(("Artist A & B".into(), "Song".into()))
        );
        assert_eq!(split_artist_title("Song-Title"), None);
    }

    #[test]
    fn normalization_and_artists() {
        assert_eq!(norm("Song (feat. Someone)"), "song");
        assert_eq!(norm("Hello, World!"), "hello world");
        let credit = vec![
            Credit {
                name: "A".into(),
                joinphrase: " feat. ".into(),
            },
            Credit {
                name: "B".into(),
                joinphrase: String::new(),
            },
        ];
        assert!(artists_match("A", &credit));
        assert!(artists_match("A & B", &credit));
        assert!(artists_match("a x b", &credit));
        assert!(!artists_match("C", &credit));
    }

    #[test]
    fn text_queries_order() {
        let t = sc_track(serde_json::json!({
            "id": 1, "title": "Real Artist - Song [FREE DL]",
            "user": {"id": 2, "username": "Label Channel"},
            "publisher_metadata": {"artist": "Real Artist"}
        }));
        let m = TrackMeta::from_soundcloud(&t, None);
        let q = text_queries(&m, &t);
        assert_eq!(q[0], ("Real Artist".into(), "Song".into()));
        assert_eq!(q.len(), 2);
        assert_eq!(q[1], ("Label Channel".into(), "Real Artist - Song".into()));
    }

    #[test]
    fn picks_recording_and_release_from_fixture() {
        let recs = fixture();
        let rec = pick_recording(&recs, Some(("Forss", "Flickermood")), 213_000).unwrap();
        assert_eq!(rec.id, "120baa6e-1b7c-4509-9e33-a69d6b852769");
        // Compilation / soundtrack by other artists are not chosen; dated album wins.
        let rel = pick_release(rec).unwrap();
        assert_eq!(rel.title, "Soulhack");
        assert_eq!(rel.date.as_deref(), Some("2003-06-02"));

        // Too far off in length, wrong title or wrong artist -> no match.
        assert!(pick_recording(&recs, Some(("Forss", "Flickermood")), 240_000).is_none());
        assert!(pick_recording(&recs, Some(("Forss", "Flickermood (Remix)")), 213_000).is_none());
        assert!(pick_recording(&recs, Some(("Someone Else", "Flickermood")), 213_000).is_none());
        // ISRC mode only checks the length.
        assert!(pick_recording(&recs, None, 213_000).is_some());
    }

    #[test]
    fn apply_match_single_and_set() {
        let recs = fixture();
        let rec = pick_recording(&recs, Some(("Forss", "Flickermood")), 213_000).unwrap();
        let sc = TrackMeta::from_soundcloud(&base_track(), None);

        let m = apply_match(sc.clone(), rec, false);
        assert_eq!(m.album.as_deref(), Some("Soulhack"));
        assert_eq!(m.album_artist.as_deref(), Some("Forss"));
        assert_eq!((m.track_no, m.track_total), (Some(4), Some(11)));
        assert_eq!(m.date.as_deref(), Some("2003-06-02"));
        assert_eq!(m.genre.as_deref(), Some("Electronic"));
        assert!(m.mb_recording_id.is_some() && m.mb_release_group_id.is_some());

        let set = SetPosition {
            title: "My Set",
            number: 2,
            total: 5,
        };
        let sc_set = TrackMeta::from_soundcloud(&base_track(), Some(set));
        let m = apply_match(sc_set, rec, true);
        assert_eq!(m.album.as_deref(), Some("My Set"));
        assert_eq!((m.track_no, m.track_total), (Some(2), Some(5)));
        assert_eq!(m.date.as_deref(), Some("2003-06-02"));
    }

    #[test]
    fn ffmpeg_tags_per_container() {
        let mut m = TrackMeta::from_soundcloud(&base_track(), None);
        m.track_no = Some(4);
        m.track_total = Some(11);
        m.isrc = Some("GBAYE0601498".into());
        m.mb_recording_id = Some("rec".into());
        let has = |tags: &[(String, String)], k: &str| tags.iter().any(|(key, _)| key == k);

        let mp3 = m.ffmpeg_tags("mp3");
        assert!(mp3.contains(&("track".into(), "4/11".into())));
        assert!(has(&mp3, "TSRC") && has(&mp3, "MusicBrainz Track Id"));
        assert!(!has(&mp3, "album")); // empty values are skipped

        let opus = m.ffmpeg_tags("opus");
        assert!(has(&opus, "ISRC") && has(&opus, "MUSICBRAINZ_TRACKID"));

        let m4a = m.ffmpeg_tags("m4a");
        assert!(has(&m4a, "genre") && !has(&m4a, "ISRC") && !has(&m4a, "TSRC"));
    }

    #[test]
    fn lucene_escaping() {
        assert_eq!(lucene_phrase(r#"Say "Hi" \o/"#), r#""Say \"Hi\" \\o/""#);
    }

    /// Hits the real MusicBrainz + Cover Art Archive: `cargo test -- --ignored enrich_live`.
    #[tokio::test]
    #[ignore]
    async fn enrich_live() {
        let mb = MusicBrainz::new(None).unwrap();
        let track = sc_track(serde_json::json!({
            "id": 1, "title": "Forss - Flickermood [FREE DL]",
            "user": {"id": 2, "username": "some channel"},
            "duration": 213500, "full_duration": 213500,
            "artwork_url": "https://i1.sndcdn.com/artworks-1-large.jpg",
        }));
        let sc = TrackMeta::from_soundcloud(&track, None);
        let (m, matched) = mb.enrich(sc, &track, false).await;
        assert!(matched, "{m:?}");
        assert_eq!(
            (m.artist.as_str(), m.title.as_str()),
            ("Forss", "Flickermood")
        );
        assert_eq!(m.album.as_deref(), Some("Soulhack"));
        assert_eq!(m.date.as_deref(), Some("2003-06-02"));
        assert!(
            m.cover_url
                .as_deref()
                .is_some_and(|u| !u.contains("sndcdn")),
            "{m:?}"
        );

        // An unknown SoundCloud original must stay untouched.
        let track = sc_track(serde_json::json!({
            "id": 2, "title": "zz untitled bedroom sketch 47",
            "user": {"id": 3, "username": "nobody_qwxz"}, "duration": 61000,
        }));
        let sc = TrackMeta::from_soundcloud(&track, None);
        let (m, matched) = mb.enrich(sc.clone(), &track, false).await;
        assert!(!matched);
        assert_eq!(m, sc);
    }
}
