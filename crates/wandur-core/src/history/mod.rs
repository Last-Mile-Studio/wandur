//! Session history: what was read and sent in each session, kept in `wandur.db` and searchable,
//! as the C# client's `Wandur.Core.History` (see `docs/session-history.md` there).
//!
//! - [`lines`]: the plain-text line reader the recorder uses (ANSI decoded, carriage returns and
//!   erase-line applied, the 4,096-character line cap).
//! - [`recorder`]: per session, a bounded queue to a thread of its own that turns server text
//!   and sent commands into entries under the privacy rules and writes them in batches.
//! - [`store`]: the SQLite tables (sessions, entries, an external-content FTS5 index) with
//!   literal word and phrase search, filters, paging, delete and retention.
//!
//! Nothing here runs on the UI thread except handing text to the recorder's queue.

pub mod lines;
pub mod recorder;
pub mod store;

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub use recorder::{Clock, HistoryRecorder, system_clock};
pub use store::SqliteHistoryStore;

/// The retention choices in Settings > General, in days; 0 keeps history forever.
pub const RETENTION_CHOICES: [u32; 4] = [30, 90, 365, 0];
/// The default retention (the C# default).
pub const DEFAULT_RETENTION_DAYS: u32 = 30;
/// Longest query, filter text or id, in UTF-16 units (the C# bound).
pub const MAX_QUERY: usize = 4096;
/// Most rows any read returns.
pub const MAX_PAGE: usize = 500;

/// What an entry is: server text, a command sent, script output, or a private stretch (a marker
/// with no text).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntryKind {
    Received,
    Sent,
    Script,
    Private,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Received => "received",
            EntryKind::Sent => "sent",
            EntryKind::Script => "script",
            EntryKind::Private => "private",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "received" => EntryKind::Received,
            "sent" => EntryKind::Sent,
            "script" => EntryKind::Script,
            "private" => EntryKind::Private,
            _ => return None,
        })
    }
}

/// One recorded connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistorySession {
    /// 32 hex digits, new for every connection.
    pub id: String,
    /// The canonical endpoint (`host:port`), or `demo`.
    pub world_key: String,
    pub world_name: String,
    pub character_name: String,
    pub started_at: SystemTime,
    pub ended_at: Option<SystemTime>,
}

impl HistorySession {
    /// A new session starting `now`.
    pub fn start(world_key: &str, world_name: &str, character_name: &str, now: SystemTime) -> Self {
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            world_key: world_key.into(),
            world_name: world_name.into(),
            character_name: character_name.into(),
            started_at: now,
            ended_at: None,
        }
    }
}

/// One line or marker of a session, in sequence order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub sequence: i64,
    pub at: SystemTime,
    pub kind: EntryKind,
    pub text: String,
}

/// A search result: the entry and its session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryHit {
    pub session: HistorySession,
    pub entry: HistoryEntry,
}

/// Search and browse filters. World and character match parts of names (the world also its
/// endpoint), ignoring case, with no wildcards; dates are on or after `from` and strictly
/// before `until`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryFilter {
    pub world: Option<String>,
    pub character: Option<String>,
    pub from: Option<SystemTime>,
    pub until: Option<SystemTime>,
}

#[derive(Debug)]
pub enum HistoryError {
    /// An argument the store refuses (the name of the argument).
    Invalid(&'static str),
    Storage(crate::db::DbError),
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HistoryError::Invalid(what) => write!(f, "{what}?"),
            HistoryError::Storage(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for HistoryError {}

impl From<crate::db::DbError> for HistoryError {
    fn from(e: crate::db::DbError) -> Self {
        HistoryError::Storage(e)
    }
}

impl From<rusqlite::Error> for HistoryError {
    fn from(e: rusqlite::Error) -> Self {
        HistoryError::Storage(e.into())
    }
}

/// Where history is kept. Every call may block on the disk: callers run it off the UI thread.
pub trait HistoryStore: Send + Sync {
    /// Add entries to a session (creating or updating its row). A sequence already stored is
    /// kept as it was; a deleted session is never brought back.
    fn append(&self, session: &HistorySession, entries: &[HistoryEntry]) -> Result<(), HistoryError>;
    /// Sessions, newest start first; dates filter the start time.
    fn sessions(
        &self,
        filter: &HistoryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<HistorySession>, HistoryError>;
    /// Entries matching every word and quoted phrase of `query` (taken literally), newest
    /// first; dates filter the entry times.
    fn search(
        &self,
        query: &str,
        filter: &HistoryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<HistoryHit>, HistoryError>;
    /// A session's entries from `from_sequence` (inclusive), in sequence order.
    fn entries(&self, session_id: &str, from_sequence: i64, limit: usize) -> Result<Vec<HistoryEntry>, HistoryError>;
    /// Remove a session, its entries and their index rows, for good.
    fn delete(&self, session_id: &str) -> Result<(), HistoryError>;
    /// Remove entries older than `before`, and sessions left empty that ended before it.
    fn prune(&self, before: SystemTime) -> Result<(), HistoryError>;
}

/// Microseconds since 1970 (negative before it).
pub fn to_micros(at: SystemTime) -> i64 {
    match at.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_micros()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_micros()).unwrap_or(i64::MAX),
    }
}

pub fn from_micros(micros: i64) -> SystemTime {
    if micros >= 0 {
        UNIX_EPOCH + Duration::from_micros(micros as u64)
    } else {
        UNIX_EPOCH - Duration::from_micros(micros.unsigned_abs())
    }
}

/// Whether `days` is one of the retention choices.
pub fn valid_retention(days: u32) -> bool {
    RETENTION_CHOICES.contains(&days)
}

/// The time before which entries expire under `days` of retention (`None`: forever).
pub fn retention_cutoff(days: u32, now: SystemTime) -> Option<SystemTime> {
    (days > 0).then(|| now - Duration::from_secs(u64::from(days) * 86_400))
}
