//! Import of the C# Wandur client's data (`docs/csharp-import.md` maps every table).
//!
//! The C# client keeps everything in one SQLite file, `wandur.db`, in its data directory
//! (`~/Library/Application Support/Wandur` on macOS): saved worlds and settings as JSON payloads,
//! world ids and addresses, usage, script libraries with macros, agent profiles, maps and
//! session history. This module reads it strictly read-only:
//!
//! 1. [`CsharpSource::open`] copies `wandur.db` (and its `-wal`, which can hold the newest
//!    writes) into a private snapshot directory; the C# directory itself is never opened for
//!    writing, not even by SQLite (which would otherwise create its `-shm` file there).
//! 2. The snapshot's write-ahead log is folded into the copy, then the copy is opened with
//!    `SQLITE_OPEN_READ_ONLY` and `immutable=1`.
//! 3. The schema version is checked: newer than [`MAX_SCHEMA`] is refused with a clear message.
//!
//! [`import`] merges what it read into this client's data in one transaction: all or nothing.
//! It never duplicates: worlds keep the C# world ids (or the id this client already has for the
//! address), scripts and agent profiles keep their ids, maps merge by revision, history sessions
//! by id. What this client already has in another version is kept and reported as skipped, so
//! running the import again gives the same result.
//!
//! Saved passwords and agent API keys live in the system credential store, not in the database.
//! [`secrets`] copies them from the C# client's entries to this client's, and only when asked.

mod apply;
pub mod secrets;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::l10n::{S, t, tf};

pub use apply::{ImportOptions, Outcome, Progress, import, import_into_dir};

/// The newest C# schema this importer reads: 7 on the C# main branch, 8 and 9 on the Mudlet
/// import branch (each script's import record, language and layer).
pub const MAX_SCHEMA: u32 = 9;
/// The C# database's file name.
pub const DB_FILE: &str = "wandur.db";
/// The C# client's data directory name under the platform's application data directory.
pub const CSHARP_APP_DIR: &str = "Wandur";

/// The C# client's data directory on this machine, if the platform has one.
pub fn default_csharp_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library").join("Application Support"))
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        // .NET's ApplicationData on Linux: $XDG_CONFIG_HOME, else ~/.config.
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|h| h.join(".config")))
    };
    base.map(|b| b.join(CSHARP_APP_DIR))
}

/// Why an import could not run. The messages name no file contents, hosts or paths.
#[derive(Debug)]
pub enum ImportError {
    /// No `wandur.db` where the person pointed.
    NotFound,
    /// A file that is not a C# client database.
    NotWandur,
    /// A database from a newer C# client.
    TooNew { found: u32 },
    /// The C# database could not be copied or read.
    Unreadable(String),
    /// This client's data could not be written; nothing was changed.
    Target(String),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::NotFound => f.write_str(t(S::CsImportNotFound)),
            ImportError::NotWandur => f.write_str(t(S::CsImportNotWandur)),
            ImportError::TooNew { found } => f.write_str(&tf(S::CsImportTooNew, &[found, &MAX_SCHEMA])),
            ImportError::Unreadable(e) => f.write_str(&tf(S::CsImportUnreadable, &[e])),
            ImportError::Target(e) => f.write_str(&tf(S::CsImportTargetFailed, &[e])),
        }
    }
}

impl std::error::Error for ImportError {}

fn unreadable(e: impl fmt::Display) -> ImportError {
    ImportError::Unreadable(e.to_string())
}

/// What a report counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Worlds,
    Addresses,
    Usage,
    Preferences,
    ColorSchemes,
    OtherSettings,
    Scripts,
    Macros,
    ChannelRules,
    AgentProfiles,
    Maps,
    Rooms,
    Exits,
    HistorySessions,
    HistoryEntries,
    Passwords,
    ApiKeys,
    Other,
}

impl Kind {
    pub const ALL: [Kind; 18] = [
        Kind::Worlds,
        Kind::Addresses,
        Kind::Usage,
        Kind::Preferences,
        Kind::ColorSchemes,
        Kind::OtherSettings,
        Kind::Scripts,
        Kind::Macros,
        Kind::ChannelRules,
        Kind::AgentProfiles,
        Kind::Maps,
        Kind::Rooms,
        Kind::Exits,
        Kind::HistorySessions,
        Kind::HistoryEntries,
        Kind::Passwords,
        Kind::ApiKeys,
        Kind::Other,
    ];

    pub fn label(self) -> &'static str {
        t(match self {
            Kind::Worlds => S::CsImportKindWorlds,
            Kind::Addresses => S::CsImportKindAddresses,
            Kind::Usage => S::CsImportKindUsage,
            Kind::Preferences => S::CsImportKindPreferences,
            Kind::ColorSchemes => S::CsImportKindColorSchemes,
            Kind::OtherSettings => S::CsImportKindOtherSettings,
            Kind::Scripts => S::CsImportKindScripts,
            Kind::Macros => S::CsImportKindMacros,
            Kind::ChannelRules => S::CsImportKindChannelRules,
            Kind::AgentProfiles => S::CsImportKindAgentProfiles,
            Kind::Maps => S::CsImportKindMaps,
            Kind::Rooms => S::CsImportKindRooms,
            Kind::Exits => S::CsImportKindExits,
            Kind::HistorySessions => S::CsImportKindHistorySessions,
            Kind::HistoryEntries => S::CsImportKindHistoryEntries,
            Kind::Passwords => S::CsImportKindPasswords,
            Kind::ApiKeys => S::CsImportKindApiKeys,
            Kind::Other => S::CsImportKindOther,
        })
    }
}

/// Why an item was not imported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Skip {
    /// This client has its own, different version; it was kept.
    KeptRust,
    /// The item fails this client's checks.
    Invalid,
    /// A channel rule pattern this client's regular expressions cannot run.
    PatternSyntax,
    /// The address belongs to another world here.
    AddressTaken,
    /// Deleted in this client (a history session), so it stays deleted.
    Deleted,
    /// The world's script library here has no room left.
    LibraryFull,
    /// The C# credential store has nothing under the item's key.
    NotInVault,
    /// A credential store failed.
    VaultFailed,
    /// Secrets were not asked for (the command line without `--include-passwords`).
    PasswordsNotRequested,
    /// The C# install id: each client keeps its own.
    InstallId,
    /// The C# update check record and skipped version: they are about C# releases.
    CsharpReleases,
    /// No counterpart in this client.
    NoCounterpart,
}

impl Skip {
    pub fn label(self) -> &'static str {
        t(match self {
            Skip::KeptRust => S::CsImportSkipKeptRust,
            Skip::Invalid => S::CsImportSkipInvalid,
            Skip::PatternSyntax => S::CsImportSkipPatternSyntax,
            Skip::AddressTaken => S::CsImportSkipAddressTaken,
            Skip::Deleted => S::CsImportSkipDeleted,
            Skip::LibraryFull => S::CsImportSkipLibraryFull,
            Skip::NotInVault => S::CsImportSkipNotInVault,
            Skip::VaultFailed => S::CsImportSkipVaultFailed,
            Skip::PasswordsNotRequested => S::CsImportSkipPasswordsNotRequested,
            Skip::InstallId => S::CsImportSkipInstallId,
            Skip::CsharpReleases => S::CsImportSkipCsharpReleases,
            Skip::NoCounterpart => S::CsImportSkipNoCounterpart,
        })
    }
}

/// Counts for one kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// In the C# data.
    pub found: u64,
    /// Added (or merged in) by this run.
    pub imported: u64,
    /// Already here, the same.
    pub unchanged: u64,
    /// Not imported, by reason.
    pub skipped: BTreeMap<Skip, u64>,
}

impl Tally {
    pub fn skipped_total(&self) -> u64 {
        self.skipped.values().sum()
    }
}

/// What an import found and did: counts per kind and skip reasons. It never holds names,
/// addresses, text or any other content, so it can be printed anywhere.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The C# schema version read.
    pub schema: u32,
    /// Nothing was written.
    pub dry_run: bool,
    pub tallies: BTreeMap<Kind, Tally>,
}

impl Report {
    pub fn tally(&self, kind: Kind) -> Tally {
        self.tallies.get(&kind).cloned().unwrap_or_default()
    }

    pub(crate) fn entry(&mut self, kind: Kind) -> &mut Tally {
        self.tallies.entry(kind).or_default()
    }

    pub(crate) fn found(&mut self, kind: Kind, n: u64) {
        self.entry(kind).found += n;
    }

    pub(crate) fn imported(&mut self, kind: Kind, n: u64) {
        self.entry(kind).imported += n;
    }

    pub(crate) fn unchanged(&mut self, kind: Kind, n: u64) {
        self.entry(kind).unchanged += n;
    }

    pub(crate) fn skip(&mut self, kind: Kind, reason: Skip, n: u64) {
        if n > 0 {
            *self.entry(kind).skipped.entry(reason).or_default() += n;
        }
    }

    /// The summary, one line per kind with anything found, then a line per skip reason.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![tf(S::CsImportSchema, &[&self.schema])];
        for kind in Kind::ALL {
            let Some(tally) = self.tallies.get(&kind).filter(|t| t.found > 0 || t.skipped_total() > 0) else {
                continue;
            };
            lines.push(tf(
                S::CsImportLine,
                &[&kind.label(), &tally.found, &tally.imported, &tally.unchanged],
            ));
            for (reason, n) in &tally.skipped {
                lines.push(format!("    {}", tf(S::CsImportSkipLine, &[n, &reason.label()])));
            }
        }
        if self.dry_run {
            lines.push(t(S::CsImportDryRun).to_string());
        }
        lines
    }

    pub fn to_text(&self) -> String {
        self.lines().join("\n")
    }
}

/// The C# database, copied into a private snapshot and opened read-only. The snapshot is
/// removed when this is dropped.
pub struct CsharpSource {
    conn: Connection,
    schema: u32,
    snapshot: PathBuf,
    /// The C# data directory (where `wandur.db` is), for the older files it may still hold.
    dir: PathBuf,
    /// Optional `scripts` columns, by name, present in this schema.
    script_columns: Vec<String>,
    has_history: bool,
}

impl fmt::Debug for CsharpSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CsharpSource").field("schema", &self.schema).finish()
    }
}

/// The database file for what the person picked: a data directory (its `wandur.db`) or the
/// file itself.
pub fn locate(path: &Path) -> Result<PathBuf, ImportError> {
    let file = if path.is_dir() {
        path.join(DB_FILE)
    } else {
        path.to_path_buf()
    };
    if file.is_file() {
        Ok(file)
    } else {
        Err(ImportError::NotFound)
    }
}

fn snapshot_dir() -> std::io::Result<PathBuf> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "wandur-csharp-import-{}-{n}-{}",
        std::process::id(),
        crate::settings::unix_now()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Copy `from` to `to` by reading it (the source is only ever opened for reading).
fn copy_read_only(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut source = std::fs::File::open(from)?;
    let mut target = std::fs::File::create(to)?;
    std::io::copy(&mut source, &mut target)?;
    target.sync_all()
}

impl CsharpSource {
    /// Snapshot and open the C# database at `path` (a data directory or a `wandur.db`).
    pub fn open(path: &Path) -> Result<CsharpSource, ImportError> {
        let file = locate(path)?;
        let snapshot = snapshot_dir().map_err(unreadable)?;
        match Self::open_snapshot(&file, &snapshot) {
            Ok(source) => Ok(source),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&snapshot);
                Err(e)
            }
        }
    }

    fn open_snapshot(file: &Path, snapshot: &Path) -> Result<CsharpSource, ImportError> {
        let copy = snapshot.join(DB_FILE);
        copy_read_only(file, &copy).map_err(unreadable)?;
        // The write-ahead log can hold the newest writes; the shared-memory index is rebuilt.
        let mut wal = file.as_os_str().to_owned();
        wal.push("-wal");
        let wal = PathBuf::from(wal);
        if wal.is_file() {
            copy_read_only(&wal, &snapshot.join(format!("{DB_FILE}-wal"))).map_err(unreadable)?;
        }
        // Fold the log into the copy (this writes the snapshot only), so the immutable open
        // below sees every committed write.
        {
            let conn = Connection::open_with_flags(
                &copy,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|_| ImportError::NotWandur)?;
            let version: u32 = conn
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .map_err(|_| ImportError::NotWandur)?;
            if version > MAX_SCHEMA {
                return Err(ImportError::TooNew { found: version });
            }
            let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
            let _: String = conn
                .query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))
                .map_err(unreadable)?;
        }
        let uri = format!("file:{}?immutable=1", uri_path(&copy));
        let conn = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(unreadable)?;
        let check: String = conn
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(|_| ImportError::NotWandur)?;
        if check != "ok" {
            return Err(ImportError::Unreadable(check));
        }
        let schema: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(unreadable)?;
        if schema > MAX_SCHEMA {
            return Err(ImportError::TooNew { found: schema });
        }
        let tables = table_names(&conn).map_err(unreadable)?;
        let required = ["worlds", "endpoints", "client_settings", "profiles", "scripts"];
        if schema == 0 || !required.iter().all(|t| tables.iter().any(|n| n == t)) {
            return Err(ImportError::NotWandur);
        }
        let script_columns = column_names(&conn, "scripts").map_err(unreadable)?;
        let has_history = ["history_sessions", "history_entries", "history_deletions"]
            .iter()
            .all(|t| tables.iter().any(|n| n == t));
        Ok(CsharpSource {
            conn,
            schema,
            snapshot: snapshot.to_path_buf(),
            dir: file.parent().map(Path::to_path_buf).unwrap_or_default(),
            script_columns,
            has_history,
        })
    }

    /// The C# schema version.
    pub fn schema(&self) -> u32 {
        self.schema
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    pub(crate) fn has_table(&self, name: &str) -> bool {
        table_names(&self.conn).is_ok_and(|t| t.iter().any(|n| n == name))
    }

    pub(crate) fn has_script_column(&self, name: &str) -> bool {
        self.script_columns.iter().any(|c| c == name)
    }

    pub(crate) fn has_history(&self) -> bool {
        self.has_history
    }

    /// Every key the C# client looked a world's older files up by: its endpoints and legacy
    /// spellings (an IPv6 address also without brackets), by C# world id.
    fn world_keys(&self) -> Result<BTreeMap<String, Vec<String>>, ImportError> {
        let mut keys: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut sql = "SELECT world_id, endpoint_key FROM endpoints".to_string();
        if self.has_table("legacy_endpoints") {
            sql.push_str(" UNION SELECT world_id, source_key FROM legacy_endpoints");
        }
        let mut s = self.conn.prepare(&sql).map_err(unreadable)?;
        let rows = s
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(unreadable)?;
        for row in rows {
            let (world, key) = row.map_err(unreadable)?;
            let list = keys.entry(world).or_default();
            if key.starts_with('[') {
                let bare = key.replace(['[', ']'], "");
                if !list.contains(&bare) {
                    list.push(bare);
                }
            }
            if !list.contains(&key) {
                list.push(key);
            }
        }
        Ok(keys)
    }

    fn imported_markers(&self) -> Result<Vec<String>, ImportError> {
        let mut s = self.conn.prepare("SELECT key FROM imports").map_err(unreadable)?;
        let rows = s.query_map([], |r| r.get::<_, String>(0)).map_err(unreadable)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(unreadable)
    }

    /// Older per-address map files (`maps/<SHA-256 of the address>.json`) the C# client has not
    /// moved into its database yet (it does so the next time it opens that world's map), by C#
    /// world id. Only read, never changed.
    pub(crate) fn legacy_maps(&self) -> Result<Vec<(String, PathBuf)>, ImportError> {
        let markers = self.imported_markers()?;
        let mut found = Vec::new();
        for (world, keys) in self.world_keys()? {
            for key in keys {
                let name = format!("{}.json", sha256_upper(&key));
                let path = self.dir.join("maps").join(&name);
                let done = markers.iter().any(|m| {
                    m.starts_with("map-file:")
                        && (m.ends_with(&format!("/maps/{name}")) || m.ends_with(&format!("\\maps\\{name}")))
                });
                if !done && path.is_file() && !found.iter().any(|(_, p)| *p == path) {
                    found.push((world.clone(), path));
                }
            }
        }
        Ok(found)
    }

    /// Older per-address script files (`scripts/<SHA-256>.scripts.json`, or a single
    /// `scripts/<SHA-256>.js`) the C# client has not moved into its database yet, by C# world id:
    /// (world, the file's hash stem, the file).
    pub(crate) fn legacy_scripts(&self) -> Result<Vec<(String, String, PathBuf)>, ImportError> {
        let markers = self.imported_markers()?;
        let mut found = Vec::new();
        for (world, keys) in self.world_keys()? {
            let mut candidates = Vec::new();
            for key in keys {
                if key != "demo" {
                    candidates.push(format!("{key}:False"));
                    candidates.push(format!("{key}:True"));
                }
                candidates.push(key);
            }
            for key in candidates {
                let stem = sha256_upper(&key);
                if markers.iter().any(|m| *m == format!("script-source:{stem}")) {
                    continue;
                }
                let library = self.dir.join("scripts").join(format!("{stem}.scripts.json"));
                let single = self.dir.join("scripts").join(format!("{stem}.js"));
                let path = if library.is_file() {
                    library
                } else if single.is_file() {
                    single
                } else {
                    continue;
                };
                if !found.iter().any(|(_, s, _): &(String, String, PathBuf)| *s == stem) {
                    found.push((world.clone(), stem, path));
                }
            }
        }
        Ok(found)
    }

    /// What the C# data holds, counted (the import's `found` column), without importing.
    pub fn found(&self) -> Result<Report, ImportError> {
        let mut report = Report {
            schema: self.schema,
            ..Report::default()
        };
        let count = |sql: &str| -> Result<u64, ImportError> {
            self.conn
                .query_row(sql, [], |r| r.get::<_, i64>(0))
                .map(|n| n.max(0) as u64)
                .map_err(unreadable)
        };
        report.found(Kind::Worlds, count("SELECT COUNT(*) FROM profiles")?);
        report.found(Kind::Addresses, count("SELECT COUNT(*) FROM endpoints")?);
        if self.has_table("world_connections") {
            report.found(Kind::Usage, count("SELECT COUNT(*) FROM world_connections")?);
        }
        report.found(
            Kind::Preferences,
            count("SELECT COUNT(*) FROM client_settings WHERE id = 1")?,
        );
        let macros = if self.has_script_column("macro_json") {
            count("SELECT COUNT(*) FROM scripts WHERE macro_json IS NOT NULL")?
        } else {
            0
        };
        report.found(Kind::Scripts, count("SELECT COUNT(*) FROM scripts")? - macros);
        // Older script files hold one script or a library each; counted as files here.
        report.found(Kind::Scripts, self.legacy_scripts()?.len() as u64);
        report.found(Kind::Macros, macros);
        if self.has_table("world_agent_profiles") {
            report.found(Kind::AgentProfiles, count("SELECT COUNT(*) FROM world_agent_profiles")?);
        }
        report.found(Kind::Maps, self.legacy_maps()?.len() as u64);
        if self.has_table("map_snapshots") {
            report.found(Kind::Maps, count("SELECT COUNT(*) FROM map_snapshots")?);
            report.found(Kind::Rooms, count("SELECT COUNT(*) FROM map_rooms")?);
            report.found(Kind::Exits, count("SELECT COUNT(*) FROM map_links")?);
        }
        if self.has_history {
            report.found(Kind::HistorySessions, count("SELECT COUNT(*) FROM history_sessions")?);
            report.found(Kind::HistoryEntries, count("SELECT COUNT(*) FROM history_entries")?);
        }
        Ok(report)
    }
}

impl Drop for CsharpSource {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.snapshot);
    }
}

/// A path for a SQLite `file:` URI: `?`, `#` and `%` escaped.
fn uri_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '%' => out.push_str("%25"),
            '?' => out.push_str("%3f"),
            '#' => out.push_str("%23"),
            _ => out.push(c),
        }
    }
    if cfg!(windows) && !out.starts_with('/') {
        out.insert(0, '/');
    }
    out
}

/// SHA-256 of `text` (UTF-8) in upper-case hex, as the C# client names its older files.
fn sha256_upper(text: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

fn table_names(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut s = conn.prepare("SELECT name FROM sqlite_master WHERE type IN ('table', 'view')")?;
    s.query_map([], |r| r.get(0))?.collect()
}

fn column_names(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut s = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    s.query_map([], |r| r.get(1))?.collect()
}
