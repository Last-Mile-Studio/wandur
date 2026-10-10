//! View > Session history: a window of its own beside the main one (non-modal, so play goes
//! on), with the C# layout: search words and filters at the top, Sessions and Search results on
//! the left, the selected session's transcript on the right, Delete session with a confirmation.
//!
//! [`HistoryModel`] is the C# `HistoryViewModel`: bounded pages (50 sessions or results, 100
//! transcript entries) read on threads of their own, never on the UI thread; a newer request makes
//! an older answer stale, so a slow search cannot overwrite a newer one. Nothing here sends
//! anything to a MUD.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use egui::{Align, Color32, CornerRadius, Layout, RichText, Stroke, Ui};
use wandur_core::history::{HistoryEntry, HistoryFilter, HistoryHit, HistorySession, HistoryStore};
use wandur_core::l10n::{self, Language, S, t, tf};

use crate::dialogs::{primary_button, secondary_button};
use crate::theme::Theme;

/// Sessions or results per page.
pub const PAGE_SIZE: usize = 50;
/// Transcript entries per page.
pub const CONTEXT_PAGE: usize = 100;
/// The window's size and smallest size (the C# window).
const SIZE: egui::Vec2 = egui::vec2(1100.0, 800.0);
const MIN_SIZE: egui::Vec2 = egui::vec2(860.0, 660.0);

/// The two lists on the left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowseTab {
    Sessions,
    Results,
}

type Pages = (Vec<HistorySession>, Vec<HistoryHit>);

struct ContextPage {
    entries: Vec<HistoryEntry>,
    previous: bool,
    next: bool,
}

enum Reply {
    Pages {
        generation: u64,
        sessions_at: usize,
        results_at: usize,
        result: Result<Pages, ()>,
    },
    Context {
        generation: u64,
        result: Result<ContextPage, ()>,
    },
    Deleted(Result<(), ()>),
}

/// The window's state and its reads (the C# `HistoryViewModel`).
pub struct HistoryModel {
    store: Arc<dyn HistoryStore>,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// Recorders still writing when the window opened: the first read waits for them.
    flushes: Vec<Receiver<()>>,
    pub query: String,
    pub world: String,
    pub character: String,
    pub from: Option<SystemTime>,
    pub until: Option<SystemTime>,
    pub sessions: Vec<HistorySession>,
    pub results: Vec<HistoryHit>,
    pub transcript: Vec<HistoryEntry>,
    session_offset: usize,
    result_offset: usize,
    pub has_next_sessions: bool,
    pub has_next_results: bool,
    pub has_previous_context: bool,
    pub has_next_context: bool,
    pub busy: bool,
    pub context_busy: bool,
    pub deleting: bool,
    pub confirm_delete: bool,
    pub error: String,
    pub active_session: Option<HistorySession>,
    /// The search hit the transcript was opened around.
    pub anchor: Option<i64>,
    pending_delete: Option<HistorySession>,
    browse_generation: u64,
    context_generation: u64,
    disposed: bool,
    pub tab: BrowseTab,
    /// Frames left in which the transcript scrolls to the anchor (the first layout of a new
    /// page may not have its final height yet).
    scroll_to_anchor: u8,
    /// Where the anchor entry sits in the transcript's content, from the last frame.
    anchor_y: Option<f32>,
}

impl HistoryModel {
    pub fn new(store: Arc<dyn HistoryStore>, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, rx) = channel();
        Self {
            store,
            tx,
            rx,
            wake,
            flushes: Vec::new(),
            query: String::new(),
            world: String::new(),
            character: String::new(),
            from: None,
            until: None,
            sessions: Vec::new(),
            results: Vec::new(),
            transcript: Vec::new(),
            session_offset: 0,
            result_offset: 0,
            has_next_sessions: false,
            has_next_results: false,
            has_previous_context: false,
            has_next_context: false,
            busy: false,
            context_busy: false,
            deleting: false,
            confirm_delete: false,
            error: String::new(),
            active_session: None,
            anchor: None,
            pending_delete: None,
            browse_generation: 0,
            context_generation: 0,
            disposed: false,
            tab: BrowseTab::Sessions,
            scroll_to_anchor: 0,
            anchor_y: None,
        }
    }

    /// Wait for these recorder flushes before the first read.
    pub fn wait_for(&mut self, flushes: Vec<Receiver<()>>) {
        self.flushes.extend(flushes);
    }

    pub fn has_previous_sessions(&self) -> bool {
        self.session_offset > 0
    }

    pub fn has_previous_results(&self) -> bool {
        self.result_offset > 0
    }

    pub fn sessions_label(&self) -> String {
        tf(
            S::HistoryPage,
            &[&(self.session_offset / PAGE_SIZE + 1), &self.sessions.len()],
        )
    }

    pub fn results_label(&self) -> String {
        tf(
            S::HistoryPage,
            &[&(self.result_offset / PAGE_SIZE + 1), &self.results.len()],
        )
    }

    pub fn context_label(&self) -> String {
        match &self.active_session {
            Some(session) => session_label(session),
            None => t(S::HistorySelect).into(),
        }
    }

    pub fn delete_prompt(&self) -> String {
        let label = self.pending_delete.as_ref().map(session_label).unwrap_or_default();
        tf(S::HistoryDeletePrompt, &[&label])
    }

    pub fn can_delete(&self) -> bool {
        self.active_session.is_some() && !self.deleting && !self.context_busy
    }

    pub fn sessions_empty(&self) -> bool {
        self.sessions.is_empty() && !self.busy
    }

    pub fn results_empty(&self) -> bool {
        self.results.is_empty() && !self.busy
    }

    /// A filter or the query changed: what is shown no longer matches it.
    pub fn filters_changed(&mut self) {
        if self.disposed {
            return;
        }
        self.cancel_browse();
        self.session_offset = 0;
        self.result_offset = 0;
        self.sessions.clear();
        self.results.clear();
        self.has_next_sessions = false;
        self.has_next_results = false;
        self.clear_context();
        self.error.clear();
    }

    /// Search / refresh (and the window opening).
    pub fn refresh(&mut self) {
        self.load_pages(None, None);
    }

    pub fn next_sessions(&mut self) {
        if self.has_next_sessions && !self.busy {
            self.load_pages(Some(self.session_offset + PAGE_SIZE), Some(self.result_offset));
        }
    }

    pub fn previous_sessions(&mut self) {
        if self.has_previous_sessions() && !self.busy {
            self.load_pages(Some(self.session_offset - PAGE_SIZE), Some(self.result_offset));
        }
    }

    pub fn next_results(&mut self) {
        if self.has_next_results && !self.busy {
            self.load_pages(Some(self.session_offset), Some(self.result_offset + PAGE_SIZE));
        }
    }

    pub fn previous_results(&mut self) {
        if self.has_previous_results() && !self.busy {
            self.load_pages(Some(self.session_offset), Some(self.result_offset - PAGE_SIZE));
        }
    }

    fn load_pages(&mut self, sessions_at: Option<usize>, results_at: Option<usize>) {
        if self.disposed || self.deleting {
            return;
        }
        self.cancel_browse();
        if let (Some(from), Some(until)) = (self.from, self.until)
            && from >= until
        {
            self.error = t(S::HistoryInvalidDates).into();
            return;
        }
        let generation = self.browse_generation;
        let filter = HistoryFilter {
            world: non_empty(&self.world),
            character: non_empty(&self.character),
            from: self.from,
            until: self.until,
        };
        let query = self.query.clone();
        let sessions_at = sessions_at.unwrap_or(self.session_offset);
        let results_at = results_at.unwrap_or(self.result_offset);
        self.busy = true;
        self.error.clear();
        let store = Arc::clone(&self.store);
        let flushes = std::mem::take(&mut self.flushes);
        self.spawn(move || {
            // Writes still queued when the window opened go in first (a bounded wait).
            let deadline = Instant::now() + Duration::from_secs(3);
            for flush in flushes {
                let _ = flush.recv_timeout(deadline.saturating_duration_since(Instant::now()));
            }
            let result = (|| {
                let sessions = store.sessions(&filter, sessions_at, PAGE_SIZE + 1)?;
                let hits = if query.trim().is_empty() {
                    Vec::new()
                } else {
                    store.search(&query, &filter, results_at, PAGE_SIZE + 1)?
                };
                Ok::<_, wandur_core::history::HistoryError>((sessions, hits))
            })()
            .map_err(drop);
            Reply::Pages {
                generation,
                sessions_at,
                results_at,
                result,
            }
        });
    }

    pub fn open_session(&mut self, session: HistorySession) {
        self.open(session, 0, None);
    }

    pub fn open_hit(&mut self, hit: &HistoryHit) {
        let start = (hit.entry.sequence - (CONTEXT_PAGE / 2) as i64).max(0);
        self.open(hit.session.clone(), start, Some(hit.entry.sequence));
    }

    fn open(&mut self, session: HistorySession, start: i64, anchor: Option<i64>) {
        if self.disposed || self.deleting {
            return;
        }
        self.clear_context();
        self.active_session = Some(session.clone());
        self.anchor = anchor;
        self.load_context(session, start, false);
    }

    pub fn next_context(&mut self) {
        if let (Some(session), Some(last)) = (self.active_session.clone(), self.transcript.last())
            && self.has_next_context
            && !self.context_busy
        {
            let from = last.sequence + 1;
            self.load_context(session, from, false);
        }
    }

    pub fn previous_context(&mut self) {
        if let (Some(session), Some(first)) = (self.active_session.clone(), self.transcript.first())
            && self.has_previous_context
            && !self.context_busy
        {
            let before = first.sequence;
            self.load_context(session, before, true);
        }
    }

    fn load_context(&mut self, session: HistorySession, start: i64, previous: bool) {
        if self.disposed || self.deleting {
            return;
        }
        self.cancel_context();
        let generation = self.context_generation;
        self.context_busy = true;
        self.error.clear();
        let store = Arc::clone(&self.store);
        self.spawn(move || {
            let result = if previous {
                read_previous(store.as_ref(), &session.id, start)
            } else {
                read_next(store.as_ref(), &session.id, start)
            };
            Reply::Context {
                generation,
                result: result.map_err(drop),
            }
        });
    }

    pub fn request_delete(&mut self) {
        if self.disposed || !self.can_delete() {
            return;
        }
        self.pending_delete = self.active_session.clone();
        self.confirm_delete = true;
    }

    pub fn cancel_delete(&mut self) {
        self.pending_delete = None;
        self.confirm_delete = false;
    }

    pub fn delete(&mut self) {
        if self.disposed || self.deleting || !self.confirm_delete {
            return;
        }
        let Some(session) = self.pending_delete.clone() else {
            return;
        };
        self.cancel_browse();
        self.cancel_context();
        self.deleting = true;
        self.error.clear();
        let store = Arc::clone(&self.store);
        self.spawn(move || Reply::Deleted(store.delete(&session.id).map_err(drop)));
    }

    /// The window closed: answers still on their way are dropped.
    pub fn dispose(&mut self) {
        self.disposed = true;
        self.cancel_browse();
        self.cancel_context();
    }

    /// Take the answers that arrived. Returns whether anything changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(reply) = self.rx.try_recv() {
            if self.disposed {
                continue;
            }
            changed = true;
            match reply {
                Reply::Pages {
                    generation,
                    sessions_at,
                    results_at,
                    result,
                } => {
                    if generation != self.browse_generation {
                        continue;
                    }
                    self.busy = false;
                    match result {
                        Ok((mut sessions, mut hits)) => {
                            self.session_offset = sessions_at;
                            self.result_offset = results_at;
                            self.has_next_sessions = sessions.len() > PAGE_SIZE;
                            self.has_next_results = hits.len() > PAGE_SIZE;
                            sessions.truncate(PAGE_SIZE);
                            hits.truncate(PAGE_SIZE);
                            self.sessions = sessions;
                            self.results = hits;
                            if !self.query.trim().is_empty() {
                                self.tab = BrowseTab::Results;
                            }
                        }
                        Err(()) => self.error = t(S::HistoryLoadFailed).into(),
                    }
                }
                Reply::Context { generation, result } => {
                    if generation != self.context_generation {
                        continue;
                    }
                    self.context_busy = false;
                    match result {
                        Ok(page) => {
                            self.transcript = page.entries;
                            self.has_previous_context = page.previous;
                            self.has_next_context = page.next;
                            self.scroll_to_anchor = 3;
                            self.anchor_y = None;
                        }
                        Err(()) => self.error = t(S::HistoryLoadFailed).into(),
                    }
                }
                Reply::Deleted(result) => {
                    self.deleting = false;
                    match result {
                        Ok(()) => {
                            self.clear_context();
                            self.session_offset = 0;
                            self.result_offset = 0;
                            self.sessions.clear();
                            self.results.clear();
                            self.has_next_sessions = false;
                            self.has_next_results = false;
                            self.refresh();
                        }
                        Err(()) => self.error = t(S::HistoryDeleteFailed).into(),
                    }
                }
            }
        }
        changed
    }

    /// Whether a read or a delete is running.
    pub fn working(&self) -> bool {
        self.busy || self.context_busy || self.deleting
    }

    /// The transcript as the C# text box shows it: each entry's time and kind, then its text.
    pub fn transcript_text(&self) -> String {
        self.transcript.iter().map(entry_text).collect::<Vec<_>>().join("\n\n")
    }

    fn clear_context(&mut self) {
        self.cancel_context();
        self.cancel_delete();
        self.anchor = None;
        self.active_session = None;
        self.transcript.clear();
        self.has_previous_context = false;
        self.has_next_context = false;
    }

    fn cancel_browse(&mut self) {
        self.browse_generation += 1;
        self.busy = false;
    }

    fn cancel_context(&mut self) {
        self.context_generation += 1;
        self.context_busy = false;
    }

    fn spawn(&self, job: impl FnOnce() -> Reply + Send + 'static) {
        let tx = self.tx.clone();
        let wake = Arc::clone(&self.wake);
        let spawned = std::thread::Builder::new()
            .name("wandur-history-read".into())
            .spawn(move || {
                let _ = tx.send(job());
                wake();
            });
        if spawned.is_err() {
            // No thread: the read is reported as failed rather than run here.
            let _ = self.tx.send(Reply::Deleted(Err(())));
        }
    }
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// A page from `start` on: whether earlier entries exist, and whether more follow.
fn read_next(
    store: &dyn HistoryStore,
    session: &str,
    start: i64,
) -> Result<ContextPage, wandur_core::history::HistoryError> {
    let mut entries = store.entries(session, start, CONTEXT_PAGE + 1)?;
    let first = store.entries(session, 0, 1)?.into_iter().next();
    let previous = matches!((entries.first(), &first), (Some(e), Some(f)) if f.sequence < e.sequence);
    let next = entries.len() > CONTEXT_PAGE;
    entries.truncate(CONTEXT_PAGE);
    Ok(ContextPage {
        entries,
        previous,
        next,
    })
}

/// The page before `before`. The store reads forward only: start one page back, widen across
/// gaps in the sequence, keep the last page (the C# `ReadPrevious`; two reads when contiguous).
fn read_previous(
    store: &dyn HistoryStore,
    session: &str,
    before: i64,
) -> Result<ContextPage, wandur_core::history::HistoryError> {
    let mut span = CONTEXT_PAGE as i64;
    loop {
        let mut kept: std::collections::VecDeque<HistoryEntry> = std::collections::VecDeque::new();
        let beginning = (before - span).max(0);
        let mut cursor = beginning;
        let mut earlier = false;
        while cursor < before {
            let batch = store.entries(session, cursor, CONTEXT_PAGE)?;
            let Some(last) = batch.last().map(|e| e.sequence) else {
                break;
            };
            for entry in batch {
                if entry.sequence >= before {
                    break;
                }
                kept.push_back(entry);
                if kept.len() > CONTEXT_PAGE {
                    kept.pop_front();
                    earlier = true;
                }
            }
            if last >= before || last < cursor || last == i64::MAX {
                break;
            }
            cursor = last + 1;
        }
        if kept.len() == CONTEXT_PAGE || beginning == 0 {
            let first = store.entries(session, 0, 1)?.into_iter().next();
            earlier |= matches!((kept.front(), &first), (Some(k), Some(f)) if f.sequence < k.sequence);
            return Ok(ContextPage {
                entries: kept.into(),
                previous: earlier,
                next: true,
            });
        }
        span = span.saturating_mul(2);
    }
}

/// "World · Character · time" (the character left out when unknown).
pub fn session_label(session: &HistorySession) -> String {
    let when = general_time(session.started_at);
    if session.character_name.is_empty() {
        format!("{} · {when}", session.world_name)
    } else {
        format!("{} · {} · {when}", session.world_name, session.character_name)
    }
}

fn kind_label(kind: wandur_core::history::EntryKind) -> &'static str {
    use wandur_core::history::EntryKind;
    match kind {
        EntryKind::Sent => t(S::HistorySent),
        EntryKind::Script => t(S::HistoryScript),
        EntryKind::Private => t(S::HistoryPrivate),
        EntryKind::Received => t(S::HistoryReceived),
    }
}

fn entry_heading(entry: &HistoryEntry) -> String {
    format!("{}  {}", general_time(entry.at), kind_label(entry.kind))
}

fn entry_text(entry: &HistoryEntry) -> String {
    format!("{}\n{}", entry_heading(entry), entry.text)
}

// Local dates and times in the UI language's short form (the C# "g" format of each culture).

/// Day order and separator of the UI language's short date.
fn date_style(language: Language) -> (bool, char) {
    match language {
        Language::En => (true, '/'),
        Language::De => (false, '.'),
        _ => (false, '/'),
    }
}

/// The short date pattern, shown in an empty date field.
pub fn date_placeholder() -> String {
    // "Any date", not the raw pattern (C# UI review, item 12).
    t(S::HistoryAnyDate).to_string()
}

fn secs(at: SystemTime) -> i64 {
    match at.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

/// A date as typed or shown (local midnight of that day).
pub fn date_text(at: SystemTime) -> String {
    let (y, m, d, _, _) = local_fields(secs(at));
    match date_style(l10n::language()) {
        (true, sep) => format!("{m}{sep}{d}{sep}{y}"),
        (false, sep) => format!("{d:02}{sep}{m:02}{sep}{y}"),
    }
}

/// Local midnight of the day `at` falls on.
pub fn local_day(at: SystemTime) -> SystemTime {
    let (y, m, d, _, _) = local_fields(secs(at));
    let midnight = local_midnight(y, m, d).unwrap_or(secs(at));
    UNIX_EPOCH + Duration::from_secs(midnight.max(0) as u64)
}

/// A local date and time in the short form ("10/8/2026 7:39 PM", "08.10.2026 19:39").
pub fn general_time(at: SystemTime) -> String {
    let (y, m, d, h, min) = local_fields(secs(at));
    let language = l10n::language();
    let date = match date_style(language) {
        (true, sep) => format!("{m}{sep}{d}{sep}{y}"),
        (false, sep) => format!("{d:02}{sep}{m:02}{sep}{y}"),
    };
    match language {
        Language::En => {
            let half = if h < 12 { "AM" } else { "PM" };
            let h12 = if h % 12 == 0 { 12 } else { h % 12 };
            format!("{date} {h12}:{min:02} {half}")
        }
        Language::Es => format!("{date} {h}:{min:02}"),
        _ => format!("{date} {h:02}:{min:02}"),
    }
}

/// A typed date in the UI language's order, as local midnight; `None` when it is not a date.
pub fn parse_date(text: &str) -> Option<SystemTime> {
    let (month_first, _) = date_style(l10n::language());
    let parts: Vec<i64> = text
        .trim()
        .split(['/', '.', '-'])
        .map(|p| p.trim().parse::<i64>().ok())
        .collect::<Option<_>>()?;
    let [a, b, y] = parts[..] else { return None };
    let (m, d) = if month_first { (a, b) } else { (b, a) };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(1..=9999).contains(&y) {
        return None;
    }
    let days = wandur_core::directory::time::days_from_civil(y, m, d);
    // A day that does not exist (April 31) comes back as another date.
    if wandur_core::directory::time::civil_from_days(days) != (y, m, d) {
        return None;
    }
    let midnight = local_midnight(y, m, d)?;
    Some(if midnight >= 0 {
        UNIX_EPOCH + Duration::from_secs(midnight as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs(midnight.unsigned_abs())
    })
}

/// Year, month, day, hour and minute of a time in the local time zone.
#[cfg(unix)]
fn local_fields(secs: i64) -> (i64, i64, i64, i64, i64) {
    let t = secs as libc::time_t;
    // SAFETY: localtime_r writes into the zeroed struct we own and reads only `t`.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if !libc::localtime_r(&t, &mut tm).is_null() {
            return (
                i64::from(tm.tm_year) + 1900,
                i64::from(tm.tm_mon) + 1,
                i64::from(tm.tm_mday),
                i64::from(tm.tm_hour),
                i64::from(tm.tm_min),
            );
        }
    }
    utc_fields(secs)
}

#[cfg(not(unix))]
fn local_fields(secs: i64) -> (i64, i64, i64, i64, i64) {
    utc_fields(secs)
}

fn utc_fields(secs: i64) -> (i64, i64, i64, i64, i64) {
    let (y, m, d) = wandur_core::directory::time::civil_from_days(secs.div_euclid(86_400));
    let in_day = secs.rem_euclid(86_400);
    (y, m, d, in_day / 3600, in_day % 3600 / 60)
}

/// Local midnight of a date, in seconds since 1970.
#[cfg(unix)]
fn local_midnight(y: i64, m: i64, d: i64) -> Option<i64> {
    // SAFETY: mktime reads and normalizes the struct we own.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = (y - 1900) as libc::c_int;
        tm.tm_mon = (m - 1) as libc::c_int;
        tm.tm_mday = d as libc::c_int;
        tm.tm_isdst = -1;
        let t = libc::mktime(&mut tm);
        (t != -1).then_some(t as i64)
    }
}

#[cfg(not(unix))]
fn local_midnight(y: i64, m: i64, d: i64) -> Option<i64> {
    Some(wandur_core::directory::time::days_from_civil(y, m, d) * 86_400)
}

/// The window: the model, the typed dates and the month pickers.
pub struct HistoryWindow {
    pub model: HistoryModel,
    from_text: String,
    until_text: String,
    calendar: Option<(DateField, i64, i64)>,
    first_frame: bool,
    /// Open the first item of this list once it is read (scenes).
    pub open_first: Option<BrowseTab>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DateField {
    From,
    Until,
}

impl HistoryWindow {
    pub fn new(model: HistoryModel) -> Self {
        Self {
            model,
            from_text: String::new(),
            until_text: String::new(),
            calendar: None,
            first_frame: true,
            open_first: None,
        }
    }

    /// The pages are read and the first item asked for is open (scenes).
    pub fn ready(&self) -> bool {
        !self.first_frame && self.open_first.is_none() && !self.model.working() && !self.model.transcript.is_empty()
    }

    /// Set the filters as if typed (scenes and tests).
    pub fn set_filters(
        &mut self,
        query: &str,
        world: &str,
        character: &str,
        from: Option<SystemTime>,
        until: Option<SystemTime>,
    ) {
        self.model.query = query.into();
        self.model.world = world.into();
        self.model.character = character.into();
        self.from_text = from.map(date_text).unwrap_or_default();
        self.until_text = until.map(date_text).unwrap_or_default();
        self.model.from = from;
        self.model.until = until;
        self.model.filters_changed();
    }

    /// Draw the window; returns false once it was closed.
    pub fn show(&mut self, ctx: &egui::Context, theme: &Theme) -> bool {
        if self.first_frame {
            self.first_frame = false;
            self.model.refresh();
        }
        self.model.poll();
        if let Some(list) = self.open_first
            && !self.model.busy
        {
            self.open_first = None;
            match list {
                BrowseTab::Results => {
                    if let Some(hit) = self.model.results.first().cloned() {
                        self.model.open_hit(&hit);
                    }
                }
                BrowseTab::Sessions => {
                    if let Some(session) = self.model.sessions.first().cloned() {
                        self.model.open_session(session);
                    }
                }
            }
        }
        let mut open = true;
        let title = t(S::SessionHistory);
        if ctx.embed_viewports() {
            // No second native window here (headless captures): a window inside the main one,
            // which can be moved and resized as the native one can.
            let screen = ctx.content_rect();
            let room = (screen.size() - egui::vec2(16.0, 16.0)).max(egui::vec2(200.0, 200.0));
            let mut window_open = true;
            egui::Window::new(RichText::new(title).size(13.0).color(theme.text))
                .id(egui::Id::new("history-window"))
                .open(&mut window_open)
                .collapsible(false)
                .resizable(true)
                .default_size(SIZE.min(room))
                .min_size(MIN_SIZE.min(room))
                // Centred by its top left corner, so the corner under the pointer follows a resize.
                .default_pos(screen.center() - SIZE.min(room) / 2.0)
                .constrain(true)
                .frame(
                    egui::Frame::window(&ctx.global_style())
                        .fill(theme.panel)
                        .stroke(Stroke::new(1.0, theme.border))
                        .inner_margin(egui::Margin::ZERO),
                )
                .show(ctx, |ui| {
                    // The content fills the window less a 20 point margin (the frame's margin
                    // would also pad the title bar).
                    let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
                    let mut content = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(20.0)));
                    if self.body(&mut content, theme) {
                        open = false;
                    }
                });
            crate::dialog_window::name_title_bar(ctx, egui::Id::new("history-window"), title);
            if !window_open {
                open = false;
            }
        } else {
            let builder = egui::ViewportBuilder::default()
                .with_title(title)
                .with_inner_size(SIZE)
                .with_min_inner_size(MIN_SIZE);
            ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("session-history"), builder, |ui, _| {
                if ui.ctx().input(|i| i.viewport().close_requested()) {
                    open = false;
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(theme.panel).inner_margin(20))
                    .show(ui, |ui| {
                        if self.body(ui, theme) {
                            open = false;
                        }
                    });
            });
        }
        if !open {
            self.model.dispose();
        }
        open
    }

    /// The window's content; returns true when Escape asked to close it (embedded only).
    fn body(&mut self, ui: &mut Ui, theme: &Theme) -> bool {
        let busy_header = self.model.deleting;
        ui.spacing_mut().item_spacing.y = 10.0;
        ui.add_enabled_ui(!busy_header, |ui| self.header(ui, theme));
        ui.add_space(6.0);
        let status_height = if self.model.confirm_delete { 110.0 } else { 26.0 };
        let panes_height = (ui.available_height() - status_height).max(200.0);
        let width = ui.available_width();
        let left = ((width - 12.0) * 0.4).max(270.0);
        let right = (width - 12.0 - left).max(200.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.allocate_ui_with_layout(egui::vec2(left, panes_height), Layout::top_down(Align::Min), |ui| {
                ui.set_min_size(egui::vec2(left, panes_height));
                ui.add_enabled_ui(!self.model.deleting, |ui| self.browse(ui, theme));
            });
            ui.allocate_ui_with_layout(egui::vec2(right, panes_height), Layout::top_down(Align::Min), |ui| {
                ui.set_min_size(egui::vec2(right, panes_height));
                self.context(ui, theme);
            });
        });
        self.status(ui, theme);
        ui.input(|i| i.key_pressed(egui::Key::Escape)) && ui.ctx().embed_viewports()
    }

    fn header(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.label(
            RichText::new(t(S::SessionHistory))
                .size(24.0)
                .strong()
                .color(theme.text),
        );
        ui.label(RichText::new(t(S::HistoryLocalNotice)).size(12.0).color(theme.muted));
        ui.label(RichText::new(t(S::HistoryPrivacyHint)).size(12.0).color(theme.muted));
        let mut search = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let button_width = 136.0;
            let query = egui::TextEdit::singleline(&mut self.model.query)
                .hint_text(t(S::HistoryQuery))
                .min_size(egui::vec2(0.0, 36.0))
                .vertical_align(Align::Center)
                .desired_width(ui.available_width() - button_width - 10.0);
            let response = ui.add(query);
            if response.changed() {
                self.model.filters_changed();
            }
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                search = true;
            }
            if primary_button(ui, t(S::HistorySearch), theme).clicked() {
                search = true;
            }
        });
        let width = ui.available_width();
        let date_width = 170.0;
        let text_width = ((width - 2.0 * date_width - 30.0) / 2.0).max(120.0);
        let mut changed = false;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            changed |= text_field(ui, theme, t(S::HistoryWorld), &mut self.model.world, text_width);
            changed |= text_field(ui, theme, t(S::HistoryCharacter), &mut self.model.character, text_width);
            changed |= self.date_field(ui, theme, DateField::From, date_width);
            changed |= self.date_field(ui, theme, DateField::Until, date_width);
        });
        if changed {
            self.model.filters_changed();
        }
        if search {
            self.model.refresh();
        }
    }

    fn date_field(&mut self, ui: &mut Ui, theme: &Theme, field: DateField, width: f32) -> bool {
        let label = match field {
            DateField::From => t(S::HistoryFrom),
            DateField::Until => t(S::HistoryUntil),
        };
        let mut changed = false;
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.set_width(width);
            ui.label(RichText::new(label).size(13.0).color(theme.muted));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let text = match field {
                    DateField::From => &mut self.from_text,
                    DateField::Until => &mut self.until_text,
                };
                let edit = egui::TextEdit::singleline(text)
                    .hint_text(date_placeholder())
                    .min_size(egui::vec2(0.0, 36.0))
                    .vertical_align(Align::Center)
                    .desired_width(width - 40.0);
                if crate::a11y::named(ui.add(edit), label).changed() {
                    let value = parse_date(text);
                    match field {
                        DateField::From => self.model.from = value,
                        DateField::Until => self.model.until = value,
                    }
                    changed = true;
                }
                let button = ui.add(
                    egui::Button::new("")
                        .min_size(egui::vec2(36.0, 36.0))
                        .fill(theme.panel)
                        .stroke(Stroke::new(1.0, theme.border)),
                );
                paint_calendar(ui, button.rect.shrink(9.0), theme.text);
                crate::a11y::label(&button, label);
                if button.clicked() {
                    self.calendar = match self.calendar {
                        Some((open, _, _)) if open == field => None,
                        _ => {
                            let current = match field {
                                DateField::From => self.model.from,
                                DateField::Until => self.model.until,
                            }
                            .unwrap_or_else(SystemTime::now);
                            let (y, m, _, _, _) = local_fields(secs(current));
                            Some((field, y, m))
                        }
                    };
                }
                if let Some((open, y, m)) = self.calendar
                    && open == field
                    && let Some(picked) = month_popup(ui, &button, theme, y, m, &mut self.calendar)
                {
                    let text = date_text(picked);
                    match field {
                        DateField::From => {
                            self.from_text = text;
                            self.model.from = Some(picked);
                        }
                        DateField::Until => {
                            self.until_text = text;
                            self.model.until = Some(picked);
                        }
                    }
                    self.calendar = None;
                    changed = true;
                }
            });
        });
        changed
    }

    fn browse(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            ui.add_space(12.0);
            for (tab, label) in [
                (BrowseTab::Sessions, t(S::HistorySessions)),
                (BrowseTab::Results, t(S::HistoryResults)),
            ] {
                let selected = self.model.tab == tab;
                let color = if selected { theme.text } else { theme.muted };
                let response =
                    ui.add(egui::Label::new(RichText::new(label).size(14.0).color(color)).sense(egui::Sense::click()));
                if selected {
                    let r = response.rect;
                    ui.painter().line_segment(
                        [
                            egui::pos2(r.left(), r.bottom() + 6.0),
                            egui::pos2(r.right(), r.bottom() + 6.0),
                        ],
                        Stroke::new(2.0, theme.accent),
                    );
                }
                if response.clicked() {
                    self.model.tab = tab;
                }
            }
        });
        ui.add_space(4.0);
        let footer = 70.0;
        let list_height = (ui.available_height() - footer).max(80.0);
        let width = ui.available_width();
        egui::Frame::new()
            .fill(list_fill(theme))
            .corner_radius(CornerRadius::same(4))
            .show(ui, |ui| {
                ui.set_min_size(egui::vec2(width, list_height));
                ui.set_max_size(egui::vec2(width, list_height));
                match self.model.tab {
                    BrowseTab::Sessions => self.session_list(ui, theme),
                    BrowseTab::Results => self.result_list(ui, theme),
                }
            });
        let (label, has_previous, has_next) = match self.model.tab {
            BrowseTab::Sessions => (
                self.model.sessions_label(),
                self.model.has_previous_sessions(),
                self.model.has_next_sessions,
            ),
            BrowseTab::Results => (
                self.model.results_label(),
                self.model.has_previous_results(),
                self.model.has_next_results,
            ),
        };
        ui.add_space(4.0);
        ui.label(RichText::new(label).size(12.0).color(theme.muted));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let previous = ui.add_enabled(has_previous, page_button(t(S::HistoryPrevious)));
            let next = ui.add_enabled(has_next, page_button(t(S::HistoryNext)));
            match (self.model.tab, previous.clicked(), next.clicked()) {
                (BrowseTab::Sessions, true, _) => self.model.previous_sessions(),
                (BrowseTab::Sessions, _, true) => self.model.next_sessions(),
                (BrowseTab::Results, true, _) => self.model.previous_results(),
                (BrowseTab::Results, _, true) => self.model.next_results(),
                _ => {}
            }
        });
    }

    fn session_list(&mut self, ui: &mut Ui, theme: &Theme) {
        if self.model.sessions_empty() {
            empty_text(ui, theme, t(S::HistoryEmptySessions));
            return;
        }
        let mut open = None;
        egui::ScrollArea::vertical()
            .id_salt("history-sessions")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for session in &self.model.sessions {
                    let selected = self.model.anchor.is_none()
                        && self.model.active_session.as_ref().is_some_and(|a| a.id == session.id);
                    if session_row(ui, theme, session, selected, None).clicked() {
                        open = Some(session.clone());
                    }
                }
            });
        if let Some(session) = open {
            self.model.open_session(session);
        }
    }

    fn result_list(&mut self, ui: &mut Ui, theme: &Theme) {
        if self.model.results_empty() {
            empty_text(ui, theme, t(S::HistoryEmptyResults));
            return;
        }
        let mut open = None;
        egui::ScrollArea::vertical()
            .id_salt("history-results")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for hit in &self.model.results {
                    let selected = self.model.anchor == Some(hit.entry.sequence)
                        && self
                            .model
                            .active_session
                            .as_ref()
                            .is_some_and(|a| a.id == hit.session.id);
                    if session_row(ui, theme, &hit.session, selected, Some(&hit.entry)).clicked() {
                        open = Some(hit.clone());
                    }
                }
            });
        if let Some(hit) = open {
            self.model.open_hit(&hit);
        }
    }

    fn context(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.add(egui::Label::new(RichText::new(self.model.context_label()).size(13.0).color(theme.text)).truncate());
        if self.model.context_busy {
            ui.label(RichText::new(t(S::HistoryLoading)).size(12.0).color(theme.muted));
        }
        let height = (ui.available_height() - 48.0).max(100.0);
        let width = ui.available_width();
        let mono = egui::FontId::monospace(13.0);
        let view_height = height - 26.0;
        let mut area = egui::ScrollArea::vertical()
            .id_salt("history-transcript")
            .auto_shrink([false, false]);
        // Laid out once to find the anchor, then scrolled so it sits in the middle.
        if self.model.scroll_to_anchor > 0
            && let Some(y) = self.model.anchor_y
        {
            area = area.vertical_scroll_offset((y - view_height / 2.0).max(0.0));
            self.model.scroll_to_anchor -= 1;
        }
        egui::Frame::new()
            .fill(editor_fill(theme))
            .stroke(Stroke::new(1.0, theme.border))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_min_size(egui::vec2(width - 26.0, height - 26.0));
                ui.set_max_size(egui::vec2(width - 26.0, height - 26.0));
                area.show(ui, |ui| {
                    let top = ui.min_rect().top();
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let text_color = theme.text;
                    // A time heading when the minute changes, a label only for what was sent,
                    // scripted or private, and blank received lines as one paragraph break (C#
                    // UI review, item 12).
                    let mut last_minute = None;
                    let mut last_blank = false;
                    for entry in &self.model.transcript {
                        let received = matches!(entry.kind, wandur_core::history::EntryKind::Received);
                        let blank = entry.text.trim().is_empty();
                        if received && blank {
                            if !last_blank {
                                ui.add_space(8.0);
                            }
                            last_blank = true;
                            continue;
                        }
                        last_blank = false;
                        let minute = general_time(entry.at);
                        if last_minute.as_ref() != Some(&minute) {
                            ui.add_space(4.0);
                            ui.add(
                                egui::Label::new(RichText::new(&minute).size(11.0).color(theme.muted))
                                    .selectable(false),
                            );
                            last_minute = Some(minute);
                        }
                        if !received {
                            ui.add(
                                egui::Label::new(RichText::new(kind_label(entry.kind)).size(11.0).color(theme.muted))
                                    .selectable(false),
                            );
                        }
                        let anchored = self.model.anchor == Some(entry.sequence);
                        let mut text = RichText::new(&entry.text).font(mono.clone()).color(text_color);
                        if anchored {
                            text = text.background_color(anchor_fill(theme));
                        }
                        // A blank line is not something to select (nor a control to name).
                        let response = ui.add(egui::Label::new(text).wrap().selectable(!entry.text.is_empty()));
                        if anchored {
                            self.model.anchor_y = Some(response.rect.center().y - top);
                        }
                        ui.add_space(4.0);
                    }
                });
            });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let previous = ui.add_enabled(
                self.model.has_previous_context && !self.model.context_busy,
                page_button(t(S::HistoryPrevious)),
            );
            let next = ui.add_enabled(
                self.model.has_next_context && !self.model.context_busy,
                page_button(t(S::HistoryNext)),
            );
            let delete = ui.add_enabled(self.model.can_delete(), page_button(t(S::HistoryDelete)));
            if previous.clicked() {
                self.model.previous_context();
            }
            if next.clicked() {
                self.model.next_context();
            }
            if delete.clicked() {
                self.model.request_delete();
            }
        });
    }

    fn status(&mut self, ui: &mut Ui, theme: &Theme) {
        if self.model.busy {
            ui.label(RichText::new(t(S::HistoryLoading)).size(12.0).color(theme.muted));
        }
        if !self.model.error.is_empty() {
            ui.label(RichText::new(&self.model.error).size(13.0).color(theme.error));
        }
        if self.model.confirm_delete {
            egui::Frame::new()
                .fill(list_fill(theme))
                .stroke(Stroke::new(1.0, theme.border))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.label(RichText::new(self.model.delete_prompt()).size(13.0).color(theme.text));
                    ui.add_enabled_ui(!self.model.deleting, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            if secondary_button(ui, t(S::HistoryDelete)).clicked() {
                                self.model.delete();
                            }
                            if secondary_button(ui, t(S::Cancel)).clicked() {
                                self.model.cancel_delete();
                            }
                        });
                    });
                });
        }
    }
}

fn text_field(ui: &mut Ui, theme: &Theme, label: &str, value: &mut String, width: f32) -> bool {
    let mut changed = false;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.set_width(width);
        ui.label(RichText::new(label).size(13.0).color(theme.muted));
        let edit = egui::TextEdit::singleline(value)
            .hint_text(label)
            .min_size(egui::vec2(0.0, 36.0))
            .vertical_align(Align::Center)
            .desired_width(width);
        changed = ui.add(edit).changed();
    });
    changed
}

fn page_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).size(14.0)).min_size(egui::vec2(82.0, 32.0))
}

fn empty_text(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.add(egui::Label::new(RichText::new(text).size(13.0).color(theme.muted)).wrap());
    });
}

/// The list background: a shade between the panel and the transcript.
fn list_fill(theme: &Theme) -> Color32 {
    if theme.light {
        theme.panel.lerp_to_gamma(Color32::WHITE, 0.45)
    } else {
        theme.panel.lerp_to_gamma(Color32::BLACK, 0.2)
    }
}

/// The transcript box: lighter than the panel on a light chrome, darker on a dark one (the C#
/// editor brushes).
fn editor_fill(theme: &Theme) -> Color32 {
    if theme.light {
        theme.panel.lerp_to_gamma(Color32::WHITE, 0.7)
    } else {
        theme.panel.lerp_to_gamma(Color32::BLACK, 0.35)
    }
}

/// Behind the search hit in the transcript.
fn anchor_fill(theme: &Theme) -> Color32 {
    if theme.light {
        Color32::from_rgb(0xEC, 0xDF, 0xA6)
    } else {
        theme.selection
    }
}

/// A session (or a result's session with its time and text): name, then character and start.
fn session_row(
    ui: &mut Ui,
    theme: &Theme,
    session: &HistorySession,
    selected: bool,
    entry: Option<&HistoryEntry>,
) -> egui::Response {
    let fill = if selected {
        theme.accent.gamma_multiply(0.45)
    } else {
        Color32::TRANSPARENT
    };
    let inner = egui::Frame::new()
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 5.0;
            ui.add(
                egui::Label::new(RichText::new(&session.world_name).size(14.0).strong().color(theme.text)).truncate(),
            );
            let meta = if session.character_name.is_empty() {
                general_time(session.started_at)
            } else {
                format!("{} · {}", session.character_name, general_time(session.started_at))
            };
            ui.add(egui::Label::new(RichText::new(meta).size(12.0).color(theme.muted)).truncate());
            if let Some(entry) = entry {
                ui.label(RichText::new(general_time(entry.at)).size(11.0).color(theme.muted));
                let mut snippet: String = entry.text.replace(['\n', '\r'], " ");
                if snippet.chars().count() > 180 {
                    snippet = snippet.chars().take(180).collect::<String>() + "...";
                }
                ui.add(egui::Label::new(RichText::new(snippet).size(13.0).color(theme.text)).wrap());
            }
        });
    let response = ui.interact(
        inner.response.rect,
        ui.id().with(("history-row", &session.id, entry.map(|e| e.sequence))),
        egui::Sense::click(),
    );
    crate::a11y::control(
        &response,
        egui::accesskit::Role::ListBoxOption,
        &match entry {
            Some(e) => format!("{}, {}", session_label(session), e.text),
            None => session_label(session),
        },
    );
    let tip = format!("{}\n{}", session_label(session), session.world_key);
    response.on_hover_text(tip)
}

/// A small calendar drawn in the date field's button.
fn paint_calendar(ui: &Ui, rect: egui::Rect, color: Color32) {
    let painter = ui.painter();
    let stroke = Stroke::new(1.2, color);
    painter.rect_stroke(rect, CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
    painter.line_segment(
        [
            rect.left_top() + egui::vec2(0.0, 4.0),
            rect.right_top() + egui::vec2(0.0, 4.0),
        ],
        stroke,
    );
    painter.text(
        rect.center() + egui::vec2(0.0, 2.0),
        egui::Align2::CENTER_CENTER,
        "8",
        egui::FontId::proportional(10.0),
        color,
    );
}

/// The month grid below a date field; returns the picked day (local midnight).
fn month_popup(
    ui: &Ui,
    anchor: &egui::Response,
    theme: &Theme,
    year: i64,
    month: i64,
    state: &mut Option<(DateField, i64, i64)>,
) -> Option<SystemTime> {
    let mut picked = None;
    let area = egui::Area::new(anchor.id.with("month"))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor.rect.left_bottom() + egui::vec2(-200.0, 4.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(230.0);
                ui.horizontal(|ui| {
                    if ui.small_button("<").clicked()
                        && let Some((_, y, m)) = state
                    {
                        (*y, *m) = if *m == 1 { (*y - 1, 12) } else { (*y, *m - 1) };
                    }
                    ui.label(RichText::new(format!("{year}-{month:02}")).color(theme.text));
                    if ui.small_button(">").clicked()
                        && let Some((_, y, m)) = state
                    {
                        (*y, *m) = if *m == 12 { (*y + 1, 1) } else { (*y, *m + 1) };
                    }
                });
                let first = wandur_core::directory::time::days_from_civil(year, month, 1);
                // 1970-01-01 was a Thursday; weeks start on Sunday.
                let weekday = (first + 4).rem_euclid(7);
                let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
                let days = wandur_core::directory::time::days_from_civil(ny, nm, 1) - first;
                egui::Grid::new(anchor.id.with("days"))
                    .spacing([2.0, 2.0])
                    .show(ui, |ui| {
                        for _ in 0..weekday {
                            ui.label("");
                        }
                        for day in 1..=days {
                            if ui
                                .add(egui::Button::new(day.to_string()).min_size(egui::vec2(28.0, 22.0)))
                                .clicked()
                                && let Some(midnight) = local_midnight(year, month, day)
                            {
                                picked = Some(UNIX_EPOCH + Duration::from_secs(midnight.max(0) as u64));
                            }
                            if (weekday + day) % 7 == 0 {
                                ui.end_row();
                            }
                        }
                    });
            });
        });
    if picked.is_none() && area.response.clicked_elsewhere() && !anchor.clicked() {
        *state = None;
    }
    picked
}

#[cfg(test)]
mod tests {
    //! The C# `HistoryViewTests`, ported to the model.
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wandur_core::history::{EntryKind, HistoryError};

    type OnSearch = Box<dyn Fn(&str) -> Result<Vec<HistoryHit>, HistoryError> + Send + Sync>;
    type OnEntries = Box<dyn Fn(&str, i64) -> Vec<HistoryEntry> + Send + Sync>;

    #[derive(Default)]
    struct Memory {
        items: Mutex<Vec<(HistorySession, Vec<HistoryEntry>)>>,
        on_search: Option<OnSearch>,
        on_entries: Option<OnEntries>,
        last_query: Mutex<Option<String>>,
        last_filter: Mutex<Option<HistoryFilter>>,
        maximum_limit: AtomicUsize,
        reads: AtomicUsize,
    }

    impl Memory {
        fn add(&self, session: HistorySession, entries: Vec<HistoryEntry>) {
            let mut items = self.items.lock().unwrap();
            items.retain(|(s, _)| s.id != session.id);
            items.push((session, entries));
        }

        fn limit(&self, limit: usize) {
            self.maximum_limit.fetch_max(limit, Ordering::Relaxed);
        }
    }

    impl HistoryStore for Memory {
        fn append(&self, session: &HistorySession, entries: &[HistoryEntry]) -> Result<(), HistoryError> {
            self.add(session.clone(), entries.to_vec());
            Ok(())
        }
        fn sessions(
            &self,
            _: &HistoryFilter,
            offset: usize,
            limit: usize,
        ) -> Result<Vec<HistorySession>, HistoryError> {
            self.limit(limit);
            let items = self.items.lock().unwrap();
            Ok(items.iter().map(|(s, _)| s.clone()).skip(offset).take(limit).collect())
        }
        fn search(
            &self,
            query: &str,
            filter: &HistoryFilter,
            offset: usize,
            limit: usize,
        ) -> Result<Vec<HistoryHit>, HistoryError> {
            *self.last_query.lock().unwrap() = Some(query.into());
            *self.last_filter.lock().unwrap() = Some(filter.clone());
            self.limit(limit);
            if let Some(on_search) = &self.on_search {
                return on_search(query);
            }
            let items = self.items.lock().unwrap();
            Ok(items
                .iter()
                .flat_map(|(s, entries)| {
                    entries.iter().map(|e| HistoryHit {
                        session: s.clone(),
                        entry: e.clone(),
                    })
                })
                .skip(offset)
                .take(limit)
                .collect())
        }
        fn entries(&self, id: &str, from: i64, limit: usize) -> Result<Vec<HistoryEntry>, HistoryError> {
            self.limit(limit);
            self.reads.fetch_add(1, Ordering::Relaxed);
            if let Some(on_entries) = &self.on_entries {
                return Ok(on_entries(id, from).into_iter().take(limit).collect());
            }
            let items = self.items.lock().unwrap();
            Ok(items
                .iter()
                .find(|(s, _)| s.id == id)
                .map(|(_, e)| e.iter().filter(|e| e.sequence >= from).take(limit).cloned().collect())
                .unwrap_or_default())
        }
        fn delete(&self, id: &str) -> Result<(), HistoryError> {
            self.items.lock().unwrap().retain(|(s, _)| s.id != id);
            Ok(())
        }
        fn prune(&self, _: SystemTime) -> Result<(), HistoryError> {
            unimplemented!()
        }
    }

    fn time() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_790_251_200)
    }

    fn session(id: &str) -> HistorySession {
        HistorySession {
            id: id.into(),
            world_key: "harbor.test:4000".into(),
            world_name: "Lantern Harbor".into(),
            character_name: "Rowan".into(),
            started_at: time(),
            ended_at: None,
        }
    }

    fn entry(sequence: i64, text: &str) -> HistoryEntry {
        HistoryEntry {
            sequence,
            at: time(),
            kind: EntryKind::Received,
            text: text.into(),
        }
    }

    fn model(store: &Arc<Memory>) -> HistoryModel {
        let store: Arc<dyn HistoryStore> = store.clone();
        HistoryModel::new(store, Arc::new(|| {}))
    }

    /// Poll until no read runs.
    fn settle(model: &mut HistoryModel) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            model.poll();
            if !model.working() {
                return;
            }
            assert!(Instant::now() < deadline, "the model never settled");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn pages_are_bounded_and_search_preserves_literal_query_and_filters() {
        let store = Arc::new(Memory::default());
        for i in 0..125 {
            store.add(session(&i.to_string()), vec![entry(i, "lantern")]);
        }
        let mut m = model(&store);
        m.refresh();
        settle(&mut m);
        assert_eq!(m.sessions.len(), 50);
        assert!(m.has_next_sessions);
        m.next_sessions();
        settle(&mut m);
        assert_eq!(m.sessions[0].id, "50");
        m.previous_sessions();
        settle(&mut m);
        assert_eq!(m.sessions[0].id, "0");

        m.query = "lantern \"quiet pier\" OR *".into();
        m.world = "harbor.test:4000".into();
        m.character = "Rowan".into();
        m.from = Some(time());
        m.until = Some(time() + Duration::from_secs(86_400));
        m.filters_changed();
        m.refresh();
        settle(&mut m);
        assert_eq!(store.last_query.lock().unwrap().as_deref(), Some(m.query.as_str()));
        assert_eq!(
            store.last_filter.lock().unwrap().clone().unwrap(),
            HistoryFilter {
                world: Some(m.world.clone()),
                character: Some(m.character.clone()),
                from: m.from,
                until: m.until,
            }
        );
        assert_eq!(m.results.len(), 50);
        assert!(m.has_next_results);
        assert_eq!(m.tab, BrowseTab::Results, "a search shows its results");
        m.next_results();
        settle(&mut m);
        assert_eq!(m.results[0].session.id, "50");
        assert!(store.maximum_limit.load(Ordering::Relaxed) <= 101);
        m.from = Some(time() + Duration::from_secs(2 * 86_400));
        m.filters_changed();
        m.refresh();
        settle(&mut m);
        assert!(!m.error.is_empty());
        assert!(m.results.is_empty());
    }

    #[test]
    fn a_slow_search_cannot_overwrite_a_new_query_or_publish_after_dispose() {
        let (entered_tx, entered) = channel::<()>();
        let entered_tx = Mutex::new(entered_tx);
        let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let gate = Arc::clone(&release);
        let store = Arc::new(Memory {
            on_search: Some(Box::new(move |query: &str| {
                if query == "old" {
                    let _ = entered_tx.lock().unwrap().send(());
                    let (open, wake) = &*gate;
                    let mut open = open.lock().unwrap();
                    while !*open {
                        open = wake.wait(open).unwrap();
                    }
                }
                Ok(vec![HistoryHit {
                    session: session("one"),
                    entry: entry(1, query),
                }])
            })),
            ..Memory::default()
        });
        let mut m = model(&store);
        m.query = "old".into();
        m.refresh();
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        m.query = "new".into();
        m.filters_changed();
        m.refresh();
        settle(&mut m);
        assert_eq!(m.results.len(), 1);
        assert_eq!(m.results[0].entry.text, "new");
        *release.0.lock().unwrap() = true;
        release.1.notify_all();
        std::thread::sleep(Duration::from_millis(50));
        m.poll();
        assert_eq!(m.results[0].entry.text, "new");
        m.dispose();
        m.refresh();
        std::thread::sleep(Duration::from_millis(20));
        m.poll();
        assert_eq!(m.results[0].entry.text, "new");
    }

    #[test]
    fn context_opens_around_a_hit_and_pages_chronologically_across_sparse_sequences() {
        let store = Arc::new(Memory::default());
        store.add(
            session("one"),
            (1..=350).map(|i| entry(i * 10, &format!("Line {i}"))).collect(),
        );
        let mut m = model(&store);
        m.open_hit(&HistoryHit {
            session: session("one"),
            entry: entry(2200, "Line 220"),
        });
        settle(&mut m);
        assert!(m.transcript.iter().any(|e| e.sequence == 2200));
        let first = m.transcript[0].sequence;
        m.previous_context();
        settle(&mut m);
        assert!(m.transcript.last().unwrap().sequence < first);
        assert_eq!(m.transcript.len(), 100);
        m.next_context();
        settle(&mut m);
        assert_eq!(m.transcript[0].sequence, first);
        assert!(m.transcript.windows(2).all(|w| w[0].sequence < w[1].sequence));
        assert!(store.maximum_limit.load(Ordering::Relaxed) <= 101);
    }

    #[test]
    fn a_new_context_and_disposal_reject_late_transcript_loads() {
        let (entered_tx, entered) = channel::<()>();
        let entered_tx = Mutex::new(entered_tx);
        let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let gate = Arc::clone(&release);
        let store = Arc::new(Memory {
            on_entries: Some(Box::new(move |id: &str, _| {
                if id == "slow" {
                    let _ = entered_tx.lock().unwrap().send(());
                    let (open, wake) = &*gate;
                    let mut open = open.lock().unwrap();
                    while !*open {
                        open = wake.wait(open).unwrap();
                    }
                }
                vec![entry(1, id)]
            })),
            ..Memory::default()
        });
        let mut m = model(&store);
        m.open_session(session("slow"));
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        m.open_session(session("new"));
        settle(&mut m);
        assert_eq!(m.transcript.len(), 1);
        assert_eq!(m.transcript[0].text, "new");
        m.dispose();
        *release.0.lock().unwrap() = true;
        release.1.notify_all();
        std::thread::sleep(Duration::from_millis(50));
        m.poll();
        assert_eq!(m.transcript[0].text, "new");
    }

    #[test]
    fn previous_context_near_the_end_does_not_rescan_a_contiguous_session() {
        let store = Arc::new(Memory {
            on_entries: Some(Box::new(|_, from| {
                (from.max(1)..from.max(1) + 101)
                    .filter(|i| *i <= 10_000)
                    .map(|i| entry(i, &format!("Line {i}")))
                    .collect()
            })),
            ..Memory::default()
        });
        let mut m = model(&store);
        m.open_hit(&HistoryHit {
            session: session("one"),
            entry: entry(9000, "Line 9000"),
        });
        settle(&mut m);
        store.reads.store(0, Ordering::Relaxed);
        m.previous_context();
        settle(&mut m);
        assert_eq!(m.transcript[0].sequence, 8850);
        assert_eq!(m.transcript.last().unwrap().sequence, 8949);
        let reads = store.reads.load(Ordering::Relaxed);
        assert!(reads <= 3, "previous context used {reads} reads for one page");
    }

    #[test]
    fn an_opened_hit_is_anchored_inside_its_surrounding_transcript() {
        let store = Arc::new(Memory::default());
        store.add(
            session("one"),
            (1..=350).map(|i| entry(i, &format!("Line {i}"))).collect(),
        );
        let mut m = model(&store);
        m.open_hit(&HistoryHit {
            session: session("one"),
            entry: entry(220, "Line 220"),
        });
        settle(&mut m);
        assert_eq!(m.anchor, Some(220));
        let text = m.transcript_text();
        for line in ["Line 219", "Line 220", "Line 221"] {
            assert!(text.contains(line), "{line}");
        }
        assert!(text.contains(t(S::HistoryReceived)));
    }

    #[test]
    fn a_real_temporary_store_filters_and_deletes_through_the_model() {
        let dir = std::env::temp_dir().join(format!("wandur-history-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (db, _) = wandur_core::db::Database::open(&dir).unwrap();
        let store: Arc<dyn HistoryStore> = Arc::new(wandur_core::history::SqliteHistoryStore::new(db));
        let at = |e: HistoryEntry, secs| HistoryEntry {
            at: time() + Duration::from_secs(secs),
            ..e
        };
        store
            .append(
                &session("one"),
                &[
                    entry(1, "The copper lantern glows."),
                    at(entry(2, "The lantern is made of copper."), 60),
                ],
            )
            .unwrap();
        let other = HistorySession {
            character_name: "Vale".into(),
            ..session("two")
        };
        store.append(&other, &[entry(1, "A copper lantern.")]).unwrap();
        let mut m = HistoryModel::new(Arc::clone(&store), Arc::new(|| {}));
        m.query = "\"copper lantern\"".into();
        m.character = "Rowan".into();
        m.from = Some(time());
        m.until = Some(time() + Duration::from_secs(86_400));
        m.refresh();
        settle(&mut m);
        assert_eq!(m.results.len(), 1);
        assert_eq!(m.results[0].entry.text, "The copper lantern glows.");
        let hit = m.results[0].clone();
        m.open_hit(&hit);
        settle(&mut m);
        assert_eq!(m.transcript.len(), 2);
        m.request_delete();
        m.delete();
        settle(&mut m);
        assert!(m.results.is_empty());
        assert!(m.sessions.is_empty());
        let left = store.sessions(&HistoryFilter::default(), 0, 100).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, "two");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deletion_requires_confirmation_and_refreshes_sessions_results_and_context() {
        let store = Arc::new(Memory::default());
        store.add(session("one"), vec![entry(1, "lantern")]);
        store.add(session("two"), vec![entry(1, "lantern")]);
        let mut m = model(&store);
        m.query = "lantern".into();
        m.refresh();
        settle(&mut m);
        let first = m.sessions[0].clone();
        m.open_session(first);
        settle(&mut m);
        m.request_delete();
        assert!(m.confirm_delete);
        assert_eq!(store.items.lock().unwrap().len(), 2);
        m.cancel_delete();
        m.delete();
        settle(&mut m);
        assert_eq!(store.items.lock().unwrap().len(), 2);
        m.request_delete();
        m.delete();
        settle(&mut m);
        assert_eq!(m.sessions.len(), 1);
        assert_eq!(m.sessions[0].id, "two");
        assert_eq!(m.results.len(), 1);
        assert_eq!(m.results[0].session.id, "two");
        assert!(m.transcript.is_empty());
        assert!(m.active_session.is_none());
    }

    #[test]
    fn store_reads_run_off_the_ui_thread_and_failures_are_visible() {
        let ui_thread = std::thread::current().id();
        let store = Arc::new(Memory {
            on_search: Some(Box::new(move |_: &str| {
                assert_ne!(std::thread::current().id(), ui_thread);
                Err(HistoryError::Storage(wandur_core::db::DbError::Invalid(
                    "database error with sensitive details".into(),
                )))
            })),
            ..Memory::default()
        });
        let mut m = model(&store);
        m.query = "lantern".into();
        m.refresh();
        assert!(m.busy);
        settle(&mut m);
        assert!(!m.error.is_empty());
        assert!(!m.error.contains("sensitive"));
        assert!(!m.busy);
    }

    #[test]
    fn dates_follow_the_ui_language_and_round_trip() {
        l10n::override_thread(Some(Language::En));
        let day = parse_date("10/1/2026").unwrap();
        assert_eq!(date_text(day), "10/1/2026");
        assert!(general_time(day).ends_with("12:00 AM"), "{}", general_time(day));
        assert!(parse_date("13/1/2026").is_none());
        assert!(parse_date("2/30/2026").is_none());
        assert!(parse_date("soon").is_none());
        l10n::override_thread(Some(Language::De));
        assert_eq!(parse_date("01.10.2026"), Some(day));
        assert_eq!(date_text(day), "01.10.2026");
        assert!(general_time(day).ends_with("00:00"));
        assert_eq!(date_placeholder(), l10n::text_in(Language::De, S::HistoryAnyDate));
        l10n::override_thread(None);
    }
}
