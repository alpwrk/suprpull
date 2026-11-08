//! Download queue: limits concurrency, runs jobs, writes history, emits events.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use futures::future::BoxFuture;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::{AbortHandle, JoinHandle};

use crate::download::{self, DownloadSpec};
use crate::history::History;
use crate::metadata::{MusicBrainz, SetPosition, TrackMeta};
use crate::sc::api::ScClient;
use crate::types::{
    Config, FormatChoice, HistoryEntry, Job, JobId, JobStatus, QueueCmd, QueueEvent, ResolvedItem,
};

/// Max bytes of a single file/dir name before the extension is appended.
const MAX_NAME_BYTES: usize = 200;
/// Minimum progress change that triggers a `Progress` event.
const PROGRESS_STEP: f32 = 0.005;

/// Make one path component safe: no separators, no illegal characters, bounded length.
fn safe_name(raw: &str) -> String {
    let mapped: String = raw
        .chars()
        .map(|c| if c == '/' || c == '\\' { '-' } else { c })
        .collect();
    let opts = sanitize_filename::Options {
        windows: true,
        truncate: true,
        replacement: "",
    };
    let s = sanitize_filename::sanitize_with_options(mapped, opts);
    let mut s = s.trim().trim_end_matches('.').trim().to_string();
    if s.len() > MAX_NAME_BYTES {
        let mut cut = MAX_NAME_BYTES;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s = s.trim_end().to_string();
    }
    if s.is_empty() { "_".to_string() } else { s }
}

/// Turn a resolved item into jobs with output paths:
/// single: `<out>/<Artist> - <Title>`; set: `<out>/<Set>/<NN> - <Artist> - <Title>` (sanitized).
pub fn jobs_from_resolved(
    item: &ResolvedItem,
    format: FormatChoice,
    cfg: &Config,
    next_id: &mut JobId,
) -> Vec<Job> {
    let width = item.tracks.len().to_string().len().max(2);
    item.tracks
        .iter()
        .enumerate()
        .map(|(i, track)| {
            let base = format!(
                "{} - {}",
                track.artist_or_default(),
                track.title_or_default()
            );
            let (out_stem, album, track_no) = match &item.set_title {
                Some(set) => {
                    let name = format!("{:0width$} - {base}", i + 1);
                    (
                        cfg.output_dir.join(safe_name(set)).join(safe_name(&name)),
                        Some(set.clone()),
                        Some(i as u32 + 1),
                    )
                }
                None => (cfg.output_dir.join(safe_name(&base)), None, None),
            };
            let id = *next_id;
            *next_id += 1;
            Job {
                id,
                track: track.clone(),
                format,
                out_stem,
                track_total: track_no.map(|_| item.tracks.len() as u32),
                album,
                track_no,
                source_url: track
                    .permalink_url
                    .clone()
                    .unwrap_or_else(|| item.source_url.clone()),
                status: JobStatus::Queued,
            }
        })
        .collect()
}

/// Progress callback handed to a runner.
type ProgressFn = Arc<dyn Fn(f32) + Send + Sync>;
/// Executes one job to completion. Abstracted so the scheduler can be tested without network.
type Runner =
    Arc<dyn Fn(Job, ProgressFn) -> BoxFuture<'static, Result<HistoryEntry>> + Send + Sync>;

/// Spawn the queue task. Emits `Added` for every job received via `QueueCmd::Add`.
pub fn spawn(
    cfg: Config,
    sc: Arc<ScClient>,
    history: History,
    cmd_rx: UnboundedReceiver<QueueCmd>,
    ev_tx: UnboundedSender<QueueEvent>,
) -> JoinHandle<()> {
    let ffmpeg: Arc<str> = cfg.ffmpeg_path.clone().into();
    let history = Arc::new(history);
    // A broken MusicBrainz client only disables the lookup, never the download.
    let mb: Option<Arc<MusicBrainz>> = cfg
        .metadata_lookup
        .then(|| MusicBrainz::new(cfg.musicbrainz_contact.as_deref()).ok())
        .flatten()
        .map(Arc::new);
    let runner: Runner = Arc::new(move |job, progress| {
        let sc = sc.clone();
        let ffmpeg = ffmpeg.clone();
        let history = history.clone();
        let mb = mb.clone();
        Box::pin(async move { run_job(&sc, mb.as_deref(), &ffmpeg, &history, job, progress).await })
    });
    spawn_with_runner(cfg.concurrency, runner, cmd_rx, ev_tx)
}

/// The real job: build metadata (SoundCloud + MusicBrainz), fetch the stream URL,
/// run ffmpeg, append to history.
async fn run_job(
    sc: &ScClient,
    mb: Option<&MusicBrainz>,
    ffmpeg: &str,
    history: &History,
    job: Job,
    progress: ProgressFn,
) -> Result<HistoryEntry> {
    let set = match (&job.album, job.track_no, job.track_total) {
        (Some(title), Some(number), Some(total)) => Some(SetPosition {
            title,
            number,
            total,
        }),
        _ => None,
    };
    let mut meta = TrackMeta::from_soundcloud(&job.track, set);
    if let Some(mb) = mb {
        // Done before requesting the stream URL, which is signed and short-lived.
        meta = mb.enrich(meta, &job.track, set.is_some()).await.0;
    }

    let stream = sc.stream_url(&job.track, job.format).await?;
    let convert_mp3 = job.format == FormatChoice::ConvertMp3;
    let ext = download::output_extension(stream.codec, convert_mp3);
    let spec = DownloadSpec {
        input_url: stream.url,
        codec: stream.codec,
        convert_mp3,
        cover_url: meta.cover_url.clone(),
        tags: meta.ffmpeg_tags(ext),
        duration_ms: job.track.duration,
        out_stem: job.out_stem.clone(),
    };
    let path = download::run(&spec, ffmpeg, move |p| progress(p)).await?;
    let entry = HistoryEntry {
        time: chrono::Utc::now(),
        title: meta.title,
        artist: meta.artist,
        path,
        source_url: job.source_url,
        format: if ext == "m4a" { "aac" } else { ext }.to_string(),
    };
    // A failing history write must not turn a finished download into a failure.
    let _ = history.append(&entry);
    Ok(entry)
}

struct Scheduler {
    concurrency: usize,
    runner: Runner,
    ev_tx: UnboundedSender<QueueEvent>,
    done_tx: UnboundedSender<Internal>,
    jobs: HashMap<JobId, Job>,
    pending: VecDeque<JobId>,
    running: HashMap<JobId, AbortHandle>,
    /// Job tasks that have not ended yet. Aborted tasks keep their slot until their
    /// future (and with it ffmpeg) is dropped, so downloads never exceed `concurrency`.
    live: usize,
}

/// Messages from job tasks back into the queue loop.
enum Internal {
    Finished(JobId, Result<HistoryEntry, String>),
    /// Sent when a job task ends, whether finished or aborted.
    TaskEnded,
}

impl Scheduler {
    fn emit(&self, ev: QueueEvent) {
        let _ = self.ev_tx.send(ev);
    }

    fn handle_cmd(&mut self, cmd: QueueCmd) {
        match cmd {
            QueueCmd::Add(jobs) => {
                for mut job in jobs {
                    if self.jobs.contains_key(&job.id) {
                        continue;
                    }
                    job.status = JobStatus::Queued;
                    self.emit(QueueEvent::Added(job.clone()));
                    self.pending.push_back(job.id);
                    self.jobs.insert(job.id, job);
                }
            }
            QueueCmd::Remove(id) => {
                if self.jobs.remove(&id).is_none() {
                    return;
                }
                if let Some(h) = self.running.remove(&id) {
                    // Dropping the future kills ffmpeg (kill_on_drop).
                    h.abort();
                }
                self.pending.retain(|p| *p != id);
                self.emit(QueueEvent::Removed(id));
            }
            QueueCmd::Retry(id) => {
                if let Some(job) = self.jobs.get_mut(&id)
                    && matches!(job.status, JobStatus::Failed(_))
                {
                    job.status = JobStatus::Queued;
                    // Re-announce so the UI resets the row to "queued" right away.
                    let job = job.clone();
                    self.emit(QueueEvent::Added(job));
                    self.pending.push_back(id);
                }
            }
        }
    }

    fn handle_done(&mut self, id: JobId, res: Result<HistoryEntry, String>) {
        // Ignore completions of jobs that were removed meanwhile.
        if self.running.remove(&id).is_none() {
            return;
        }
        let Some(job) = self.jobs.get_mut(&id) else {
            return;
        };
        match res {
            Ok(entry) => {
                job.status = JobStatus::Done(entry.path.clone());
                self.emit(QueueEvent::Done(id, entry));
            }
            Err(e) => {
                job.status = JobStatus::Failed(e.clone());
                self.emit(QueueEvent::Failed(id, e));
            }
        }
    }

    fn schedule(&mut self) {
        while self.live < self.concurrency {
            let Some(id) = self.pending.pop_front() else {
                break;
            };
            let Some(job) = self.jobs.get_mut(&id) else {
                continue;
            };
            job.status = JobStatus::Downloading(0.0);
            let job = job.clone();
            self.emit(QueueEvent::Started(id));

            let ev_tx = self.ev_tx.clone();
            let last = Mutex::new(0.0f32);
            let progress: ProgressFn = Arc::new(move |p: f32| {
                let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
                if (p - *last).abs() >= PROGRESS_STEP || (p >= 1.0 && *last < 1.0) {
                    *last = p;
                    let _ = ev_tx.send(QueueEvent::Progress(id, p));
                }
            });
            let fut = (self.runner)(job, progress);
            let done_tx = self.done_tx.clone();
            let handle = tokio::spawn(async move {
                // Frees the slot once the job future (and with it ffmpeg) is gone.
                struct EndNotice(UnboundedSender<Internal>);
                impl Drop for EndNotice {
                    fn drop(&mut self) {
                        let _ = self.0.send(Internal::TaskEnded);
                    }
                }
                let _notice = EndNotice(done_tx.clone());
                let res = fut.await.map_err(|e| format!("{e:#}"));
                let _ = done_tx.send(Internal::Finished(id, res));
            });
            self.live += 1;
            self.running.insert(id, handle.abort_handle());
        }
    }
}

fn spawn_with_runner(
    concurrency: usize,
    runner: Runner,
    mut cmd_rx: UnboundedReceiver<QueueCmd>,
    ev_tx: UnboundedSender<QueueEvent>,
) -> JoinHandle<()> {
    let (done_tx, mut done_rx) = mpsc::unbounded_channel();
    let mut s = Scheduler {
        concurrency: concurrency.max(1),
        runner,
        ev_tx,
        done_tx,
        jobs: HashMap::new(),
        pending: VecDeque::new(),
        running: HashMap::new(),
        live: 0,
    };
    tokio::spawn(async move {
        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => match cmd {
                    Some(cmd) => s.handle_cmd(cmd),
                    None => break,
                },
                Some(msg) = done_rx.recv() => match msg {
                    Internal::Finished(id, res) => s.handle_done(id, res),
                    Internal::TaskEnded => s.live = s.live.saturating_sub(1),
                },
            }
            s.schedule();
        }
        for (_, h) in s.running.drain() {
            h.abort();
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sc::model::{Track, User};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::oneshot;

    fn track(id: u64, title: &str, artist: &str) -> Track {
        Track {
            id,
            title: Some(title.into()),
            user: Some(User {
                id: 1,
                username: artist.into(),
                avatar_url: None,
                permalink_url: None,
            }),
            duration: Some(1000),
            full_duration: Some(1000),
            policy: Some("ALLOW".into()),
            artwork_url: None,
            permalink_url: Some(format!("https://soundcloud.com/a/{id}")),
            track_authorization: None,
            media: None,
            genre: None,
            release_date: None,
            created_at: None,
            label_name: None,
            publisher_metadata: None,
        }
    }

    fn item(tracks: Vec<Track>, set: Option<&str>) -> ResolvedItem {
        ResolvedItem {
            source_url: "https://soundcloud.com/a/sets/s".into(),
            title: set.unwrap_or("t").into(),
            artist: "a".into(),
            set_title: set.map(Into::into),
            tracks,
            skipped: vec![],
            format_options: vec![],
        }
    }

    fn cfg() -> Config {
        Config {
            output_dir: PathBuf::from("/out"),
            default_format: FormatChoice::Best,
            concurrency: 2,
            ffmpeg_path: "ffmpeg".into(),
            metadata_lookup: false,
            musicbrainz_contact: None,
        }
    }

    #[test]
    fn single_track_naming() {
        let mut next = 7;
        let mut t = track(1, "Song: A/B", "AC/DC");
        t.permalink_url = None;
        let jobs = jobs_from_resolved(&item(vec![t], None), FormatChoice::Opus, &cfg(), &mut next);
        assert_eq!(next, 8);
        assert_eq!(jobs.len(), 1);
        let j = &jobs[0];
        assert_eq!(j.id, 7);
        assert_eq!(j.out_stem, PathBuf::from("/out/AC-DC - Song A-B"));
        assert_eq!(j.album, None);
        assert_eq!(j.track_no, None);
        assert_eq!(j.format, FormatChoice::Opus);
        assert_eq!(j.status, JobStatus::Queued);
        assert_eq!(j.source_url, "https://soundcloud.com/a/sets/s");
    }

    #[test]
    fn set_naming_and_padding() {
        let mut next = 0;
        let tracks: Vec<_> = (0..3).map(|i| track(i, &format!("T{i}"), "Ar")).collect();
        let jobs = jobs_from_resolved(
            &item(tracks, Some("My/Set")),
            FormatChoice::Best,
            &cfg(),
            &mut next,
        );
        assert_eq!(next, 3);
        assert_eq!(jobs[0].out_stem, PathBuf::from("/out/My-Set/01 - Ar - T0"));
        assert_eq!(jobs[2].out_stem, PathBuf::from("/out/My-Set/03 - Ar - T2"));
        assert_eq!(jobs[2].track_no, Some(3));
        assert_eq!(jobs[2].album.as_deref(), Some("My/Set"));
        assert_eq!(jobs[1].source_url, "https://soundcloud.com/a/1");

        let tracks: Vec<_> = (0..120).map(|i| track(i, "T", "Ar")).collect();
        let jobs = jobs_from_resolved(
            &item(tracks, Some("S")),
            FormatChoice::Best,
            &cfg(),
            &mut next,
        );
        assert_eq!(jobs[0].out_stem, PathBuf::from("/out/S/001 - Ar - T"));
        assert_eq!(jobs[119].out_stem, PathBuf::from("/out/S/120 - Ar - T"));
        assert_eq!(jobs[119].id, 122);
    }

    #[test]
    fn safe_name_edge_cases() {
        assert_eq!(safe_name(""), "_");
        assert_eq!(safe_name("..."), "_");
        assert_eq!(safe_name(" a?b* "), "ab");
        let long = "é".repeat(300);
        let s = safe_name(&long);
        assert!(s.len() <= MAX_NAME_BYTES);
        assert!(s.chars().all(|c| c == 'é'));
    }

    type Gates = Arc<Mutex<HashMap<JobId, oneshot::Sender<Result<(), String>>>>>;

    struct Fake {
        gates: Gates,
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
    }

    struct ActiveGuard(Arc<AtomicUsize>);
    impl Drop for ActiveGuard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    fn fake_runner() -> (Runner, Fake) {
        let f = Fake {
            gates: Arc::default(),
            active: Arc::default(),
            max_active: Arc::default(),
        };
        let (gates, active, max_active) = (f.gates.clone(), f.active.clone(), f.max_active.clone());
        let runner: Runner = Arc::new(move |job: Job, progress: ProgressFn| {
            let (tx, rx) = oneshot::channel();
            gates.lock().unwrap().insert(job.id, tx);
            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
            max_active.fetch_max(now, Ordering::SeqCst);
            let guard = ActiveGuard(active.clone());
            Box::pin(async move {
                let _guard = guard;
                progress(0.5);
                progress(0.501); // throttled
                match rx.await {
                    Ok(Ok(())) => Ok(HistoryEntry {
                        time: chrono::Utc::now(),
                        title: job.track.title_or_default().into(),
                        artist: job.track.artist_or_default().into(),
                        path: PathBuf::from(format!("/out/{}.mp3", job.id)),
                        source_url: job.source_url,
                        format: "mp3".into(),
                    }),
                    Ok(Err(e)) => Err(anyhow::anyhow!(e)),
                    Err(_) => Err(anyhow::anyhow!("gate dropped")),
                }
            })
        });
        (runner, f)
    }

    fn finish(f: &Fake, id: JobId, res: Result<(), String>) {
        let tx = f
            .gates
            .lock()
            .unwrap()
            .remove(&id)
            .expect("job not running");
        tx.send(res).unwrap();
    }

    /// Next event, skipping progress events.
    async fn next(rx: &mut UnboundedReceiver<QueueEvent>) -> QueueEvent {
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .expect("timeout waiting for event")
                .expect("channel closed");
            if !matches!(ev, QueueEvent::Progress(..)) {
                return ev;
            }
        }
    }

    async fn assert_quiet(rx: &mut UnboundedReceiver<QueueEvent>) {
        tokio::time::sleep(Duration::from_millis(50)).await;
        while let Ok(ev) = rx.try_recv() {
            assert!(
                matches!(ev, QueueEvent::Progress(..)),
                "unexpected event {ev:?}"
            );
        }
    }

    fn started(ev: QueueEvent) -> JobId {
        match ev {
            QueueEvent::Started(id) => id,
            other => panic!("expected Started, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn scheduling_concurrency_remove_retry() {
        let (runner, fake) = fake_runner();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
        let handle = spawn_with_runner(2, runner, cmd_rx, ev_tx);

        let mut next_id = 1;
        let tracks: Vec<_> = (0..4).map(|i| track(i, "T", "A")).collect();
        let jobs = jobs_from_resolved(
            &item(tracks, Some("S")),
            FormatChoice::Best,
            &cfg(),
            &mut next_id,
        );
        cmd_tx.send(QueueCmd::Add(jobs)).unwrap();

        for id in 1..=4 {
            assert!(matches!(next(&mut ev_rx).await, QueueEvent::Added(j) if j.id == id));
        }
        assert_eq!(started(next(&mut ev_rx).await), 1);
        assert_eq!(started(next(&mut ev_rx).await), 2);
        assert_quiet(&mut ev_rx).await;

        // Job 1 succeeds -> job 3 starts.
        finish(&fake, 1, Ok(()));
        assert!(
            matches!(next(&mut ev_rx).await, QueueEvent::Done(1, e) if e.path == std::path::Path::new("/out/1.mp3"))
        );
        assert_eq!(started(next(&mut ev_rx).await), 3);

        // Removing a running job aborts it and frees its slot -> job 4 starts.
        cmd_tx.send(QueueCmd::Remove(3)).unwrap();
        assert!(matches!(next(&mut ev_rx).await, QueueEvent::Removed(3)));
        assert_eq!(started(next(&mut ev_rx).await), 4);

        // Removing a pending job drops it without starting.
        let mut extra = jobs_from_resolved(
            &item(vec![track(9, "X", "Y")], None),
            FormatChoice::Best,
            &cfg(),
            &mut next_id,
        );
        let extra_id = extra[0].id;
        cmd_tx
            .send(QueueCmd::Add(std::mem::take(&mut extra)))
            .unwrap();
        assert!(matches!(next(&mut ev_rx).await, QueueEvent::Added(j) if j.id == extra_id));
        cmd_tx.send(QueueCmd::Remove(extra_id)).unwrap();
        assert!(matches!(next(&mut ev_rx).await, QueueEvent::Removed(id) if id == extra_id));
        assert_quiet(&mut ev_rx).await;

        // Failure, then retry.
        finish(&fake, 2, Err("boom".into()));
        assert!(matches!(next(&mut ev_rx).await, QueueEvent::Failed(2, e) if e == "boom"));
        cmd_tx.send(QueueCmd::Retry(4)).unwrap(); // running -> ignored
        cmd_tx.send(QueueCmd::Retry(2)).unwrap();
        let mut prog2 = Vec::new();
        let mut next_non_progress = async || loop {
            match ev_rx.recv().await.unwrap() {
                QueueEvent::Progress(id, p) => prog2.push((id, p)),
                ev => break ev,
            }
        };
        // Retry re-announces the job (UI resets the row), then starts it.
        assert!(
            matches!(next_non_progress().await, QueueEvent::Added(j) if j.id == 2 && j.status == JobStatus::Queued)
        );
        assert_eq!(started(next_non_progress().await), 2);
        // Throttled progress: 0.5 is sent, 0.501 is dropped.
        tokio::time::sleep(Duration::from_millis(50)).await;
        while let Ok(QueueEvent::Progress(id, p)) = ev_rx.try_recv() {
            prog2.push((id, p));
        }
        assert_eq!(
            prog2.iter().filter(|(id, _)| *id == 2).count(),
            1,
            "{prog2:?}"
        );

        // Retrying a done job is ignored.
        cmd_tx.send(QueueCmd::Retry(1)).unwrap();
        assert_quiet(&mut ev_rx).await;

        finish(&fake, 2, Ok(()));
        finish(&fake, 4, Ok(()));
        let mut done = vec![];
        for _ in 0..2 {
            match next(&mut ev_rx).await {
                QueueEvent::Done(id, _) => done.push(id),
                ev => panic!("unexpected {ev:?}"),
            }
        }
        done.sort();
        assert_eq!(done, [2, 4]);

        assert!(fake.max_active.load(Ordering::SeqCst) <= 2);
        assert_eq!(
            fake.active.load(Ordering::SeqCst),
            0,
            "removed job was not aborted"
        );

        drop(cmd_tx);
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn closing_commands_aborts_running_jobs() {
        let (runner, fake) = fake_runner();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
        let handle = spawn_with_runner(1, runner, cmd_rx, ev_tx);
        let mut n = 0;
        let jobs = jobs_from_resolved(
            &item(vec![track(1, "a", "b")], None),
            FormatChoice::Best,
            &cfg(),
            &mut n,
        );
        cmd_tx.send(QueueCmd::Add(jobs)).unwrap();
        next(&mut ev_rx).await;
        assert_eq!(started(next(&mut ev_rx).await), 0);
        drop(cmd_tx);
        handle.await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(fake.active.load(Ordering::SeqCst), 0);
    }
}
