//! Agent profiles per world (the C# `SqliteAgentProfileStore`), in `wandur.db`'s
//! `world_agent_profiles` (migration 8): one JSON payload per world id.
//!
//! A world is named by its id, or by an address key (`host:port`, the C# `host:port:tls` form,
//! or `demo` for the offline demo) that resolves to its world, so every address of a world
//! shares its profile. Loading a world that has none stores the defaults once, so the profile
//! keeps its id (the API key is bound to it); every later load only reads.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use rusqlite::{Connection, OptionalExtension, params};

use super::AgentError;
use super::profile::{self, AgentProfile};
use crate::db::{Database, DbError, worlds};

/// The largest stored payload read (characters, as C#).
const MAX_PAYLOAD: usize = 256_000;

/// Which world a profile belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AgentWorld {
    /// A saved world's id.
    Id(String),
    /// An address key, resolved to its world (one is made when the address is new).
    Key(String),
}

impl AgentWorld {
    /// The key for an address (TLS does not change the world).
    pub fn endpoint(host: &str, port: u16) -> Self {
        AgentWorld::Key(worlds::endpoint_key(host, port).unwrap_or_else(|_| format!("{}:{port}", host.trim())))
    }

    /// The offline demo.
    pub fn demo() -> Self {
        AgentWorld::Key(DEMO.into())
    }

    /// The canonical key: an id as it is, an address without a trailing `:True`/`:False`.
    fn canonical(&self) -> Result<String, AgentError> {
        match self {
            AgentWorld::Id(id) if worlds::valid_world_id(id) => Ok(id.clone()),
            AgentWorld::Id(_) => Err(AgentError::Invalid("Invalid agent profile identity.")),
            AgentWorld::Key(key) => {
                let key = key.trim();
                let key = match key.rsplit_once(':') {
                    Some((rest, tls)) if tls.eq_ignore_ascii_case("true") || tls.eq_ignore_ascii_case("false") => rest,
                    _ => key,
                };
                worlds::canonical_endpoint_key(key).map_err(|_| AgentError::Invalid("Invalid agent world."))
            }
        }
    }
}

const DEMO: &str = "demo";

/// Where profiles are kept.
pub trait AgentProfileStore: Send + Sync {
    /// The world's profile; a world without one gets the defaults (stored).
    fn load(&self, world: &AgentWorld) -> Result<AgentProfile, AgentError>;
    /// Replace the world's profile. An invalid profile is refused and nothing changes.
    fn save(&self, world: &AgentWorld, profile: &AgentProfile) -> Result<(), AgentError>;
}

fn storage(e: DbError) -> AgentError {
    AgentError::Storage(e.to_string())
}

pub(crate) fn parse(json: &str) -> Result<AgentProfile, AgentError> {
    let invalid = AgentError::Invalid("Stored agent profile is invalid.");
    if super::len16(json) > MAX_PAYLOAD {
        return Err(AgentError::Invalid("Stored agent profile is too large."));
    }
    let mut profile: AgentProfile = serde_json::from_str(json).map_err(|_| invalid.clone())?;
    profile.goals = profile::single_default(std::mem::take(&mut profile.goals));
    profile::validate(&profile, false).map_err(|_| invalid)?;
    Ok(profile)
}

pub(crate) fn encode(profile: &AgentProfile) -> Result<String, AgentError> {
    serde_json::to_string(profile).map_err(|_| AgentError::Invalid("Invalid agent profile."))
}

/// `wandur.db`.
#[derive(Clone, Debug)]
pub struct SqliteAgentProfileStore {
    db: Database,
}

impl SqliteAgentProfileStore {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// The world id a key leads to, if it exists yet (reads only).
    fn find(conn: &Connection, key: &str, world: &AgentWorld) -> Result<Option<String>, DbError> {
        match world {
            AgentWorld::Id(_) => Ok(Some(key.to_string())),
            AgentWorld::Key(_) => worlds::find_world_key(conn, key),
        }
    }

    /// The world id a key leads to, made when it is new. Inside a write transaction.
    fn resolve(conn: &Connection, key: &str, world: &AgentWorld) -> Result<String, DbError> {
        if let Some(id) = Self::find(conn, key, world)? {
            conn.execute("INSERT OR IGNORE INTO worlds(id) VALUES(?1)", [&id])?;
            return Ok(id);
        }
        let id = worlds::new_world_id();
        conn.execute("INSERT INTO worlds(id) VALUES(?1)", [&id])?;
        conn.execute(
            "INSERT INTO endpoints(endpoint_key, world_id) VALUES(?1, ?2)",
            params![key, id],
        )?;
        Ok(id)
    }

    fn stored(conn: &Connection, world_id: &str) -> Result<Option<String>, DbError> {
        Ok(conn
            .query_row(
                "SELECT payload FROM world_agent_profiles WHERE world_id=?1",
                [world_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    fn write(conn: &Connection, world_id: &str, payload: &str) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO world_agent_profiles(world_id, payload) VALUES(?1, ?2)
             ON CONFLICT(world_id) DO UPDATE SET payload=excluded.payload",
            params![world_id, payload],
        )?;
        Ok(())
    }
}

impl AgentProfileStore for SqliteAgentProfileStore {
    fn load(&self, world: &AgentWorld) -> Result<AgentProfile, AgentError> {
        let key = world.canonical()?;
        // Opening a session reads the profile: the settled case is one read, no write.
        let stored = self
            .db
            .read(|conn| match Self::find(conn, &key, world)? {
                Some(id) => Self::stored(conn, &id),
                None => Ok(None),
            })
            .map_err(storage)?;
        if let Some(json) = stored {
            return parse(&json);
        }
        let mut found = None;
        let defaults = AgentProfile::default();
        let payload = encode(&defaults)?;
        self.db
            .write(|tx| {
                let id = Self::resolve(tx, &key, world)?;
                match Self::stored(tx, &id)? {
                    Some(json) => found = Some(json),
                    None => Self::write(tx, &id, &payload)?,
                }
                Ok(())
            })
            .map_err(storage)?;
        match found {
            Some(json) => parse(&json),
            None => Ok(defaults),
        }
    }

    fn save(&self, world: &AgentWorld, profile: &AgentProfile) -> Result<(), AgentError> {
        profile::validate(profile, false)?;
        let key = world.canonical()?;
        let payload = encode(profile)?;
        self.db
            .write(|tx| {
                let id = Self::resolve(tx, &key, world)?;
                Self::write(tx, &id, &payload)
            })
            .map_err(storage)
    }
}

/// Profiles in memory (tests and scenes), keyed as the database keys them.
#[derive(Debug, Default)]
pub struct MemoryAgentProfileStore {
    profiles: Mutex<HashMap<String, AgentProfile>>,
}

impl MemoryAgentProfileStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// A store holding `profile` for `world`.
    pub fn with(world: &AgentWorld, profile: AgentProfile) -> Self {
        let store = Self::new();
        if let Ok(key) = world.canonical() {
            store
                .profiles
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(key, profile);
        }
        store
    }
}

impl AgentProfileStore for MemoryAgentProfileStore {
    fn load(&self, world: &AgentWorld) -> Result<AgentProfile, AgentError> {
        let key = world.canonical()?;
        Ok(self
            .profiles
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key)
            .or_default()
            .clone())
    }

    fn save(&self, world: &AgentWorld, profile: &AgentProfile) -> Result<(), AgentError> {
        profile::validate(profile, false)?;
        let key = world.canonical()?;
        self.profiles
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, profile.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::profile::AgentGoal;

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("wandur-agent-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn open(name: &str) -> (std::path::PathBuf, Database) {
        let d = dir(name);
        let (db, _) = Database::open(&d).unwrap();
        (d, db)
    }

    /// AgentProfileStoreTests.OlderMultipleDefaultsLoadAsOneWithoutLosingAnyGoals.
    #[test]
    fn older_multiple_defaults_load_as_one_without_losing_any_goals() {
        let (d, db) = open("defaults");
        let store = SqliteAgentProfileStore::new(db.clone());
        let world = AgentWorld::Key("world".into());
        let old = AgentProfile {
            goals: vec![AgentGoal::new("One", true), AgentGoal::new("Two", true)],
            ..store.load(&world).unwrap()
        };
        let payload = serde_json::to_string(&old).unwrap();
        db.write(|tx| {
            tx.execute("UPDATE world_agent_profiles SET payload=?1", [&payload])?;
            Ok(())
        })
        .unwrap();
        let loaded = store.load(&world).unwrap();
        assert_eq!(loaded.goals.len(), 2);
        assert!(loaded.goals[0].enabled);
        assert!(!loaded.goals[1].enabled);
        assert!(matches!(store.save(&world, &old), Err(AgentError::Invalid(_))));
        let _ = std::fs::remove_dir_all(d);
    }

    /// AgentProfileStoreTests.GoalsRejectCombiningObjectivesAndPersistDescriptionsAndRules.
    #[test]
    fn goals_persist_descriptions_and_rules() {
        let (d, db) = open("goals");
        let store = SqliteAgentProfileStore::new(db);
        let world = AgentWorld::Key("world".into());
        let first = AgentGoal {
            name: "Explore".into(),
            rules: "- Do not fight".into(),
            ..AgentGoal::new("Explore carefully", true)
        };
        let second = AgentGoal::new("Observe the room", false);
        let profile = AgentProfile {
            goals: vec![first, second],
            ..store.load(&world).unwrap()
        };
        store.save(&world, &profile).unwrap();
        let loaded = store.load(&world).unwrap();
        assert_eq!(loaded.goals[0].name, "Explore");
        assert_eq!(loaded.goals[0].text, "Explore carefully");
        assert_eq!(loaded.goals[0].rules, "- Do not fight");
        let _ = std::fs::remove_dir_all(d);
    }

    /// AgentProfileStoreTests.DefaultIdentityPersistsAndCanonicalWorldAliasesShareProfile.
    #[test]
    fn the_default_identity_persists_and_world_aliases_share_a_profile() {
        let (d, db) = open("identity");
        let store = SqliteAgentProfileStore::new(db.clone());
        let original = store.load(&AgentWorld::Key("MUD.Example.:4000:False".into())).unwrap();
        let reopened = SqliteAgentProfileStore::new(Database::open(&d).unwrap().0);
        assert_eq!(
            original.id,
            reopened
                .load(&AgentWorld::Key("mud.example:4000:True".into()))
                .unwrap()
                .id
        );
        let profile = AgentProfile {
            model: "gemma-12b".into(),
            credential_id: Some(uuid::Uuid::new_v4()),
            goals: vec![AgentGoal::new("Explore", true), AgentGoal::new("Rest", false)],
            ..original
        };
        store
            .save(&AgentWorld::Key("mud.example:4000".into()), &profile)
            .unwrap();
        let loaded = store.load(&AgentWorld::Key("mud.example:4000".into())).unwrap();
        assert_eq!(
            serde_json::to_string(&profile).unwrap(),
            serde_json::to_string(&loaded).unwrap()
        );
        // The world id names the same profile as its address.
        let id = db
            .read(|c| worlds::find_world_key(c, "mud.example:4000"))
            .unwrap()
            .unwrap();
        assert_eq!(store.load(&AgentWorld::Id(id)).unwrap(), loaded);
        // A second load reads only.
        let rows: i64 = db
            .read(|c| Ok(c.query_row("SELECT count(*) FROM world_agent_profiles", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(rows, 1);
        let _ = std::fs::remove_dir_all(d);
    }

    /// AgentProfileStoreTests.InvalidSaveDoesNotPublishEventOrReplaceProfile.
    #[test]
    fn an_invalid_save_does_not_replace_the_profile() {
        let (d, db) = open("invalid");
        let store = SqliteAgentProfileStore::new(db);
        let world = AgentWorld::demo();
        let original = store.load(&world).unwrap();
        let bad = AgentProfile {
            max_decisions: 0,
            ..original.clone()
        };
        assert!(matches!(store.save(&world, &bad), Err(AgentError::Invalid(_))));
        assert_eq!(store.load(&world).unwrap(), original);
        let _ = std::fs::remove_dir_all(d);
    }

    /// AgentProfileStoreTests.VersionTwoMigratesWithoutLosingExistingMacroPayload, for this
    /// client's schema: a version 7 file gains the profile table and keeps its scripts.
    #[test]
    fn a_version_7_file_migrates_and_keeps_its_scripts() {
        let d = dir("migrate");
        std::fs::create_dir_all(&d).unwrap();
        {
            let mut conn = rusqlite::Connection::open(d.join(crate::db::DB_FILE)).unwrap();
            crate::db::migrate(&mut conn, 7).unwrap();
            conn.execute_batch(
                "INSERT INTO worlds(id) VALUES('old');
                 INSERT INTO scripts(world_id,id,name,source,enabled,macro_json) VALUES('old','script','macro','',1,'{\"steps\":[]}');",
            )
            .unwrap();
        }
        let (db, _) = Database::open(&d).unwrap();
        SqliteAgentProfileStore::new(db.clone())
            .load(&AgentWorld::demo())
            .unwrap();
        assert_eq!(db.schema_version().unwrap(), crate::db::SCHEMA_VERSION);
        let kept: String = db
            .read(|c| Ok(c.query_row("SELECT macro_json FROM scripts WHERE id='script'", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(kept, "{\"steps\":[]}");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_damaged_payload_is_an_error_not_a_reset() {
        let (d, db) = open("damaged");
        let store = SqliteAgentProfileStore::new(db.clone());
        let world = AgentWorld::Key("world:23".into());
        store.load(&world).unwrap();
        db.write(|tx| {
            tx.execute("UPDATE world_agent_profiles SET payload='{not json'", [])?;
            Ok(())
        })
        .unwrap();
        assert!(matches!(store.load(&world), Err(AgentError::Invalid(_))));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_memory_store_keys_as_the_database_does() {
        let store = MemoryAgentProfileStore::new();
        let a = store.load(&AgentWorld::Key("Mud.Example:4000:True".into())).unwrap();
        let b = store.load(&AgentWorld::endpoint("mud.example", 4000)).unwrap();
        assert_eq!(a.id, b.id);
        assert!(
            store
                .save(&AgentWorld::demo(), &AgentProfile { max_decisions: 0, ..a })
                .is_err()
        );
    }
}
