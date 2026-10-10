//! The history recorder of one connection (the C# `HistoryRecorder`).
//!
//! The session hands it server text and sent commands as they are applied, each with whether it
//! was private (private input or a login running). That is the only work on the caller's thread:
//! a copy into a bounded queue. A thread of its own reads the text into lines, applies the
//! privacy rules and writes batches to the store:
//!
//! - Private and login stretches become one `private` marker; their text is never read. A line
//!   that was still unfinished when a stretch began is dropped, and after private server text
//!   recording resumes at the next complete public line (so a private line split across reads
//!   cannot leak its end).
//! - Remembered secrets (saved and private passwords) are masked as `[redacted]`, also when a
//!   secret is split across a command boundary: the end of an unfinished line that starts a
//!   secret, and the start of the next line that ends one.
//! - A line that reached the 4,096-character cap is dropped (a marker instead): a secret in it may
//!   be cut short and could not be masked.
//! - Retention runs before a batch is written: at the first batch, after a change and hourly.
//!
//! A full queue or a storage error pauses recording for the connection (the session goes on);
//! [`HistoryRecorder::take_failure`] reports it once. An orderly end writes what is queued.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError, channel, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use super::lines::{Line, LineParser, MAX_LINE};
use super::{EntryKind, HistoryEntry, HistorySession, HistoryStore, retention_cutoff};
use crate::diagnostics::console::mask;

/// The time source (tests use a fixed or stepped clock).
pub type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

pub fn system_clock() -> Clock {
    Arc::new(SystemTime::now)
}

/// Messages waiting for the recorder thread at most.
pub const QUEUE_MESSAGES: usize = 1024;
/// Text waiting for the recorder thread at most, in bytes.
pub const QUEUE_BYTES: usize = 4 << 20;
/// A batch is written once it has this many entries...
const BATCH_ENTRIES: usize = 32_768;
/// ...or this many bytes of text...
const BATCH_CHARS: usize = 4 << 20;
/// ...or its first entry has waited this long. Few large transactions cost a fraction of many
/// small ones (each commit merges index segments); an abrupt crash can lose this much text,
/// which the C# rule accepts, and the History window flushes open sessions before reading.
const BATCH_WAIT: Duration = Duration::from_secs(2);
/// How often retention runs while recording.
const PRUNE_EVERY: Duration = Duration::from_secs(3600);
const REDACTED: &str = "[redacted]";

enum Message {
    Received {
        text: String,
        hidden: bool,
        at: SystemTime,
    },
    Local {
        kind: EntryKind,
        text: String,
        hidden: bool,
        at: SystemTime,
    },
    Hide {
        at: SystemTime,
    },
    Secrets(Vec<String>),
    Character(String),
    Retention(u32),
    Flush(Option<Sender<()>>),
    Complete {
        at: SystemTime,
    },
}

#[derive(Default)]
struct Shared {
    failed: AtomicBool,
    queued_bytes: AtomicUsize,
}

/// One connection's recorder, owned by its session.
pub struct HistoryRecorder {
    tx: Option<SyncSender<Message>>,
    worker: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
    clock: Clock,
    session_id: String,
    /// What the recorder thread last heard, so repeated calls send nothing.
    hidden: bool,
    secrets: Vec<String>,
    character: String,
    retention_days: u32,
    reported: bool,
}

impl HistoryRecorder {
    /// Start recording `session` on a thread of its own.
    pub fn start(
        store: Arc<dyn HistoryStore>,
        session: HistorySession,
        retention_days: u32,
        clock: Clock,
    ) -> std::io::Result<Self> {
        let (tx, rx) = sync_channel(QUEUE_MESSAGES);
        let shared = Arc::new(Shared::default());
        let session_id = session.id.clone();
        let character = session.character_name.clone();
        let worker = {
            let shared = Arc::clone(&shared);
            let clock = Arc::clone(&clock);
            std::thread::Builder::new()
                .name(format!("wandur-history {}", &session_id[..session_id.len().min(8)]))
                .spawn(move || {
                    Worker {
                        store,
                        session,
                        retention_days,
                        clock,
                        shared,
                        capture: Capture::default(),
                        pending: Vec::new(),
                        pending_chars: 0,
                        first_pending: None,
                        next_prune: None,
                        last_retention: None,
                    }
                    .run(&rx)
                })?
        };
        Ok(Self {
            tx: Some(tx),
            worker: Some(worker),
            shared,
            clock,
            session_id,
            hidden: false,
            secrets: Vec::new(),
            character,
            retention_days,
            reported: false,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Recording stopped (a full queue or a storage error).
    pub fn is_faulted(&self) -> bool {
        self.shared.failed.load(Ordering::Acquire)
    }

    /// True once, the first time this is asked after recording stopped (for the notice).
    pub fn take_failure(&mut self) -> bool {
        if self.reported || !self.is_faulted() {
            return false;
        }
        self.reported = true;
        true
    }

    /// Server text as applied; `hidden` when it arrived in a private or login stretch.
    pub fn received(&mut self, text: &str, hidden: bool) {
        if hidden {
            self.hide_with(!text.is_empty());
            return;
        }
        if text.is_empty() {
            return;
        }
        self.hidden = false;
        let at = (self.clock)();
        self.send(Message::Received {
            text: text.to_string(),
            hidden: false,
            at,
        });
    }

    /// A command sent to the server; `hidden` when it was private input or a login.
    pub fn sent(&mut self, text: &str, hidden: bool) {
        self.local(EntryKind::Sent, text, hidden);
    }

    /// Text a script wrote into the transcript.
    pub fn script(&mut self, text: &str, hidden: bool) {
        self.local(EntryKind::Script, text, hidden);
    }

    /// A private or login stretch is under way: one marker, and the unfinished line is dropped.
    pub fn hide(&mut self) {
        self.hide_with(false);
    }

    /// The secrets to mask (the session's remembered passwords), when they changed.
    pub fn set_secrets(&mut self, secrets: &[String]) {
        if self.secrets != secrets {
            self.secrets = secrets.to_vec();
            self.send(Message::Secrets(self.secrets.clone()));
        }
    }

    /// The character the session plays (it may become known after the start).
    pub fn update_character(&mut self, name: &str) {
        if self.character != name {
            self.character = name.to_string();
            self.send(Message::Character(self.character.clone()));
        }
    }

    /// Days of retention (0: forever), from saved settings.
    pub fn update_retention(&mut self, days: u32) {
        if self.retention_days != days {
            self.retention_days = days;
            self.send(Message::Retention(days));
        }
    }

    /// Write what is batched without waiting for it.
    pub fn flush(&mut self) {
        self.send(Message::Flush(None));
    }

    /// Write what is batched; the receiver hears when it is written (or recording stopped).
    pub fn flush_and_wait(&mut self) -> Receiver<()> {
        let (done, wait) = channel();
        if !self.is_faulted() {
            self.send(Message::Flush(Some(done)));
        }
        wait
    }

    /// End the session: the unfinished line and the end time are written, then the thread
    /// stops. The handle lets a caller wait for that (the app does at exit).
    pub fn complete(mut self) -> Option<JoinHandle<()>> {
        self.finish();
        self.worker.take()
    }

    fn finish(&mut self) {
        if let Some(tx) = self.tx.take() {
            let at = (self.clock)();
            // Never wait: with the queue full the end time is lost, and the thread still writes
            // what is queued when it sees the queue closed.
            let _ = tx.try_send(Message::Complete { at });
        }
    }

    fn local(&mut self, kind: EntryKind, text: &str, hidden: bool) {
        if hidden {
            self.hide_with(false);
            return;
        }
        self.hidden = false;
        let at = (self.clock)();
        self.send(Message::Local {
            kind,
            text: text.to_string(),
            hidden: false,
            at,
        });
    }

    fn hide_with(&mut self, had_text: bool) {
        // Repeated hides only matter when private text arrived (the next public line is skipped).
        if self.hidden && !had_text {
            return;
        }
        self.hidden = true;
        let at = (self.clock)();
        if had_text {
            self.send(Message::Received {
                text: String::new(),
                hidden: true,
                at,
            });
        } else {
            self.send(Message::Hide { at });
        }
    }

    fn send(&mut self, message: Message) {
        if self.is_faulted() {
            return;
        }
        let Some(tx) = &self.tx else { return };
        let bytes = match &message {
            Message::Received { text, .. } | Message::Local { text, .. } => text.len(),
            _ => 0,
        };
        let queued = self.shared.queued_bytes.fetch_add(bytes, Ordering::AcqRel) + bytes;
        let refused = queued > QUEUE_BYTES
            || match tx.try_send(message) {
                Ok(()) => false,
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => true,
            };
        if refused {
            self.shared.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
            self.shared.failed.store(true, Ordering::Release);
        }
    }
}

impl Drop for HistoryRecorder {
    fn drop(&mut self) {
        // The thread finishes on its own; nothing waits for it here.
        self.finish();
    }
}

/// Waits for recorder threads that were completed, for at most `limit` in all (app exit).
pub fn join_all(handles: Vec<JoinHandle<()>>, limit: Duration) {
    if handles.is_empty() {
        return;
    }
    let (done, wait) = channel();
    let spawned = std::thread::Builder::new()
        .name("wandur-history-drain".into())
        .spawn(move || {
            for handle in handles {
                let _ = handle.join();
            }
            let _ = done.send(());
        });
    if spawned.is_ok() {
        let _ = wait.recv_timeout(limit);
    }
}

struct Worker {
    store: Arc<dyn HistoryStore>,
    session: HistorySession,
    retention_days: u32,
    clock: Clock,
    shared: Arc<Shared>,
    capture: Capture,
    pending: Vec<HistoryEntry>,
    pending_chars: usize,
    first_pending: Option<Instant>,
    next_prune: Option<SystemTime>,
    last_retention: Option<u32>,
}

impl Worker {
    fn run(mut self, rx: &Receiver<Message>) {
        loop {
            let message = match self.first_pending {
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                Some(first) => rx.recv_timeout((first + BATCH_WAIT).saturating_duration_since(Instant::now())),
            };
            match message {
                Ok(Message::Complete { at }) => {
                    self.capture.finish_partial(at, &mut self.pending);
                    self.session.ended_at = Some(at.max(self.session.started_at));
                    self.write(true);
                    return;
                }
                Ok(message) => self.apply(message),
                Err(RecvTimeoutError::Timeout) => self.write(false),
                Err(RecvTimeoutError::Disconnected) => {
                    self.write(false);
                    return;
                }
            }
            if self.pending.len() >= BATCH_ENTRIES || self.pending_chars >= BATCH_CHARS {
                self.write(false);
            }
        }
    }

    fn apply(&mut self, message: Message) {
        let before = self.pending.len();
        match message {
            Message::Received { text, hidden, at } => {
                self.shared.queued_bytes.fetch_sub(text.len(), Ordering::AcqRel);
                self.capture.received(&text, hidden, at, &mut self.pending);
            }
            Message::Local { kind, text, hidden, at } => {
                self.shared.queued_bytes.fetch_sub(text.len(), Ordering::AcqRel);
                self.capture.local(kind, &text, hidden, at, &mut self.pending);
            }
            Message::Hide { at } => self.capture.hide(at, &mut self.pending),
            Message::Secrets(secrets) => self.capture.secrets = secrets,
            Message::Character(name) => self.session.character_name = name,
            Message::Retention(days) => self.retention_days = days,
            Message::Flush(done) => {
                self.write(done.is_some());
                if let Some(done) = done {
                    let _ = done.send(());
                }
            }
            Message::Complete { .. } => {}
        }
        if self.pending.len() > before {
            self.pending_chars += self.pending[before..].iter().map(|e| e.text.len()).sum::<usize>();
            self.first_pending.get_or_insert_with(Instant::now);
        }
    }

    /// Write the batch (`always`: even an empty one, which updates the session row).
    fn write(&mut self, always: bool) {
        let entries = std::mem::take(&mut self.pending);
        self.pending_chars = 0;
        self.first_pending = None;
        if self.shared.failed.load(Ordering::Acquire) || (entries.is_empty() && !always) {
            return;
        }
        let now = (self.clock)();
        let days = self.retention_days;
        let result = (|| {
            if let Some(cutoff) = retention_cutoff(days, now)
                && (self.next_prune.is_none_or(|next| now >= next) || self.last_retention != Some(days))
            {
                self.store.prune(cutoff)?;
                self.next_prune = Some(now + PRUNE_EVERY);
            }
            self.last_retention = Some(days);
            self.store.append(&self.session, &entries)
        })();
        if result.is_err() {
            self.shared.failed.store(true, Ordering::Release);
        }
    }
}

/// The privacy rules over the line reader (runs on the recorder's thread).
#[derive(Default)]
struct Capture {
    parser: LineParser,
    lines: Vec<Line>,
    secrets: Vec<String>,
    private: bool,
    /// Private server text arrived: skip received text up to the next complete line.
    skip_received_line: bool,
    /// The last received line saved was unfinished (or a private stretch began): the next one
    /// may start with the end of a secret.
    leading_fragment: bool,
    sequence: i64,
}

impl Capture {
    fn received(&mut self, text: &str, hidden: bool, at: SystemTime, out: &mut Vec<HistoryEntry>) {
        if hidden {
            self.hide(at, out);
            // Private text is never read. Resume only after a complete public line, so a private
            // line split across reads cannot leak its end.
            self.skip_received_line = true;
            return;
        }
        self.private = false;
        let mut lines = std::mem::take(&mut self.lines);
        self.parser.append(text, &mut lines);
        for line in lines.drain(..) {
            self.save_line(EntryKind::Received, line.text, line.truncated, false, at, out);
        }
        self.lines = lines;
    }

    fn local(&mut self, kind: EntryKind, text: &str, hidden: bool, at: SystemTime, out: &mut Vec<HistoryEntry>) {
        if hidden {
            self.hide(at, out);
            return;
        }
        self.finish_partial(at, out);
        self.private = false;
        let mut lines = std::mem::take(&mut self.lines);
        self.parser.append_local(text, &mut lines);
        self.parser.append_local("\n", &mut lines);
        for line in lines.drain(..) {
            self.save_line(kind, line.text, line.truncated, false, at, out);
        }
        self.lines = lines;
        self.parser.clear();
    }

    fn hide(&mut self, at: SystemTime, out: &mut Vec<HistoryEntry>) {
        // Unfinished public text may be the start of a secret split at the boundary.
        self.parser.clear();
        if !self.private {
            self.add(EntryKind::Private, String::new(), at, out);
        }
        self.private = true;
        self.leading_fragment = true;
    }

    fn finish_partial(&mut self, at: SystemTime, out: &mut Vec<HistoryEntry>) {
        let text = self.parser.current();
        if !text.is_empty() {
            let truncated = self.parser.current_truncated();
            self.save_line(EntryKind::Received, text, truncated, true, at, out);
        }
        self.parser.clear();
    }

    fn save_line(
        &mut self,
        kind: EntryKind,
        text: String,
        truncated: bool,
        partial: bool,
        at: SystemTime,
        out: &mut Vec<HistoryEntry>,
    ) {
        let received = kind == EntryKind::Received;
        if received && self.skip_received_line {
            if !partial {
                self.skip_received_line = false;
                self.leading_fragment = false;
            }
            return;
        }
        // At the cap a remembered secret may itself be cut short, so it could not be masked.
        if truncated || text.chars().count() >= MAX_LINE {
            self.add(EntryKind::Private, String::new(), at, out);
            if partial && received {
                self.skip_received_line = true;
            }
        } else {
            let mut text = if self.secrets.is_empty() {
                text
            } else {
                mask(&text, &self.secrets)
            };
            if received && self.leading_fragment {
                text = mask_boundary(&text, &self.secrets, true);
            }
            if partial {
                text = mask_boundary(&text, &self.secrets, false);
            }
            self.add(kind, text, at, out);
        }
        if received {
            self.leading_fragment = partial;
        }
    }

    fn add(&mut self, kind: EntryKind, text: String, at: SystemTime, out: &mut Vec<HistoryEntry>) {
        self.sequence += 1;
        out.push(HistoryEntry {
            sequence: self.sequence,
            at,
            kind,
            text,
        });
    }
}

/// Mask the longest part of a secret at the start (`leading`: its end) or at the end of `text`
/// (its start), shorter than the whole secret.
fn mask_boundary(text: &str, secrets: &[String], leading: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut matched = 0;
    for secret in secrets {
        let secret: Vec<char> = secret.chars().collect();
        let longest = secret.len().saturating_sub(1).min(chars.len());
        for length in (matched + 1..=longest).rev() {
            let (fragment, edge) = if leading {
                (&secret[secret.len() - length..], &chars[..length])
            } else {
                (&secret[..length], &chars[chars.len() - length..])
            };
            if fragment == edge {
                matched = length;
                break;
            }
        }
    }
    if matched == 0 {
        return text.to_string();
    }
    if leading {
        format!("{REDACTED}{}", chars[matched..].iter().collect::<String>())
    } else {
        format!(
            "{}{REDACTED}",
            chars[..chars.len() - matched].iter().collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    //! The C# `HistoryRecorderTests`, ported, plus retention with a clock.
    use super::*;
    use crate::history::{HistoryError, HistoryFilter, HistoryHit};
    use std::sync::atomic::AtomicU32;
    use std::sync::{Condvar, Mutex};

    #[derive(Default)]
    struct Recording {
        records: Mutex<Vec<HistoryEntry>>,
        last_session: Mutex<Option<HistorySession>>,
        appends: AtomicU32,
        prunes: Mutex<Vec<SystemTime>>,
        fail: AtomicBool,
        /// While closed, appends wait.
        gate: Option<(Mutex<bool>, Condvar)>,
    }

    impl Recording {
        fn gated() -> Self {
            Self {
                gate: Some((Mutex::new(false), Condvar::new())),
                ..Self::default()
            }
        }

        fn open_gate(&self) {
            if let Some((open, wake)) = &self.gate {
                *open.lock().unwrap() = true;
                wake.notify_all();
            }
        }

        fn texts(&self) -> Vec<String> {
            self.records.lock().unwrap().iter().map(|e| e.text.clone()).collect()
        }

        fn kinds(&self) -> Vec<EntryKind> {
            self.records.lock().unwrap().iter().map(|e| e.kind).collect()
        }
    }

    impl HistoryStore for Recording {
        fn append(&self, session: &HistorySession, entries: &[HistoryEntry]) -> Result<(), HistoryError> {
            if let Some((open, wake)) = &self.gate {
                let mut open = open.lock().unwrap();
                while !*open {
                    open = wake.wait(open).unwrap();
                }
            }
            if self.fail.load(Ordering::Relaxed) {
                return Err(HistoryError::Storage(crate::db::DbError::Io(std::io::Error::other(
                    "unavailable",
                ))));
            }
            self.records.lock().unwrap().extend_from_slice(entries);
            *self.last_session.lock().unwrap() = Some(session.clone());
            self.appends.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn sessions(&self, _: &HistoryFilter, _: usize, _: usize) -> Result<Vec<HistorySession>, HistoryError> {
            unimplemented!()
        }
        fn search(&self, _: &str, _: &HistoryFilter, _: usize, _: usize) -> Result<Vec<HistoryHit>, HistoryError> {
            unimplemented!()
        }
        fn entries(&self, _: &str, _: i64, _: usize) -> Result<Vec<HistoryEntry>, HistoryError> {
            unimplemented!()
        }
        fn delete(&self, _: &str) -> Result<(), HistoryError> {
            unimplemented!()
        }
        fn prune(&self, before: SystemTime) -> Result<(), HistoryError> {
            self.prunes.lock().unwrap().push(before);
            Ok(())
        }
    }

    fn session() -> HistorySession {
        HistorySession::start("example.test:4000", "Starfall", "Mira", SystemTime::now())
    }

    fn recorder(store: &Arc<Recording>, retention: u32) -> HistoryRecorder {
        let store: Arc<dyn HistoryStore> = store.clone();
        HistoryRecorder::start(store, session(), retention, system_clock()).unwrap()
    }

    fn finish(recorder: HistoryRecorder) {
        recorder.complete().unwrap().join().unwrap();
    }

    fn hunter() -> Vec<String> {
        vec!["hunter2".into()]
    }

    #[test]
    fn a_command_boundary_does_not_expose_the_two_halves_of_a_known_secret() {
        let store = Arc::new(Recording::default());
        let mut r = recorder(&store, 0);
        r.set_secrets(&hunter());
        r.received("echo hun", false);
        r.sent("look", false);
        r.received("\u{1b}[32mter2\u{1b}[0m\n", false);
        finish(r);
        let texts = store.texts();
        assert!(
            !texts.iter().any(|t| t.contains("hun") || t.contains("ter2")),
            "{texts:?}"
        );
        assert!(
            store
                .records
                .lock()
                .unwrap()
                .iter()
                .any(|e| e.kind == EntryKind::Sent && e.text == "look")
        );
    }

    #[test]
    fn private_line_continuations_stay_hidden_even_without_a_known_secret() {
        let store = Arc::new(Recording::default());
        let mut r = recorder(&store, 0);
        r.received("echo hun", true);
        r.received("ter2\nA safe new line\n", false);
        finish(r);
        let texts = store.texts();
        assert!(!texts.iter().any(|t| t.contains("ter2")), "{texts:?}");
        assert!(texts.iter().any(|t| t == "A safe new line"), "{texts:?}");
    }

    #[test]
    fn erasing_the_end_of_a_truncated_line_does_not_make_its_secret_prefix_safe() {
        let store = Arc::new(Recording::default());
        let mut r = recorder(&store, 0);
        let secret = "x".repeat(5000);
        r.set_secrets(std::slice::from_ref(&secret));
        r.received(&format!("{secret}\r{}\u{1b}[K\n", "x".repeat(4000)), false);
        finish(r);
        assert!(!store.texts().iter().any(|t| t.contains(&"x".repeat(20))));
    }

    #[test]
    fn does_not_persist_a_truncated_prefix_of_a_long_secret() {
        let store = Arc::new(Recording::default());
        let mut r = recorder(&store, 0);
        let secret = "x".repeat(5000);
        r.set_secrets(std::slice::from_ref(&secret));
        r.received(&format!("{secret}\n"), false);
        r.received(&secret, false);
        finish(r);
        assert!(!store.texts().iter().any(|t| t.contains(&"x".repeat(20))));
        assert!(store.kinds().iter().all(|k| *k == EntryKind::Private));
    }

    #[test]
    fn lines_at_the_length_cap_are_dropped() {
        let store = Arc::new(Recording::default());
        let mut r = recorder(&store, 0);
        r.received(&format!("{}\nshort\n", "y".repeat(MAX_LINE)), false);
        r.sent(&"z".repeat(MAX_LINE + 10), false);
        finish(r);
        assert_eq!(
            store.kinds(),
            [EntryKind::Private, EntryKind::Received, EntryKind::Private]
        );
        assert_eq!(store.texts(), ["", "short", ""]);
    }

    #[test]
    fn joins_network_fragments_and_strips_ansi_before_masking_known_secrets() {
        let store = Arc::new(Recording::default());
        let mut r = recorder(&store, 30);
        r.received("A silver fre", false);
        r.set_secrets(&hunter());
        r.received("ighter\r\nEcho: hun\u{1b}[3", false);
        r.received("2mter2\u{1b}[0m is hidden.\r\n", false);
        r.set_secrets(&[]);
        r.sent("look", false);
        r.received("A quiet deck > ", false);
        finish(r);
        assert_eq!(
            store.texts(),
            [
                "A silver freighter",
                "Echo: [redacted] is hidden.",
                "look",
                "A quiet deck > "
            ]
        );
        assert_eq!(
            store.kinds(),
            [
                EntryKind::Received,
                EntryKind::Received,
                EntryKind::Sent,
                EntryKind::Received
            ]
        );
        let sequences: Vec<_> = store.records.lock().unwrap().iter().map(|e| e.sequence).collect();
        assert_eq!(sequences, [1, 2, 3, 4]);
        assert!(
            !store
                .texts()
                .iter()
                .any(|t| t.contains('\u{1b}') || t.contains("hunter2"))
        );
    }

    #[test]
    fn private_boundaries_discard_partial_text_and_never_queue_secrets() {
        let store = Arc::new(Recording::default());
        let mut r = recorder(&store, 0);
        r.received("hun", false);
        r.set_secrets(&hunter());
        r.received("ter2\r\n", true);
        r.sent("hunter2", true);
        r.received("Checking credentials\r\n", true);
        r.received("Possible private continuation.\r\nWelcome aboard.\r\n", false);
        finish(r);
        assert_eq!(store.kinds(), [EntryKind::Private, EntryKind::Received]);
        assert_eq!(store.texts().last().map(String::as_str), Some("Welcome aboard."));
        assert!(
            !store
                .texts()
                .iter()
                .any(|t| t.contains("hun") || t.contains("credentials"))
        );
        assert!(store.prunes.lock().unwrap().is_empty());
    }

    #[test]
    fn a_login_stretch_becomes_one_marker() {
        let store = Arc::new(Recording::default());
        let mut r = recorder(&store, 0);
        r.received("Welcome!\nName: ", false);
        r.hide();
        r.hide();
        r.sent("Odo", true);
        r.sent("secret", true);
        r.received("Hello Odo.\n", false);
        r.sent("look", false);
        finish(r);
        assert_eq!(store.texts(), ["Welcome!", "", "Hello Odo.", "look"]);
        assert_eq!(store.kinds()[1], EntryKind::Private);
    }

    #[test]
    fn storage_failures_pause_recording_without_failing_the_caller() {
        let store = Arc::new(Recording::default());
        store.fail.store(true, Ordering::Relaxed);
        let mut r = recorder(&store, 30);
        r.received("public\n", false);
        r.flush_and_wait().recv_timeout(Duration::from_secs(5)).unwrap();
        r.sent("look", false);
        assert!(r.is_faulted());
        assert!(r.take_failure());
        assert!(!r.take_failure(), "reported once");
        finish(r);
        assert!(store.records.lock().unwrap().is_empty());
    }

    #[test]
    fn slow_storage_uses_a_bounded_queue_and_signals_overflow() {
        let store = Arc::new(Recording::gated());
        let mut r = recorder(&store, 0);
        let mut i = 0;
        while i < 10_000 && !r.is_faulted() {
            r.received(&format!("line {i}\n"), false);
            r.flush();
            i += 1;
        }
        assert!(r.is_faulted());
        assert!(r.take_failure());
        store.open_gate();
        finish(r);
        assert!(store.records.lock().unwrap().len() < 1000);
    }

    #[test]
    fn flushes_batches_and_updates_character_and_retention_without_rewriting_history() {
        let store = Arc::new(Recording::default());
        let mut r = recorder(&store, 30);
        for i in 0..100 {
            r.received(&format!("Room {i}\n"), false);
        }
        r.update_character("Tarin");
        r.flush_and_wait().recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(store.records.lock().unwrap().len(), 100);
        finish(r);
        let appends = store.appends.load(Ordering::Relaxed);
        assert!((1..=10).contains(&appends), "{appends} appends");
        let last = store.last_session.lock().unwrap().clone().unwrap();
        assert_eq!(last.character_name, "Tarin");
        assert!(last.ended_at.is_some());
        assert!(!store.prunes.lock().unwrap().is_empty());
    }

    #[test]
    fn retention_runs_first_after_a_change_and_hourly_by_the_clock() {
        let store = Arc::new(Recording::default());
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000);
        let offset = Arc::new(AtomicU32::new(0));
        let clock: Clock = {
            let offset = Arc::clone(&offset);
            Arc::new(move || start + Duration::from_secs(u64::from(offset.load(Ordering::Relaxed))))
        };
        let dynstore: Arc<dyn HistoryStore> = store.clone();
        let mut r = HistoryRecorder::start(dynstore, session(), 30, clock).unwrap();
        let wait = |r: &mut HistoryRecorder| r.flush_and_wait().recv_timeout(Duration::from_secs(5)).unwrap();
        let day = 86_400;
        r.received("one\n", false);
        wait(&mut r);
        // A batch ten minutes later: no new pass.
        offset.store(600, Ordering::Relaxed);
        r.received("two\n", false);
        wait(&mut r);
        // A changed retention prunes at once.
        r.update_retention(90);
        r.received("three\n", false);
        wait(&mut r);
        // An hour after the last pass, again.
        offset.store(600 + 3600, Ordering::Relaxed);
        r.received("four\n", false);
        wait(&mut r);
        // Forever: never.
        r.update_retention(0);
        offset.store(600 + 7 * 3600, Ordering::Relaxed);
        r.received("five\n", false);
        finish(r);
        let prunes = store.prunes.lock().unwrap().clone();
        let at = |secs: u64, days: u64| start + Duration::from_secs(secs) - Duration::from_secs(days * day);
        assert_eq!(prunes, [at(0, 30), at(600, 90), at(4200, 90)]);
    }

    #[test]
    fn the_caller_only_queues_and_never_waits_for_the_store() {
        let store = Arc::new(Recording::gated());
        let mut r = recorder(&store, 0);
        let started = Instant::now();
        for i in 0..200 {
            r.received(&format!("line {i}\n"), false);
        }
        r.flush();
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "{:?}",
            started.elapsed()
        );
        assert!(!r.is_faulted());
        store.open_gate();
        finish(r);
        assert_eq!(store.records.lock().unwrap().len(), 200);
    }

    #[test]
    fn boundary_masks_take_the_longest_fragment() {
        let secrets = hunter();
        assert_eq!(mask_boundary("echo hun", &secrets, false), "echo [redacted]");
        assert_eq!(mask_boundary("ter2 ok", &secrets, true), "[redacted] ok");
        assert_eq!(mask_boundary("nothing", &secrets, true), "nothing");
        assert_eq!(mask_boundary("x", &[String::new()], true), "x");
    }
}
