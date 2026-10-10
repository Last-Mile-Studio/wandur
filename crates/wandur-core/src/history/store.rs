//! Session history in `wandur.db` (the C# `SqliteHistoryStore`): every call opens a short
//! connection of its own, writes run in one immediate transaction, reads return at most
//! [`MAX_PAGE`] rows. Search takes the query literally: words and double-quoted phrases, all
//! required, with FTS operators and wildcards quoted as plain text.

use std::sync::Mutex;
use std::time::SystemTime;

use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, Row, ToSql, params};

use super::{
    EntryKind, HistoryEntry, HistoryError, HistoryFilter, HistoryHit, HistorySession, HistoryStore, MAX_PAGE,
    MAX_QUERY, from_micros, to_micros,
};
use crate::db::Database;

/// Entries written per INSERT statement (fewer statements, the same rows).
const INSERT_ROWS: usize = 64;

/// `INSERT ... VALUES (...), (...)` for `rows` entries, skipping sequences already stored.
fn insert_sql(rows: usize) -> String {
    let values = vec!["(?, ?, ?, ?, ?)"; rows].join(", ");
    format!(
        "INSERT INTO history_entries(session, sequence, at, kind, text) VALUES {values}
         ON CONFLICT(session, sequence) DO NOTHING"
    )
}

const SESSION_COLUMNS: &str = "s.id, s.world_key, s.world_name, s.character_name, s.started_at, s.ended_at";

/// History over the client database.
#[derive(Debug)]
pub struct SqliteHistoryStore {
    db: Database,
    /// The connection writes reuse, so its page cache and prepared statements stay warm while a
    /// session records (a new connection per batch reread the indexes every time). Dropped after
    /// an error; reads always open a short connection of their own.
    writer: Mutex<Option<Connection>>,
}

impl SqliteHistoryStore {
    pub fn new(db: Database) -> Self {
        Self {
            db,
            writer: Mutex::new(None),
        }
    }

    pub fn database(&self) -> &Database {
        &self.db
    }

    fn read<T>(&self, read: impl FnOnce(&Connection) -> Result<T, HistoryError>) -> Result<T, HistoryError> {
        let conn = self.db.connect()?;
        read(&conn)
    }

    /// Run `write` in one immediate transaction on the write connection. `foreign_keys` off is
    /// for appends only: their session row is written first in the same transaction, and a check
    /// of it per line was half the cost of recording. Delete and prune keep it on (the cascade).
    fn write<T>(
        &self,
        foreign_keys: bool,
        write: impl FnOnce(&Connection) -> Result<T, HistoryError>,
    ) -> Result<T, HistoryError> {
        let mut slot = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let conn = match &mut *slot {
            Some(conn) => conn,
            empty => {
                let conn = self.db.connect()?;
                // Losing the last moments of text in a power cut is accepted (the C# rule for a
                // crash); a full sync per batch is not needed for that, and WAL keeps the file
                // consistent.
                conn.pragma_update(None, "synchronous", "NORMAL")?;
                empty.insert(conn)
            }
        };
        let result = (|| {
            conn.pragma_update(None, "foreign_keys", foreign_keys)?;
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let value = write(&tx)?;
            tx.commit()?;
            Ok(value)
        })();
        if result.is_err() {
            *slot = None;
        }
        result
    }
}

impl HistoryStore for SqliteHistoryStore {
    fn append(&self, session: &HistorySession, entries: &[HistoryEntry]) -> Result<(), HistoryError> {
        validate_id(&session.id)?;
        validate_id(&session.world_key)?;
        if session.ended_at.is_some_and(|end| end < session.started_at) {
            return Err(HistoryError::Invalid("session"));
        }
        // The whole batch is checked before anything is written.
        if entries.iter().any(|e| e.sequence < 0) {
            return Err(HistoryError::Invalid("entries"));
        }
        self.write(false, |c| {
            let deleted = c
                .prepare_cached("SELECT 1 FROM history_deletions WHERE session_id = ?1")?
                .exists([&session.id])?;
            // Delete and append serialize under the same write lock, so a session deleted in
            // the meantime stays deleted.
            if deleted {
                return Ok(());
            }
            let key: i64 = c.query_row(
                "INSERT INTO history_sessions(id, world_key, world_name, character_name, started_at, ended_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                     world_key = excluded.world_key, world_name = excluded.world_name,
                     character_name = CASE WHEN excluded.character_name <> '' AND
                         (history_sessions.ended_at IS NULL OR excluded.ended_at IS NOT NULL)
                         THEN excluded.character_name ELSE history_sessions.character_name END,
                     ended_at = CASE WHEN history_sessions.ended_at IS NULL THEN excluded.ended_at
                         WHEN excluded.ended_at IS NULL THEN history_sessions.ended_at
                         ELSE MAX(history_sessions.ended_at, excluded.ended_at) END
                 RETURNING key",
                params![
                    session.id,
                    session.world_key,
                    session.world_name,
                    session.character_name,
                    to_micros(session.started_at),
                    session.ended_at.map(to_micros),
                ],
                |r| r.get(0),
            )?;
            // New rows get ids above the highest one now (this transaction is the only writer),
            // so the index takes them in one statement afterwards. A retried sequence keeps its
            // first text and is not indexed twice.
            let before: i64 = c.query_row("SELECT COALESCE(MAX(id), 0) FROM history_entries", [], |r| r.get(0))?;
            let times: Vec<i64> = entries.iter().map(|e| to_micros(e.at)).collect();
            let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
            let row =
                |i: usize| -> [&dyn ToSql; 5] { [&key, &entries[i].sequence, &times[i], &kinds[i], &entries[i].text] };
            let full = entries.len() / INSERT_ROWS * INSERT_ROWS;
            let mut many = c.prepare_cached(&insert_sql(INSERT_ROWS))?;
            let mut values: Vec<&dyn ToSql> = Vec::with_capacity(INSERT_ROWS * 5);
            for start in (0..full).step_by(INSERT_ROWS) {
                values.clear();
                for i in start..start + INSERT_ROWS {
                    values.extend(row(i));
                }
                many.execute(values.as_slice())?;
            }
            let mut one = c.prepare_cached(&insert_sql(1))?;
            for i in full..entries.len() {
                one.execute(row(i).as_slice())?;
            }
            c.execute(
                "INSERT INTO history_entries_fts(rowid, text) SELECT id, text FROM history_entries WHERE id > ?1",
                [before],
            )?;
            Ok(())
        })
    }

    fn sessions(
        &self,
        filter: &HistoryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<HistorySession>, HistoryError> {
        validate_filter(filter)?;
        let limit = page(limit)?;
        self.read(|c| {
            let (condition, mut args) = bind_filter(c, filter, "s.started_at")?;
            let sql = format!(
                "SELECT {SESSION_COLUMNS} FROM history_sessions s WHERE {condition}
                 ORDER BY s.started_at DESC, s.id LIMIT ? OFFSET ?"
            );
            args.push(Box::new(limit as i64));
            args.push(Box::new(offset_value(offset)));
            let mut statement = c.prepare(&sql)?;
            let rows = statement.query_map(rusqlite::params_from_iter(args.iter()), read_session)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    fn search(
        &self,
        query: &str,
        filter: &HistoryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<HistoryHit>, HistoryError> {
        validate_filter(filter)?;
        let limit = page(limit)?;
        if query.encode_utf16().count() > MAX_QUERY {
            return Err(HistoryError::Invalid("query"));
        }
        let matching = literal_match(query);
        if matching.is_empty() {
            return Ok(Vec::new());
        }
        self.read(|c| {
            let (condition, filter_args) = bind_filter(c, filter, "e.at")?;
            let sql = format!(
                "SELECT {SESSION_COLUMNS}, e.sequence, e.at, e.kind, e.text
                 FROM history_entries_fts
                 JOIN history_entries e ON e.id = history_entries_fts.rowid
                 JOIN history_sessions s ON s.key = e.session
                 WHERE history_entries_fts MATCH ? AND {condition}
                 ORDER BY e.at DESC, e.id DESC LIMIT ? OFFSET ?"
            );
            let mut args: Vec<Box<dyn ToSql>> = vec![Box::new(matching)];
            args.extend(filter_args);
            args.push(Box::new(limit as i64));
            args.push(Box::new(offset_value(offset)));
            let mut statement = c.prepare(&sql)?;
            let rows = statement.query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Ok(HistoryHit {
                    session: read_session(r)?,
                    entry: read_entry(r, 6)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    fn entries(&self, session_id: &str, from_sequence: i64, limit: usize) -> Result<Vec<HistoryEntry>, HistoryError> {
        validate_id(session_id)?;
        if from_sequence < 0 {
            return Err(HistoryError::Invalid("from_sequence"));
        }
        let limit = page(limit)?;
        self.read(|c| {
            let mut statement = c.prepare_cached(
                "SELECT sequence, at, kind, text FROM history_entries
                 WHERE session = (SELECT key FROM history_sessions WHERE id = ?1) AND sequence >= ?2
                 ORDER BY sequence LIMIT ?3",
            )?;
            let rows = statement.query_map(params![session_id, from_sequence, limit as i64], |r| read_entry(r, 0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    fn delete(&self, session_id: &str) -> Result<(), HistoryError> {
        validate_id(session_id)?;
        self.write(true, |c| {
            c.execute(
                "INSERT INTO history_deletions(session_id) VALUES(?1) ON CONFLICT DO NOTHING",
                [session_id],
            )?;
            // The foreign key cascades to the entries, and their trigger to the index.
            c.execute("DELETE FROM history_sessions WHERE id = ?1", [session_id])?;
            Ok(())
        })
    }

    fn prune(&self, before: SystemTime) -> Result<(), HistoryError> {
        let before = to_micros(before);
        self.write(true, |c| {
            c.execute("DELETE FROM history_entries WHERE at < ?1", [before])?;
            c.execute(
                "DELETE FROM history_sessions WHERE COALESCE(ended_at, started_at) < ?1
                 AND NOT EXISTS(SELECT 1 FROM history_entries WHERE session = history_sessions.key)",
                [before],
            )?;
            Ok(())
        })
    }
}

/// The FTS5 expression for a query: each word or double-quoted phrase becomes a quoted FTS
/// string (two quotes inside a phrase stand for one), joined with AND. Terms without a letter or
/// digit have nothing to search and are dropped; operators and wildcards are therefore always
/// plain text.
pub fn literal_match(query: &str) -> String {
    // NUL must not cut the expression short inside SQLite.
    let chars: Vec<char> = query.chars().map(|c| if c == '\0' { ' ' } else { c }).collect();
    let mut terms = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let quoted = chars[i] == '"';
        if quoted {
            i += 1;
        }
        let mut term = String::new();
        while i < chars.len() {
            let c = chars[i];
            if c == '"' {
                if !quoted {
                    break;
                }
                i += 1;
                if i == chars.len() || chars[i] != '"' {
                    break;
                }
                term.push('"');
                i += 1;
            } else {
                if !quoted && c.is_whitespace() {
                    break;
                }
                term.push(c);
                i += 1;
            }
        }
        if term.chars().any(char::is_alphanumeric) {
            terms.push(format!("\"{}\"", term.replace('"', "\"\"")));
        }
    }
    terms.join(" AND ")
}

/// Whether `value` contains `fragment`, ignoring case, as plain text.
pub fn contains_ignoring_case(value: &str, fragment: &str) -> bool {
    value.to_lowercase().contains(&fragment.to_lowercase())
}

type Args = Vec<Box<dyn ToSql>>;

fn bind_filter(c: &Connection, filter: &HistoryFilter, time_column: &str) -> Result<(String, Args), HistoryError> {
    // A function of our own, not LIKE: no wildcards, and case folding beyond ASCII.
    c.create_scalar_function(
        "history_contains",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let value: String = ctx.get(0)?;
            let fragment: String = ctx.get(1)?;
            Ok(contains_ignoring_case(&value, &fragment))
        },
    )?;
    let mut conditions = vec!["1 = 1".to_string()];
    let mut args: Args = Vec::new();
    if let Some(world) = filter.world.as_deref().map(str::trim).filter(|w| !w.is_empty()) {
        conditions.push("(history_contains(s.world_key, ?) OR history_contains(s.world_name, ?))".into());
        args.push(Box::new(world.to_string()));
        args.push(Box::new(world.to_string()));
    }
    if let Some(character) = filter.character.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        conditions.push("history_contains(s.character_name, ?)".into());
        args.push(Box::new(character.to_string()));
    }
    if let Some(from) = filter.from {
        conditions.push(format!("{time_column} >= ?"));
        args.push(Box::new(to_micros(from)));
    }
    if let Some(until) = filter.until {
        conditions.push(format!("{time_column} < ?"));
        args.push(Box::new(to_micros(until)));
    }
    Ok((conditions.join(" AND "), args))
}

fn validate_filter(filter: &HistoryFilter) -> Result<(), HistoryError> {
    let too_long = |text: &Option<String>| text.as_deref().is_some_and(|t| t.encode_utf16().count() > MAX_QUERY);
    if let (Some(from), Some(until)) = (filter.from, filter.until)
        && from > until
    {
        return Err(HistoryError::Invalid("filter"));
    }
    if too_long(&filter.world) || too_long(&filter.character) {
        return Err(HistoryError::Invalid("filter"));
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), HistoryError> {
    if id.trim().is_empty() || id.encode_utf16().count() > MAX_QUERY || id.chars().any(char::is_control) {
        return Err(HistoryError::Invalid("id"));
    }
    Ok(())
}

fn page(limit: usize) -> Result<usize, HistoryError> {
    if limit == 0 {
        return Err(HistoryError::Invalid("limit"));
    }
    Ok(limit.min(MAX_PAGE))
}

fn offset_value(offset: usize) -> i64 {
    i64::try_from(offset).unwrap_or(i64::MAX)
}

fn read_session(r: &Row<'_>) -> rusqlite::Result<HistorySession> {
    Ok(HistorySession {
        id: r.get(0)?,
        world_key: r.get(1)?,
        world_name: r.get(2)?,
        character_name: r.get(3)?,
        started_at: from_micros(r.get(4)?),
        ended_at: r.get::<_, Option<i64>>(5)?.map(from_micros),
    })
}

fn read_entry(r: &Row<'_>, start: usize) -> rusqlite::Result<HistoryEntry> {
    let kind: String = r.get(start + 2)?;
    Ok(HistoryEntry {
        sequence: r.get(start)?,
        at: from_micros(r.get(start + 1)?),
        kind: EntryKind::parse(&kind).unwrap_or(EntryKind::Received),
        text: r.get(start + 3)?,
    })
}

#[cfg(test)]
mod tests {
    //! The C# `SessionHistoryStoreTests`, ported.
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    struct Fixture {
        dir: PathBuf,
        store: SqliteHistoryStore,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("wandur-history-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let (db, _) = Database::open(&dir).unwrap();
            Self {
                dir,
                store: SqliteHistoryStore::new(db),
            }
        }

        /// A second instance over the same file (as a new `ClientDatabase` would be).
        fn other(&self) -> SqliteHistoryStore {
            SqliteHistoryStore::new(Database::open(&self.dir).unwrap().0)
        }

        fn scalar<T: rusqlite::types::FromSql>(&self, sql: &str) -> T {
            let c = self.store.database().connect().unwrap();
            c.query_row(sql, [], |r| r.get(0)).unwrap()
        }

        fn exec(&self, sql: &str) {
            let c = self.store.database().connect().unwrap();
            c.execute_batch(sql).unwrap();
        }

        fn check_index(&self) {
            self.exec("INSERT INTO history_entries_fts(history_entries_fts, rank) VALUES('integrity-check', 1)");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn noon() -> SystemTime {
        // 2026-09-24 12:00 UTC.
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_251_200)
    }

    fn days(n: i64) -> SystemTime {
        if n >= 0 {
            noon() + Duration::from_secs(n as u64 * 86_400)
        } else {
            noon() - Duration::from_secs(n.unsigned_abs() * 86_400)
        }
    }

    fn session(id: &str) -> HistorySession {
        HistorySession {
            id: id.into(),
            world_key: "fixture.example:4000".into(),
            world_name: "Copper Harbor".into(),
            character_name: "Mira".into(),
            started_at: noon(),
            ended_at: None,
        }
    }

    fn entry(sequence: i64, text: &str) -> HistoryEntry {
        HistoryEntry {
            sequence,
            at: noon() + Duration::from_micros(sequence as u64),
            kind: EntryKind::Received,
            text: text.into(),
        }
    }

    fn lantern(sequence: i64) -> HistoryEntry {
        entry(sequence, "copper lantern")
    }

    fn all() -> HistoryFilter {
        HistoryFilter::default()
    }

    fn seqs(hits: &[HistoryHit]) -> Vec<i64> {
        hits.iter().map(|h| h.entry.sequence).collect()
    }

    #[test]
    fn append_reopens_in_sequence_order_and_retries_do_not_overwrite_or_duplicate_text() {
        let f = Fixture::new("order");
        let s = &f.store;
        s.append(&session("one"), &[entry(2, "second"), entry(0, "first")])
            .unwrap();
        s.append(&session("one"), &[entry(2, "replacement"), entry(1, "middle")])
            .unwrap();
        let s = f.other();
        let texts: Vec<_> = s.entries("one", 0, 200).unwrap().into_iter().map(|e| e.text).collect();
        assert_eq!(texts, ["first", "middle", "second"]);
        let from_one: Vec<_> = s.entries("one", 1, 200).unwrap().iter().map(|e| e.sequence).collect();
        assert_eq!(from_one, [1, 2]);
        assert_eq!(
            s.entries("one", 0, 200).unwrap()[2].at,
            noon() + Duration::from_micros(2)
        );
        assert!(s.search("replacement", &all(), 0, 100).unwrap().is_empty());
        assert_eq!(s.search("second", &all(), 0, 100).unwrap().len(), 1);
        assert_eq!(s.sessions(&all(), 0, 100).unwrap(), [session("one")]);
        f.check_index();
    }

    #[test]
    fn empty_append_updates_character_and_end_time_and_an_older_open_batch_cannot_clear_the_end() {
        let f = Fixture::new("end");
        let s = &f.store;
        s.append(&session("one"), &[lantern(0)]).unwrap();
        let closed = HistorySession {
            character_name: "Mira Vale".into(),
            ended_at: Some(noon() + Duration::from_secs(7200)),
            ..session("one")
        };
        s.append(&closed, &[]).unwrap();
        assert_eq!(s.sessions(&all(), 0, 100).unwrap(), std::slice::from_ref(&closed));
        s.append(&session("one"), &[lantern(1)]).unwrap();
        assert_eq!(s.sessions(&all(), 0, 100).unwrap(), std::slice::from_ref(&closed));
        let earlier = HistorySession {
            ended_at: Some(noon() + Duration::from_secs(3600)),
            ..closed.clone()
        };
        s.append(&earlier, &[]).unwrap();
        assert_eq!(s.sessions(&all(), 0, 100).unwrap(), [closed]);
        assert_eq!(s.entries("one", 0, 200).unwrap().len(), 2);
    }

    #[test]
    fn query_words_are_anded_and_fts_syntax_is_always_literal() {
        let cases: &[(&str, &[i64])] = &[
            ("LANTERN copper", &[3, 2, 1, 0]),
            ("\"copper lantern\"", &[2, 0]),
            ("\"copper lantern\" gleams", &[2]),
            ("copper OR lantern", &[3]),
            ("copp*", &[]),
            ("text:copper", &[]),
            ("NEAR(copper,lantern)", &[]),
            ("\"copper lantern", &[2, 0]),
            ("\"\"copper\"\" lantern", &[3, 2, 1, 0]),
            ("copper -lantern", &[3, 2, 1, 0]),
            ("copper\" OR 1=1; --", &[]),
            ("", &[]),
            (" \t\n", &[]),
            ("\"\"", &[]),
            ("* : () -", &[]),
        ];
        let f = Fixture::new("literal");
        let s = &f.store;
        s.append(
            &session("one"),
            &[
                entry(0, "copper lantern"),
                entry(1, "lantern beside copper"),
                entry(2, "a copper lantern gleams"),
                entry(3, "copper or distant lantern"),
            ],
        )
        .unwrap();
        for (query, expected) in cases {
            assert_eq!(seqs(&s.search(query, &all(), 0, 100).unwrap()), *expected, "{query:?}");
        }
        assert_eq!(s.entries("one", 0, 200).unwrap().len(), 4);
    }

    #[test]
    fn search_supports_unicode_and_does_not_treat_embedded_null_as_the_end_of_a_query() {
        let f = Fixture::new("unicode");
        let s = &f.store;
        s.append(&session("one"), &[entry(0, "éclat 漢字"), entry(1, "copper")])
            .unwrap();
        assert_eq!(seqs(&s.search("ÉCLAT 漢字", &all(), 0, 100).unwrap()), [0]);
        assert!(s.search("copper\0missing", &all(), 0, 100).unwrap().is_empty());
    }

    #[test]
    fn doubled_quotes_inside_a_phrase_still_require_adjacent_words() {
        let f = Fixture::new("quotes");
        let s = &f.store;
        s.append(
            &session("one"),
            &[
                entry(0, "copper \"lantern\" gleams"),
                entry(1, "copper then lantern gleams"),
            ],
        )
        .unwrap();
        assert_eq!(
            seqs(&s.search("\"copper \"\"lantern\"\" gleams\"", &all(), 0, 100).unwrap()),
            [0]
        );
    }

    #[test]
    fn supplementary_unicode_letters_are_searchable() {
        let f = Fixture::new("supplementary");
        f.store.append(&session("one"), &[entry(0, "𐐀𐐁")]).unwrap();
        assert_eq!(f.store.search("𐐀𐐁", &all(), 0, 100).unwrap().len(), 1);
    }

    #[test]
    fn metadata_filters_match_unicode_without_interpreting_wildcards() {
        let f = Fixture::new("meta-unicode");
        let s = &f.store;
        let named = HistorySession {
            world_name: "Éclat_100%".into(),
            character_name: "Ária".into(),
            ..session("one")
        };
        s.append(&named, &[lantern(0)]).unwrap();
        let filter = HistoryFilter {
            world: Some("éCLAT_100%".into()),
            character: Some("ária".into()),
            ..all()
        };
        assert_eq!(s.search("lantern", &filter, 0, 100).unwrap().len(), 1);
        let other = HistoryFilter {
            world: Some("Éclat_200%".into()),
            ..all()
        };
        assert!(s.sessions(&other, 0, 100).unwrap().is_empty());
    }

    #[test]
    fn maximum_length_query_and_many_words_remain_bounded_and_valid() {
        let f = Fixture::new("long");
        let s = &f.store;
        s.append(&session("one"), &[entry(0, "a")]).unwrap();
        let padded = format!("a{}", " ".repeat(4095));
        assert_eq!(s.search(&padded, &all(), 0, 100).unwrap().len(), 1);
        let many = vec!["a"; 2048].join(" ");
        assert_eq!(s.search(&many, &all(), 0, 100).unwrap().len(), 1);
    }

    #[test]
    fn database_failure_rolls_back_metadata_entries_and_fts_together() {
        let f = Fixture::new("rollback");
        let s = &f.store;
        s.append(&session("one"), &[lantern(0)]).unwrap();
        f.exec(
            "CREATE TRIGGER reject_fixture_entry BEFORE INSERT ON history_entries WHEN new.sequence = 2
             BEGIN SELECT RAISE(ABORT, 'fixture failure'); END",
        );
        let changed = HistorySession {
            character_name: "Changed".into(),
            ..session("one")
        };
        let err = s
            .append(&changed, &[entry(1, "rollbackmarker"), lantern(2)])
            .unwrap_err();
        assert!(matches!(err, HistoryError::Storage(_)), "{err}");
        assert_eq!(s.sessions(&all(), 0, 100).unwrap()[0].character_name, "Mira");
        let entries = s.entries("one", 0, 200).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].sequence, 0);
        assert!(s.search("rollbackmarker", &all(), 0, 100).unwrap().is_empty());
        f.check_index();
    }

    #[test]
    fn concurrent_first_open_and_appends_preserve_every_session() {
        let f = Fixture::new("concurrent");
        let dir = f.dir.clone();
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    let store = SqliteHistoryStore::new(Database::open(&dir).unwrap().0);
                    store.append(&session(&format!("session{i}")), &[lantern(0)]).unwrap();
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(f.store.sessions(&all(), 0, 100).unwrap().len(), 8);
        assert_eq!(f.store.search("lantern", &all(), 0, 100).unwrap().len(), 8);
        f.check_index();
    }

    #[test]
    fn all_kinds_and_multiline_text_round_trip_with_duplicate_sequences_in_one_batch() {
        let f = Fixture::new("kinds");
        let s = &f.store;
        let with = |e: HistoryEntry, kind| HistoryEntry { kind, ..e };
        s.append(
            &session("one"),
            &[
                with(lantern(0), EntryKind::Sent),
                entry(0, "duplicate"),
                with(entry(1, "first\nsecond\tthird"), EntryKind::Script),
                with(entry(2, "[private]"), EntryKind::Private),
            ],
        )
        .unwrap();
        let entries = s.entries("one", 0, 200).unwrap();
        let kinds: Vec<_> = entries.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, [EntryKind::Sent, EntryKind::Script, EntryKind::Private]);
        assert_eq!(entries[1].text, "first\nsecond\tthird");
        assert_eq!(s.search("\"first second\"", &all(), 0, 100).unwrap().len(), 1);
        assert!(s.search("duplicate", &all(), 0, 100).unwrap().is_empty());
        f.check_index();
    }

    #[test]
    fn metadata_filters_are_literal_substrings_and_search_dates_are_half_open_entry_times() {
        let f = Fixture::new("filters");
        let s = &f.store;
        s.append(&session("one"), &[lantern(0), lantern(1), lantern(2)])
            .unwrap();
        let other = HistorySession {
            world_key: "other.example:23".into(),
            world_name: "Blue Harbor".into(),
            character_name: "Sol".into(),
            ..session("other")
        };
        s.append(&other, &[lantern(0)]).unwrap();
        let world = |w: &str| HistoryFilter {
            world: Some(w.into()),
            ..all()
        };
        assert_eq!(s.search("lantern", &world("FIXTURE.EXAMPLE"), 0, 100).unwrap().len(), 3);
        let both = HistoryFilter {
            world: Some("COPPER".into()),
            character: Some("ir".into()),
            ..all()
        };
        assert_eq!(s.search("lantern", &both, 0, 100).unwrap().len(), 3);
        assert!(s.search("lantern", &world("%"), 0, 100).unwrap().is_empty());
        let underscore = HistoryFilter {
            character: Some("_".into()),
            ..all()
        };
        assert!(s.search("lantern", &underscore, 0, 100).unwrap().is_empty());
        assert!(s.search("lantern", &world("' OR 1=1 --"), 0, 100).unwrap().is_empty());
        let window = HistoryFilter {
            from: Some(noon() + Duration::from_micros(1)),
            until: Some(noon() + Duration::from_micros(2)),
            ..all()
        };
        assert_eq!(seqs(&s.search("lantern", &window, 0, 100).unwrap()), [1]);
        let empty = HistoryFilter {
            from: Some(noon()),
            until: Some(noon()),
            ..all()
        };
        assert!(s.search("lantern", &empty, 0, 100).unwrap().is_empty());
    }

    #[test]
    fn sessions_use_start_time_filters_and_stable_newest_first_pagination() {
        let f = Fixture::new("sessions");
        let s = &f.store;
        s.append(&session("a"), &[]).unwrap();
        s.append(&session("b"), &[]).unwrap();
        let later = HistorySession {
            started_at: days(1),
            ..session("c")
        };
        s.append(&later, &[]).unwrap();
        let ids = |list: Vec<HistorySession>| list.into_iter().map(|s| s.id).collect::<Vec<_>>();
        assert_eq!(ids(s.sessions(&all(), 0, 100).unwrap()), ["c", "a", "b"]);
        assert_eq!(ids(s.sessions(&all(), 1, 1).unwrap()), ["a"]);
        let day = HistoryFilter {
            from: Some(noon()),
            until: Some(days(1)),
            ..all()
        };
        assert_eq!(ids(s.sessions(&day, 0, 100).unwrap()), ["a", "b"]);
        let named = HistoryFilter {
            world: Some("fixture".into()),
            character: Some("MIR".into()),
            ..all()
        };
        assert_eq!(s.sessions(&named, 0, 100).unwrap().len(), 3);
        let percent = HistoryFilter {
            character: Some("%".into()),
            ..all()
        };
        assert!(s.sessions(&percent, 0, 100).unwrap().is_empty());
    }

    #[test]
    fn search_pagination_is_stable_across_timestamp_ties_and_entries_include_the_starting_sequence() {
        let f = Fixture::new("ties");
        let s = &f.store;
        s.append(&session("a"), &[lantern(0), lantern(1)]).unwrap();
        s.append(&session("b"), &[lantern(0)]).unwrap();
        let hits = s.search("lantern", &all(), 0, 100).unwrap();
        let ids: Vec<_> = hits.iter().map(|h| h.session.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "a"]);
        assert_eq!(s.search("lantern", &all(), 1, 1).unwrap(), hits[1..2]);
        assert_eq!(s.entries("a", 1, 1).unwrap()[0].sequence, 1);
        assert!(s.entries("a", i64::MAX, 200).unwrap().is_empty());
        assert!(s.sessions(&all(), usize::MAX, 100).unwrap().is_empty());
    }

    #[test]
    fn page_sizes_are_capped_for_all_read_methods() {
        let f = Fixture::new("caps");
        let s = &f.store;
        let entries: Vec<_> = (0..510).map(lantern).collect();
        s.append(&session("one"), &entries).unwrap();
        assert_eq!(s.entries("one", 0, usize::MAX).unwrap().len(), 500);
        assert_eq!(s.search("lantern", &all(), 0, usize::MAX).unwrap().len(), 500);
        for i in 0..501 {
            s.append(&session(&format!("session{i}")), &[]).unwrap();
        }
        assert_eq!(s.sessions(&all(), 0, usize::MAX).unwrap().len(), 500);
    }

    #[test]
    fn delete_removes_entries_and_fts_and_persists_a_tombstone_across_reopen() {
        let f = Fixture::new("delete");
        let s = &f.store;
        s.append(&session("one"), &[entry(0, "forgottenmarker")]).unwrap();
        s.append(&session("keep"), &[entry(0, "retainedmarker")]).unwrap();
        s.delete("one").unwrap();
        s.delete("one").unwrap();
        let s = f.other();
        s.append(&session("one"), &[entry(1, "forgottenmarker")]).unwrap();
        let ended = HistorySession {
            ended_at: Some(noon() + Duration::from_secs(3600)),
            ..session("one")
        };
        s.append(&ended, &[]).unwrap();
        assert!(s.entries("one", 0, 200).unwrap().is_empty());
        let sessions = s.sessions(&all(), 0, 100).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "keep");
        assert!(s.search("forgottenmarker", &all(), 0, 100).unwrap().is_empty());
        assert_eq!(s.search("retainedmarker", &all(), 0, 100).unwrap().len(), 1);
        let left: i64 =
            f.scalar("SELECT count(*) FROM history_entries_fts WHERE history_entries_fts MATCH 'forgottenmarker'");
        assert_eq!(left, 0);
        f.check_index();
    }

    #[test]
    fn deletion_before_the_first_queued_append_also_prevents_resurrection() {
        let f = Fixture::new("tombstone-first");
        let s = &f.store;
        s.delete("one").unwrap();
        s.append(&session("one"), &[lantern(0)]).unwrap();
        assert!(s.sessions(&all(), 0, 100).unwrap().is_empty());
        assert!(s.search("lantern", &all(), 0, 100).unwrap().is_empty());
    }

    #[test]
    fn concurrent_appends_and_deletion_across_database_instances_never_resurrect_the_session() {
        let f = Fixture::new("race");
        f.store.append(&session("one"), &[lantern(0)]).unwrap();
        let dir = f.dir.clone();
        let threads: Vec<_> = (1..=16)
            .map(|i| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    let store = SqliteHistoryStore::new(Database::open(&dir).unwrap().0);
                    if i == 8 {
                        store.delete("one").unwrap();
                    } else {
                        store.append(&session("one"), &[lantern(i)]).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        // Appends that ran before the delete are removed with it; later ones are refused.
        assert!(f.store.sessions(&all(), 0, 100).unwrap().is_empty());
        assert!(f.store.entries("one", 0, 200).unwrap().is_empty());
        f.check_index();
    }

    #[test]
    fn retention_removes_abandoned_empty_metadata_but_recording_can_resume() {
        let f = Fixture::new("abandoned");
        let s = &f.store;
        let abandoned = HistorySession {
            started_at: days(-60),
            ..session("one")
        };
        s.append(
            &abandoned,
            &[HistoryEntry {
                at: days(-50),
                ..lantern(0)
            }],
        )
        .unwrap();
        s.prune(days(-30)).unwrap();
        assert!(s.sessions(&all(), 0, 100).unwrap().is_empty());
        assert!(s.search("copper", &all(), 0, 100).unwrap().is_empty());
        // A connection still running can bring its row back with new output.
        s.append(&abandoned, &[entry(1, "new public output")]).unwrap();
        assert_eq!(s.sessions(&all(), 0, 100).unwrap().len(), 1);
        assert_eq!(s.search("public", &all(), 0, 100).unwrap().len(), 1);
    }

    #[test]
    fn prune_uses_entry_times_including_active_sessions_and_keeps_recent_entries_in_old_sessions() {
        let f = Fixture::new("prune");
        let s = &f.store;
        let at = |e: HistoryEntry, when| HistoryEntry { at: when, ..e };
        let old = HistorySession {
            started_at: days(-60),
            ..session("one")
        };
        s.append(
            &old,
            &[
                at(entry(0, "expiredmarker"), days(-31)),
                at(entry(1, "boundarymarker"), days(-30)),
                lantern(2),
            ],
        )
        .unwrap();
        let closed = HistorySession {
            id: "closed".into(),
            ended_at: Some(days(-1)),
            ..old.clone()
        };
        s.append(&closed, &[at(entry(0, "expiredmarker"), days(-31)), lantern(1)])
            .unwrap();
        let expired = HistorySession {
            id: "expired".into(),
            ended_at: Some(days(-40)),
            ..old.clone()
        };
        s.append(&expired, &[at(entry(0, "expiredmarker"), days(-41))]).unwrap();
        let recent = HistorySession {
            id: "old-end-recent-content".into(),
            ended_at: Some(days(-40)),
            ..old.clone()
        };
        s.append(&recent, &[entry(0, "recentmarker")]).unwrap();
        s.prune(days(-30)).unwrap();
        let kept: Vec<_> = s.entries("one", 0, 200).unwrap().iter().map(|e| e.sequence).collect();
        assert_eq!(kept, [1, 2]);
        let closed_kept = s.entries("closed", 0, 200).unwrap();
        assert_eq!(closed_kept.len(), 1);
        assert_eq!(closed_kept[0].sequence, 1);
        assert_eq!(s.sessions(&all(), 0, 100).unwrap().len(), 3);
        assert_eq!(s.search("recentmarker", &all(), 0, 100).unwrap().len(), 1);
        assert!(s.search("expiredmarker", &all(), 0, 100).unwrap().is_empty());
        assert_eq!(s.search("boundarymarker", &all(), 0, 100).unwrap().len(), 1);
        s.append(&old, &[entry(3, "stillrecording")]).unwrap();
        assert_eq!(s.search("stillrecording", &all(), 0, 100).unwrap().len(), 1);
        f.check_index();
    }

    #[test]
    fn external_content_triggers_keep_updates_and_cascaded_deletes_consistent() {
        let f = Fixture::new("triggers");
        let s = &f.store;
        s.append(&session("one"), &[entry(0, "oldmarker")]).unwrap();
        f.exec("UPDATE history_entries SET text = 'newmarker'");
        assert!(s.search("oldmarker", &all(), 0, 100).unwrap().is_empty());
        assert_eq!(s.search("newmarker", &all(), 0, 100).unwrap().len(), 1);
        f.check_index();
        f.exec("DELETE FROM history_sessions");
        assert!(s.search("newmarker", &all(), 0, 100).unwrap().is_empty());
        f.check_index();
    }

    #[test]
    fn invalid_arguments_fail_before_writing_any_part_of_a_batch() {
        let f = Fixture::new("invalid");
        let s = &f.store;
        let invalid = |r: Result<(), HistoryError>| assert!(matches!(r, Err(HistoryError::Invalid(_))));
        invalid(s.append(&session("one"), &[lantern(0), lantern(-1)]));
        invalid(s.append(&session(" "), &[]));
        invalid(s.append(
            &HistorySession {
                world_key: String::new(),
                ..session("one")
            },
            &[],
        ));
        invalid(s.append(
            &HistorySession {
                ended_at: Some(days(-1)),
                ..session("one")
            },
            &[],
        ));
        invalid(s.search(&"a".repeat(4097), &all(), 0, 100).map(drop));
        invalid(s.sessions(&all(), 0, 0).map(drop));
        invalid(s.search("a", &all(), 0, 0).map(drop));
        invalid(s.entries("one", -1, 200).map(drop));
        invalid(s.entries("one", 0, 0).map(drop));
        invalid(s.delete(""));
        invalid(s.entries("", 0, 200).map(drop));
        let backwards = HistoryFilter {
            from: Some(days(1)),
            until: Some(noon()),
            ..all()
        };
        invalid(s.sessions(&backwards, 0, 100).map(drop));
        assert!(s.sessions(&all(), 0, 100).unwrap().is_empty());
    }

    #[test]
    fn version_three_migrates_without_losing_existing_data_and_reopens_with_fts_triggers() {
        let dir = std::env::temp_dir().join(format!("wandur-history-v3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        {
            let mut conn = Connection::open(dir.join(crate::db::DB_FILE)).unwrap();
            crate::db::migrate(&mut conn, 3).unwrap();
            conn.execute_batch(
                "INSERT INTO worlds VALUES('fixture-world');
                 INSERT INTO world_usage VALUES('fixture-world', 2, 100, 'Fixture');",
            )
            .unwrap();
        }
        let (db, _) = Database::open(&dir).unwrap();
        let c = db.connect().unwrap();
        let version: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, crate::db::SCHEMA_VERSION);
        let character: String = c
            .query_row("SELECT last_character FROM world_usage", [], |r| r.get(0))
            .unwrap();
        assert_eq!(character, "Fixture");
        let triggers: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'trigger' AND tbl_name = 'history_entries'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // Update and delete; new entries are indexed by the store (see the migration).
        assert_eq!(triggers, 2);
        drop(c);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
