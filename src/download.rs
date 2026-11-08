//! Runs ffmpeg to fetch a stream (HLS or progressive) into a tagged audio file.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use regex::Regex;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

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
    /// `-metadata key=value` pairs, see `metadata::TrackMeta::ffmpeg_tags`.
    pub tags: Vec<(String, String)>,
    pub duration_ms: Option<u64>,
    /// Without extension.
    pub out_stem: PathBuf,
}

/// Number of trailing stderr lines kept for error messages.
const STDERR_TAIL: usize = 8;

/// "m4a" | "opus" | "mp3"
pub fn output_extension(codec: Codec, convert_mp3: bool) -> &'static str {
    if convert_mp3 {
        "mp3"
    } else {
        codec.extension()
    }
}

/// ffmpeg muxer name for an output extension (needed because the `.part` suffix hides it).
fn muxer_for(ext: &str) -> &'static str {
    match ext {
        "m4a" => "ipod",
        "opus" => "opus",
        _ => "mp3",
    }
}

fn is_network(input: &str) -> bool {
    let l = input.to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// Full ffmpeg argument list (without the program name) writing to `out_path`.
pub fn build_ffmpeg_args(spec: &DownloadSpec, out_path: &Path) -> Vec<String> {
    let ext = output_extension(spec.codec, spec.convert_mp3);
    let cover = spec.cover_url.as_deref().filter(|_| ext != "opus");

    let mut a: Vec<String> = Vec::with_capacity(48);
    let mut push = |items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));

    push(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y"]);
    if is_network(&spec.input_url) {
        push(&[
            "-reconnect",
            "1",
            "-reconnect_streamed",
            "1",
            "-reconnect_delay_max",
            "5",
        ]);
    }
    push(&["-i", &spec.input_url]);
    if let Some(c) = cover {
        push(&["-i", c]);
    }
    push(&["-map", "0:a:0"]);
    if cover.is_some() {
        push(&[
            "-map",
            "1:v:0",
            // Re-encode instead of copy: the containers reject WebP/AVIF artwork
            // ("codec not currently supported") and copied images may lack
            // dimensions ("dimensions not set"). Decoding always yields a valid JPEG.
            "-c:v",
            "mjpeg",
            "-q:v",
            "2",
            "-disposition:v:0",
            "attached_pic",
        ]);
        if ext == "mp3" {
            push(&["-id3v2_version", "3"]);
        }
    }
    if spec.convert_mp3 {
        push(&["-c:a", "libmp3lame", "-q:a", "0"]);
    } else {
        push(&["-c:a", "copy"]);
        if spec.codec == Codec::Aac {
            // HLS delivers ADTS; the mp4 muxer needs ASC. No-op for mp4 input.
            push(&["-bsf:a", "aac_adtstoasc"]);
        }
    }

    let meta = |key: &str, val: &str| ["-metadata".to_string(), format!("{key}={val}")];
    // Drop tags inherited from the source stream, then write ours.
    push(&["-map_metadata", "-1"]);
    for (key, val) in &spec.tags {
        a.extend(meta(key, val));
    }

    a.extend(
        ["-progress", "pipe:1", "-nostats", "-f", muxer_for(ext)]
            .iter()
            .map(|s| s.to_string()),
    );
    a.push(out_path.to_string_lossy().into_owned());
    a
}

/// Parse one line of `-progress pipe:1` output; returns progress 0..=1 for `out_time_us=`/`out_time_ms=` lines.
pub fn parse_progress_line(line: &str, duration_ms: Option<u64>) -> Option<f32> {
    let (key, val) = line.trim().split_once('=')?;
    let val = val.trim();
    match key.trim() {
        "progress" if val == "end" => Some(1.0),
        // ffmpeg's out_time_ms is (despite its name) also in microseconds.
        "out_time_us" | "out_time_ms" => {
            let us: i64 = val.parse().ok()?;
            let dur = duration_ms.filter(|d| *d > 0)?;
            let p = (us.max(0) as f64 / 1000.0) / dur as f64;
            Some(p.min(1.0) as f32)
        }
        _ => None,
    }
}

/// `<stem>.<ext>`, or `<stem> (N).<ext>` with the first N that does not exist yet.
fn unique_path(stem: &Path, ext: &str) -> PathBuf {
    let with_ext = |suffix: &str| {
        let mut s = stem.as_os_str().to_os_string();
        s.push(suffix);
        s.push(".");
        s.push(ext);
        PathBuf::from(s)
    };
    let first = with_ext("");
    if !first.exists() {
        return first;
    }
    (1..)
        .map(|n| with_ext(&format!(" ({n})")))
        .find(|p| !p.exists())
        .expect("infinite iterator")
}

fn part_path(out_path: &Path) -> PathBuf {
    let mut s = out_path.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

static LOG_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[[\w#:/ ]+ @ 0x[0-9a-f]+\]\s*").unwrap());

/// Readable error from ffmpeg's stderr: last lines, without `[ipod @ 0x...]` prefixes,
/// consecutive duplicates removed.
fn ffmpeg_error_message(stderr: &str) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for l in stderr.lines() {
        let l = LOG_PREFIX.replace_all(l.trim(), "").trim().to_string();
        if !l.is_empty() && lines.last() != Some(&l) {
            lines.push(l);
        }
    }
    let start = lines.len().saturating_sub(STDERR_TAIL);
    (!lines.is_empty()).then(|| lines[start..].join(" | "))
}

async fn run_ffmpeg(
    ffmpeg: &str,
    args: &[String],
    duration_ms: Option<u64>,
    progress: &(impl Fn(f32) + Send),
) -> Result<()> {
    let mut child = Command::new(ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("could not start ffmpeg ({ffmpeg})"))?;

    let stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");

    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        String::from_utf8_lossy(&buf).into_owned()
    });

    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(p) = parse_progress_line(&line, duration_ms) {
            progress(p);
        }
    }

    let status = child.wait().await.context("waiting for ffmpeg failed")?;
    let err_text = stderr_task.await.unwrap_or_default();
    if status.success() {
        return Ok(());
    }
    // Put ffmpeg's own message first: the UI only has room for one line.
    match ffmpeg_error_message(&err_text) {
        Some(msg) => bail!("{msg} (ffmpeg exited with {status})"),
        None => bail!("ffmpeg exited with {status}"),
    }
}

/// Run ffmpeg, report progress, return the final file path. Writes to a `.part` temp file first.
pub async fn run(
    spec: &DownloadSpec,
    ffmpeg: &str,
    progress: impl Fn(f32) + Send,
) -> Result<PathBuf> {
    let ext = output_extension(spec.codec, spec.convert_mp3);
    if let Some(parent) = spec.out_stem.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("could not create {}", parent.display()))?;
    }

    let tentative = unique_path(&spec.out_stem, ext);
    let part = part_path(&tentative);

    let mut spec_now = spec.clone();
    if ext == "opus" {
        spec_now.cover_url = None;
    }
    let result = loop {
        let args = build_ffmpeg_args(&spec_now, &part);
        match run_ffmpeg(ffmpeg, &args, spec_now.duration_ms, &progress).await {
            Ok(()) => break Ok(()),
            // Artwork is optional and the most fragile input (404, odd formats, missing
            // dimensions...), so any failure with a cover gets one retry without it.
            // If the audio itself is broken, the second attempt reports that.
            Err(_) if spec_now.cover_url.is_some() => spec_now.cover_url = None,
            Err(e) => break Err(e),
        }
    };
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }

    // Re-check: another job may have created the same name in the meantime.
    let final_path = unique_path(&spec.out_stem, ext);
    if let Err(e) = tokio::fs::rename(&part, &final_path).await {
        let _ = tokio::fs::remove_file(&part).await;
        bail!("could not move download to {}: {e}", final_path.display());
    }
    progress(1.0);
    Ok(final_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(codec: Codec, convert: bool, cover: bool) -> DownloadSpec {
        DownloadSpec {
            input_url: "https://cf-hls-media.sndcdn.com/playlist.m3u8".into(),
            codec,
            convert_mp3: convert,
            cover_url: cover.then(|| "https://i1.sndcdn.com/artworks-x-t500x500.jpg".into()),
            tags: vec![
                ("title".into(), "My / Title".into()),
                ("artist".into(), "Artist".into()),
                ("album".into(), "Set".into()),
                ("track".into(), "4".into()),
            ],
            duration_ms: Some(10_000),
            out_stem: PathBuf::from("/music/x"),
        }
    }

    fn has_seq(args: &[String], seq: &[&str]) -> bool {
        args.windows(seq.len())
            .any(|w| w.iter().zip(seq).all(|(a, b)| a == b))
    }

    #[test]
    fn extensions() {
        assert_eq!(output_extension(Codec::Aac, false), "m4a");
        assert_eq!(output_extension(Codec::Opus, false), "opus");
        assert_eq!(output_extension(Codec::Mp3, false), "mp3");
        assert_eq!(output_extension(Codec::Opus, true), "mp3");
        assert_eq!(output_extension(Codec::Aac, true), "mp3");
    }

    #[test]
    fn args_aac_with_cover() {
        let out = PathBuf::from("/music/x.m4a.part");
        let a = build_ffmpeg_args(&spec(Codec::Aac, false, true), &out);
        assert_eq!(
            &a[..5],
            ["-hide_banner", "-nostdin", "-loglevel", "error", "-y"]
        );
        assert!(has_seq(
            &a,
            &["-reconnect", "1", "-reconnect_streamed", "1"]
        ));
        assert!(has_seq(
            &a,
            &["-i", "https://i1.sndcdn.com/artworks-x-t500x500.jpg"]
        ));
        assert!(has_seq(
            &a,
            &[
                "-map", "0:a:0", "-map", "1:v:0", "-c:v", "mjpeg", "-q:v", "2"
            ]
        ));
        assert!(has_seq(&a, &["-disposition:v:0", "attached_pic"]));
        assert!(has_seq(&a, &["-c:a", "copy", "-bsf:a", "aac_adtstoasc"]));
        assert!(!a.contains(&"-id3v2_version".to_string()));
        assert!(has_seq(&a, &["-metadata", "title=My / Title"]));
        assert!(has_seq(&a, &["-metadata", "artist=Artist"]));
        assert!(has_seq(&a, &["-metadata", "album=Set"]));
        assert!(has_seq(&a, &["-metadata", "track=4"]));
        assert!(has_seq(&a, &["-map_metadata", "-1"]));
        assert!(has_seq(
            &a,
            &["-progress", "pipe:1", "-nostats", "-f", "ipod"]
        ));
        assert_eq!(a.last().unwrap(), "/music/x.m4a.part");
    }

    #[test]
    fn args_mp3_copy_with_cover() {
        let a = build_ffmpeg_args(&spec(Codec::Mp3, false, true), Path::new("o.mp3.part"));
        assert!(has_seq(&a, &["-id3v2_version", "3"]));
        assert!(has_seq(&a, &["-c:a", "copy"]));
        assert!(!a.contains(&"-bsf:a".to_string()));
        assert!(has_seq(&a, &["-f", "mp3"]));
    }

    #[test]
    fn args_convert() {
        let a = build_ffmpeg_args(&spec(Codec::Opus, true, true), Path::new("o.mp3.part"));
        assert!(has_seq(&a, &["-c:a", "libmp3lame", "-q:a", "0"]));
        assert!(has_seq(&a, &["-map", "1:v:0"]));
        assert!(has_seq(&a, &["-id3v2_version", "3"]));
        assert!(has_seq(&a, &["-f", "mp3"]));
        assert!(!a.contains(&"-bsf:a".to_string()));
    }

    #[test]
    fn args_opus_skips_cover() {
        let mut s = spec(Codec::Opus, false, true);
        s.tags.retain(|(k, _)| k != "album" && k != "track");
        s.input_url = "/tmp/local.opus".into();
        let a = build_ffmpeg_args(&s, Path::new("o.opus.part"));
        assert_eq!(a.iter().filter(|x| *x == "-i").count(), 1);
        assert!(!a.contains(&"1:v:0".to_string()));
        assert!(!a.contains(&"-reconnect".to_string()));
        assert!(
            !a.iter()
                .any(|x| x.starts_with("album=") || x.starts_with("track="))
        );
        assert!(has_seq(
            &a,
            &["-c:a", "copy", "-map_metadata", "-1", "-metadata"]
        ));
        assert!(has_seq(&a, &["-f", "opus"]));
    }

    #[test]
    fn progress_parsing() {
        let d = Some(10_000);
        assert_eq!(parse_progress_line("out_time_us=5000000", d), Some(0.5));
        assert_eq!(parse_progress_line("out_time_ms=2500000", d), Some(0.25));
        assert_eq!(parse_progress_line("out_time_us=99000000", d), Some(1.0));
        assert_eq!(parse_progress_line("out_time_us=N/A", d), None);
        assert_eq!(parse_progress_line("out_time_us=-5", d), Some(0.0));
        assert_eq!(parse_progress_line("out_time_us=5000000", None), None);
        assert_eq!(parse_progress_line("out_time=00:00:05.000000", d), None);
        assert_eq!(parse_progress_line("progress=continue", d), None);
        assert_eq!(parse_progress_line("progress=end", None), Some(1.0));
        assert_eq!(parse_progress_line("garbage", d), None);
    }

    #[test]
    fn unique_paths() {
        let dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/b/test-tmp/download/unique");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let stem = dir.join("A - B");
        assert_eq!(unique_path(&stem, "mp3"), dir.join("A - B.mp3"));
        std::fs::write(dir.join("A - B.mp3"), b"").unwrap();
        assert_eq!(unique_path(&stem, "mp3"), dir.join("A - B (1).mp3"));
        std::fs::write(dir.join("A - B (1).mp3"), b"").unwrap();
        assert_eq!(unique_path(&stem, "mp3"), dir.join("A - B (2).mp3"));
    }

    #[test]
    fn error_message_cleanup() {
        let stderr = "[ipod @ 0x55bbf7406e40] dimensions not set\n\
            [out#0/ipod @ 0x55bbf7406e41] Could not write header (incorrect codec parameters ?): Invalid argument\n\
            [out#0/ipod @ 0x55bbf7406e41] Could not write header (incorrect codec parameters ?): Invalid argument\n\n";
        assert_eq!(
            ffmpeg_error_message(stderr).unwrap(),
            "dimensions not set | Could not write header (incorrect codec parameters ?): Invalid argument"
        );
        assert_eq!(ffmpeg_error_message(" \n"), None);
    }

    /// End-to-end with real ffmpeg on locally generated input (no network).
    #[tokio::test]
    #[ignore = "needs ffmpeg"]
    async fn run_real_ffmpeg() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/b/test-tmp/download/run");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("in.m4a");
        let cover = dir.join("cover.jpg");
        let cover_webp = dir.join("cover.webp");
        for args in [
            vec![
                "-f",
                "lavfi",
                "-i",
                "sine=d=3",
                "-c:a",
                "aac",
                input.to_str().unwrap(),
            ],
            vec![
                "-f",
                "lavfi",
                "-i",
                "testsrc=s=64x64",
                "-frames:v",
                "1",
                cover.to_str().unwrap(),
            ],
            vec![
                "-f",
                "lavfi",
                "-i",
                "testsrc=s=64x64",
                "-frames:v",
                "1",
                "-c:v",
                "libwebp",
                cover_webp.to_str().unwrap(),
            ],
        ] {
            let st = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args(args)
                .status()
                .unwrap();
            assert!(st.success());
        }

        let mut s = DownloadSpec {
            input_url: input.to_string_lossy().into_owned(),
            codec: Codec::Aac,
            convert_mp3: false,
            cover_url: Some(cover.to_string_lossy().into_owned()),
            tags: vec![
                ("title".into(), "Title".into()),
                ("artist".into(), "Artist".into()),
                ("album".into(), "Album".into()),
                ("track".into(), "1/9".into()),
                ("date".into(), "2003-06-02".into()),
                ("genre".into(), "Electronic".into()),
            ],
            duration_ms: Some(3000),
            out_stem: dir.join("out/Artist - Title"),
        };
        let seen = std::sync::Mutex::new(Vec::new());
        let p = run(&s, "ffmpeg", |p| seen.lock().unwrap().push(p))
            .await
            .unwrap();
        assert_eq!(p, dir.join("out/Artist - Title.m4a"));
        assert!(p.exists());
        assert!(!part_path(&p).exists());
        assert_eq!(*seen.lock().unwrap().last().unwrap(), 1.0);

        // A WebP cover (rejected by the mp4 muxer as-is) is converted to JPEG and embedded.
        s.cover_url = Some(cover_webp.to_string_lossy().into_owned());
        s.out_stem = dir.join("out/webp");
        let pw = run(&s, "ffmpeg", |_| {}).await.unwrap();
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v", "-show_entries"])
            .args([
                "stream=codec_name:stream_disposition=attached_pic",
                "-of",
                "csv=p=0",
            ])
            .arg(&pw)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&probe.stdout).trim(), "mjpeg,1");
        s.cover_url = Some(cover.to_string_lossy().into_owned());
        s.out_stem = dir.join("out/Artist - Title");

        // Same name again -> " (1)"; broken cover -> retried without cover; convert to mp3.
        let p2 = run(&s, "ffmpeg", |_| {}).await.unwrap();
        assert_eq!(p2, dir.join("out/Artist - Title (1).m4a"));
        s.cover_url = Some(dir.join("missing.jpg").to_string_lossy().into_owned());
        s.convert_mp3 = true;
        let p3 = run(&s, "ffmpeg", |_| {}).await.unwrap();
        assert_eq!(p3, dir.join("out/Artist - Title.mp3"));

        // Broken input -> error with ffmpeg's message, no leftovers.
        s.input_url = dir.join("nope.m4a").to_string_lossy().into_owned();
        let err = run(&s, "ffmpeg", |_| {}).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("ffmpeg exited"), "{msg}");
        assert!(msg.contains("No such file"), "{msg}");
        assert!(!dir.join("out/Artist - Title (1).mp3.part").exists());
    }
}
