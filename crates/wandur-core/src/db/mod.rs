//! The client database, `wandur.db`, beside `settings.json` in the data directory.
//!
//! One SQLite file (rusqlite, SQLite compiled in, FTS5 included) in WAL mode with foreign keys on,
//! as the C# client's `ClientDatabase`. The schema is a list of numbered migrations applied in one
//! transaction; the file's `user_version` records how far it got. Reads open a short connection
//! of their own ([`Database::read`]); writes go through one writer thread ([`DbWriter`]), which
//! batches whatever is queued into one transaction so the UI thread never waits on the disk.
//!
//! A file SQLite cannot read is moved aside (`wandur.db.corrupt-<seconds>`) and a new one is made,
//! so the app always starts. A file from a newer client is left alone and the database is not
//! used in this run.
//!
//! - [`worlds`]: stable world ids, endpoint keys and connection usage.
//! - [`scripts`]: each world's script library (scripts and macros).
//! - [`writer`]: the batching writer thread.
//!
//! Saved maps (`crate::map::store`) keep their tables here too (migration 7) and are written by
//! the map worker's own thread.
//!
//! Agent profiles (`crate::agent::store`) keep one table here (migration 8), written on the
//! calling thread: once when a world gets its defaults, and on Save world.
//!
//! Session history (`crate::history`) keeps its own tables here (migration 4) and writes them
//! from each recorder's thread, not through [`DbWriter`].

pub mod scripts;
pub mod worlds;
pub mod writer;

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

pub use writer::{BatchSink, DbWriter, Job, Writer, WriterStats};

pub const DB_FILE: &str = "wandur.db";

/// The schema version this build writes. Each entry of [`MIGRATIONS`] brings a file from the
/// version before it to its own number.
pub const SCHEMA_VERSION: u32 = 9;

/// Numbered migrations, oldest first. Never edit a released one; add the next number instead.
pub const MIGRATIONS: &[(u32, &str)] = &[
    (
        1,
        // Worlds and the endpoints that lead to them. A world id survives an address edit: the
        // new endpoint is added for the same world and the old one stays as an alias.
        "CREATE TABLE worlds(id TEXT PRIMARY KEY NOT NULL);
         CREATE TABLE endpoints(
             endpoint_key TEXT PRIMARY KEY NOT NULL,
             world_id TEXT NOT NULL REFERENCES worlds(id));
         CREATE INDEX endpoints_world ON endpoints(world_id);",
    ),
    (
        2,
        // Connection usage per world (the C# WorldUsageStore): a summary row and a log of
        // connection times (seconds since 1970) for a recency-weighted order.
        "CREATE TABLE world_usage(
             world_id TEXT PRIMARY KEY NOT NULL REFERENCES worlds(id),
             connections INTEGER NOT NULL DEFAULT 0,
             last_connected_at INTEGER NOT NULL,
             last_character TEXT);
         CREATE TABLE world_connections(
             id INTEGER PRIMARY KEY,
             world_id TEXT NOT NULL REFERENCES worlds(id),
             connected_at INTEGER NOT NULL);
         CREATE INDEX world_connections_world ON world_connections(world_id);",
    ),
    (
        3,
        // Each world's script library, scripts and macros side by side, in the C# client's shape:
        // a macro keeps its definition as JSON beside the source generated from it.
        "CREATE TABLE scripts(
             world_id TEXT NOT NULL REFERENCES worlds(id),
             id TEXT NOT NULL,
             name TEXT NOT NULL,
             source TEXT NOT NULL,
             enabled INTEGER NOT NULL,
             macro_json TEXT,
             pack_json TEXT,
             PRIMARY KEY(world_id, id));",
    ),
    (
        4,
        // Session history, after the C# schema version 7: sessions, their entries, tombstones
        // of deleted sessions so a late write cannot bring one back, and an external-content
        // FTS5 index. Times are microseconds since 1970 (UTC). Three changes from C#, all for
        // the cost of recording at 1 MB/s (docs/measurements.md, Parity t07):
        // - no insert trigger: the store adds each new entry to the index itself, in the same
        //   transaction (FTS5 run from a trigger flushes its pending terms at every row, about
        //   five times the CPU);
        // - entries refer to their session by an integer key, not its 32-character id (a third
        //   less to write per line, in the row and in its unique index);
        // - the index keeps no column sizes (`columnsize=0`: nothing ranks by relevance).
        // Updates and deletes (rare) keep their triggers.
        "CREATE TABLE history_sessions(
             key INTEGER PRIMARY KEY,
             id TEXT NOT NULL UNIQUE,
             world_key TEXT NOT NULL,
             world_name TEXT NOT NULL,
             character_name TEXT NOT NULL,
             started_at INTEGER NOT NULL,
             ended_at INTEGER);
         CREATE INDEX history_sessions_started ON history_sessions(started_at DESC, id);
         CREATE TABLE history_deletions(session_id TEXT PRIMARY KEY NOT NULL);
         CREATE TABLE history_entries(
             id INTEGER PRIMARY KEY,
             session INTEGER NOT NULL REFERENCES history_sessions(key) ON DELETE CASCADE,
             sequence INTEGER NOT NULL CHECK(sequence >= 0),
             at INTEGER NOT NULL,
             kind TEXT NOT NULL CHECK(kind IN ('received', 'sent', 'script', 'private')),
             text TEXT NOT NULL,
             UNIQUE(session, sequence));
         CREATE INDEX history_entries_at ON history_entries(at DESC, id DESC);
         CREATE VIRTUAL TABLE history_entries_fts USING fts5(
             text, content='history_entries', content_rowid='id', columnsize=0);
         CREATE TRIGGER history_entries_delete AFTER DELETE ON history_entries BEGIN
             INSERT INTO history_entries_fts(history_entries_fts, rowid, text) VALUES('delete', old.id, old.text);
         END;
         CREATE TRIGGER history_entries_update AFTER UPDATE ON history_entries BEGIN
             INSERT INTO history_entries_fts(history_entries_fts, rowid, text) VALUES('delete', old.id, old.text);
             INSERT INTO history_entries_fts(rowid, text) VALUES(new.id, new.text);
         END;",
    ),
    (
        5,
        // One-time steps already done, by key (the C# `imports` table): `script-library:<world>`
        // marks a world whose library was set up, so its starter script is added only once.
        "CREATE TABLE imports(key TEXT PRIMARY KEY NOT NULL);",
    ),
    (
        6,
        // The C# schema versions 8 and 9: where an imported script came from with the original
        // items it could not convert, and each script's language and compatibility layer. NULL
        // is JavaScript with no layer, as every script before Lua.
        "ALTER TABLE scripts ADD COLUMN import_json TEXT;
         ALTER TABLE scripts ADD COLUMN language TEXT;
         ALTER TABLE scripts ADD COLUMN compatibility TEXT;",
    ),
    (
        7,
        // Saved maps (the C# map tables): per world, the map's own fields as JSON, then rooms,
        // exits, area settings, room aliases and the tombstones of deleted rooms and exits.
        // Rooms and exits keep their whole record as JSON beside the columns used for lookups.
        "CREATE TABLE map_snapshots(
             world_id TEXT PRIMARY KEY NOT NULL REFERENCES worlds(id),
             payload TEXT NOT NULL);
         CREATE TABLE map_rooms(
             world_id TEXT NOT NULL REFERENCES worlds(id),
             room_id TEXT NOT NULL,
             area TEXT,
             x REAL NOT NULL, y REAL NOT NULL, z REAL NOT NULL,
             payload TEXT NOT NULL,
             PRIMARY KEY(world_id, room_id));
         CREATE INDEX map_rooms_coordinates ON map_rooms(world_id, area, z, x, y);
         CREATE TABLE map_links(
             world_id TEXT NOT NULL REFERENCES worlds(id),
             from_id TEXT NOT NULL,
             direction TEXT NOT NULL,
             to_id TEXT NOT NULL,
             payload TEXT NOT NULL,
             PRIMARY KEY(world_id, from_id, direction));
         CREATE INDEX map_links_destination ON map_links(world_id, to_id);
         CREATE TABLE map_areas(
             world_id TEXT NOT NULL REFERENCES worlds(id),
             area TEXT NOT NULL,
             payload TEXT NOT NULL,
             PRIMARY KEY(world_id, area));
         CREATE TABLE map_room_aliases(
             world_id TEXT NOT NULL REFERENCES worlds(id),
             source_id TEXT NOT NULL,
             target_id TEXT NOT NULL,
             revision INTEGER NOT NULL,
             PRIMARY KEY(world_id, source_id));
         CREATE TABLE map_room_deletions(
             world_id TEXT NOT NULL REFERENCES worlds(id),
             room_id TEXT NOT NULL,
             revision INTEGER NOT NULL,
             PRIMARY KEY(world_id, room_id));
         CREATE TABLE map_link_deletions(
             world_id TEXT NOT NULL REFERENCES worlds(id),
             from_id TEXT NOT NULL,
             direction TEXT NOT NULL,
             revision INTEGER NOT NULL,
             PRIMARY KEY(world_id, from_id, direction));",
    ),
    (
        8,
        // Agent profiles (the C# schema version 3): one JSON payload per world.
        "CREATE TABLE world_agent_profiles(
             world_id TEXT PRIMARY KEY NOT NULL REFERENCES worlds(id),
             payload TEXT NOT NULL);",
    ),
    (
        9,
        // Map labels (text or a picture) and their tombstones, and the pictures they show: PNG
        // or JPEG bytes per world, keyed by their SHA-256, stored once however many labels show
        // them. A label keeps its whole record as JSON; `image` is the picture's hash.
        "CREATE TABLE map_labels(
             world_id TEXT NOT NULL REFERENCES worlds(id),
             label_id TEXT NOT NULL,
             area TEXT,
             z REAL NOT NULL,
             image TEXT,
             payload TEXT NOT NULL,
             PRIMARY KEY(world_id, label_id));
         CREATE TABLE map_label_deletions(
             world_id TEXT NOT NULL REFERENCES worlds(id),
             label_id TEXT NOT NULL,
             revision INTEGER NOT NULL,
             PRIMARY KEY(world_id, label_id));
         CREATE TABLE map_images(
             world_id TEXT NOT NULL REFERENCES worlds(id),
             hash TEXT NOT NULL,
             bytes BLOB NOT NULL,
             PRIMARY KEY(world_id, hash));",
    ),
];

#[derive(Debug)]
pub enum DbError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    /// The file was written by a newer client.
    TooNew {
        found: u32,
        supported: u32,
    },
    /// The endpoint already belongs to another world.
    WorldConflict {
        endpoint: String,
        world: String,
    },
    /// Not an address a world can have.
    InvalidEndpoint(String),
    /// Data that fails its checks (a script library entry); the message is for the person.
    Invalid(String),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::Sqlite(e) => write!(f, "database error: {e}"),
            DbError::Io(e) => write!(f, "database file error: {e}"),
            DbError::TooNew { found, supported } => write!(
                f,
                "the database is from a newer Wandur (schema {found}, this one knows {supported})"
            ),
            DbError::WorldConflict { endpoint, world } => {
                write!(f, "{endpoint} already belongs to another world ({world})")
            }
            DbError::InvalidEndpoint(key) => write!(f, "{key:?} is not a world address"),
            DbError::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for DbError {}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError::Sqlite(e)
    }
}

impl From<std::io::Error> for DbError {
    fn from(e: std::io::Error) -> Self {
        DbError::Io(e)
    }
}

impl DbError {
    /// Whether the file itself is unreadable (not a database, or damaged).
    fn is_corrupt(&self) -> bool {
        matches!(
            self,
            DbError::Sqlite(rusqlite::Error::SqliteFailure(e, _))
                if matches!(e.code, rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt)
        )
    }
}

/// The database file. Cheap to clone; every read opens its own short connection.
#[derive(Clone, Debug)]
pub struct Database {
    path: PathBuf,
}

impl Database {
    /// Open (creating or migrating) `wandur.db` in `dir`. A damaged file is moved aside and
    /// replaced; the message says so, for the person.
    pub fn open(dir: &Path) -> Result<(Database, Option<String>), DbError> {
        std::fs::create_dir_all(dir)?;
        let db = Database {
            path: dir.join(DB_FILE),
        };
        match db.initialize() {
            Ok(()) => Ok((db, None)),
            Err(e) if e.is_corrupt() => {
                let aside = move_aside(&db.path)?;
                db.initialize()?;
                Ok((
                    db,
                    Some(crate::l10n::tf(
                        crate::l10n::S::DatabaseUnreadable,
                        &[&e, &aside.display()],
                    )),
                ))
            }
            Err(e) => Err(e),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A new connection with the client's settings (foreign keys on, a busy timeout).
    pub fn connect(&self) -> Result<Connection, DbError> {
        let conn = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        Ok(conn)
    }

    /// Run a read on a short connection of its own.
    pub fn read<T>(&self, read: impl FnOnce(&Connection) -> Result<T, DbError>) -> Result<T, DbError> {
        let conn = self.connect()?;
        read(&conn)
    }

    /// Run a write now, in one immediate transaction, on the calling thread. The app writes
    /// through [`Database::writer`]; this is for start-up and tests.
    pub fn write<T>(&self, write: impl FnOnce(&Transaction) -> Result<T, DbError>) -> Result<T, DbError> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = write(&tx)?;
        tx.commit()?;
        Ok(value)
    }

    /// Start the writer thread.
    pub fn writer(&self) -> Result<DbWriter, DbError> {
        let conn = self.connect()?;
        Ok(Writer::spawn("wandur-db-writer", writer::SqliteSink::new(conn))?)
    }

    /// The schema version recorded in the file.
    pub fn schema_version(&self) -> Result<u32, DbError> {
        self.read(|c| Ok(user_version(c)?))
    }

    fn initialize(&self) -> Result<(), DbError> {
        let mut conn = self.connect()?;
        // Touches the file header: a file that is not a database fails here.
        let found = user_version(&conn)?;
        if found > SCHEMA_VERSION {
            return Err(DbError::TooNew {
                found,
                supported: SCHEMA_VERSION,
            });
        }
        let mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        debug_assert!(mode.eq_ignore_ascii_case("wal"), "journal mode {mode}");
        let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(DbError::Sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
                Some(check),
            )));
        }
        migrate(&mut conn, SCHEMA_VERSION)
    }
}

fn user_version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

/// Apply the migrations up to `target` in one transaction. Another process may have migrated
/// while this one waited for the write lock, so the version is read again inside it.
pub fn migrate(conn: &mut Connection, target: u32) -> Result<(), DbError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let found = user_version(&tx)?;
    if found > SCHEMA_VERSION {
        return Err(DbError::TooNew {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    for (version, sql) in MIGRATIONS {
        if *version > found && *version <= target {
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", version)?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Rename a damaged database (and its WAL sidecars) out of the way; returns the new name.
fn move_aside(path: &Path) -> std::io::Result<PathBuf> {
    let stamp = crate::settings::unix_now();
    let mut aside = path.with_file_name(format!("{DB_FILE}.corrupt-{stamp}"));
    let mut n = 1;
    while aside.exists() {
        aside = path.with_file_name(format!("{DB_FILE}.corrupt-{stamp}-{n}"));
        n += 1;
    }
    std::fs::rename(path, &aside)?;
    for sidecar in ["-wal", "-shm"] {
        let mut from = path.as_os_str().to_owned();
        from.push(sidecar);
        let mut to = aside.as_os_str().to_owned();
        to.push(sidecar);
        let _ = std::fs::rename(PathBuf::from(from), PathBuf::from(to));
    }
    Ok(aside)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wandur-db-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn tables(db: &Database) -> Vec<String> {
        db.read(|c| {
            let mut s = c.prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
            let names = s.query_map([], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
            Ok(names)
        })
        .unwrap()
    }

    #[test]
    fn migrates_an_empty_directory_to_the_current_schema() {
        let dir = temp_dir("empty");
        let (db, note) = Database::open(&dir).unwrap();
        assert!(note.is_none());
        assert!(dir.join(DB_FILE).exists());
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        let tables = tables(&db);
        for table in [
            "endpoints",
            "history_deletions",
            "history_entries",
            "history_entries_fts",
            "history_sessions",
            "imports",
            "map_images",
            "map_label_deletions",
            "map_labels",
            "map_link_deletions",
            "map_links",
            "map_rooms",
            "map_snapshots",
            "scripts",
            "world_agent_profiles",
            "world_connections",
            "world_usage",
            "worlds",
        ] {
            assert!(tables.iter().any(|t| t == table), "{table} missing from {tables:?}");
        }
        let (journal, fk): (String, bool) = db
            .read(|c| {
                Ok((
                    c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?,
                    c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?,
                ))
            })
            .unwrap();
        assert_eq!(journal, "wal");
        assert!(fk);
        // Opening again is a no-op.
        let (db, _) = Database::open(&dir).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrates_a_version_1_file_and_keeps_its_rows() {
        let dir = temp_dir("v1");
        std::fs::create_dir_all(&dir).unwrap();
        {
            let mut conn = Connection::open(dir.join(DB_FILE)).unwrap();
            migrate(&mut conn, 1).unwrap();
            conn.execute_batch(
                "INSERT INTO worlds(id) VALUES('w1');
                 INSERT INTO endpoints(endpoint_key, world_id) VALUES('mud.example.org:4000', 'w1');",
            )
            .unwrap();
            assert_eq!(user_version(&conn).unwrap(), 1);
        }
        let (db, note) = Database::open(&dir).unwrap();
        assert!(note.is_none());
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(
            worlds::find_world_key(&db.connect().unwrap(), "mud.example.org:4000").unwrap(),
            Some("w1".into())
        );
        // The new tables work and reference the old rows.
        db.write(|tx| worlds::record_connection(tx, "w1", 100)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let dir = temp_dir("fk");
        let (db, _) = Database::open(&dir).unwrap();
        let err = db
            .write(|tx| {
                tx.execute(
                    "INSERT INTO endpoints(endpoint_key, world_id) VALUES('h:1', 'missing')",
                    [],
                )?;
                Ok(())
            })
            .unwrap_err();
        assert!(err.to_string().contains("FOREIGN KEY"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_is_moved_aside_and_a_new_one_started() {
        let dir = temp_dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(DB_FILE), vec![0x5a; 8192]).unwrap();
        let (db, note) = Database::open(&dir).unwrap();
        let note = note.expect("a message for the person");
        assert!(note.contains("unreadable"), "{note}");
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        let aside: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("wandur.db.corrupt-"))
            .collect();
        assert_eq!(aside.len(), 1, "{aside:?}");
        assert_eq!(std::fs::read(dir.join(&aside[0])).unwrap(), vec![0x5a; 8192]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_from_a_newer_client_is_left_alone() {
        let dir = temp_dir("newer");
        std::fs::create_dir_all(&dir).unwrap();
        {
            let conn = Connection::open(dir.join(DB_FILE)).unwrap();
            conn.pragma_update(None, "user_version", SCHEMA_VERSION + 5).unwrap();
        }
        let err = Database::open(&dir).unwrap_err();
        assert!(matches!(err, DbError::TooNew { .. }), "{err}");
        assert!(dir.join(DB_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn full_text_search_is_compiled_in() {
        // Session history (t07) needs FTS5 from the bundled SQLite.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE t USING fts5(text);
             INSERT INTO t(text) VALUES('the lantern road'), ('a marsh wisp');",
        )
        .unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM t WHERE t MATCH 'lantern'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }
}
