//! UI state and pure update logic. No terminal I/O happens here, so everything
//! is testable by feeding keys and messages and inspecting the state / effects.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::format_popup::FormatPopup;
use super::input::Input;
use crate::types::{Config, HistoryEntry, Job, JobId, JobStatus, QueueEvent, UiAction, UiMsg};

/// Number of ticks (~100 ms each) a transient status message stays visible.
const STATUS_TTL_TICKS: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Queue,
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Input,
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub text: String,
    pub kind: StatusKind,
    expires_at: usize,
}

/// Side effects requested by the state machine; executed by the event loop.
#[derive(Debug)]
pub enum Effect {
    Action(UiAction),
    /// Open this directory in the file manager.
    Open(PathBuf),
}

pub struct App {
    pub cfg: Config,
    pub jobs: Vec<Job>,
    pub history: Vec<HistoryEntry>,
    pub tab: Tab,
    pub focus: Focus,
    pub input: Input,
    pub queue_sel: usize,
    pub history_sel: usize,
    pub popup: Option<FormatPopup>,
    /// Resolved items waiting for the popup while another one is open.
    pub pending_popups: VecDeque<crate::types::ResolvedItem>,
    pub status: Option<Status>,
    /// Number of resolve requests in flight.
    pub resolving: usize,
    pub tick: usize,
    pub should_quit: bool,
}

impl App {
    pub fn new(cfg: Config, history: Vec<HistoryEntry>) -> Self {
        Self {
            cfg,
            jobs: Vec::new(),
            history,
            tab: Tab::Queue,
            focus: Focus::Input,
            input: Input::default(),
            queue_sel: 0,
            history_sel: 0,
            popup: None,
            pending_popups: VecDeque::new(),
            status: None,
            resolving: 0,
            tick: 0,
            should_quit: false,
        }
    }

    // ---- status -----------------------------------------------------------

    pub fn set_status(&mut self, kind: StatusKind, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            kind,
            expires_at: self.tick + STATUS_TTL_TICKS,
        });
    }

    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if self
            .status
            .as_ref()
            .is_some_and(|s| self.tick >= s.expires_at)
        {
            self.status = None;
        }
    }

    // ---- stats ------------------------------------------------------------

    /// (running, queued, done, failed)
    pub fn stats(&self) -> (usize, usize, usize, usize) {
        let mut s = (0, 0, 0, 0);
        for j in &self.jobs {
            match j.status {
                JobStatus::Downloading(_) => s.0 += 1,
                JobStatus::Queued => s.1 += 1,
                JobStatus::Done(_) => s.2 += 1,
                JobStatus::Failed(_) => s.3 += 1,
            }
        }
        s
    }

    // ---- events -----------------------------------------------------------

    pub fn handle_event(&mut self, ev: Event) -> Option<Effect> {
        match ev {
            Event::Key(k) => self.handle_key(k),
            Event::Paste(s) => {
                if self.popup.is_none() {
                    self.focus = Focus::Input;
                    self.input.insert_str(&s);
                }
                None
            }
            _ => None,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Effect> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        if ctrl && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return None;
        }
        if self.popup.is_some() {
            return self.popup_key(key);
        }

        // Global keys, independent of focus.
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Input => Focus::List,
                    Focus::List => Focus::Input,
                };
                return None;
            }
            KeyCode::F(1) => {
                self.tab = Tab::Queue;
                return None;
            }
            KeyCode::F(2) => {
                self.tab = Tab::History;
                return None;
            }
            KeyCode::Char('1') if alt => {
                self.tab = Tab::Queue;
                return None;
            }
            KeyCode::Char('2') if alt => {
                self.tab = Tab::History;
                return None;
            }
            KeyCode::Up => {
                self.move_sel(-1);
                return None;
            }
            KeyCode::Down => {
                self.move_sel(1);
                return None;
            }
            _ => {}
        }

        match self.focus {
            Focus::Input => self.input_key(key, ctrl, alt),
            Focus::List => self.list_key(key, ctrl || alt),
        }
    }

    fn input_key(&mut self, key: KeyEvent, ctrl: bool, alt: bool) -> Option<Effect> {
        match key.code {
            KeyCode::Enter => {
                let url = self.input.take();
                if url.is_empty() {
                    return None;
                }
                self.resolving += 1;
                self.status = None;
                return Some(Effect::Action(UiAction::Resolve(url)));
            }
            KeyCode::Esc => {
                if self.input.is_empty() {
                    self.should_quit = true;
                } else {
                    self.input.clear();
                }
            }
            KeyCode::Char('u') if ctrl => self.input.clear(),
            KeyCode::Char('w') if ctrl => self.input.delete_word(),
            KeyCode::Char('a') if ctrl => self.input.home(),
            KeyCode::Char('e') if ctrl => self.input.end(),
            KeyCode::Char(c) if !ctrl && !alt => self.input.insert(c),
            KeyCode::Backspace if ctrl || alt => self.input.delete_word(),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            _ => {}
        }
        None
    }

    fn list_key(&mut self, key: KeyEvent, modified: bool) -> Option<Effect> {
        if modified {
            return None;
        }
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc | KeyCode::Char('i') | KeyCode::Char('/') => self.focus = Focus::Input,
            KeyCode::Char('1') => self.tab = Tab::Queue,
            KeyCode::Char('2') => self.tab = Tab::History,
            KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Home | KeyCode::Char('g') => self.move_sel(isize::MIN),
            KeyCode::End | KeyCode::Char('G') => self.move_sel(isize::MAX),
            KeyCode::PageUp => self.move_sel(-10),
            KeyCode::PageDown => self.move_sel(10),
            KeyCode::Char('d') | KeyCode::Delete if self.tab == Tab::Queue => {
                return self
                    .selected_job()
                    .map(|j| Effect::Action(UiAction::Remove(j.id)));
            }
            KeyCode::Char('r') if self.tab == Tab::Queue => {
                let job = self.selected_job()?;
                if matches!(job.status, JobStatus::Failed(_)) {
                    return Some(Effect::Action(UiAction::Retry(job.id)));
                }
                self.set_status(StatusKind::Info, "Only failed jobs can be retried");
            }
            KeyCode::Char('o') => return self.selected_dir().map(Effect::Open),
            _ => {}
        }
        None
    }

    fn popup_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let popup = self.popup.as_mut()?;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => popup.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => popup.move_sel(1),
            KeyCode::Home => popup.move_sel(isize::MIN),
            KeyCode::End => popup.move_sel(isize::MAX),
            KeyCode::Enter => {
                let popup = self.popup.take()?;
                let format = popup.selected_choice();
                let n = popup.item.tracks.len();
                let skipped = popup.item.skipped.len();
                self.next_popup();
                let mut msg = format!("Added {n} track{}", if n == 1 { "" } else { "s" });
                if skipped > 0 {
                    msg.push_str(&format!(" ({skipped} skipped)"));
                }
                self.set_status(StatusKind::Success, msg);
                self.tab = Tab::Queue;
                return Some(Effect::Action(UiAction::Enqueue {
                    item: popup.item,
                    format,
                }));
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.popup = None;
                self.next_popup();
                self.set_status(StatusKind::Info, "Cancelled");
            }
            _ => {}
        }
        None
    }

    fn next_popup(&mut self) {
        if self.popup.is_none()
            && let Some(item) = self.pending_popups.pop_front()
        {
            self.popup = Some(FormatPopup::new(item, self.cfg.default_format));
        }
    }

    // ---- selection --------------------------------------------------------

    fn list_len(&self) -> usize {
        match self.tab {
            Tab::Queue => self.jobs.len(),
            Tab::History => self.history.len(),
        }
    }

    fn move_sel(&mut self, delta: isize) {
        let len = self.list_len();
        let sel = match self.tab {
            Tab::Queue => &mut self.queue_sel,
            Tab::History => &mut self.history_sel,
        };
        if len == 0 {
            *sel = 0;
            return;
        }
        *sel = sel.saturating_add_signed(delta).min(len - 1);
    }

    fn clamp_selection(&mut self) {
        self.queue_sel = self.queue_sel.min(self.jobs.len().saturating_sub(1));
        self.history_sel = self.history_sel.min(self.history.len().saturating_sub(1));
    }

    pub fn selected_job(&self) -> Option<&Job> {
        self.jobs.get(self.queue_sel)
    }

    pub fn selected_entry(&self) -> Option<&HistoryEntry> {
        self.history.get(self.history_sel)
    }

    /// Directory to open for the current selection.
    pub fn selected_dir(&self) -> Option<PathBuf> {
        match self.tab {
            Tab::Queue => {
                let job = self.selected_job()?;
                let file = match &job.status {
                    JobStatus::Done(p) => p.as_path(),
                    _ => job.out_stem.as_path(),
                };
                Some(parent_or(file, &self.cfg.output_dir))
            }
            Tab::History => {
                let e = self.selected_entry()?;
                Some(parent_or(&e.path, &self.cfg.output_dir))
            }
        }
    }

    // ---- messages ---------------------------------------------------------

    pub fn handle_msg(&mut self, msg: UiMsg) {
        match msg {
            UiMsg::Resolved(res) => {
                self.resolving = self.resolving.saturating_sub(1);
                match res {
                    Ok(item) => {
                        if item.tracks.is_empty() {
                            self.set_status(
                                StatusKind::Error,
                                format!(
                                    "Nothing downloadable in \"{}\" ({} skipped)",
                                    item.title,
                                    item.skipped.len()
                                ),
                            );
                        } else if self.popup.is_some() {
                            self.pending_popups.push_back(item);
                        } else {
                            self.popup = Some(FormatPopup::new(item, self.cfg.default_format));
                        }
                    }
                    Err(e) => self.set_status(StatusKind::Error, e),
                }
            }
            UiMsg::Queue(ev) => self.handle_queue_event(ev),
        }
    }

    fn job_mut(&mut self, id: JobId) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    pub fn handle_queue_event(&mut self, ev: QueueEvent) {
        match ev {
            QueueEvent::Added(mut job) => {
                job.status = JobStatus::Queued;
                match self.job_mut(job.id) {
                    Some(existing) => *existing = job,
                    None => self.jobs.push(job),
                }
            }
            QueueEvent::Started(id) => {
                if let Some(j) = self.job_mut(id) {
                    j.status = JobStatus::Downloading(0.0);
                }
            }
            QueueEvent::Progress(id, p) => {
                if let Some(j) = self.job_mut(id) {
                    j.status = JobStatus::Downloading(p.clamp(0.0, 1.0));
                }
            }
            QueueEvent::Done(id, entry) => {
                if let Some(j) = self.job_mut(id) {
                    j.status = JobStatus::Done(entry.path.clone());
                }
                self.history.insert(0, entry);
                // Keep the same history entry selected unless we're at the top.
                if self.history_sel > 0 {
                    self.history_sel += 1;
                }
            }
            QueueEvent::Failed(id, err) => {
                if let Some(j) = self.job_mut(id) {
                    j.status = JobStatus::Failed(err);
                }
            }
            QueueEvent::Removed(id) => {
                if let Some(pos) = self.jobs.iter().position(|j| j.id == id) {
                    self.jobs.remove(pos);
                    if pos < self.queue_sel {
                        self.queue_sel -= 1;
                    }
                }
            }
        }
        self.clamp_selection();
    }
}

fn parent_or(file: &Path, fallback: &Path) -> PathBuf {
    file.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| fallback.to_path_buf())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::sc::model::Track;
    use crate::types::{FormatChoice, FormatOption, ResolvedItem};
    use chrono::Utc;

    pub fn cfg() -> Config {
        Config {
            output_dir: PathBuf::from("/music"),
            default_format: FormatChoice::Opus,
            concurrency: 2,
            ffmpeg_path: "ffmpeg".into(),
            metadata_lookup: false,
            musicbrainz_contact: None,
        }
    }

    pub fn track(id: u64, title: &str, artist: &str) -> Track {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "title": title,
            "user": { "id": 1, "username": artist },
        }))
        .unwrap()
    }

    pub fn job(id: JobId, title: &str) -> Job {
        Job {
            id,
            track: track(id, title, "Artist"),
            format: FormatChoice::Best,
            out_stem: PathBuf::from(format!("/music/Set/{id:02} - {title}")),
            album: Some("Set".into()),
            track_no: Some(id as u32),
            track_total: Some(10),
            source_url: "https://soundcloud.com/a/sets/s".into(),
            status: JobStatus::Queued,
        }
    }

    pub fn entry(title: &str) -> HistoryEntry {
        HistoryEntry {
            time: Utc::now(),
            title: title.into(),
            artist: "Artist".into(),
            path: PathBuf::from(format!("/music/x/Artist - {title}.m4a")),
            source_url: "https://soundcloud.com/a/t".into(),
            format: "aac".into(),
        }
    }

    pub fn item(n: usize, skipped: usize) -> ResolvedItem {
        ResolvedItem {
            source_url: "https://soundcloud.com/a/sets/s".into(),
            title: "My Set".into(),
            artist: "Artist".into(),
            set_title: Some("My Set".into()),
            tracks: (0..n as u64)
                .map(|i| track(i, &format!("T{i}"), "Artist"))
                .collect(),
            skipped: (0..skipped)
                .map(|i| (format!("S{i}"), "blocked".into()))
                .collect(),
            format_options: vec![
                FormatOption {
                    choice: FormatChoice::Best,
                    label: "Best (AAC 160 kbps) -> .m4a".into(),
                },
                FormatOption {
                    choice: FormatChoice::Aac,
                    label: "AAC 160 kbps -> .m4a".into(),
                },
                FormatOption {
                    choice: FormatChoice::Opus,
                    label: "Opus 64 kbps -> .opus".into(),
                },
                FormatOption {
                    choice: FormatChoice::ConvertMp3,
                    label: "Convert to MP3 -> .mp3".into(),
                },
            ],
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn press(app: &mut App, code: KeyCode) -> Option<Effect> {
        app.handle_key(key(code))
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            assert!(press(app, KeyCode::Char(c)).is_none());
        }
    }

    pub fn app_with_jobs(n: u64) -> App {
        let mut app = App::new(cfg(), vec![]);
        for id in 1..=n {
            app.handle_msg(UiMsg::Queue(QueueEvent::Added(job(
                id,
                &format!("Song {id}"),
            ))));
        }
        app
    }

    #[test]
    fn typing_and_enter_emits_resolve() {
        let mut app = App::new(cfg(), vec![]);
        type_str(&mut app, "q12 https://sc/x");
        assert!(!app.should_quit, "q while typing must not quit");
        assert_eq!(app.tab, Tab::Queue, "2 while typing must not switch tabs");
        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Delete);
        press(&mut app, KeyCode::Delete);
        press(&mut app, KeyCode::Delete);
        match press(&mut app, KeyCode::Enter) {
            Some(Effect::Action(UiAction::Resolve(u))) => assert_eq!(u, "https://sc/x"),
            other => panic!("unexpected {other:?}"),
        }
        assert!(app.input.is_empty());
        assert_eq!(app.resolving, 1);
        // Empty enter does nothing.
        assert!(press(&mut app, KeyCode::Enter).is_none());
        assert_eq!(app.resolving, 1);
    }

    #[test]
    fn ctrl_u_clears_and_paste_inserts() {
        let mut app = App::new(cfg(), vec![]);
        type_str(&mut app, "abc");
        app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(app.input.is_empty());
        app.focus = Focus::List;
        app.handle_event(Event::Paste("https://soundcloud.com/x\n".into()));
        assert_eq!(app.input.value(), "https://soundcloud.com/x");
        assert_eq!(app.focus, Focus::Input);
    }

    #[test]
    fn quit_keys() {
        let mut app = App::new(cfg(), vec![]);
        type_str(&mut app, "x");
        press(&mut app, KeyCode::Esc);
        assert!(!app.should_quit && app.input.is_empty());
        press(&mut app, KeyCode::Esc);
        assert!(app.should_quit);

        let mut app = App::new(cfg(), vec![]);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.should_quit);

        let mut app = App::new(cfg(), vec![]);
        type_str(&mut app, "abc");
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }

    #[test]
    fn tab_focus_and_tab_switching() {
        let mut app = App::new(cfg(), vec![]);
        assert_eq!(app.focus, Focus::Input);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::List);
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.tab, Tab::History);
        press(&mut app, KeyCode::Char('1'));
        assert_eq!(app.tab, Tab::Queue);
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.focus, Focus::Input);
        app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::ALT));
        assert_eq!(app.tab, Tab::History);
        assert!(app.input.is_empty());
        press(&mut app, KeyCode::F(1));
        assert_eq!(app.tab, Tab::Queue);
    }

    #[test]
    fn remove_and_retry_use_selected_id() {
        let mut app = app_with_jobs(3);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down); // clamps at the end
        assert_eq!(app.queue_sel, 2);
        match press(&mut app, KeyCode::Char('d')) {
            Some(Effect::Action(UiAction::Remove(3))) => {}
            other => panic!("unexpected {other:?}"),
        }
        // Retry only on failed jobs.
        assert!(press(&mut app, KeyCode::Char('r')).is_none());
        assert!(app.status.is_some());
        app.handle_msg(UiMsg::Queue(QueueEvent::Failed(3, "boom".into())));
        match press(&mut app, KeyCode::Char('r')) {
            Some(Effect::Action(UiAction::Retry(3))) => {}
            other => panic!("unexpected {other:?}"),
        }
        press(&mut app, KeyCode::Char('k'));
        match press(&mut app, KeyCode::Char('d')) {
            Some(Effect::Action(UiAction::Remove(2))) => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn list_keys_on_empty_lists_are_noops() {
        let mut app = App::new(cfg(), vec![]);
        press(&mut app, KeyCode::Tab);
        assert!(press(&mut app, KeyCode::Char('d')).is_none());
        assert!(press(&mut app, KeyCode::Char('r')).is_none());
        assert!(press(&mut app, KeyCode::Char('o')).is_none());
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.queue_sel, 0);
    }

    #[test]
    fn open_folder_paths() {
        let mut app = app_with_jobs(1);
        app.handle_msg(UiMsg::Queue(QueueEvent::Done(1, entry("Song 1"))));
        press(&mut app, KeyCode::Tab);
        match press(&mut app, KeyCode::Char('o')) {
            Some(Effect::Open(p)) => assert_eq!(p, PathBuf::from("/music/x")),
            other => panic!("unexpected {other:?}"),
        }
        press(&mut app, KeyCode::Char('2'));
        match press(&mut app, KeyCode::Char('o')) {
            Some(Effect::Open(p)) => assert_eq!(p, PathBuf::from("/music/x")),
            other => panic!("unexpected {other:?}"),
        }
        let mut app = app_with_jobs(1);
        press(&mut app, KeyCode::Tab);
        match press(&mut app, KeyCode::Char('o')) {
            Some(Effect::Open(p)) => assert_eq!(p, PathBuf::from("/music/Set")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn resolved_opens_popup_with_default_preselected() {
        let mut app = App::new(cfg(), vec![]);
        app.resolving = 1;
        app.handle_msg(UiMsg::Resolved(Ok(item(3, 1))));
        assert_eq!(app.resolving, 0);
        let p = app.popup.as_ref().expect("popup open");
        assert_eq!(p.selected_choice(), FormatChoice::Opus);

        // Keys go to the popup, not the input.
        press(&mut app, KeyCode::Char('x'));
        assert!(app.input.is_empty());
        press(&mut app, KeyCode::Down);
        match press(&mut app, KeyCode::Enter) {
            Some(Effect::Action(UiAction::Enqueue { item, format })) => {
                assert_eq!(format, FormatChoice::ConvertMp3);
                assert_eq!(item.tracks.len(), 3);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(app.popup.is_none());
        assert_eq!(
            app.status.as_ref().unwrap().text,
            "Added 3 tracks (1 skipped)"
        );
    }

    #[test]
    fn popup_falls_back_to_first_and_esc_cancels() {
        let mut cfg = cfg();
        cfg.default_format = FormatChoice::Mp3; // not offered
        let mut app = App::new(cfg, vec![]);
        app.handle_msg(UiMsg::Resolved(Ok(item(1, 0))));
        assert_eq!(
            app.popup.as_ref().unwrap().selected_choice(),
            FormatChoice::Best
        );
        press(&mut app, KeyCode::Up); // clamps
        assert_eq!(
            app.popup.as_ref().unwrap().selected_choice(),
            FormatChoice::Best
        );
        assert!(press(&mut app, KeyCode::Esc).is_none());
        assert!(app.popup.is_none());
        assert!(!app.should_quit);
    }

    #[test]
    fn second_resolve_waits_for_first_popup() {
        let mut app = App::new(cfg(), vec![]);
        app.handle_msg(UiMsg::Resolved(Ok(item(1, 0))));
        app.handle_msg(UiMsg::Resolved(Ok(item(2, 0))));
        assert_eq!(app.pending_popups.len(), 1);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.popup.as_ref().unwrap().item.tracks.len(), 2);
    }

    #[test]
    fn resolve_error_and_empty_item_set_error_status() {
        let mut app = App::new(cfg(), vec![]);
        app.handle_msg(UiMsg::Resolved(Err("not found".into())));
        let s = app.status.as_ref().unwrap();
        assert_eq!((s.kind, s.text.as_str()), (StatusKind::Error, "not found"));
        app.handle_msg(UiMsg::Resolved(Ok(item(0, 2))));
        assert!(app.popup.is_none());
        assert_eq!(app.status.as_ref().unwrap().kind, StatusKind::Error);
    }

    #[test]
    fn status_expires() {
        let mut app = App::new(cfg(), vec![]);
        app.set_status(StatusKind::Info, "hi");
        for _ in 0..STATUS_TTL_TICKS {
            app.on_tick();
        }
        assert!(app.status.is_none());
    }

    #[test]
    fn queue_event_lifecycle() {
        let mut app = app_with_jobs(2);
        assert_eq!(app.jobs.len(), 2);
        assert_eq!(app.stats(), (0, 2, 0, 0));

        app.handle_msg(UiMsg::Queue(QueueEvent::Started(1)));
        assert_eq!(app.jobs[0].status, JobStatus::Downloading(0.0));
        app.handle_msg(UiMsg::Queue(QueueEvent::Progress(1, 0.5)));
        assert_eq!(app.jobs[0].status, JobStatus::Downloading(0.5));
        app.handle_msg(UiMsg::Queue(QueueEvent::Progress(1, 7.0)));
        assert_eq!(app.jobs[0].status, JobStatus::Downloading(1.0));

        app.handle_msg(UiMsg::Queue(QueueEvent::Done(1, entry("Song 1"))));
        assert_eq!(
            app.jobs[0].status,
            JobStatus::Done(PathBuf::from("/music/x/Artist - Song 1.m4a"))
        );
        assert_eq!(app.history.len(), 1);

        app.handle_msg(UiMsg::Queue(QueueEvent::Failed(2, "403".into())));
        assert_eq!(app.jobs[1].status, JobStatus::Failed("403".into()));
        assert_eq!(app.stats(), (0, 0, 1, 1));

        // Retry: Added with an existing id resets it to Queued in place.
        let mut again = job(2, "Song 2");
        again.status = JobStatus::Failed("stale".into());
        app.handle_msg(UiMsg::Queue(QueueEvent::Added(again)));
        assert_eq!(app.jobs.len(), 2);
        assert_eq!(app.jobs[1].status, JobStatus::Queued);

        // Unknown ids are ignored.
        app.handle_msg(UiMsg::Queue(QueueEvent::Progress(99, 0.3)));
        app.handle_msg(UiMsg::Queue(QueueEvent::Removed(99)));
        assert_eq!(app.jobs.len(), 2);
    }

    #[test]
    fn removed_fixes_selection() {
        let mut app = app_with_jobs(3);
        app.queue_sel = 2;
        app.handle_msg(UiMsg::Queue(QueueEvent::Removed(1)));
        assert_eq!(app.queue_sel, 1);
        assert_eq!(app.selected_job().unwrap().id, 3);
        app.handle_msg(UiMsg::Queue(QueueEvent::Removed(3)));
        assert_eq!(app.queue_sel, 0);
        assert_eq!(app.selected_job().unwrap().id, 2);
        app.handle_msg(UiMsg::Queue(QueueEvent::Removed(2)));
        assert_eq!(app.queue_sel, 0);
        assert!(app.selected_job().is_none());
    }

    #[test]
    fn done_keeps_history_selection_stable() {
        let mut app = App::new(cfg(), vec![entry("a"), entry("b")]);
        app.history_sel = 1;
        app.handle_msg(UiMsg::Queue(QueueEvent::Done(42, entry("c"))));
        assert_eq!(app.history[0].title, "c");
        assert_eq!(app.selected_entry().unwrap().title, "b");
    }
}
