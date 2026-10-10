//! The importer against synthetic C# data directories made by the C# client's own stores
//! (`tests/fixtures/csharp-data`, generator beside it). No real data is ever used.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

use super::secrets::{csharp_login_key, dotnet_json_string};
use super::*;
use crate::db::Database;
use crate::history::{HistoryFilter, HistoryStore, store::SqliteHistoryStore};
use crate::login::{MemoryVault, PasswordVault};
use crate::map::model::DoorState;
use crate::map::store::{MapStore, MapWorld};
use crate::map::tracker::RoomMapTracker;
use crate::settings::{SETTINGS_FILE, SavedWorld, Settings};

const LANTERN_PROFILE: &str = "11111111-2222-4333-8444-555555555501";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/csharp-data")
        .join(name)
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wandur-csimport-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn hash(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap();
    Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Every file in a directory with its hash.
fn hashes(dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                out.insert(entry.file_name().to_string_lossy().into_owned(), hash(&path));
            }
        }
    }
    out
}

/// A world id of the C# fixture, by the name of its saved world.
fn cs_world(name: &str) -> String {
    let source = CsharpSource::open(&fixture("main")).unwrap();
    let mut s = source.conn().prepare("SELECT world_id, payload FROM profiles").unwrap();
    let rows: Vec<(String, String)> = s
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows.into_iter()
        .find(|(_, payload)| payload.contains(&format!("\"Name\":\"{name}\"")))
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no world {name}"))
}

fn run(dir: &Path, options: ImportOptions) -> Report {
    let source = CsharpSource::open(&fixture("main")).unwrap();
    let vault = MemoryVault::new();
    import_into_dir(&source, dir, options, &vault, &vault, &|_| {}).unwrap()
}

/// Every row of every table of a database (the FTS shadow tables included), for comparisons.
fn dump(dir: &Path) -> Vec<String> {
    let conn = Connection::open_with_flags(dir.join(DB_FILE), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let tables = table_names(&conn).unwrap();
    let mut out = Vec::new();
    for table in tables {
        if table.starts_with("sqlite_") || table == "history_entries_fts" {
            continue;
        }
        let mut s = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let columns = s.column_count();
        let mut rows: Vec<String> = s
            .query_map([], |r| {
                let mut cells = Vec::new();
                for i in 0..columns {
                    cells.push(format!("{:?}", r.get_ref(i)?));
                }
                Ok(cells.join("|"))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        rows.sort();
        out.extend(rows.into_iter().map(|r| format!("{table}: {r}")));
    }
    out
}

fn check(report: &Report, kind: Kind, found: u64, imported: u64, unchanged: u64) {
    let t = report.tally(kind);
    assert_eq!(
        (t.found, t.imported, t.unchanged),
        (found, imported, unchanged),
        "{kind:?}: {t:?}\n{}",
        report.to_text()
    );
}

fn skipped(report: &Report, kind: Kind, reason: Skip) -> u64 {
    report.tally(kind).skipped.get(&reason).copied().unwrap_or(0)
}

#[test]
fn every_kind_round_trips_into_an_empty_data_directory() {
    let dir = temp("empty");
    let before = hash(&fixture("main").join(DB_FILE));
    let report = run(&dir, ImportOptions::default());
    assert_eq!(report.schema, 7);
    check(&report, Kind::Worlds, 4, 4, 0);
    check(&report, Kind::Addresses, 5, 5, 0);
    check(&report, Kind::Usage, 5, 5, 0);
    check(&report, Kind::Preferences, 1, 1, 0);
    check(&report, Kind::ColorSchemes, 1, 1, 0);
    check(&report, Kind::OtherSettings, 3, 0, 0);
    assert_eq!(skipped(&report, Kind::OtherSettings, Skip::InstallId), 1);
    assert_eq!(skipped(&report, Kind::OtherSettings, Skip::CsharpReleases), 2);
    check(&report, Kind::Scripts, 4, 4, 0);
    check(&report, Kind::Macros, 4, 4, 0);
    check(&report, Kind::ChannelRules, 4, 3, 0);
    assert_eq!(skipped(&report, Kind::ChannelRules, Skip::PatternSyntax), 1);
    check(&report, Kind::AgentProfiles, 2, 2, 0);
    check(&report, Kind::Maps, 2, 2, 0);
    check(&report, Kind::Rooms, 7, 7, 0);
    check(&report, Kind::Exits, 5, 5, 0);
    check(&report, Kind::HistorySessions, 3, 3, 0);
    check(&report, Kind::HistoryEntries, 9, 9, 0);
    check(&report, Kind::Passwords, 2, 0, 0);
    assert_eq!(skipped(&report, Kind::Passwords, Skip::PasswordsNotRequested), 2);
    check(&report, Kind::ApiKeys, 1, 0, 0);
    check(&report, Kind::Other, 1, 0, 0);
    assert_eq!(skipped(&report, Kind::Other, Skip::NoCounterpart), 1);

    // Saved worlds, with the C# ids and fields.
    let (settings, warning) = Settings::load(&dir, 50_000);
    assert!(warning.is_none());
    let lantern_id = cs_world("Fixture Lantern Road");
    let names: Vec<&str> = settings.worlds.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Fixture Lantern Road",
            "Fixture Glass Harbor",
            "Fixture Moss Hollow",
            "Fixture Ash Keep"
        ]
    );
    let lantern = &settings.worlds[0];
    assert_eq!(lantern.world_id, lantern_id);
    assert_eq!(
        (lantern.host.as_str(), lantern.port, lantern.tls),
        ("lantern.fixture.example", 4000, false)
    );
    assert_eq!(lantern.username, "wayfarer");
    assert!(lantern.auto_login);
    assert_eq!(
        lantern.password_id.as_deref(),
        Some("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeee01")
    );
    assert_eq!(lantern.codebase, "SMAUG 1.4a");
    assert_eq!(lantern.channel_rules.len(), 3);
    assert!(lantern.channel_rules.iter().any(|r| r.exclude));
    assert!(
        lantern
            .channel_rules
            .iter()
            .any(|r| r.private && r.reply_command.as_deref() == Some("tell {speaker}"))
    );
    assert_eq!(lantern.connections, 4);
    let sept_20 = crate::directory::time::parse_rfc3339("2026-09-20T19:30:00Z").unwrap() as u64;
    assert_eq!(lantern.last_connected, Some(sept_20));
    let harbor = &settings.worlds[1];
    assert!(harbor.tls);
    assert_eq!(harbor.port, 6697);
    assert_eq!(harbor.charset, crate::charset::Charset::Latin1);
    assert_eq!(harbor.theme.as_ref().map(|t| t.id.as_str()), Some("fixture-harbor"));
    assert_eq!(settings.worlds[3].host, "2001:db8::5");
    assert_eq!(settings.worlds[2].connections, 0);
    assert_eq!(settings.worlds[2].last_connected, None);

    // Preferences.
    assert_eq!(settings.theme, "custom-0123456789abcdef0123456789abcdef");
    assert_eq!(settings.custom_themes.len(), 1);
    assert_eq!(settings.custom_themes[0].name, "Fixture Dusk");
    assert_eq!(
        settings.custom_themes[0].ansi_colors.get(&1).map(String::as_str),
        Some("#CC5555")
    );
    assert_eq!(settings.skin, "Armored");
    assert_eq!(settings.language, "fr");
    assert_eq!(settings.font_size, 17.0);
    assert_eq!(settings.foreground.as_deref(), Some("#E0E0E0"));
    assert!(settings.local_echo && settings.allow_blinking_text && settings.hide_history_recording_notice);
    assert!(!settings.use_world_themes && !settings.map_auto_center && !settings.show_channels);
    assert!(!settings.composer_suggestions && !settings.check_for_updates && !settings.send_install_id);
    assert!(!settings.classify_rooms_locally);
    assert_eq!(settings.room_classification_threshold, 0.85);
    assert!((settings.scroll_tail_share - 0.3).abs() < 1e-6);
    assert_eq!(settings.history_retention_days, 90);
    // This client keeps its own install id and update record.
    assert_ne!(
        settings.install_id.as_deref(),
        Some("99999999-8888-4777-8666-555555555555")
    );
    assert!(settings.install_id.is_some());
    assert!(settings.last_update_check.is_none() && settings.skipped_update_version.is_none());

    // Addresses: the old address and the C# legacy spellings lead to the same world.
    let (db, _) = Database::open(&dir).unwrap();
    let conn = db.connect().unwrap();
    for key in [
        "old-lantern.fixture.example:4000",
        "lantern.fixture.example:4000:True",
        "LANTERN.fixture.example.:4000",
    ] {
        assert_eq!(
            crate::db::worlds::find_world_key(&conn, key).unwrap().as_deref(),
            Some(lantern_id.as_str()),
            "{key}"
        );
    }
    assert!(
        crate::db::worlds::find_world_key(&conn, "2001:db8::5:4000")
            .unwrap()
            .is_some()
    );
    let usage = crate::db::worlds::usage(&conn, &lantern_id).unwrap();
    assert_eq!(usage, Some((4, sept_20)));

    // Scripts and macros.
    let library = crate::db::scripts::load(&conn, &lantern_id).unwrap();
    assert_eq!(library.len(), 6);
    assert!(
        library
            .iter()
            .any(|e| e.name == "Fixture greeter" && e.enabled && !e.is_macro())
    );
    let macros: Vec<_> = library.iter().filter_map(|e| e.as_saved_macro()).collect();
    assert_eq!(macros.len(), 4);
    assert!(crate::db::scripts::is_started(&conn, &lantern_id).unwrap());
    let harbor_library = crate::db::scripts::load(&conn, &harbor.world_id).unwrap();
    let pack = harbor_library.iter().find(|e| e.is_pack()).unwrap();
    assert!(pack.allow_send());
    assert_eq!(pack.pack().unwrap().version, 3);

    // Agent settings, with the C# ids.
    #[cfg(feature = "agent")]
    {
        use crate::agent::store::{AgentProfileStore, AgentWorld, SqliteAgentProfileStore};
        let store = SqliteAgentProfileStore::new(db.clone());
        let profile = store.load(&AgentWorld::Id(lantern_id.clone())).unwrap();
        assert_eq!(profile.id.to_string(), "dddddddd-0000-4000-8000-000000000001");
        assert_eq!(profile.model, "fixture-model");
        assert_eq!(profile.goals.len(), 2);
        assert_eq!(profile.max_decisions, 12);
        assert!(profile.json_mode && profile.credential_id.is_some());
        // An address of the world finds the same profile.
        let by_address = store
            .load(&AgentWorld::Key("old-lantern.fixture.example:4000".into()))
            .unwrap();
        assert_eq!(by_address.id, profile.id);
    }
    assert_eq!(before, hash(&fixture("main").join(DB_FILE)), "the C# file was changed");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn imported_maps_load_in_the_tracker_with_the_same_coordinates_and_links() {
    let dir = temp("maps");
    run(&dir, ImportOptions::default());
    let lantern_id = cs_world("Fixture Lantern Road");
    let source = CsharpSource::open(&fixture("main")).unwrap();
    let original = crate::map::store::read_map(source.conn(), &lantern_id)
        .unwrap()
        .unwrap();
    let (db, _) = Database::open(&dir).unwrap();
    let store = MapStore::new(db);
    let map = store.load(&MapWorld::Id(lantern_id.clone())).unwrap().unwrap();
    assert_eq!(map.rooms, original.rooms);
    assert_eq!(map.links, original.links);
    assert_eq!(map.area_settings, original.area_settings);
    assert_eq!(map.room_aliases, original.room_aliases);
    assert_eq!(map.deleted_rooms, original.deleted_rooms);
    assert_eq!(map.deleted_links, original.deleted_links);
    assert_eq!(map.observation_count, 42);
    // The old address finds the same map.
    let by_alias = store
        .load(&MapWorld::Endpoint {
            host: "old-lantern.fixture.example".into(),
            port: 4000,
        })
        .unwrap()
        .unwrap();
    assert_eq!(by_alias.rooms.len(), 6);

    let tracker = RoomMapTracker::from_snapshot(map);
    assert_eq!(tracker.room_count(), 6);
    assert_eq!(tracker.link_count(), 5);
    for room in &original.rooms {
        let loaded = tracker.room(&room.id).unwrap();
        assert_eq!((loaded.x, loaded.y, loaded.z), (room.x, room.y, room.z), "{}", room.id);
    }
    let far = tracker.room("text:fixture-far").unwrap();
    assert_eq!((far.x, far.y), (1000.0, 1000.0));
    let lighthouse = tracker.room("gmcp:2002").unwrap();
    assert!(lighthouse.is_locked && lighthouse.is_manually_edited);
    assert_eq!(lighthouse.notes, "Fixture note.");
    assert_eq!(
        tracker.link("gmcp:1002", "south").unwrap().door_state,
        DoorState::Locked
    );
    assert_eq!(
        tracker.link("gmcp:1001", "down").unwrap().command.as_deref(),
        Some("climb down")
    );
    assert_eq!(tracker.link("gmcp:1001", "east").unwrap().line_points.len(), 2);
    assert_eq!(tracker.link("gmcp:1001", "north").unwrap().to_id, "gmcp:1002");
    assert!(tracker.grid_mode("Fixture Coast"));
    // The tombstones keep deleted rooms and exits deleted.
    let snapshot = tracker.snapshot();
    assert!(snapshot.deleted_rooms.iter().any(|d| d.id == "gmcp:9999"));
    assert!(
        snapshot
            .room_aliases
            .iter()
            .any(|a| a.source_id == "text:fixture-old-square")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn imported_history_is_searchable_and_deleted_sessions_stay_deleted() {
    let dir = temp("history");
    run(&dir, ImportOptions::default());
    let (db, _) = Database::open(&dir).unwrap();
    let store = SqliteHistoryStore::new(db.clone());
    let hits = store.search("lamplighter", &HistoryFilter::default(), 0, 50).unwrap();
    assert_eq!(hits.len(), 1);
    let hit = &hits[0];
    assert_eq!(hit.session.id, "eeeeeeee000040008000000000000001");
    assert_eq!(hit.session.world_key, "lantern.fixture.example:4000");
    assert_eq!(hit.session.character_name, "Wayfarer");
    let at = crate::directory::time::parse_rfc3339("2026-09-20T19:30:12Z").unwrap() as u64;
    assert_eq!(hit.entry.at, UNIX_EPOCH + Duration::from_secs(at));
    assert_eq!(
        store
            .search("\"fixture harbor\"", &HistoryFilter::default(), 0, 50)
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .search("must not come back", &HistoryFilter::default(), 0, 50)
            .unwrap()
            .is_empty()
    );
    let entries = store.entries("eeeeeeee000040008000000000000001", 0, 50).unwrap();
    assert_eq!(entries.len(), 6);
    assert_eq!(entries[1].kind, crate::history::EntryKind::Private);
    assert_eq!(entries[1].text, "");
    let sessions = store.sessions(&HistoryFilter::default(), 0, 50).unwrap();
    assert_eq!(sessions.len(), 3);
    let deleted: i64 = db
        .read(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM history_deletions WHERE session_id = 'eeeeeeee000040008000000000000004'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(deleted, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn running_the_import_again_changes_nothing() {
    let dir = temp("again");
    let first = run(&dir, ImportOptions::default());
    let rows = dump(&dir);
    let settings = std::fs::read(dir.join(SETTINGS_FILE)).unwrap();
    let second = run(&dir, ImportOptions::default());
    assert_eq!(dump(&dir), rows, "the database changed on the second run");
    assert_eq!(std::fs::read(dir.join(SETTINGS_FILE)).unwrap(), settings);
    for kind in Kind::ALL {
        let (a, b) = (first.tally(kind), second.tally(kind));
        assert_eq!(a.found, b.found, "{kind:?}");
        assert_eq!(b.imported, 0, "{kind:?} imported again: {b:?}");
        if !matches!(
            kind,
            Kind::Passwords | Kind::ApiKeys | Kind::OtherSettings | Kind::Other
        ) {
            assert_eq!(b.unchanged, a.imported, "{kind:?}: {b:?}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_newer_schema_is_refused_with_a_clear_message() {
    let dir = temp("newer");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(fixture("main").join(DB_FILE), dir.join(DB_FILE)).unwrap();
    {
        let conn = Connection::open(dir.join(DB_FILE)).unwrap();
        conn.pragma_update(None, "user_version", MAX_SCHEMA + 1).unwrap();
    }
    let err = CsharpSource::open(&dir).unwrap_err();
    assert!(
        matches!(err, ImportError::TooNew { found } if found == MAX_SCHEMA + 1),
        "{err}"
    );
    crate::l10n::override_thread(Some(crate::l10n::Language::En));
    let message = err.to_string();
    crate::l10n::override_thread(None);
    assert!(message.contains("schema 10") && message.contains("(9)"), "{message}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn other_files_are_refused() {
    let dir = temp("other");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(matches!(CsharpSource::open(&dir), Err(ImportError::NotFound)));
    std::fs::write(dir.join(DB_FILE), vec![0x5a; 4096]).unwrap();
    assert!(matches!(CsharpSource::open(&dir), Err(ImportError::NotWandur)));
    // This client's own database is not a C# one.
    let rust = temp("other-rust");
    Database::open(&rust).unwrap();
    assert!(matches!(CsharpSource::open(&rust), Err(ImportError::NotWandur)));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&rust);
}

#[test]
fn the_csharp_directory_is_never_written_and_its_wal_is_read() {
    // A copy of the fixture with a write still in its write-ahead log, as a running C# client
    // leaves it.
    let dir = temp("wal");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(fixture("main").join(DB_FILE), dir.join(DB_FILE)).unwrap();
    let writer = Connection::open(dir.join(DB_FILE)).unwrap();
    let mode: String = writer.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0)).unwrap();
    assert_eq!(mode, "wal");
    writer.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    writer
        .execute(
            "INSERT INTO history_sessions(id, world_key, world_name, character_name, started_at, ended_at)
             VALUES('eeeeeeee000040008000000000000009', 'demo', 'Demo', '', 639256158000000000, NULL)",
            [],
        )
        .unwrap();
    assert!(dir.join(format!("{DB_FILE}-wal")).is_file());
    let before = hashes(&dir);
    let source = CsharpSource::open(&dir).unwrap();
    assert_eq!(source.found().unwrap().tally(Kind::HistorySessions).found, 4);
    let target = temp("wal-target");
    let vault = MemoryVault::new();
    let report = import_into_dir(&source, &target, ImportOptions::default(), &vault, &vault, &|_| {}).unwrap();
    assert_eq!(report.tally(Kind::HistorySessions).imported, 4);
    drop(source);
    assert_eq!(hashes(&dir), before, "a file in the C# directory changed or appeared");
    drop(writer);
    // The committed fixture itself never gets sidecar files.
    let committed = hashes(&fixture("main"));
    assert!(
        committed.keys().all(|n| n == DB_FILE || n.starts_with("._")),
        "{committed:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn a_dry_run_writes_nothing() {
    // Into a data directory that does not exist yet: it is not even created.
    let dir = temp("dry");
    let report = run(
        &dir,
        ImportOptions {
            dry_run: true,
            include_secrets: false,
        },
    );
    assert!(report.dry_run);
    check(&report, Kind::Worlds, 4, 4, 0);
    assert!(!dir.exists());
    // Into one that has data: every file stays as it was.
    run(&dir, ImportOptions::default());
    std::fs::write(dir.join("marker"), b"x").unwrap();
    let before = hashes(&dir);
    let report = run(
        &dir,
        ImportOptions {
            dry_run: true,
            include_secrets: true,
        },
    );
    check(&report, Kind::Worlds, 4, 0, 4);
    assert_eq!(hashes(&dir), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn existing_data_is_merged_and_kept_never_duplicated() {
    let dir = temp("merge");
    std::fs::create_dir_all(&dir).unwrap();
    let (db, _) = Database::open(&dir).unwrap();
    // This client already saved the lantern road (another id, another name), changed its theme,
    // and mapped one room of it.
    let mut settings = Settings::default();
    let own_id = crate::db::worlds::new_world_id();
    let world = crate::settings::SavedWorld {
        world_id: own_id.clone(),
        name: "My Lantern".into(),
        host: "lantern.fixture.example".into(),
        port: 4000,
        ..Default::default()
    };
    db.write(|tx| crate::db::worlds::resolve_world(tx, &world.host, world.port, Some(&own_id)))
        .unwrap();
    settings.worlds.push(world);
    settings.theme = "Ember".into();
    settings.save(&dir).unwrap();
    let own_room = crate::map::model::MapRoom::new("gmcp:7777", "Own Room", "", None, 9.0, 9.0, 0.0, false);
    MapStore::new(db.clone())
        .save(
            &MapWorld::Id(own_id.clone()),
            &crate::map::model::MapSnapshot::of(vec![own_room], Vec::new()),
        )
        .unwrap();

    let report = run(&dir, ImportOptions::default());
    // The world is this client's: kept, and everything of the C# world joined it.
    check(&report, Kind::Worlds, 4, 3, 0);
    assert_eq!(skipped(&report, Kind::Worlds, Skip::KeptRust), 1);
    check(&report, Kind::Preferences, 1, 0, 0);
    assert_eq!(skipped(&report, Kind::Preferences, Skip::KeptRust), 1);
    let (settings, _) = Settings::load(&dir, 50_000);
    assert_eq!(settings.worlds.len(), 4);
    assert_eq!(settings.worlds[0].name, "My Lantern");
    assert_eq!(settings.worlds[0].world_id, own_id);
    assert_eq!(settings.worlds[0].connections, 4, "usage joins the kept world");
    assert_eq!(settings.theme, "Ember");
    let conn = db.connect().unwrap();
    assert_eq!(
        crate::db::worlds::find_world_key(&conn, "old-lantern.fixture.example:4000").unwrap(),
        Some(own_id.clone())
    );
    let map = MapStore::new(db.clone())
        .load(&MapWorld::Id(own_id.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(map.rooms.len(), 7, "the C# rooms merged with the one already here");
    assert!(map.room("gmcp:7777").is_some() && map.room("gmcp:2002").is_some());
    assert_eq!(crate::db::scripts::load(&conn, &own_id).unwrap().len(), 6);
    // Again: nothing more.
    let rows = dump(&dir);
    let again = run(&dir, ImportOptions::default());
    assert_eq!(again.tally(Kind::Worlds).imported, 0);
    assert_eq!(dump(&dir), rows);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_commit_leaves_nothing_behind() {
    let dir = temp("rollback");
    let (db, _) = Database::open(&dir).unwrap();
    let before = dump(&dir);
    let source = CsharpSource::open(&fixture("main")).unwrap();
    let result = import(&source, &db, &Settings::default(), false, &|_| {}, &mut |_| {
        Err("disk full".into())
    });
    assert!(matches!(result, Err(ImportError::Target(_))));
    assert_eq!(dump(&dir), before);
    assert!(!dir.join(SETTINGS_FILE).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saved_passwords_and_api_keys_move_from_the_csharp_entries_only_when_asked() {
    let dir = temp("secrets");
    let from = MemoryVault::new();
    let from_key = csharp_login_key(
        LANTERN_PROFILE,
        "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeee01",
        "lantern.fixture.example",
        4000,
        false,
        "wayfarer",
    );
    from.write(&from_key, "fixture-secret").unwrap();
    let to = MemoryVault::new();
    let source = CsharpSource::open(&fixture("main")).unwrap();
    // Not asked: nothing is read or written.
    from.set_locked(true);
    let report = import_into_dir(&source, &dir, ImportOptions::default(), &from, &to, &|_| {}).unwrap();
    assert_eq!(skipped(&report, Kind::Passwords, Skip::PasswordsNotRequested), 2);
    assert!(to.is_empty());
    from.set_locked(false);
    let options = ImportOptions {
        dry_run: false,
        include_secrets: true,
    };
    let report = import_into_dir(&source, &dir, options, &from, &to, &|_| {}).unwrap();
    check(&report, Kind::Passwords, 2, 1, 0);
    assert_eq!(skipped(&report, Kind::Passwords, Skip::NotInVault), 1);
    assert_eq!(skipped(&report, Kind::ApiKeys, Skip::NotInVault), 1);
    let (settings, _) = Settings::load(&dir, 50_000);
    let key = crate::login::vault::key(&settings.worlds[0]).unwrap();
    assert_eq!(to.read(&key).unwrap().as_deref(), Some("fixture-secret"));
    // The report never carries the secret.
    assert!(!report.to_text().contains("fixture-secret"));
    let again = import_into_dir(&source, &dir, options, &from, &to, &|_| {}).unwrap();
    check(&again, Kind::Passwords, 2, 0, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_csharp_vault_key_matches_the_csharp_client() {
    // Keys printed by the C# `PasswordVault.Key` code for these inputs.
    let id = LANTERN_PROFILE;
    let pw = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeee01";
    assert_eq!(
        csharp_login_key(id, pw, "Lantern.Fixture.Example", 4000, false, "wayfarer"),
        "A3B65321ED89280D14594D3CB7A8FC063B362095BC5E3E466422D9A877B20CAC"
    );
    assert_eq!(
        csharp_login_key(
            id,
            pw,
            "harbor.fixture.example",
            6697,
            true,
            "Quill <&> 'q' \"x\" a+b `c` back\\slash"
        ),
        "E32B9776595B0A31034BE477578BCB715DB89BD5C30809B820B1DCA35C20B744"
    );
    assert_eq!(
        csharp_login_key(id, pw, "bücher.example", 7000, false, "Ünïcode 名前 😀 ~^|{}"),
        "488D73BC0B1F040D61A83ABD8E26EF79CFDB10A0ABC15F000F5317C6FB89A238"
    );
    assert_eq!(
        csharp_login_key(id, pw, "2001:db8::5", 4000, false, ""),
        "A7ED6E21CCF38A28B3D7DF57C698129AEA48E805CD9019980FC5E5AC50314AFE"
    );
    let escaped = ["\"a", "u002Bb", "u003C", "u0022", "u00E9\""].join("\\");
    assert_eq!(dotnet_json_string("a+b<\"é"), escaped);
}

#[test]
fn a_schema_9_library_keeps_language_layer_and_import_record() {
    let dir = temp("schema9");
    let source = CsharpSource::open(&fixture("schema9")).unwrap();
    assert_eq!(source.schema(), 9);
    let vault = MemoryVault::new();
    let report = import_into_dir(&source, &dir, ImportOptions::default(), &vault, &vault, &|_| {}).unwrap();
    check(&report, Kind::Scripts, 3, 3, 0);
    let (settings, _) = Settings::load(&dir, 50_000);
    let (db, _) = Database::open(&dir).unwrap();
    let library = crate::db::scripts::load(&db.connect().unwrap(), &settings.worlds[0].world_id).unwrap();
    let lua = library.iter().find(|e| e.name == "Fixture Lua greeter").unwrap();
    assert!(lua.is_lua());
    assert_eq!(lua.compatibility, crate::scripting::Compatibility::Mudlet);
    let imported = library.iter().find(|e| e.name == "Fixture imported").unwrap();
    assert!(imported.is_imported());
    assert_eq!(imported.import.as_ref().unwrap().items.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_report_names_no_content() {
    let dir = temp("report");
    let report = run(&dir, ImportOptions::default());
    crate::l10n::override_thread(Some(crate::l10n::Language::En));
    let text = report.to_text();
    crate::l10n::override_thread(None);
    for secret in [
        "Fixture",
        "fixture",
        "lantern",
        "harbor",
        "203.0.113.7",
        "2001:db8",
        "wayfarer",
        "Quill",
        "SMAUG",
    ] {
        assert!(!text.contains(secret), "{secret} in:\n{text}");
    }
    assert!(
        text.contains("Saved worlds: 4 found, 4 imported, 0 already there"),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn sha_upper(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

#[test]
fn older_csharp_map_and_script_files_are_imported_once() {
    use crate::macros::{MacroDefinition, MacroKind};
    use crate::map::model::{MapLink, MapRoom, MapSnapshot};
    // The C# fixture with files an older C# client left behind, as the C# stores name them.
    let cs = temp("legacy-source");
    std::fs::create_dir_all(cs.join("maps")).unwrap();
    std::fs::create_dir_all(cs.join("scripts")).unwrap();
    std::fs::copy(fixture("main").join(DB_FILE), cs.join(DB_FILE)).unwrap();
    let moss = MapSnapshot::of(
        vec![
            MapRoom::new("t:moss-a", "Moss Gate", "Wet stone.", Some("Moss"), 0.0, 0.0, 0.0, true),
            MapRoom::new("t:moss-b", "Moss Hall", "Dry stone.", Some("Moss"), 0.0, 1.0, 0.0, true),
        ],
        vec![MapLink::new("t:moss-a", "t:moss-b", "north", false)],
    );
    let moss_file = format!("{}.json", sha_upper("203.0.113.7:2323"));
    std::fs::write(
        cs.join("maps").join(&moss_file),
        crate::map::format::serialize(&moss).unwrap(),
    )
    .unwrap();
    // A file the C# client already moved into its database stays behind.
    let lantern_file = format!("{}.json", sha_upper("lantern.fixture.example:4000"));
    std::fs::write(
        cs.join("maps").join(&lantern_file),
        crate::map::format::serialize(&moss).unwrap(),
    )
    .unwrap();
    {
        let conn = Connection::open(cs.join(DB_FILE)).unwrap();
        conn.execute(
            "INSERT INTO imports(key) VALUES(?1)",
            [format!("map-file:/an/older/place/maps/{lantern_file}")],
        )
        .unwrap();
    }
    std::fs::write(
        cs.join("scripts")
            .join(format!("{}.js", sha_upper("203.0.113.7:2323:True"))),
        "\u{feff}mud.send('legacy');",
    )
    .unwrap();
    let alias = MacroDefinition::new(MacroKind::Alias, "lh", "legacy home");
    let library = serde_json::json!([
        {"Id": "cccccccc-0000-4000-8000-0000000000e1", "Name": "Legacy script", "Source": "// legacy", "Enabled": true, "Macro": null},
        {"Id": "cccccccc-0000-4000-8000-0000000000e2", "Name": "Legacy alias", "Source": alias.compile_javascript().unwrap(),
         "Enabled": false, "Macro": serde_json::to_value(&alias).unwrap()},
        // Deleted in the C# client: stays deleted.
        {"Id": "cccccccc-0000-4000-8000-000000000009", "Name": "Removed", "Source": "// gone", "Enabled": false},
    ]);
    std::fs::write(
        cs.join("scripts").join(format!(
            "{}.scripts.json",
            sha_upper("old-lantern.fixture.example:4000")
        )),
        library.to_string(),
    )
    .unwrap();
    let before = hashes(&cs);

    let dir = temp("legacy");
    let source = CsharpSource::open(&cs).unwrap();
    let vault = MemoryVault::new();
    let report = import_into_dir(&source, &dir, ImportOptions::default(), &vault, &vault, &|_| {}).unwrap();
    check(&report, Kind::Maps, 3, 3, 0);
    check(&report, Kind::Scripts, 7, 6, 0);
    check(&report, Kind::Macros, 5, 5, 0);
    assert_eq!(skipped(&report, Kind::Scripts, Skip::Deleted), 1);
    let (settings, _) = Settings::load(&dir, 50_000);
    let (db, _) = Database::open(&dir).unwrap();
    let store = MapStore::new(db.clone());
    let moss_world = settings.worlds[2].world_id.clone();
    let map = store.load(&MapWorld::Id(moss_world.clone())).unwrap().unwrap();
    assert_eq!(map.rooms.len(), 2);
    assert_eq!(map.links[0].to_id, "t:moss-b");
    let lantern = store
        .load(&MapWorld::Id(settings.worlds[0].world_id.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(lantern.rooms.len(), 6, "a moved file is not imported again");
    let conn = db.connect().unwrap();
    let moss_library = crate::db::scripts::load(&conn, &moss_world).unwrap();
    assert_eq!(moss_library.len(), 1);
    assert_eq!(moss_library[0].source, "mud.send('legacy');");
    let lantern_library = crate::db::scripts::load(&conn, &settings.worlds[0].world_id).unwrap();
    assert!(lantern_library.iter().any(|e| e.name == "Legacy alias" && e.is_macro()));
    assert!(!lantern_library.iter().any(|e| e.name == "Removed"));
    drop(source);
    assert_eq!(hashes(&cs), before, "the C# files were changed");

    let rows = dump(&dir);
    let source = CsharpSource::open(&cs).unwrap();
    let again = import_into_dir(&source, &dir, ImportOptions::default(), &vault, &vault, &|_| {}).unwrap();
    check(&again, Kind::Maps, 3, 0, 3);
    check(&again, Kind::Scripts, 7, 0, 6);
    assert_eq!(dump(&dir), rows);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&cs);
}

/// A copy of the main fixture whose Lantern Road login has the odd spellings a C# file can hold:
/// the username with spaces around it, the host in capitals, and an older `host:port:True`
/// address. Returns the C# entry's key for that login.
fn odd_login_fixture(dir: &Path) -> String {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::copy(fixture("main").join(DB_FILE), dir.join(DB_FILE)).unwrap();
    let conn = Connection::open(dir.join(DB_FILE)).unwrap();
    conn.execute(
        "UPDATE profiles SET payload = replace(replace(payload, '\"Username\":\"wayfarer\"', '\"Username\":\" Talek \"'),
             '\"Host\":\"lantern.fixture.example\"', '\"Host\":\"Lantern.Fixture.Example\"') WHERE id = ?1",
        [LANTERN_PROFILE],
    )
    .unwrap();
    let world: String = conn
        .query_row("SELECT world_id FROM profiles WHERE id = ?1", [LANTERN_PROFILE], |r| {
            r.get(0)
        })
        .unwrap();
    conn.execute(
        "INSERT INTO legacy_endpoints(endpoint_key, source_key, world_id) VALUES('lantern.fixture.example:4000', 'lantern.fixture.example:4000:True', ?1)",
        [&world],
    )
    .unwrap();
    drop(conn);
    csharp_login_key(
        LANTERN_PROFILE,
        "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeee01",
        "Lantern.Fixture.Example",
        4000,
        false,
        " Talek ",
    )
}

/// The world editor saves a username trimmed, so an imported one is trimmed too: the editor's
/// copy then has the imported world's vault key and a blank password keeps the copied entry
/// (owner report: "Enter a password to save" on an untouched imported world). Without
/// `--include-passwords` the world keeps its password reference (a later import with passwords
/// fills the same entry) and `has_saved` reports the entry missing, which the editor shows.
#[test]
fn an_imported_login_with_odd_spelling_keeps_its_password_in_the_editor() {
    use crate::login::credentials::{has_saved, keep_saved};
    let source_dir = temp("odd-login-source");
    let from_key = odd_login_fixture(&source_dir);
    let source = CsharpSource::open(&source_dir).unwrap();
    let from = MemoryVault::new();
    from.write(&from_key, "fixture-secret").unwrap();

    // Without passwords: the reference stays, the entry is reported missing.
    let plain = temp("odd-login-plain");
    let to = MemoryVault::new();
    import_into_dir(&source, &plain, ImportOptions::default(), &from, &to, &|_| {}).unwrap();
    let (settings, _) = Settings::load(&plain, 50_000);
    let lantern = &settings.worlds[0];
    assert_eq!(lantern.username, "Talek");
    assert_eq!(lantern.host, "Lantern.Fixture.Example");
    assert!(lantern.password_id.is_some());
    assert_eq!(has_saved(&to, lantern), Ok(false));
    assert!(to.is_empty());

    // With passwords: the entry is under the imported world's key, and the editor's copy (the
    // fields trimmed, nothing changed) keeps it.
    let with = temp("odd-login-with");
    let options = ImportOptions {
        dry_run: false,
        include_secrets: true,
    };
    let report = import_into_dir(&source, &with, options, &from, &to, &|_| {}).unwrap();
    assert_eq!(report.tally(Kind::Passwords).imported, 1, "{}", report.to_text());
    let (settings, _) = Settings::load(&with, 50_000);
    let lantern = settings.worlds[0].clone();
    assert_eq!(has_saved(&to, &lantern), Ok(true));
    let edited = SavedWorld {
        username: lantern.username.trim().to_string(),
        host: lantern.host.trim().to_string(),
        name: "Renamed".into(),
        ..lantern.clone()
    };
    assert_eq!(keep_saved(Some(&lantern), &edited), Ok(()));
    let key = crate::login::vault::key(&edited).unwrap();
    assert_eq!(to.read(&key).unwrap().as_deref(), Some("fixture-secret"));
    // The older `:True` address maps to the same world: no second Lantern Road.
    assert_eq!(
        settings
            .worlds
            .iter()
            .filter(|w| w.name == "Fixture Lantern Road")
            .count(),
        1
    );
    for dir in [&source_dir, &plain, &with] {
        let _ = std::fs::remove_dir_all(dir);
    }
}
