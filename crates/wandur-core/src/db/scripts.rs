//! A world's script library in `wandur.db`: scripts and macros side by side, as the C#
//! `SqliteWorldScriptLibraryStore` keeps them (table `scripts`: id, name, source, enabled,
//! `macro_json`, `pack_json`, `import_json`, `language`, `compatibility`, keyed by world id). A macro's source is the JavaScript the C#
//! client generates from its definition, and a row whose source does not match its definition is
//! refused. Macros and scripts share the 64-entry, 4 MiB library limit.
//!
//! Reads take a connection (a short one on any thread); writes take the writer thread's
//! connection inside its transaction. Every write validates first, so an invalid entry never
//! replaces a saved one.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::DbError;
use crate::l10n::{S, t};
use crate::macros::{MacroDefinition, SavedMacro};
use crate::scripting::{Compatibility, Language, Runtime};

/// Most entries (scripts and macros) per world.
pub const MAX_ENTRIES: usize = 64;
/// Largest library, as JSON, in bytes.
pub const MAX_LIBRARY_BYTES: usize = 4_194_304;
/// Largest script source, in UTF-8 bytes.
pub const MAX_SOURCE_BYTES: usize = 262_144;
/// Longest entry name, in UTF-16 units.
pub const MAX_NAME: usize = 120;

/// One script or macro of a world's library.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LibraryEntry {
    /// A UUID in its hyphenated form (the C# `Guid.ToString("D")`).
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Source")]
    pub source: String,
    /// Starts with the world's sessions.
    #[serde(rename = "Enabled")]
    pub enabled: bool,
    /// Set for a macro; its source is generated from it.
    #[serde(rename = "Macro")]
    pub macro_def: Option<MacroDefinition>,
    /// A directory script pack's details as stored (the C# `StoredPack` JSON: `Info` and
    /// `AllowSend`); `None` for a hand-written script. See [`LibraryEntry::pack`].
    #[serde(skip)]
    pub pack_json: Option<String>,
    /// Set when the script came from another client's profile (a Mudlet import).
    #[serde(rename = "Import")]
    pub import: Option<ImportInfo>,
    /// The script's language; macros are always JavaScript.
    #[serde(rename = "Language", serialize_with = "language_name")]
    pub language: Language,
    /// A layer the script runs with (Mudlet names for a Lua script).
    #[serde(rename = "Compatibility", serialize_with = "compatibility_name")]
    pub compatibility: Compatibility,
}

fn language_name<S2: serde::Serializer>(language: &Language, serializer: S2) -> Result<S2::Ok, S2::Error> {
    serializer.serialize_str(language.as_str())
}

fn compatibility_name<S2: serde::Serializer>(
    compatibility: &Compatibility,
    serializer: S2,
) -> Result<S2::Ok, S2::Error> {
    serializer.serialize_str(compatibility.as_str())
}

fn invalid(key: S) -> DbError {
    DbError::Invalid(t(key).into())
}

impl LibraryEntry {
    /// A new macro entry with a fresh id, disabled (new macros start disabled, as in C#).
    pub fn new_macro(name: &str, definition: MacroDefinition) -> Self {
        let source = definition.compile_javascript().unwrap_or_default();
        Self {
            id: uuid::Uuid::new_v4().hyphenated().to_string(),
            name: name.into(),
            source,
            enabled: false,
            macro_def: Some(definition),
            ..Self::default()
        }
    }

    pub fn is_macro(&self) -> bool {
        self.macro_def.is_some()
    }

    /// How the script loads: its language and layer.
    pub fn runtime(&self) -> Runtime {
        Runtime {
            language: self.language,
            compatibility: self.compatibility,
        }
    }

    pub fn is_lua(&self) -> bool {
        self.language == Language::Lua
    }

    /// Brought in from another client's profile (a Mudlet import).
    pub fn is_imported(&self) -> bool {
        self.import.is_some()
    }

    /// Holds another client's code that has to be rewritten before it can run; it never runs.
    pub fn needs_conversion(&self) -> bool {
        self.import.as_ref().is_some_and(|i| i.needs_conversion)
    }

    /// Regenerate a macro's source from its definition (after an edit).
    pub fn compile(&mut self) -> Result<(), DbError> {
        if let Some(def) = &self.macro_def {
            self.source = def.compile_javascript().map_err(|e| DbError::Invalid(e.to_string()))?;
        }
        Ok(())
    }

    /// A directory pack script whose send policy the person has not lifted (the choice is kept
    /// in the pack details as `AllowSend`).
    pub fn restricted_send(&self) -> bool {
        self.is_pack() && !self.allow_send()
    }

    /// Supplied by a world's directory listing (read only in the editor).
    pub fn is_pack(&self) -> bool {
        self.pack_json.is_some()
    }

    /// The pack details, when this is a pack script whose details read.
    pub fn pack(&self) -> Option<PackInfo> {
        let stored: StoredPack = serde_json::from_str(self.pack_json.as_deref()?).ok()?;
        Some(stored.info)
    }

    /// The person lifted this pack script's send policy.
    pub fn allow_send(&self) -> bool {
        self.pack_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<StoredPack>(json).ok())
            .is_some_and(|stored| stored.allow_send)
    }

    /// Mark this entry as a pack script with these details and send choice.
    pub fn set_pack(&mut self, info: &PackInfo, allow_send: bool) {
        let stored = StoredPack {
            info: info.clone(),
            allow_send,
        };
        self.pack_json = serde_json::to_string(&stored).ok();
    }

    /// Lift or restore the pack send policy (nothing for a hand-written script).
    pub fn set_allow_send(&mut self, allow: bool) {
        if let Some(info) = self.pack() {
            self.set_pack(&info, allow);
        }
    }

    /// The entry as a runnable macro, if it is one.
    pub fn as_saved_macro(&self) -> Option<SavedMacro> {
        self.macro_def.as_ref().map(|definition| SavedMacro {
            id: self.id.clone(),
            name: self.name.clone(),
            enabled: self.enabled,
            definition: definition.clone(),
        })
    }

    /// The C# entry checks: an id, a name of 1 to 120 characters without control characters, a
    /// source within 256 KiB, and for a macro a valid definition whose source is its compiled form.
    pub fn validate(&self) -> Result<(), DbError> {
        if uuid::Uuid::parse_str(&self.id).map_or(true, |u| u.is_nil()) {
            return Err(invalid(S::ScriptLibraryInvalid));
        }
        if self.name.trim().is_empty()
            || self.name.encode_utf16().count() > MAX_NAME
            || self.name.chars().any(char::is_control)
        {
            return Err(invalid(S::ScriptInvalidName));
        }
        if self.source.len() > MAX_SOURCE_BYTES {
            return Err(invalid(S::ScriptSourceTooLarge));
        }
        if self.pack_json.is_some() && !self.pack().is_some_and(|p| p.is_valid()) {
            return Err(invalid(S::ScriptPackInvalid));
        }
        if self.import.as_ref().is_some_and(|i| !i.is_valid()) {
            return Err(invalid(S::ScriptImportInvalid));
        }
        // Language and layer fit together: only Lua takes a layer, and macros are JavaScript.
        if !self.runtime().is_valid() || (self.is_lua() && self.is_macro()) {
            return Err(invalid(S::ScriptLibraryInvalid));
        }
        if let Some(def) = &self.macro_def {
            let compiled = def.compile_javascript().map_err(|e| DbError::Invalid(e.to_string()))?;
            if compiled != self.source {
                return Err(invalid(S::MacroInvalid));
            }
        }
        Ok(())
    }
}

/// Identifies a script a world's directory supplied (the C# `ScriptPackInfo`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackInfo {
    /// The script's id in the pack.
    #[serde(rename = "PackId")]
    pub pack_id: String,
    /// `generated` or `reviewed`.
    #[serde(rename = "Provenance")]
    pub provenance: String,
    #[serde(rename = "Version")]
    pub version: i64,
    #[serde(rename = "Description", default)]
    pub description: String,
}

impl PackInfo {
    pub const GENERATED: &'static str = "generated";
    pub const REVIEWED: &'static str = "reviewed";

    /// The C# `Validate`.
    pub fn is_valid(&self) -> bool {
        let units = |s: &str| s.encode_utf16().count();
        !self.pack_id.trim().is_empty()
            && units(&self.pack_id) <= 120
            && !self.pack_id.chars().any(char::is_control)
            && crate::directory::listing::is_supported_provenance(&self.provenance)
            && (0..=i64::from(i32::MAX)).contains(&self.version)
            && units(&self.description) <= 1024
            && !self
                .description
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
    }
}

/// Where an imported script came from and, for anything the importer could not convert, the
/// original items exactly as the other client stored them (the C# `ScriptImportInfo`, stored as
/// `import_json` in its JSON shape). A script that needs conversion holds source in another
/// language and never runs until the person rewrites it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportInfo {
    /// `mudlet`.
    #[serde(rename = "Origin")]
    pub origin: String,
    /// The profile or package name.
    #[serde(rename = "Source")]
    pub source: String,
    /// The top-level folder the script holds.
    #[serde(rename = "Group")]
    pub group: String,
    #[serde(rename = "NeedsConversion")]
    pub needs_conversion: bool,
    #[serde(rename = "Items")]
    pub items: Vec<ImportedItem>,
    /// The hash of the source as imported, so a later import can tell whether it was edited.
    #[serde(rename = "SourceHash", default)]
    pub source_hash: String,
}

/// One item of another client's profile kept as it was. `code` is the original script text;
/// `reason` is a stable key saying why it was not converted (empty when it was).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedItem {
    #[serde(rename = "Kind")]
    pub kind: String,
    #[serde(rename = "Path")]
    pub path: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Reason")]
    pub reason: String,
    #[serde(rename = "Code")]
    pub code: String,
    #[serde(rename = "Patterns")]
    pub patterns: Vec<String>,
    #[serde(rename = "Command", default)]
    pub command: String,
    #[serde(rename = "Active", default = "yes")]
    pub active: bool,
}

fn yes() -> bool {
    true
}

impl ImportInfo {
    pub const MUDLET: &'static str = "mudlet";
    /// Most kept items per script.
    pub const MAX_ITEMS: usize = 2000;

    /// The C# `ScriptImportInfo.Hash`: SHA-256 of the UTF-8 source as upper-case hex.
    pub fn hash(source: &str) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(source.as_bytes())
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect()
    }

    /// The source is still exactly what the import wrote.
    pub fn is_unchanged(&self, source: &str) -> bool {
        !self.source_hash.is_empty() && self.source_hash == Self::hash(source)
    }

    /// A stable library id, so importing the same profile again updates the same entry (the
    /// C# `IdFor`: the first 16 bytes of SHA-256 read as a .NET `Guid`).
    pub fn id_for(world_key: &str, origin: &str, source: &str, bucket: &str) -> String {
        use sha2::Digest;
        let text = format!("wandur-script-import\n{world_key}\n{origin}\n{source}\n{bucket}");
        let hash = sha2::Sha256::digest(text.as_bytes());
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&hash[..16]);
        uuid::Uuid::from_bytes_le(bytes).hyphenated().to_string()
    }

    /// The C# `Validate`.
    pub fn is_valid(&self) -> bool {
        let line = |value: &str, maximum: usize, allow_empty: bool| {
            (allow_empty || !value.trim().is_empty())
                && value.encode_utf16().count() <= maximum
                && !value.chars().any(char::is_control)
        };
        self.origin == Self::MUDLET
            && line(&self.source, 200, false)
            && line(&self.group, 200, true)
            && self.source_hash.len() <= 64
            && self.source_hash.chars().all(|c| c.is_ascii_hexdigit())
            && self.items.len() <= Self::MAX_ITEMS
            && self.items.iter().all(|item| {
                line(&item.kind, 40, false)
                    && line(&item.path, 1000, true)
                    && line(&item.name, 400, true)
                    && line(&item.reason, 80, true)
                    && item.patterns.len() <= 256
                    && item.patterns.iter().all(|p| p.encode_utf16().count() <= 8192)
            })
    }
}

/// The stored pack details (`pack_json`), as the C# `SqliteWorldScriptLibraryStore` writes them.
#[derive(Serialize, Deserialize)]
struct StoredPack {
    #[serde(rename = "Info")]
    info: PackInfo,
    #[serde(rename = "AllowSend", default)]
    allow_send: bool,
}

/// The library id of a pack script: stable, so a refreshed listing updates the same entry. The
/// C# `ScriptPackInfo.IdFor`: the first 16 bytes of SHA-256 over
/// `"wandur-script-pack\n" + world + "\n" + pack id`, read as a .NET `Guid`.
pub fn pack_entry_id(world: &str, pack_id: &str) -> String {
    use sha2::Digest;
    let hash = sha2::Sha256::digest(format!("wandur-script-pack\n{world}\n{pack_id}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash[..16]);
    uuid::Uuid::from_bytes_le(bytes).hyphenated().to_string()
}

/// The library checks: each entry, at most 64, distinct ids, within 4 MiB as JSON.
pub fn validate_library(entries: &[LibraryEntry]) -> Result<(), DbError> {
    if entries.len() > MAX_ENTRIES {
        return Err(invalid(S::ScriptLibraryTooLarge));
    }
    for entry in entries {
        entry.validate()?;
    }
    let mut ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() != entries.len() {
        return Err(invalid(S::ScriptLibraryInvalid));
    }
    let bytes = serde_json::to_vec(entries).map_or(usize::MAX, |b| b.len());
    if bytes > MAX_LIBRARY_BYTES {
        return Err(invalid(S::ScriptLibraryTooLarge));
    }
    Ok(())
}

/// A world's library, in the order entries were added.
pub fn load(conn: &Connection, world_id: &str) -> Result<Vec<LibraryEntry>, DbError> {
    let mut query = conn.prepare(
        "SELECT id, name, source, enabled, macro_json, pack_json, import_json, language, compatibility
         FROM scripts WHERE world_id = ?1 ORDER BY rowid",
    )?;
    let rows = query.query_map(params![world_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, Option<String>>(7)?,
            r.get::<_, Option<String>>(8)?,
        ))
    })?;
    let mut entries = Vec::new();
    for row in rows {
        let (id, name, source, enabled, macro_json, pack_json, import_json, language, compatibility) = row?;
        if !matches!(enabled, 0 | 1) {
            return Err(invalid(S::ScriptLibraryInvalid));
        }
        let macro_def = match macro_json {
            Some(json) => Some(serde_json::from_str(&json).map_err(|_| invalid(S::MacroInvalid))?),
            None => None,
        };
        let import = match import_json {
            Some(json) => Some(serde_json::from_str(&json).map_err(|_| invalid(S::ScriptImportInvalid))?),
            None => None,
        };
        let language = Language::parse(language.as_deref()).ok_or_else(|| invalid(S::ScriptLibraryInvalid))?;
        let compatibility =
            Compatibility::parse(compatibility.as_deref()).ok_or_else(|| invalid(S::ScriptLibraryInvalid))?;
        entries.push(LibraryEntry {
            id,
            name,
            source,
            enabled: enabled == 1,
            macro_def,
            pack_json,
            import,
            language,
            compatibility,
        });
    }
    validate_library(&entries).map_err(|_| invalid(S::ScriptLibraryInvalid))?;
    Ok(entries)
}

/// Add or replace one entry. The library with it must still be valid.
pub fn upsert(conn: &Connection, world_id: &str, entry: &LibraryEntry) -> Result<(), DbError> {
    entry.validate()?;
    let mut library = load(conn, world_id)?;
    match library.iter_mut().find(|e| e.id == entry.id) {
        Some(existing) => *existing = entry.clone(),
        None => library.push(entry.clone()),
    }
    validate_library(&library)?;
    conn.execute("INSERT OR IGNORE INTO worlds(id) VALUES (?1)", params![world_id])?;
    let macro_json = entry
        .macro_def
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_default());
    let import_json = entry
        .import
        .as_ref()
        .map(|i| serde_json::to_string(i).unwrap_or_default());
    // JavaScript, the language of every script written before Lua, and no layer are NULL.
    let language = entry.is_lua().then_some(Language::Lua.as_str());
    let compatibility = (entry.compatibility != Compatibility::None).then_some(entry.compatibility.as_str());
    conn.execute(
        "INSERT INTO scripts(world_id, id, name, source, enabled, macro_json, pack_json, import_json,
             language, compatibility)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(world_id, id) DO UPDATE SET name = excluded.name, source = excluded.source,
             enabled = excluded.enabled, macro_json = excluded.macro_json, pack_json = excluded.pack_json,
             import_json = excluded.import_json, language = excluded.language,
             compatibility = excluded.compatibility",
        params![
            world_id,
            entry.id,
            entry.name,
            entry.source,
            entry.enabled as i64,
            macro_json,
            entry.pack_json,
            import_json,
            language,
            compatibility
        ],
    )?;
    Ok(())
}

/// Remove one entry; nothing happens if it is not there.
pub fn delete(conn: &Connection, world_id: &str, id: &str) -> Result<(), DbError> {
    conn.execute(
        "DELETE FROM scripts WHERE world_id = ?1 AND id = ?2",
        params![world_id, id],
    )?;
    Ok(())
}

/// Apply an editor's draft: entries gone from `before` are deleted, new or changed ones written.
/// Entries the draft did not touch are left alone (another session may have changed them), as
/// the C# draft store's commit does. Validates the whole draft first.
pub fn save_changes(
    conn: &Connection,
    world_id: &str,
    before: &[LibraryEntry],
    after: &[LibraryEntry],
) -> Result<(), DbError> {
    validate_library(after)?;
    for old in before.iter().filter(|b| after.iter().all(|a| a.id != b.id)) {
        delete(conn, world_id, &old.id)?;
    }
    for entry in after.iter().filter(|a| !before.contains(a)) {
        upsert(conn, world_id, entry)?;
    }
    Ok(())
}

/// A new entry id: a UUID in its hyphenated form.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string()
}

/// The starter script the C# client gives a world's library the first time it is opened empty:
/// "New script" with the exits example, disabled.
pub fn starter() -> LibraryEntry {
    LibraryEntry {
        id: new_id(),
        name: t(S::ScriptDefaultName).into(),
        source: t(S::ScriptStarterExample).into(),
        enabled: false,
        ..LibraryEntry::default()
    }
}

fn started_key(world_id: &str) -> String {
    format!("script-library:{world_id}")
}

/// Whether the world's library was set up (its starter script given, or found not needed).
pub fn is_started(conn: &Connection, world_id: &str) -> Result<bool, DbError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM imports WHERE key = ?1",
            params![started_key(world_id)],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Set the world's library up once: mark it, and add `starter` when it is still empty. Returns
/// whether the starter was added.
pub fn ensure_started(conn: &Connection, world_id: &str, starter: &LibraryEntry) -> Result<bool, DbError> {
    if is_started(conn, world_id)? {
        return Ok(false);
    }
    conn.execute(
        "INSERT OR IGNORE INTO imports(key) VALUES (?1)",
        params![started_key(world_id)],
    )?;
    if !load(conn, world_id)?.is_empty() {
        return Ok(false);
    }
    upsert(conn, world_id, starter)?;
    Ok(true)
}

/// Whether the world has an entry with this id (tests and checks).
pub fn contains(conn: &Connection, world_id: &str, id: &str) -> Result<bool, DbError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM scripts WHERE world_id = ?1 AND id = ?2",
            params![world_id, id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{DB_FILE, Database, migrate};
    use crate::macros::{MacroKind, MacroMatch};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("wandur-scripts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn plain_script(name: &str) -> LibraryEntry {
        LibraryEntry {
            id: uuid::Uuid::new_v4().hyphenated().to_string(),
            name: name.into(),
            source: "mud.echo('kept');".into(),
            enabled: true,
            ..LibraryEntry::default()
        }
    }

    /// C# `MacroStorageTests`: an older database upgrades and keeps its rows; a macro saved
    /// beside a script reads back equal after reopening; a macro whose source does not match its
    /// definition is refused and the saved one stays; other worlds do not see it; delete works.
    #[test]
    fn an_older_database_upgrades_and_keeps_scripts_alongside_macros() {
        let dir = temp_dir("upgrade");
        std::fs::create_dir_all(&dir).unwrap();
        let existing = plain_script("Existing");
        {
            let mut conn = Connection::open(dir.join(DB_FILE)).unwrap();
            migrate(&mut conn, 2).unwrap();
            conn.execute_batch(
                "INSERT INTO worlds(id) VALUES ('world'), ('other');
                 INSERT INTO endpoints(endpoint_key, world_id) VALUES ('mud.example:4000', 'world');",
            )
            .unwrap();
        }
        let (db, _) = Database::open(&dir).unwrap();
        assert_eq!(db.schema_version().unwrap(), crate::db::SCHEMA_VERSION);
        db.write(|tx| upsert(tx, "world", &existing)).unwrap();
        let macro_def = MacroDefinition::new(MacroKind::Trigger, "[Hungry]", "eat bread").ignoring_case();
        let mut hunger = LibraryEntry::new_macro("Hunger", macro_def);
        hunger.enabled = true;
        db.write(|tx| upsert(tx, "world", &hunger)).unwrap();

        let (reopened, _) = Database::open(&dir).unwrap();
        let read = |db: &Database, world: &str| db.read(|c| load(c, world)).unwrap();
        assert_eq!(read(&reopened, "world"), [existing.clone(), hunger.clone()]);
        let wrong = LibraryEntry {
            source: "mud.send('wrong');".into(),
            ..hunger.clone()
        };
        let err = reopened.write(|tx| upsert(tx, "world", &wrong)).unwrap_err();
        assert_eq!(err.to_string(), "This macro definition is invalid.");
        assert_eq!(read(&reopened, "world")[1], hunger);
        assert!(read(&reopened, "other").iter().all(|e| e.id != hunger.id));
        reopened.write(|tx| delete(tx, "world", &hunger.id)).unwrap();
        assert_eq!(read(&reopened, "world"), [existing]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_world_without_a_row_gets_one_and_drafts_merge() {
        let dir = temp_dir("drafts");
        let (db, _) = Database::open(&dir).unwrap();
        let a = LibraryEntry::new_macro("Look", MacroDefinition::new(MacroKind::Shortcut, "F2", "look"));
        let b = LibraryEntry::new_macro(
            "Ford",
            MacroDefinition::new(MacroKind::Alias, "ford", "east\neast\neast"),
        );
        db.write(|tx| save_changes(tx, "fresh", &[], &[a.clone(), b.clone()]))
            .unwrap();
        let before = db.read(|c| load(c, "fresh")).unwrap();
        assert_eq!(before, [a.clone(), b.clone()]);
        // Another session adds an entry meanwhile; the draft deletes `a` and edits `b`.
        let other = plain_script("Other");
        db.write(|tx| upsert(tx, "fresh", &other)).unwrap();
        let mut edited = b.clone();
        edited.macro_def.as_mut().unwrap().match_kind = MacroMatch::Exact;
        edited.name = "Ford crossing".into();
        edited.compile().unwrap();
        db.write(|tx| save_changes(tx, "fresh", &before, std::slice::from_ref(&edited)))
            .unwrap();
        assert_eq!(db.read(|c| load(c, "fresh")).unwrap(), [edited, other]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// C# `SqliteWorldScriptLibraryStore`: an empty library gets the starter script once; one
    /// that has entries, or was set up before, does not.
    #[test]
    fn the_starter_script_is_added_once_to_an_empty_library() {
        let dir = temp_dir("starter");
        let (db, _) = Database::open(&dir).unwrap();
        let first = starter();
        assert!(!first.enabled && !first.is_macro());
        assert_eq!(first.name, "New script");
        assert!(db.write(|tx| ensure_started(tx, "w", &first)).unwrap());
        assert_eq!(db.read(|c| load(c, "w")).unwrap(), std::slice::from_ref(&first));
        db.write(|tx| delete(tx, "w", &first.id)).unwrap();
        assert!(!db.write(|tx| ensure_started(tx, "w", &starter())).unwrap());
        assert!(db.read(|c| load(c, "w")).unwrap().is_empty(), "deleted for good");
        let kept = plain_script("Kept");
        db.write(|tx| upsert(tx, "v", &kept)).unwrap();
        assert!(!db.write(|tx| ensure_started(tx, "v", &starter())).unwrap());
        assert!(db.read(|c| is_started(c, "v")).unwrap());
        assert_eq!(db.read(|c| load(c, "v")).unwrap(), [kept]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn library_limits_are_checked_before_writing() {
        let mut entry = plain_script("");
        assert_eq!(entry.validate().unwrap_err().to_string(), t(S::ScriptInvalidName));
        entry.name = "x".repeat(121);
        assert!(entry.validate().is_err());
        entry.name = "ok".into();
        entry.id = "not an id".into();
        assert!(entry.validate().is_err());
        let full: Vec<LibraryEntry> = (0..MAX_ENTRIES).map(|i| plain_script(&format!("s{i}"))).collect();
        assert!(validate_library(&full).is_ok());
        let mut over = full.clone();
        over.push(plain_script("one more"));
        assert_eq!(
            validate_library(&over).unwrap_err().to_string(),
            t(S::ScriptLibraryTooLarge)
        );
        let mut twice = full[..2].to_vec();
        twice[1].id = twice[0].id.clone();
        assert!(validate_library(&twice).is_err());
        let bad = LibraryEntry::new_macro("Bad", MacroDefinition::new(MacroKind::Trigger, "", "look"));
        assert!(bad.validate().is_err(), "an invalid macro has no valid source");
    }

    /// C# `MudletLuaWrapperTests.LanguageAndCompatibilityRoundTripThroughStorage`, with the
    /// import record: Lua with the Mudlet layer reads back as saved, scripts saved before Lua
    /// read as JavaScript with no layer, and a layer without Lua (or a Lua macro) is refused.
    #[test]
    fn language_compatibility_and_import_round_trip_through_storage() {
        let dir = temp_dir("lua");
        let (db, _) = Database::open(&dir).unwrap();
        let world = "lanternroad.example.net:4100:False";
        let javascript = plain_script("Old");
        let mut lua = plain_script("Lantern");
        lua.language = Language::Lua;
        lua.compatibility = Compatibility::Mudlet;
        lua.import = Some(ImportInfo {
            origin: ImportInfo::MUDLET.into(),
            source: "The Lantern Road".into(),
            group: "Combat".into(),
            needs_conversion: false,
            items: vec![ImportedItem {
                kind: "trigger".into(),
                path: "Combat".into(),
                name: "Low health".into(),
                reason: "lua".into(),
                code: "send(\"quaff tonic\")".into(),
                patterns: vec!["regex:^HP: (\\d+)".into()],
                command: String::new(),
                active: true,
            }],
            source_hash: ImportInfo::hash("mud.echo('kept');"),
        });
        db.write(|tx| upsert(tx, world, &javascript)).unwrap();
        db.write(|tx| upsert(tx, world, &lua)).unwrap();
        let (reopened, _) = Database::open(&dir).unwrap();
        let read = reopened.read(|c| load(c, world)).unwrap();
        assert_eq!(read, [javascript.clone(), lua.clone()]);
        assert_eq!(read[0].runtime(), Runtime::JAVASCRIPT);
        assert_eq!(read[1].runtime(), Runtime::MUDLET);
        assert!(read[1].import.as_ref().unwrap().is_unchanged(&read[1].source));
        let stored: (Option<String>, Option<String>) = reopened
            .read(|c| {
                Ok(c.query_row(
                    "SELECT language, compatibility FROM scripts WHERE id = ?1",
                    params![javascript.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            })
            .unwrap();
        assert_eq!(stored, (None, None), "JavaScript with no layer is stored as NULL");
        let mut layer_without_lua = plain_script("Wrong");
        layer_without_lua.compatibility = Compatibility::Mudlet;
        assert!(reopened.write(|tx| upsert(tx, world, &layer_without_lua)).is_err());
        let mut lua_macro = LibraryEntry::new_macro("Key", MacroDefinition::new(MacroKind::Shortcut, "F1", "look"));
        lua_macro.language = Language::Lua;
        assert!(lua_macro.validate().is_err());
        let mut bad_import = lua.clone();
        bad_import.id = new_id();
        bad_import.import.as_mut().unwrap().origin = "tintin".into();
        assert_eq!(
            reopened
                .write(|tx| upsert(tx, world, &bad_import))
                .unwrap_err()
                .to_string(),
            t(S::ScriptImportInvalid)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The import ids are the C# ones: stable for the same world, origin, source and bucket.
    #[test]
    fn import_ids_and_hashes_are_stable() {
        let a = ImportInfo::id_for("w:1:False", "mudlet", "The Lantern Road", "on\nTravel\n0");
        assert_eq!(
            a,
            ImportInfo::id_for("w:1:False", "mudlet", "The Lantern Road", "on\nTravel\n0")
        );
        assert_ne!(
            a,
            ImportInfo::id_for("w:1:False", "mudlet", "The Lantern Road", "on\nCombat\n0")
        );
        assert!(uuid::Uuid::parse_str(&a).is_ok());
        assert_eq!(
            ImportInfo::hash("abc"),
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        );
    }
}
