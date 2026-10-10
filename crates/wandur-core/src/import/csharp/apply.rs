//! The merge of a [`CsharpSource`] into this client's data, in one transaction.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Deserialize;
use serde_json::Value;

use super::secrets::{self, SecretJob, SecretKind};
use super::{CsharpSource, DB_FILE, ImportError, Kind, Report, Skip, copy_read_only, snapshot_dir, unreadable};
use crate::channels::ChannelRule;
use crate::charset::Charset;
use crate::db::scripts::{self as library, LibraryEntry};
use crate::db::{Database, worlds};
use crate::login::PasswordVault;
use crate::map::merge::combine;
use crate::map::model::MapSnapshot;
use crate::map::store::{read_map, write_map};
use crate::scripting::{Compatibility, Language};
use crate::settings::{CustomTheme, SETTINGS_FILE, SavedWorld, Settings};

/// .NET ticks (100 ns since 0001-01-01) at 1970-01-01.
const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;
/// The settings scrollback bound used while importing (the terminal's own maximum).
const MAX_SCROLLBACK: usize = 50_000;

/// How to run an import from the command line or the app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportOptions {
    /// Work on a copy and write nothing.
    pub dry_run: bool,
    /// Copy saved passwords and agent API keys too (the system may ask the person).
    pub include_secrets: bool,
}

/// Where an import is: `done` of `total` in the step that imports `kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub kind: Kind,
    pub done: usize,
    pub total: usize,
}

/// What [`import`] did.
#[derive(Debug)]
pub struct Outcome {
    pub report: Report,
    /// This client's settings with the imported worlds and preferences: what the caller keeps.
    pub settings: Settings,
    /// Passwords and API keys to copy after the commit ([`secrets::copy`]).
    pub secrets: Vec<SecretJob>,
}

fn target(e: impl std::fmt::Display) -> ImportError {
    ImportError::Target(e.to_string())
}

/// Merge the C# data into `db` and a copy of `settings`, in one transaction. `before_commit`
/// gets the new settings just before the commit (the command line writes `settings.json` there);
/// when it fails nothing is committed. With `dry_run` the transaction is rolled back.
pub fn import(
    source: &CsharpSource,
    db: &Database,
    settings: &Settings,
    dry_run: bool,
    progress: &dyn Fn(Progress),
    before_commit: &mut dyn FnMut(&Settings) -> Result<(), String>,
) -> Result<Outcome, ImportError> {
    let mut conn = db.connect().map_err(target)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(target)?;
    let mut run = Run {
        src: source,
        tx: &tx,
        report: Report {
            schema: source.schema(),
            dry_run,
            ..Report::default()
        },
        settings: settings.clone(),
        worlds: HashMap::new(),
        secrets: Vec::new(),
        progress,
    };
    run.worlds()?;
    run.saved_worlds()?;
    run.usage()?;
    run.preferences()?;
    run.scripts()?;
    run.legacy_scripts()?;
    run.agents()?;
    run.maps()?;
    run.legacy_maps()?;
    run.history()?;
    run.others()?;
    let Run {
        report,
        settings,
        secrets,
        ..
    } = run;
    if dry_run {
        drop(tx);
    } else {
        before_commit(&settings).map_err(ImportError::Target)?;
        tx.commit().map_err(target)?;
    }
    Ok(Outcome {
        report,
        settings,
        secrets,
    })
}

/// Import into the data directory `dir` (its `wandur.db` and `settings.json`), as the command
/// line does. With [`ImportOptions::dry_run`] the work happens on a copy of both files and
/// nothing in `dir` is written (or created). Secrets are copied from `from` to `to` only with
/// [`ImportOptions::include_secrets`].
pub fn import_into_dir(
    source: &CsharpSource,
    dir: &Path,
    options: ImportOptions,
    from: &dyn PasswordVault,
    to: &dyn PasswordVault,
    progress: &dyn Fn(Progress),
) -> Result<Report, ImportError> {
    if options.dry_run {
        let copy = snapshot_dir().map_err(target)?;
        let result = (|| {
            for name in [DB_FILE.to_string(), format!("{DB_FILE}-wal"), SETTINGS_FILE.to_string()] {
                let file = dir.join(&name);
                if file.is_file() {
                    copy_read_only(&file, &copy.join(&name)).map_err(target)?;
                }
            }
            let (db, _) = Database::open(&copy).map_err(target)?;
            let (settings, _) = Settings::load(&copy, MAX_SCROLLBACK);
            let outcome = import(source, &db, &settings, true, progress, &mut |_| Ok(()))?;
            let mut report = outcome.report;
            if !options.include_secrets {
                secrets::not_requested(&outcome.secrets, &mut report);
            }
            Ok(report)
        })();
        let _ = std::fs::remove_dir_all(&copy);
        return result;
    }
    let (db, _) = Database::open(dir).map_err(target)?;
    let (settings, _) = Settings::load(dir, MAX_SCROLLBACK);
    let path = dir.join(SETTINGS_FILE);
    let previous = std::fs::read(&path).ok();
    let mut wrote = false;
    let result = import(source, &db, &settings, false, progress, &mut |new| {
        wrote = true;
        new.save(dir).map_err(|e| e.to_string())
    });
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(e) => {
            // The database rolled back; put the settings file back as it was.
            if wrote {
                let _ = match &previous {
                    Some(bytes) => crate::settings::write_atomic(&path, bytes),
                    None => std::fs::remove_file(&path),
                };
            }
            return Err(e);
        }
    };
    let mut report = outcome.report;
    if options.include_secrets {
        secrets::copy(&outcome.secrets, from, to, &mut report, &|done, total| {
            progress(Progress {
                kind: Kind::Passwords,
                done,
                total,
            })
        });
    } else {
        secrets::not_requested(&outcome.secrets, &mut report);
    }
    Ok(report)
}

/// A C# saved world (`ConnectionProfile`), as its JSON payload has it.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct CsProfile {
    id: String,
    name: String,
    host: String,
    port: i64,
    use_tls: bool,
    encoding: String,
    #[serde(deserialize_with = "crate::directory::world_theme::lenient")]
    theme: Option<crate::directory::WorldTheme>,
    codebase: String,
    channel_rules: Vec<Value>,
    username: String,
    password_id: Option<String>,
    auto_login: bool,
    username_prompt: Option<String>,
    password_prompt: Option<String>,
    #[serde(deserialize_with = "crate::protocol::mapping::lenient")]
    protocol_mapping: Option<crate::protocol::mapping::WorldMapping>,
}

struct Run<'a> {
    src: &'a CsharpSource,
    tx: &'a Connection,
    report: Report,
    settings: Settings,
    /// C# world id to this client's world id.
    worlds: HashMap<String, String>,
    secrets: Vec<SecretJob>,
    progress: &'a dyn Fn(Progress),
}

type R<T> = Result<T, ImportError>;

/// A C# world's usage summary: connections, the last one (seconds), the character last played.
type CsUsage = (u32, Option<u64>, Option<String>);

impl Run<'_> {
    fn step(&self, kind: Kind, done: usize, total: usize) {
        (self.progress)(Progress { kind, done, total });
    }

    fn src_rows<T>(&self, sql: &str, row: impl FnMut(&rusqlite::Row) -> rusqlite::Result<T>) -> R<Vec<T>> {
        let mut s = self.src.conn().prepare(sql).map_err(unreadable)?;
        let rows = s.query_map([], row).map_err(unreadable)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(unreadable)
    }

    fn mapped(&self, cs_world: &str) -> String {
        self.worlds
            .get(cs_world)
            .cloned()
            .unwrap_or_else(|| cs_world.to_string())
    }

    /// World ids and addresses. A C# world keeps its id unless this client already knows one of
    /// its addresses under another id; then the data goes to that world.
    fn worlds(&mut self) -> R<()> {
        self.step(Kind::Worlds, 0, 1);
        // Worlds of saved profiles first, in their order, so their addresses win a conflict.
        let mut order: Vec<String> = self.src_rows(
            "SELECT world_id FROM profiles WHERE world_id IS NOT NULL ORDER BY position, id",
            |r| r.get(0),
        )?;
        for id in self.src_rows("SELECT id FROM worlds ORDER BY id", |r| r.get::<_, String>(0))? {
            if !order.contains(&id) {
                order.push(id);
            }
        }
        let mut keys: HashMap<String, Vec<String>> = HashMap::new();
        for (key, world) in self.src_rows("SELECT endpoint_key, world_id FROM endpoints ORDER BY rowid", |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            keys.entry(world).or_default().push(key);
        }
        let mut legacy: HashMap<String, Vec<String>> = HashMap::new();
        if self.src.has_table("legacy_endpoints") {
            for (key, world) in self.src_rows(
                "SELECT source_key, world_id FROM legacy_endpoints ORDER BY rowid",
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )? {
                legacy.entry(world).or_default().push(key);
            }
        }
        for world in order {
            if !worlds::valid_world_id(&world) {
                continue;
            }
            let mut canonical: Vec<String> = Vec::new();
            let mut invalid = 0;
            for key in keys.get(&world).into_iter().flatten() {
                match worlds::canonical_endpoint_key(key) {
                    Ok(k) if !canonical.contains(&k) => canonical.push(k),
                    Ok(_) => {}
                    Err(_) => invalid += 1,
                }
            }
            let main = canonical.len() as u64;
            // An older spelling (with the C# `:True` suffix, an IPv6 host without brackets)
            // that leads to a key not listed yet is an address too.
            for key in legacy.get(&world).into_iter().flatten() {
                if let Ok(k) = worlds::canonical_endpoint_key(key)
                    && !canonical.contains(&k)
                {
                    canonical.push(k);
                }
            }
            self.report
                .found(Kind::Addresses, main + invalid + (canonical.len() as u64 - main));
            self.report.skip(Kind::Addresses, Skip::Invalid, invalid);
            let exists = self
                .tx
                .query_row("SELECT 1 FROM worlds WHERE id = ?1", [&world], |_| Ok(()))
                .optional()
                .map_err(target)?
                .is_some();
            let mut mapped = world.clone();
            if !exists {
                for key in &canonical {
                    if let Some(owner) = worlds::find_world_key(self.tx, key).map_err(target)? {
                        mapped = owner;
                        break;
                    }
                }
            }
            self.tx
                .execute("INSERT OR IGNORE INTO worlds(id) VALUES(?1)", [&mapped])
                .map_err(target)?;
            for key in &canonical {
                match worlds::find_world_key(self.tx, key).map_err(target)? {
                    None => {
                        self.tx
                            .execute(
                                "INSERT INTO endpoints(endpoint_key, world_id) VALUES(?1, ?2)",
                                params![key, mapped],
                            )
                            .map_err(target)?;
                        self.report.imported(Kind::Addresses, 1);
                    }
                    Some(owner) if owner == mapped => self.report.unchanged(Kind::Addresses, 1),
                    Some(_) => self.report.skip(Kind::Addresses, Skip::AddressTaken, 1),
                }
            }
            self.worlds.insert(world, mapped);
        }
        Ok(())
    }

    /// The C# usage summary per world: connections and the last connection (seconds).
    fn cs_usage(&self) -> R<HashMap<String, CsUsage>> {
        if !self.src.has_table("world_usage") {
            return Ok(HashMap::new());
        }
        let has_character = super::column_names(self.src.conn(), "world_usage")
            .map_err(unreadable)?
            .iter()
            .any(|c| c == "last_character");
        let sql = if has_character {
            "SELECT world_id, connections, last_connected_at, last_character FROM world_usage"
        } else {
            "SELECT world_id, connections, last_connected_at, NULL FROM world_usage"
        };
        let rows = self.src_rows(sql, |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        Ok(rows
            .into_iter()
            .map(|(world, n, at, character)| {
                let at = at.as_deref().and_then(crate::directory::time::parse_rfc3339);
                let at = at.and_then(|s| u64::try_from(s).ok());
                (world, (n.clamp(0, u32::MAX as i64) as u32, at, character))
            })
            .collect())
    }

    /// Saved worlds (C# profiles) into `settings.json`'s list.
    fn saved_worlds(&mut self) -> R<()> {
        let usage = self.cs_usage()?;
        let rows = self.src_rows(
            "SELECT id, world_id, payload FROM profiles ORDER BY position, id",
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )?;
        let total = rows.len();
        for (i, (_, world, payload)) in rows.into_iter().enumerate() {
            self.step(Kind::Worlds, i, total);
            self.report.found(Kind::Worlds, 1);
            let Ok(profile) = serde_json::from_str::<CsProfile>(&payload) else {
                self.report.skip(Kind::Worlds, Skip::Invalid, 1);
                continue;
            };
            let Some(world) = world.filter(|w| worlds::valid_world_id(w)) else {
                self.report.skip(Kind::Worlds, Skip::Invalid, 1);
                continue;
            };
            let has_password = profile.password_id.is_some();
            if has_password {
                self.report.found(Kind::Passwords, 1);
            }
            let rules_found = profile.channel_rules.len() as u64;
            self.report.found(Kind::ChannelRules, rules_found);
            let (rules, bad_syntax, invalid) = channel_rules(&profile.channel_rules);
            let Some(port) = u16::try_from(profile.port).ok().filter(|p| *p > 0) else {
                self.report.skip(Kind::Worlds, Skip::Invalid, 1);
                self.report.skip(Kind::ChannelRules, Skip::Invalid, rules_found);
                self.skip_password(has_password, Skip::Invalid);
                continue;
            };
            let (connections, last_connected, _) = usage.get(&world).cloned().unwrap_or_default();
            let mapped = self.mapped(&world);
            let imported = SavedWorld {
                world_id: mapped.clone(),
                name: profile.name.trim().to_string(),
                host: profile.host.trim().to_string(),
                port,
                tls: profile.use_tls,
                charset: if profile.encoding.eq_ignore_ascii_case("latin1") {
                    Charset::Latin1
                } else {
                    Charset::Utf8
                },
                // Trimmed as the world editor saves it, so the editor's copy has the same vault
                // key and a blank password keeps the saved one (the C# key below keeps the C#
                // spelling, to find the C# entry).
                username: profile.username.trim().to_string(),
                password_id: profile.password_id.as_ref().map(|p| p.to_lowercase()),
                auto_login: profile.auto_login,
                username_prompt: profile
                    .username_prompt
                    .clone()
                    .unwrap_or_else(|| crate::login::DEFAULT_USERNAME_PROMPT.into()),
                password_prompt: profile
                    .password_prompt
                    .clone()
                    .unwrap_or_else(|| crate::login::DEFAULT_PASSWORD_PROMPT.into()),
                protocol_mapping: profile.protocol_mapping.clone(),
                codebase: profile.codebase.clone(),
                channel_rules: rules.clone(),
                theme: profile.theme.clone(),
                last_connected: last_connected.filter(|_| connections > 0),
                connections,
                ..SavedWorld::default()
            };
            if imported.validate().is_err() {
                self.report.skip(Kind::Worlds, Skip::Invalid, 1);
                self.report.skip(Kind::ChannelRules, Skip::Invalid, rules_found);
                self.skip_password(has_password, Skip::Invalid);
                continue;
            }
            self.report.skip(Kind::ChannelRules, Skip::PatternSyntax, bad_syntax);
            self.report.skip(Kind::ChannelRules, Skip::Invalid, invalid);
            let existing = self
                .settings
                .worlds
                .iter()
                .position(|w| w.world_id == mapped && w.is_at(&imported.endpoint()));
            let saved = match existing {
                None => {
                    self.settings.worlds.push(imported.clone());
                    self.report.imported(Kind::Worlds, 1);
                    self.report.imported(Kind::ChannelRules, rules.len() as u64);
                    imported.clone()
                }
                Some(index) => {
                    let current = &mut self.settings.worlds[index];
                    if same_world(current, &imported) {
                        self.report.unchanged(Kind::Worlds, 1);
                        self.report.unchanged(Kind::ChannelRules, rules.len() as u64);
                    } else {
                        self.report.skip(Kind::Worlds, Skip::KeptRust, 1);
                        let kept = rules.iter().filter(|r| current.channel_rules.contains(r)).count() as u64;
                        self.report.unchanged(Kind::ChannelRules, kept);
                        self.report
                            .skip(Kind::ChannelRules, Skip::KeptRust, rules.len() as u64 - kept);
                    }
                    // Usage only ever grows.
                    current.connections = current.connections.max(imported.connections);
                    current.last_connected = current.last_connected.max(imported.last_connected);
                    current.clone()
                }
            };
            if let Some(password) = &profile.password_id {
                if saved.password_id.as_deref() == Some(password.to_lowercase().as_str()) {
                    let from_key = secrets::csharp_login_key(
                        &profile.id,
                        password,
                        &profile.host,
                        port,
                        profile.use_tls,
                        &profile.username,
                    );
                    match crate::login::vault::key(&saved) {
                        Ok(to_key) => self.secrets.push(SecretJob {
                            kind: SecretKind::Password,
                            from_key,
                            to_key,
                        }),
                        Err(_) => self.report.skip(Kind::Passwords, Skip::Invalid, 1),
                    }
                } else {
                    self.report.skip(Kind::Passwords, Skip::KeptRust, 1);
                }
            }
        }
        self.step(Kind::Worlds, total, total);
        Ok(())
    }

    fn skip_password(&mut self, has_password: bool, reason: Skip) {
        if has_password {
            self.report.skip(Kind::Passwords, reason, 1);
        }
    }

    /// The connection log and the usage summary per world.
    fn usage(&mut self) -> R<()> {
        if !self.src.has_table("world_connections") {
            return Ok(());
        }
        let rows = self.src_rows(
            "SELECT world_id, connected_at FROM world_connections ORDER BY id",
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?;
        let mut added: HashMap<String, (u32, i64)> = HashMap::new();
        for (world, at) in rows {
            self.report.found(Kind::Usage, 1);
            let Some(at) = crate::directory::time::parse_rfc3339(&at).filter(|s| *s >= 0) else {
                self.report.skip(Kind::Usage, Skip::Invalid, 1);
                continue;
            };
            let mapped = self.mapped(&world);
            let seen = self
                .tx
                .query_row(
                    "SELECT 1 FROM world_connections WHERE world_id = ?1 AND connected_at = ?2",
                    params![mapped, at],
                    |_| Ok(()),
                )
                .optional()
                .map_err(target)?
                .is_some();
            if seen {
                self.report.unchanged(Kind::Usage, 1);
                continue;
            }
            self.tx
                .execute("INSERT OR IGNORE INTO worlds(id) VALUES(?1)", [&mapped])
                .map_err(target)?;
            self.tx
                .execute(
                    "INSERT INTO world_connections(world_id, connected_at) VALUES(?1, ?2)",
                    params![mapped, at],
                )
                .map_err(target)?;
            self.report.imported(Kind::Usage, 1);
            let entry = added.entry(mapped).or_insert((0, 0));
            entry.0 += 1;
            entry.1 = entry.1.max(at);
        }
        for (world, (_, last, character)) in self.cs_usage()? {
            let mapped = self.mapped(&world);
            let (count, newest) = added.remove(&mapped).unwrap_or((0, 0));
            let last = last.map_or(newest, |l| (l as i64).max(newest));
            self.upsert_usage(&mapped, count, last, character.as_deref())?;
        }
        for (mapped, (count, newest)) in added {
            self.upsert_usage(&mapped, count, newest, None)?;
        }
        Ok(())
    }

    fn upsert_usage(&self, world: &str, count: u32, last: i64, character: Option<&str>) -> R<()> {
        self.tx
            .execute("INSERT OR IGNORE INTO worlds(id) VALUES(?1)", [world])
            .map_err(target)?;
        self.tx
            .execute(
                "INSERT INTO world_usage(world_id, connections, last_connected_at, last_character)
                 VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(world_id) DO UPDATE SET
                     connections = connections + excluded.connections,
                     last_connected_at = MAX(last_connected_at, excluded.last_connected_at),
                     last_character = COALESCE(world_usage.last_character, excluded.last_character)",
                params![world, count, last, character.filter(|c| !c.trim().is_empty())],
            )
            .map_err(target)?;
        Ok(())
    }

    /// Preferences that have a counterpart here, and the colour schemes. They are taken when
    /// this client's preferences are still the defaults; otherwise this client's are kept.
    fn preferences(&mut self) -> R<()> {
        let payload: Option<String> = self
            .src
            .conn()
            .query_row("SELECT payload FROM client_settings WHERE id = 1", [], |r| r.get(0))
            .optional()
            .map_err(unreadable)?;
        let Some(payload) = payload else { return Ok(()) };
        self.report.found(Kind::Preferences, 1);
        let Ok(Value::Object(cs)) = serde_json::from_str::<Value>(&payload) else {
            self.report.skip(Kind::Preferences, Skip::Invalid, 1);
            return Ok(());
        };
        let themes: Vec<Option<CustomTheme>> = match cs.get("CustomThemes") {
            Some(Value::Array(list)) => list.iter().map(custom_theme).collect(),
            _ => Vec::new(),
        };
        self.report.found(Kind::ColorSchemes, themes.len() as u64);
        let invalid_themes = themes.iter().filter(|t| t.is_none()).count() as u64;
        self.report.skip(Kind::ColorSchemes, Skip::Invalid, invalid_themes);
        let themes: Vec<CustomTheme> = themes.into_iter().flatten().collect();
        let mut candidate = self.settings.clone();
        apply_preferences(&mut candidate, &cs, &themes);
        let decision = if preferences(&candidate) == preferences(&self.settings) {
            self.report.unchanged(Kind::Preferences, 1);
            false
        } else if preferences(&self.settings) == preferences(&Settings::default()) {
            self.settings = candidate;
            self.report.imported(Kind::Preferences, 1);
            true
        } else {
            self.report.skip(Kind::Preferences, Skip::KeptRust, 1);
            false
        };
        for theme in &themes {
            match self.settings.custom_themes.iter().find(|t| t.id == theme.id) {
                Some(t) if same_theme(t, theme) && decision => self.report.imported(Kind::ColorSchemes, 1),
                Some(t) if same_theme(t, theme) => self.report.unchanged(Kind::ColorSchemes, 1),
                _ => self.report.skip(Kind::ColorSchemes, Skip::KeptRust, 1),
            }
        }
        for (key, reason) in [
            ("InstallId", Skip::InstallId),
            ("LastUpdateCheck", Skip::CsharpReleases),
            ("SkippedUpdateVersion", Skip::CsharpReleases),
        ] {
            if cs.get(key).is_some_and(|v| !v.is_null()) {
                self.report.found(Kind::OtherSettings, 1);
                self.report.skip(Kind::OtherSettings, reason, 1);
            }
        }
        Ok(())
    }

    /// Script libraries: scripts and macros keep their ids.
    fn scripts(&mut self) -> R<()> {
        let column = |name: &str| {
            if self.src.has_script_column(name) {
                name.to_string()
            } else {
                "NULL".to_string()
            }
        };
        let sql = format!(
            "SELECT world_id, id, name, source, enabled, {}, {}, {}, {}, {} FROM scripts ORDER BY world_id, rowid",
            column("macro_json"),
            column("pack_json"),
            column("import_json"),
            column("language"),
            column("compatibility"),
        );
        type Row = (
            String,
            String,
            String,
            String,
            i64,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let rows: Vec<Row> = self.src_rows(&sql, |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
            ))
        })?;
        let mut libraries: HashMap<String, Option<Vec<LibraryEntry>>> = HashMap::new();
        let total = rows.len();
        for (i, (world, id, name, source, enabled, macro_json, pack_json, import_json, language, compatibility)) in
            rows.into_iter().enumerate()
        {
            self.step(Kind::Scripts, i, total);
            let kind = if macro_json.is_some() {
                Kind::Macros
            } else {
                Kind::Scripts
            };
            self.report.found(kind, 1);
            let entry = (|| {
                Some(LibraryEntry {
                    id: id.to_lowercase(),
                    name,
                    source,
                    enabled: match enabled {
                        0 => false,
                        1 => true,
                        _ => return None,
                    },
                    macro_def: match &macro_json {
                        Some(json) => Some(serde_json::from_str(json).ok()?),
                        None => None,
                    },
                    pack_json,
                    import: match &import_json {
                        Some(json) => Some(serde_json::from_str(json).ok()?),
                        None => None,
                    },
                    language: Language::parse(language.as_deref())?,
                    compatibility: Compatibility::parse(compatibility.as_deref())?,
                })
            })()
            .filter(|e| e.validate().is_ok());
            let Some(entry) = entry else {
                self.report.skip(kind, Skip::Invalid, 1);
                continue;
            };
            let mapped = self.mapped(&world);
            if !libraries.contains_key(&mapped) {
                libraries.insert(mapped.clone(), library::load(self.tx, &mapped).ok());
            }
            let Some(existing) = libraries.get_mut(&mapped).and_then(Option::as_mut) else {
                // This client's library for the world cannot be read; it is left alone.
                self.report.skip(kind, Skip::Invalid, 1);
                continue;
            };
            add_entry(self.tx, &mut self.report, existing, &mapped, kind, entry)?;
        }
        // A library the C# client set up (its starter script given) is set up here too.
        let started = self.src_rows("SELECT key FROM imports WHERE key LIKE 'script-library:%'", |r| {
            r.get::<_, String>(0)
        })?;
        for key in started {
            let world = &key["script-library:".len()..];
            let mapped = self.mapped(world);
            self.tx
                .execute("INSERT OR IGNORE INTO worlds(id) VALUES(?1)", [&mapped])
                .map_err(target)?;
            self.tx
                .execute(
                    "INSERT OR IGNORE INTO imports(key) VALUES(?1)",
                    [format!("script-library:{mapped}")],
                )
                .map_err(target)?;
        }
        self.step(Kind::Scripts, total, total);
        Ok(())
    }

    /// Older C# script files not yet in the C# database: their scripts join the library unless a
    /// script with that id is there (or was deleted in the C# client).
    fn legacy_scripts(&mut self) -> R<()> {
        let legacy = self.src.legacy_scripts()?;
        if legacy.is_empty() {
            return Ok(());
        }
        let deleted: Vec<(String, String)> = if self.src.has_table("script_deletions") {
            self.src_rows("SELECT world_id, id FROM script_deletions", |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
        } else {
            Vec::new()
        };
        for (world, stem, path) in legacy {
            let bytes = std::fs::read(&path)
                .ok()
                .filter(|b| b.len() <= library::MAX_LIBRARY_BYTES);
            let text = bytes.and_then(|b| String::from_utf8(b).ok());
            let text = text.map(|t| t.trim_start_matches('\u{feff}').to_string());
            let entries: Vec<Option<LibraryEntry>> = match text {
                None => vec![None],
                Some(text) if path.extension().is_some_and(|e| e == "js") => {
                    // One script without an id: a stable one from the file, so a second import
                    // finds it.
                    use sha2::Digest;
                    let hash = sha2::Sha256::digest(format!("wandur-legacy-script\n{stem}").as_bytes());
                    let mut id = [0u8; 16];
                    id.copy_from_slice(&hash[..16]);
                    let entry = LibraryEntry {
                        id: uuid::Builder::from_random_bytes(id)
                            .into_uuid()
                            .hyphenated()
                            .to_string(),
                        name: crate::l10n::t(crate::l10n::S::ScriptDefaultName).into(),
                        source: text,
                        ..LibraryEntry::default()
                    };
                    vec![entry.validate().is_ok().then_some(entry)]
                }
                Some(text) => match serde_json::from_str::<Value>(&text) {
                    Ok(Value::Array(items)) => items.iter().map(legacy_entry).collect(),
                    _ => vec![None],
                },
            };
            let mapped = self.mapped(&world);
            let Ok(mut existing) = library::load(self.tx, &mapped) else {
                for entry in &entries {
                    let kind = if entry.as_ref().is_some_and(LibraryEntry::is_macro) {
                        Kind::Macros
                    } else {
                        Kind::Scripts
                    };
                    self.report.found(kind, 1);
                    self.report.skip(kind, Skip::Invalid, 1);
                }
                continue;
            };
            for entry in entries {
                let kind = if entry.as_ref().is_some_and(LibraryEntry::is_macro) {
                    Kind::Macros
                } else {
                    Kind::Scripts
                };
                self.report.found(kind, 1);
                let Some(entry) = entry else {
                    self.report.skip(kind, Skip::Invalid, 1);
                    continue;
                };
                if deleted
                    .iter()
                    .any(|(w, id)| *w == world && id.eq_ignore_ascii_case(&entry.id))
                {
                    self.report.skip(kind, Skip::Deleted, 1);
                    continue;
                }
                add_entry(self.tx, &mut self.report, &mut existing, &mapped, kind, entry)?;
            }
        }
        Ok(())
    }

    /// Agent profiles, one per world, with their ids (an API key is bound to the id).
    fn agents(&mut self) -> R<()> {
        if !self.src.has_table("world_agent_profiles") {
            return Ok(());
        }
        let rows = self.src_rows(
            "SELECT world_id, payload FROM world_agent_profiles ORDER BY world_id",
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?;
        let total = rows.len();
        for (i, (world, payload)) in rows.into_iter().enumerate() {
            self.step(Kind::AgentProfiles, i, total);
            self.report.found(Kind::AgentProfiles, 1);
            let Some((encoded, credential)) = agent_payload(&payload) else {
                self.report.skip(Kind::AgentProfiles, Skip::Invalid, 1);
                continue;
            };
            if credential.is_some() {
                self.report.found(Kind::ApiKeys, 1);
            }
            let mapped = self.mapped(&world);
            let stored: Option<String> = self
                .tx
                .query_row(
                    "SELECT payload FROM world_agent_profiles WHERE world_id = ?1",
                    [&mapped],
                    |r| r.get(0),
                )
                .optional()
                .map_err(target)?;
            let same = match &stored {
                None => {
                    self.tx
                        .execute("INSERT OR IGNORE INTO worlds(id) VALUES(?1)", [&mapped])
                        .map_err(target)?;
                    self.tx
                        .execute(
                            "INSERT INTO world_agent_profiles(world_id, payload) VALUES(?1, ?2)",
                            params![mapped, encoded],
                        )
                        .map_err(target)?;
                    self.report.imported(Kind::AgentProfiles, 1);
                    true
                }
                Some(s) if agent_payload(s).is_some_and(|(e, _)| e == encoded) => {
                    self.report.unchanged(Kind::AgentProfiles, 1);
                    true
                }
                Some(_) => {
                    self.report.skip(Kind::AgentProfiles, Skip::KeptRust, 1);
                    false
                }
            };
            if let Some(key) = credential {
                if same {
                    self.secrets.push(SecretJob {
                        kind: SecretKind::ApiKey,
                        from_key: key.clone(),
                        to_key: key,
                    });
                } else {
                    self.report.skip(Kind::ApiKeys, Skip::KeptRust, 1);
                }
            }
        }
        self.step(Kind::AgentProfiles, total, total);
        Ok(())
    }

    /// Saved maps: every room, exit, area, alias and tombstone, merged by revision with a map
    /// this client already has for the world.
    fn maps(&mut self) -> R<()> {
        if !self.src.has_table("map_snapshots") {
            return Ok(());
        }
        let worlds = self.src_rows("SELECT world_id FROM map_snapshots ORDER BY world_id", |r| {
            r.get::<_, String>(0)
        })?;
        let total = worlds.len();
        for (i, world) in worlds.into_iter().enumerate() {
            self.step(Kind::Maps, i, total);
            self.report.found(Kind::Maps, 1);
            let cs = match read_map(self.src.conn(), &world) {
                Ok(Some(map)) => map,
                _ => {
                    let count = |sql: &str| -> u64 {
                        self.src
                            .conn()
                            .query_row(sql, [&world], |r| r.get::<_, i64>(0))
                            .map_or(0, |n| n.max(0) as u64)
                    };
                    let (rooms, exits) = (
                        count("SELECT COUNT(*) FROM map_rooms WHERE world_id = ?1"),
                        count("SELECT COUNT(*) FROM map_links WHERE world_id = ?1"),
                    );
                    self.report.found(Kind::Rooms, rooms);
                    self.report.found(Kind::Exits, exits);
                    self.report.skip(Kind::Maps, Skip::Invalid, 1);
                    self.report.skip(Kind::Rooms, Skip::Invalid, rooms);
                    self.report.skip(Kind::Exits, Skip::Invalid, exits);
                    continue;
                }
            };
            let mapped = self.mapped(&world);
            self.merge_map(&mapped, &cs, false)?;
        }
        self.step(Kind::Maps, total, total);
        Ok(())
    }

    /// Merge one C# map into this client's map of the world `mapped` and count it.
    fn merge_map(&mut self, mapped: &str, cs: &MapSnapshot, older: bool) -> R<()> {
        self.report.found(Kind::Rooms, cs.rooms.len() as u64);
        self.report.found(Kind::Exits, cs.links.len() as u64);
        let ours = match read_map(self.tx, mapped) {
            Ok(map) => map,
            Err(_) => {
                // This client's map for the world cannot be read; it is left alone.
                self.report.skip(Kind::Maps, Skip::KeptRust, 1);
                self.report.skip(Kind::Rooms, Skip::KeptRust, cs.rooms.len() as u64);
                self.report.skip(Kind::Exits, Skip::KeptRust, cs.links.len() as u64);
                return Ok(());
            }
        };
        // A map file the C# client never moved into its database is older than its database.
        let merged = match &ours {
            Some(ours) if older => combine(cs, ours),
            Some(ours) => combine(ours, cs),
            None => cs.clone(),
        };
        if crate::map::format::validate(&merged).is_err() {
            self.report.skip(Kind::Maps, Skip::Invalid, 1);
            self.report.skip(Kind::Rooms, Skip::Invalid, cs.rooms.len() as u64);
            self.report.skip(Kind::Exits, Skip::Invalid, cs.links.len() as u64);
            return Ok(());
        }
        if ours.as_ref().is_some_and(|o| same_map(o, &merged)) {
            self.report.unchanged(Kind::Maps, 1);
        } else {
            self.tx
                .execute("INSERT OR IGNORE INTO worlds(id) VALUES(?1)", [mapped])
                .map_err(target)?;
            write_map(self.tx, mapped, &merged).map_err(target)?;
            self.report.imported(Kind::Maps, 1);
        }
        for room in &cs.rooms {
            match merged.room(&room.id) {
                Some(r) if r == room => {
                    if ours.as_ref().and_then(|o| o.room(&room.id)) == Some(room) {
                        self.report.unchanged(Kind::Rooms, 1);
                    } else {
                        self.report.imported(Kind::Rooms, 1);
                    }
                }
                Some(_) => self.report.skip(Kind::Rooms, Skip::KeptRust, 1),
                None => self.report.skip(Kind::Rooms, Skip::Deleted, 1),
            }
        }
        let link = |map: &MapSnapshot, from: &str, direction: &str| {
            map.links
                .iter()
                .find(|l| l.from_id == from && l.direction == direction)
                .cloned()
        };
        for exit in &cs.links {
            match link(&merged, &exit.from_id, &exit.direction) {
                Some(l) if l == *exit => {
                    if ours
                        .as_ref()
                        .and_then(|o| link(o, &exit.from_id, &exit.direction))
                        .as_ref()
                        == Some(exit)
                    {
                        self.report.unchanged(Kind::Exits, 1);
                    } else {
                        self.report.imported(Kind::Exits, 1);
                    }
                }
                Some(_) => self.report.skip(Kind::Exits, Skip::KeptRust, 1),
                None => self.report.skip(Kind::Exits, Skip::Deleted, 1),
            }
        }
        Ok(())
    }

    /// Older C# map files not yet in the C# database, merged under the database's maps.
    fn legacy_maps(&mut self) -> R<()> {
        for (world, path) in self.src.legacy_maps()? {
            self.report.found(Kind::Maps, 1);
            let map = std::fs::metadata(&path)
                .ok()
                .filter(|m| m.len() <= crate::map::format::MAX_BYTES as u64)
                .and_then(|_| std::fs::read_to_string(&path).ok())
                .and_then(|text| crate::map::format::deserialize(&text).ok());
            let Some(map) = map else {
                self.report.skip(Kind::Maps, Skip::Invalid, 1);
                continue;
            };
            let mapped = self.mapped(&world);
            self.merge_map(&mapped, &map, true)?;
        }
        Ok(())
    }

    /// Session history: sessions by id with their entries, deletions kept; the search index is
    /// rebuilt afterwards.
    fn history(&mut self) -> R<()> {
        if !self.src.has_history() {
            return Ok(());
        }
        type SessionRow = (String, String, String, String, i64, Option<i64>);
        let sessions: Vec<SessionRow> = self.src_rows(
            "SELECT id, world_key, world_name, character_name, started_at, ended_at FROM history_sessions
             ORDER BY started_at, id",
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )?;
        let total = sessions.len();
        let mut added = false;
        for (i, (id, world_key, world_name, character, started, ended)) in sessions.into_iter().enumerate() {
            self.step(Kind::HistorySessions, i, total);
            let count: i64 = self
                .src
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM history_entries WHERE session_id = ?1",
                    [&id],
                    |r| r.get(0),
                )
                .map_err(unreadable)?;
            let count = count.max(0) as u64;
            self.report.found(Kind::HistorySessions, 1);
            self.report.found(Kind::HistoryEntries, count);
            let exists = self
                .tx
                .query_row("SELECT 1 FROM history_sessions WHERE id = ?1", [&id], |_| Ok(()))
                .optional()
                .map_err(target)?
                .is_some();
            if exists {
                self.report.unchanged(Kind::HistorySessions, 1);
                self.report.unchanged(Kind::HistoryEntries, count);
                continue;
            }
            let deleted = self
                .tx
                .query_row("SELECT 1 FROM history_deletions WHERE session_id = ?1", [&id], |_| {
                    Ok(())
                })
                .optional()
                .map_err(target)?
                .is_some();
            if deleted {
                self.report.skip(Kind::HistorySessions, Skip::Deleted, 1);
                self.report.skip(Kind::HistoryEntries, Skip::Deleted, count);
                continue;
            }
            let valid_id = |s: &str| !s.trim().is_empty() && s.len() <= 256 && !s.chars().any(char::is_control);
            if !valid_id(&id) || !valid_id(&world_key) || ended.is_some_and(|e| e < started) {
                self.report.skip(Kind::HistorySessions, Skip::Invalid, 1);
                self.report.skip(Kind::HistoryEntries, Skip::Invalid, count);
                continue;
            }
            let key: i64 = self
                .tx
                .query_row(
                    "INSERT INTO history_sessions(id, world_key, world_name, character_name, started_at, ended_at)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6) RETURNING key",
                    params![id, world_key, world_name, character, micros(started), ended.map(micros)],
                    |r| r.get(0),
                )
                .map_err(target)?;
            let mut entries = self
                .src
                .conn()
                .prepare("SELECT sequence, at, kind, text FROM history_entries WHERE session_id = ?1 ORDER BY sequence")
                .map_err(unreadable)?;
            let mut rows = entries.query([&id]).map_err(unreadable)?;
            let mut insert = self
                .tx
                .prepare_cached(
                    "INSERT INTO history_entries(session, sequence, at, kind, text) VALUES(?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(session, sequence) DO NOTHING",
                )
                .map_err(target)?;
            while let Some(row) = rows.next().map_err(unreadable)? {
                let sequence: i64 = row.get(0).map_err(unreadable)?;
                let at: i64 = row.get(1).map_err(unreadable)?;
                let kind: String = row.get(2).map_err(unreadable)?;
                let text: String = row.get(3).map_err(unreadable)?;
                if sequence < 0 || crate::history::EntryKind::parse(&kind).is_none() {
                    self.report.skip(Kind::HistoryEntries, Skip::Invalid, 1);
                    continue;
                }
                insert
                    .execute(params![key, sequence, micros(at), kind, text])
                    .map_err(target)?;
                self.report.imported(Kind::HistoryEntries, 1);
            }
            self.report.imported(Kind::HistorySessions, 1);
            added = true;
        }
        // Sessions deleted in the C# client stay deleted here.
        for id in self.src_rows("SELECT session_id FROM history_deletions", |r| r.get::<_, String>(0))? {
            self.tx
                .execute("INSERT OR IGNORE INTO history_deletions(session_id) VALUES(?1)", [&id])
                .map_err(target)?;
        }
        if added {
            self.tx
                .execute(
                    "INSERT INTO history_entries_fts(history_entries_fts) VALUES('rebuild')",
                    [],
                )
                .map_err(target)?;
        }
        self.step(Kind::HistorySessions, total, total);
        Ok(())
    }

    /// C# records with no counterpart here, counted so the summary says what stayed behind.
    fn others(&mut self) -> R<()> {
        for table in ["world_capabilities", "cache_entries", "script_deletions"] {
            if !self.src.has_table(table) {
                continue;
            }
            let n: i64 = self
                .src
                .conn()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .map_err(unreadable)?;
            let n = n.max(0) as u64;
            self.report.found(Kind::Other, n);
            self.report.skip(Kind::Other, Skip::NoCounterpart, n);
        }
        Ok(())
    }
}

/// Add one entry to a world's library (`existing` is the library as stored) unless it is there.
fn add_entry(
    tx: &Connection,
    report: &mut Report,
    existing: &mut Vec<LibraryEntry>,
    world: &str,
    kind: Kind,
    entry: LibraryEntry,
) -> R<()> {
    match existing.iter().find(|e| e.id == entry.id) {
        Some(e) if *e == entry => report.unchanged(kind, 1),
        Some(_) => report.skip(kind, Skip::KeptRust, 1),
        None if existing.len() >= library::MAX_ENTRIES => report.skip(kind, Skip::LibraryFull, 1),
        None => {
            let mut with = existing.clone();
            with.push(entry.clone());
            if library::validate_library(&with).is_err() {
                report.skip(kind, Skip::LibraryFull, 1);
                return Ok(());
            }
            tx.execute("INSERT OR IGNORE INTO worlds(id) VALUES(?1)", [world])
                .map_err(target)?;
            library::upsert(tx, world, &entry).map_err(target)?;
            existing.push(entry);
            report.imported(kind, 1);
        }
    }
    Ok(())
}

/// One script of a C# library file (`WorldScriptDefinition` JSON), or `None` when it is not
/// valid here.
fn legacy_entry(value: &Value) -> Option<LibraryEntry> {
    let object = value.as_object()?;
    let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_string);
    let mut entry = LibraryEntry {
        id: text("Id")?.to_lowercase(),
        name: text("Name")?,
        source: text("Source")?,
        enabled: object.get("Enabled").and_then(Value::as_bool).unwrap_or(false),
        macro_def: match object.get("Macro") {
            None | Some(Value::Null) => None,
            Some(m) => Some(serde_json::from_value(m.clone()).ok()?),
        },
        import: match object.get("Import") {
            None | Some(Value::Null) => None,
            Some(i) => Some(serde_json::from_value(i.clone()).ok()?),
        },
        language: Language::parse(object.get("Language").and_then(Value::as_str))?,
        compatibility: Compatibility::parse(object.get("Compatibility").and_then(Value::as_str))?,
        ..LibraryEntry::default()
    };
    if let Some(pack) = object.get("Pack").filter(|p| !p.is_null()) {
        let info: library::PackInfo = serde_json::from_value(pack.clone()).ok()?;
        let allow = object.get("AllowSend").and_then(Value::as_bool).unwrap_or(false);
        entry.set_pack(&info, allow);
    }
    entry.validate().is_ok().then_some(entry)
}

/// .NET UTC ticks to microseconds since 1970.
fn micros(ticks: i64) -> i64 {
    (ticks - UNIX_EPOCH_TICKS) / 10
}

/// The rules this client can keep, how many use syntax it cannot run, and how many are invalid
/// otherwise.
fn channel_rules(values: &[Value]) -> (Vec<ChannelRule>, u64, u64) {
    let mut rules = Vec::new();
    let (mut syntax, mut invalid) = (0, 0);
    for value in values {
        match serde_json::from_value::<ChannelRule>(value.clone()) {
            Ok(rule) if rule.is_valid() && rules.len() < crate::channels::rules::MAX_RULES => rules.push(rule),
            Ok(rule) if !rule.pattern.is_empty() && crate::channels::rules::compile(&rule.pattern).is_none() => {
                syntax += 1
            }
            _ => invalid += 1,
        }
    }
    (rules, syntax, invalid)
}

/// Whether two saved worlds hold the same C# fields (usage and fields only this client has are
/// not compared).
fn same_world(a: &SavedWorld, b: &SavedWorld) -> bool {
    a.name == b.name
        && a.host == b.host
        && a.port == b.port
        && a.tls == b.tls
        && a.charset == b.charset
        && a.username == b.username
        && a.password_id == b.password_id
        && a.auto_login == b.auto_login
        && a.username_prompt == b.username_prompt
        && a.password_prompt == b.password_prompt
        && a.codebase == b.codebase
        && a.channel_rules == b.channel_rules
        && a.theme == b.theme
        && a.protocol_mapping == b.protocol_mapping
}

fn same_theme(a: &CustomTheme, b: &CustomTheme) -> bool {
    a.id == b.id
        && a.name == b.name
        && a.is_light == b.is_light
        && a.colors == b.colors
        && a.ansi_colors == b.ansi_colors
}

/// Rooms, exits, areas, aliases and tombstones the same, in any order.
fn same_map(a: &MapSnapshot, b: &MapSnapshot) -> bool {
    fn sorted<T: serde::Serialize>(items: &[T]) -> Vec<String> {
        let mut v: Vec<String> = items
            .iter()
            .map(|i| serde_json::to_string(i).unwrap_or_default())
            .collect();
        v.sort();
        v
    }
    sorted(&a.rooms) == sorted(&b.rooms)
        && sorted(&a.links) == sorted(&b.links)
        && sorted(&a.area_settings) == sorted(&b.area_settings)
        && sorted(&a.room_aliases) == sorted(&b.room_aliases)
        && sorted(&a.deleted_rooms) == sorted(&b.deleted_rooms)
        && sorted(&a.deleted_links) == sorted(&b.deleted_links)
        && a.source == b.source
        && a.observation_count == b.observation_count
}

/// A C# `UserTheme` as a colour scheme here, or `None` when it is not valid.
fn custom_theme(value: &Value) -> Option<CustomTheme> {
    let object = value.as_object()?;
    let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_string);
    let is_light = object.get("IsLight").and_then(Value::as_bool).unwrap_or(false);
    let colors: BTreeMap<String, String> = object
        .get("Colors")
        .and_then(Value::as_object)
        .map(|c| {
            c.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let mut ansi_colors = BTreeMap::new();
    if let Some(ansi) = object.get("AnsiColors").and_then(Value::as_object) {
        for (k, v) in ansi {
            ansi_colors.insert(k.parse::<u8>().ok()?, v.as_str()?.to_string());
        }
    }
    let theme = CustomTheme {
        id: text("Id")?,
        name: text("Name")?,
        // The preset it starts from only matters for colours it does not name; a C# scheme names
        // them all. The default dark and light presets.
        base: if is_light { "Hull".into() } else { "Ember".into() },
        ansi_colors,
        is_light,
        colors,
    };
    theme.is_valid().then_some(theme)
}

/// The preferences both clients have, as one comparable value.
fn preferences(s: &Settings) -> Value {
    serde_json::json!({
        "theme": s.theme, "skin": s.skin, "language": s.language, "font_size": s.font_size,
        "foreground": s.foreground, "background": s.background, "local_echo": s.local_echo,
        "allow_blinking_text": s.allow_blinking_text, "scroll_tail_share": s.scroll_tail_share,
        "use_world_themes": s.use_world_themes, "classify_rooms_locally": s.classify_rooms_locally,
        "room_classification_threshold": s.room_classification_threshold, "map_auto_center": s.map_auto_center,
        "show_channels": s.show_channels, "composer_suggestions": s.composer_suggestions,
        "history_enabled": s.history_enabled, "hide_history_recording_notice": s.hide_history_recording_notice,
        "check_for_updates": s.check_for_updates, "send_install_id": s.send_install_id,
        "history_retention_days": s.history_retention_days, "enable_lua_scripts": s.enable_lua_scripts,
        "custom_themes": s.custom_themes,
    })
}

/// Take the C# preferences into `settings` (each only when the C# value is usable).
fn apply_preferences(settings: &mut Settings, cs: &serde_json::Map<String, Value>, themes: &[CustomTheme]) {
    let flag = |key: &str| cs.get(key).and_then(Value::as_bool);
    let number = |key: &str| cs.get(key).and_then(Value::as_f64).filter(|n| n.is_finite());
    let text = |key: &str| cs.get(key).and_then(Value::as_str);
    for theme in themes {
        match settings.custom_themes.iter_mut().find(|t| t.id == theme.id) {
            Some(existing) => *existing = theme.clone(),
            None => settings.custom_themes.push(theme.clone()),
        }
    }
    if let Some(theme) = text("Theme") {
        settings.theme = theme.to_string();
    }
    if let Some(skin) = text("Skin") {
        settings.skin = crate::settings::normalize_skin(skin).into();
    }
    if let Some(language) = text("Language") {
        settings.language = language.to_string();
    }
    if let Some(size) = number("FontSize") {
        settings.font_size = size as f32;
    }
    for (key, slot) in [
        ("Foreground", &mut settings.foreground),
        ("Background", &mut settings.background),
    ] {
        match cs.get(key) {
            Some(Value::Null) => *slot = None,
            Some(Value::String(c)) if crate::settings::is_color(c) => *slot = Some(c.clone()),
            _ => {}
        }
    }
    let flags: [(&str, &mut bool); 12] = [
        ("LocalEcho", &mut settings.local_echo),
        ("AllowBlinkingText", &mut settings.allow_blinking_text),
        ("UseWorldThemes", &mut settings.use_world_themes),
        ("ClassifyRoomsLocally", &mut settings.classify_rooms_locally),
        ("MapAutoCenter", &mut settings.map_auto_center),
        ("ShowChannelsPanel", &mut settings.show_channels),
        ("ComposerSuggestions", &mut settings.composer_suggestions),
        ("HistoryEnabled", &mut settings.history_enabled),
        (
            "HideHistoryRecordingNotice",
            &mut settings.hide_history_recording_notice,
        ),
        ("CheckForUpdates", &mut settings.check_for_updates),
        ("SendInstallId", &mut settings.send_install_id),
        // The Mudlet import branch's preference for Lua scripts.
        ("EnableLuaScripts", &mut settings.enable_lua_scripts),
    ];
    for (key, slot) in flags {
        if let Some(value) = flag(key) {
            *slot = value;
        }
    }
    if let Some(share) = number("ScrollTailShare") {
        settings.scroll_tail_share = share as f32;
    }
    if let Some(threshold) = number("RoomClassificationThreshold") {
        settings.room_classification_threshold = threshold;
    }
    if let Some(days) = cs.get("HistoryRetentionDays").and_then(Value::as_u64) {
        settings.history_retention_days = u32::try_from(days).unwrap_or(u32::MAX);
    }
    settings.clamp(MAX_SCROLLBACK.max(settings.scrollback));
}

/// A C# agent profile as this client stores it, and its API key's vault key when it has one.
#[cfg(feature = "agent")]
fn agent_payload(payload: &str) -> Option<(String, Option<String>)> {
    let profile = crate::agent::store::parse(payload).ok()?;
    let encoded = crate::agent::store::encode(&profile).ok()?;
    let key = profile
        .credential_id
        .map(|_| crate::agent::credentials::credential_key(&profile));
    Some((encoded, key))
}

/// Without the agent feature a profile is kept as the C# client wrote it (when it is a JSON
/// object of a sane size); its API key is not copied.
#[cfg(not(feature = "agent"))]
fn agent_payload(payload: &str) -> Option<(String, Option<String>)> {
    (payload.len() <= 256_000 && serde_json::from_str::<Value>(payload).ok()?.is_object())
        .then(|| (payload.to_string(), None))
}
