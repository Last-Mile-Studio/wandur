//! The C# `MudletImportTests` and `MudletLuaWrapperTests`, from the hand-written fictional
//! fixtures (The Lantern Road; Wren, Odo and Bastian) in `tests/fixtures/mudlet`.

use std::path::{Path, PathBuf};

use super::converter::{Converter, ImportPlan};
use super::importer::{apply, resolve_world};
use super::model::{ImportError, ItemKind, Package, Pattern, Source};
use super::summary::reason;
use super::zip::writer::{Entry as ZipEntry, build as build_zip};
use super::{lua_wrapper, parser, source};
use crate::db::scripts::{self, LibraryEntry};
use crate::macros::MacroKind;
use crate::scripting::engines::EngineSet;
use crate::scripting::{ActionKind, EventKind, Runtime, ScriptEvent, ScriptResult};
use crate::settings::SavedWorld;

const WORLD_KEY: &str = "lanternroad.example.net:4100:True";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mudlet")
        .join(name)
}

fn profile_xml() -> Vec<u8> {
    std::fs::read(fixture("lantern-road-profile.xml")).unwrap()
}

fn profile() -> Package {
    parser::parse(&profile_xml()).expect("the fixture parses")
}

fn source_of(packages: Vec<Package>) -> Source {
    Source {
        name: "The Lantern Road".into(),
        host: "lanternroad.example.net".into(),
        port: Some(4100),
        tls: Some(true),
        is_profile: true,
        packages,
        ..Source::default()
    }
}

fn convert(source: &Source, available: usize) -> ImportPlan {
    Converter::new(WORLD_KEY, available).convert(source)
}

fn profile_plan() -> ImportPlan {
    convert(&source_of(vec![profile()]), scripts::MAX_ENTRIES)
}

fn named<'a>(plan: &'a ImportPlan, name: &str) -> &'a LibraryEntry {
    plan.scripts.iter().find(|s| s.name == name).unwrap_or_else(|| {
        panic!(
            "no script {name}: {:?}",
            plan.scripts.iter().map(|s| &s.name).collect::<Vec<_>>()
        )
    })
}

/// A generated script running in its own engine.
struct Running(EngineSet);

impl Running {
    fn new(script: &LibraryEntry) -> Self {
        let mut set = EngineSet::default();
        let result = set.load("s", &script.source, false);
        assert_eq!(result.error, None, "{}", script.source);
        Self(set)
    }

    fn lua(source: &str) -> (Self, ScriptResult) {
        let mut set = EngineSet::default();
        let result = set.load_with("s", source, false, Runtime::MUDLET);
        (Self(set), result)
    }

    fn event(&mut self, kind: EventKind, text: &str) -> ScriptResult {
        self.0.dispatch_one("s", &ScriptEvent::new(kind, text))
    }

    fn line(&mut self, text: &str) -> Vec<String> {
        sent(&self.event(EventKind::Line, text))
    }

    fn tick(&mut self, ms: u64) -> ScriptResult {
        self.0.dispatch_one("s", &ScriptEvent::tick(ms))
    }
}

fn sent(result: &ScriptResult) -> Vec<String> {
    result
        .actions
        .iter()
        .filter(|a| a.kind == ActionKind::Send)
        .map(|a| a.text.clone())
        .collect()
}

fn actions(result: &ScriptResult) -> Vec<String> {
    result
        .actions
        .iter()
        .map(|a| {
            format!(
                "{}:{}",
                if a.kind == ActionKind::Send { "send" } else { "echo" },
                a.text
            )
        })
        .collect()
}

#[test]
fn the_profile_reads_host_items_and_folders() {
    let package = profile();
    assert!(package.has_host);
    assert_eq!(package.host_name, "The Lantern Road");
    assert_eq!(package.url, "lanternroad.example.net");
    assert_eq!(package.port, Some(4100));
    assert_eq!(package.tls, Some(true));
    assert_eq!(package.command_separator, ";;");
    let names: Vec<&str> = package.triggers.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Travel", "Combat", "Idle"]);
    assert!(package.triggers.iter().all(|t| t.is_folder));
    assert!(!package.triggers[2].is_active);
    assert_eq!(package.variable_count, 2);
    assert_eq!(package.modules, ["RoadSigns"]);
    let greet = package.triggers[0]
        .children
        .iter()
        .find(|t| t.name == "Greet arrivals")
        .unwrap();
    assert_eq!(
        greet.patterns,
        [Pattern::new(Pattern::REGEX, r"^(\w+) arrives from the (?P<dir>\w+)\.$")]
    );
    let ambush = package.triggers[1]
        .children
        .iter()
        .find(|t| t.name == "Ambush")
        .unwrap();
    assert_eq!(ambush.children[0].patterns[0].text, "A bandit flees");
    let low = package.triggers[1]
        .children
        .iter()
        .find(|t| t.name == "Low health")
        .unwrap();
    assert_eq!(
        low.script,
        "local hp = tonumber(matches[2])\nif hp < 50 then\n  send(\"quaff tonic\")\nend"
    );
}

#[test]
fn each_pattern_type_is_mapped_or_named() {
    let plan = profile_plan();
    let travel = named(&plan, "Travel (Mudlet)");
    assert!(travel.enabled);
    let mut engine = Running::new(travel);
    // Substring anywhere in the line.
    assert_eq!(engine.line("Far off, A lantern flickers to life."), ["warm hands"]);
    // Beginning of line, with the command field split at the separator.
    assert_eq!(engine.line("The gate opens with a groan."), ["north", "look"]);
    assert!(engine.line("Odo says The gate opens").is_empty());
    // Exact match only; a dot is literal.
    assert_eq!(engine.line("The tollkeeper waits."), ["pay toll"]);
    assert!(engine.line("The tollkeeper waits!").is_empty());
    assert!(engine.line("The tollkeeper waits. Again.").is_empty());
    // Perl regex with captures numbered as Mudlet numbers them, and a named group rewritten.
    assert_eq!(
        engine.line("Odo arrives from the east."),
        ["wave Odo", "say Welcome to the road, Odo!"]
    );
    // A prompt pattern fires on the prompt event.
    assert_eq!(sent(&engine.event(EventKind::Prompt, "HP 30/30 >")), ["rest"]);

    let mut combat = Running::new(named(&plan, "Combat (Mudlet)"));
    // A leading (?i) becomes the case-insensitive flag.
    assert_eq!(combat.line("WREN is bleeding badly."), ["bandage wren"]);

    let reasons = |path: &str| {
        plan.summary
            .needs_conversion
            .iter()
            .find(|e| e.path == path)
            .map(|e| e.reason)
    };
    assert_eq!(reasons("Combat / Low health"), Some(reason::LUA));
    assert_eq!(reasons("Combat / Two lines"), Some(reason::MULTILINE));
    assert_eq!(reasons("Combat / Red text"), Some(reason::PATTERN_TYPE));
    assert_eq!(reasons("Combat / Possessive"), Some(reason::REGEX));
    assert_eq!(reasons("Combat / Highlight Bastian"), Some(reason::EFFECTS));
}

#[test]
fn groups_become_scripts_and_inactive_folders_come_in_switched_off() {
    let plan = profile_plan();
    let names: Vec<&str> = plan.scripts.iter().map(|s| s.name.as_str()).collect();
    for name in [
        "Travel (Mudlet)",
        "Combat (Mudlet)",
        "Combat (Mudlet, needs conversion)",
    ] {
        assert!(names.contains(&name), "{names:?}");
    }
    let idle = named(&plan, "Idle (Mudlet, was off)");
    assert!(!idle.enabled);
    assert!(!idle.needs_conversion());
    assert_eq!(Running::new(idle).line("You feel sleepy."), ["sleep"]);
    assert_eq!(plan.summary.off_in_mudlet.get(&ItemKind::Trigger), Some(&1));
    // Aliases and triggers of the same folder name share one script.
    let mut travel = Running::new(named(&plan, "Travel (Mudlet)"));
    assert!(travel.event(EventKind::Command, "k bandit").handled);
    assert!(
        plan.scripts
            .iter()
            .all(|s| s.import.as_ref().is_some_and(|i| i.origin == "mudlet"))
    );
}

#[test]
fn aliases_expand_through_their_own_script_and_keys_become_macros() {
    let plan = profile_plan();
    let mut travel = Running::new(named(&plan, "Travel (Mudlet)"));
    let home = travel.event(EventKind::Command, "home");
    assert!(home.handled);
    assert_eq!(sent(&home), ["recall", "rest"]);
    assert_eq!(
        sent(&travel.event(EventKind::Command, "k hooded figure")),
        ["kill hooded figure"]
    );
    assert_eq!(
        sent(&travel.event(EventKind::Command, "gp")),
        ["say Hello, Odo!", "say Hello, Bastian!"]
    );
    assert!(!travel.event(EventKind::Command, "look").handled);

    let key = plan
        .scripts
        .iter()
        .find(|s| s.macro_def.as_ref().is_some_and(|m| m.kind == MacroKind::Shortcut))
        .expect("a key macro");
    let definition = key.macro_def.as_ref().unwrap();
    assert_eq!(definition.pattern, "F1");
    assert_eq!(definition.commands, "cast heal");
    assert!(key.enabled);
    assert_eq!(key.name, "F1: Heal (Mudlet)");
    assert!(key.validate().is_ok(), "the key macro's source is its compiled form");
    assert!(
        plan.summary
            .needs_conversion
            .iter()
            .any(|e| e.path == "Ctrl K" && e.reason == reason::KEY)
    );

    let mut timers = Running::new(named(&plan, "Ungrouped (Mudlet)"));
    assert!(sent(&timers.tick(29_000)).is_empty());
    assert_eq!(sent(&timers.tick(30_000)), ["drink waterskin"]);
    assert!(
        plan.summary
            .needs_conversion
            .iter()
            .any(|e| e.path == "Too fast" && e.reason == reason::TIMER)
    );
}

#[test]
fn everything_else_is_kept_off_with_its_lua_unchanged() {
    let plan = profile_plan();
    let lua: Vec<&LibraryEntry> = plan.scripts.iter().filter(|s| s.needs_conversion()).collect();
    assert!(lua.iter().all(|s| !s.enabled));
    let kept: Vec<_> = lua
        .iter()
        .flat_map(|s| s.import.as_ref().unwrap().items.iter())
        .collect();
    let item = |name: &str| *kept.iter().find(|i| i.name == name).unwrap();
    let low = item("Low health");
    assert_eq!(
        low.code,
        "local hp = tonumber(matches[2])\nif hp < 50 then\n  send(\"quaff tonic\")\nend"
    );
    assert_eq!(low.patterns, [r"regex:^HP: (\d+)/(\d+)"]);
    assert_eq!(low.path, "Combat");
    assert!(
        named(&plan, "Combat (Mudlet, needs conversion)")
            .source
            .contains(&low.code)
    );
    // A chain head and its child stay together as needing conversion.
    assert_eq!(item("Ambush").reason, reason::CHAIN);
    assert_eq!(item("Bandit flees").reason, reason::CHAIN);
    assert_eq!(item("status window").reason, reason::GEYSER);
    assert_eq!(item("Road mapper").reason, reason::MAPPER);
    let vitals = item("Vitals");
    assert_eq!(vitals.reason, reason::SCRIPT);
    assert_eq!(vitals.patterns, ["event:gmcp.Char.Vitals"]);
    assert_eq!(item("Camp").reason, reason::BUTTON);

    assert!(
        plan.summary
            .left_out
            .iter()
            .any(|e| e.reason == reason::MODULES && e.path == "RoadSigns")
    );
    assert_eq!(plan.summary.variables_left_out, 2);
    let text = plan.summary.to_text(5);
    assert!(text.contains("Working now"), "{text}");
    assert!(text.contains("needing conversion"), "{text}");
    assert!(text.contains("Left out"), "{text}");
}

/// The summary text matches the C# capture of the two fixtures imported into The Lantern Road
/// (`ref-shots/mudlet-import-summary.png`).
#[test]
fn the_summary_reads_as_the_reference() {
    let gate = parser::parse(&std::fs::read(fixture("gate-helper-package.xml")).unwrap()).unwrap();
    let mut plan = convert(&source_of(vec![profile(), gate]), scripts::MAX_ENTRIES);
    plan.summary.world_name = "The Lantern Road".into();
    plan.summary.world_created = true;
    let text = plan.summary.to_text(5);
    let expected = [
        "Added the world The Lantern Road.",
        "",
        "Working now: Triggers (7), Aliases (5), Timers (1), Keys (1).",
        "Converted but switched off, because they were off in Mudlet: Triggers (1).",
        "",
        "Brought in switched off, with the Lua kept unchanged and marked as needing conversion: 13",
        "  part of a trigger chain or filter folder: 2",
        "    Combat / Ambush, Combat / Ambush / Bandit flees",
        "  runs Lua that does more than send commands: 1",
        "    Combat / Low health",
        "  a multi-line (AND) trigger: 1",
        "    Combat / Two lines",
        "  uses a pattern type Wandur cannot match yet (Lua function, line spacer or colour): 1",
        "    Combat / Red text",
        "  uses regular expression features JavaScript does not have: 1",
        "    Combat / Possessive",
        "  highlights, filters, plays a sound or matches colours: 1",
        "    Combat / Highlight Bastian",
        "  builds Geyser windows, which Wandur does not have: 1",
        "    status window",
        "  a timer under one second, an offset timer or an unreadable time: 1",
        "    Too fast",
        "  a key Wandur cannot bind (Wandur binds F1 to F12 without modifiers): 1",
    ];
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(&lines[..expected.len()], expected, "{text}");
}

#[test]
fn a_package_goes_into_the_selected_world_and_keeps_its_name() {
    let gate = std::fs::read(fixture("gate-helper-package.xml")).unwrap();
    let package = parser::parse(&gate).unwrap();
    assert!(!package.has_host);
    let archive = build_zip(&[
        ZipEntry {
            name: "gate-helper.xml",
            data: gate.clone(),
            deflate: true,
            declared: None,
        },
        ZipEntry {
            name: "config.lua",
            data: b"mpackage = \"Gate Helper\"\ncreated = \"2026-10-01\"\n".to_vec(),
            deflate: false,
            declared: None,
        },
        ZipEntry {
            name: "art/lantern.txt",
            data: b"not a package".to_vec(),
            deflate: false,
            declared: None,
        },
    ]);
    let dir = scratch("package");
    let path = dir.join("gate-helper.mpackage");
    std::fs::write(&path, &archive).unwrap();
    let read = source::read(&path).unwrap();
    assert_eq!(read.name, "Gate Helper");
    assert!(!read.is_profile);
    assert!(resolve_world(&read, &[], None).is_err());
    let selected = SavedWorld {
        name: "The Lantern Road".into(),
        host: "lanternroad.example.net".into(),
        port: 4100,
        tls: true,
        ..SavedWorld::default()
    };
    let target = resolve_world(&read, std::slice::from_ref(&selected), Some(0)).unwrap();
    assert!(!target.created());
    assert_eq!(target.world, selected);
    let plan = convert(&read, scripts::MAX_ENTRIES);
    let mut names: Vec<&str> = plan.scripts.iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["Gate helper (Mudlet)", "Ungrouped (Mudlet)"]);
    let mut alias = Running::new(named(&plan, "Ungrouped (Mudlet)"));
    assert_eq!(
        sent(&alias.event(EventKind::Command, "og")),
        ["unlock gate", "open gate"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_archive_with_an_unsafe_path_is_refused_whole() {
    let gate = std::fs::read(fixture("gate-helper-package.xml")).unwrap();
    for entry in [
        "../escape.xml",
        "/absolute.xml",
        "nested/../../escape.xml",
        "C:/windows.xml",
        "back\\slash.xml",
        "NUL.xml",
    ] {
        let archive = build_zip(&[
            ZipEntry {
                name: "package.xml",
                data: gate.clone(),
                deflate: false,
                declared: None,
            },
            ZipEntry {
                name: entry,
                data: b"<MudletPackage />".to_vec(),
                deflate: false,
                declared: None,
            },
        ]);
        let error = source::read_archive(&archive, "evil").unwrap_err();
        assert!(error.0.contains("outside"), "{entry}: {error}");
    }
}

#[test]
fn an_archive_that_expands_past_the_limit_is_refused() {
    // Declared past the limit: refused before anything is inflated.
    let declared = (source::MAX_EXPANDED_BYTES + 1) as u32;
    let bomb = build_zip(&[ZipEntry {
        name: "bomb.xml",
        data: vec![0; 1024 * 1024],
        deflate: true,
        declared: Some(declared),
    }]);
    assert!(bomb.len() < 2 * 1024 * 1024);
    assert_eq!(
        source::read_archive(&bomb, "bomb").unwrap_err(),
        ImportError::new(crate::l10n::S::MudletImportTooLarge)
    );
    // Declared small but inflating to more: refused when the data passes what was declared.
    let liar = build_zip(&[ZipEntry {
        name: "liar.xml",
        data: vec![b' '; 4 * 1024 * 1024],
        deflate: true,
        declared: Some(100),
    }]);
    assert!(source::read_archive(&liar, "liar").is_err());
}

#[test]
fn entity_expansion_and_external_entities_are_refused() {
    let laughs = br#"<?xml version="1.0"?>
<!DOCTYPE MudletPackage [
  <!ENTITY a "aaaaaaaaaa">
  <!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">
  <!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;">
]>
<MudletPackage><AliasPackage><Alias isActive="yes"><name>&c;</name><regex>^x$</regex></Alias></AliasPackage></MudletPackage>"#;
    assert!(parser::parse(laughs).is_err());
    let external = br#"<?xml version="1.0"?>
<!DOCTYPE MudletPackage [ <!ENTITY leak SYSTEM "file:///etc/hosts"> ]>
<MudletPackage><AliasPackage><Alias isActive="yes"><name>&leak;</name><regex>^x$</regex></Alias></AliasPackage></MudletPackage>"#;
    assert!(parser::parse(external).is_err());
    assert!(parser::parse(b"<NotMudlet />").is_err());
    assert!(parser::parse(b"<MudletPackage><unclosed></MudletPackage>").is_err());
    let mut deep = String::from("<MudletPackage><TriggerPackage>");
    for _ in 0..parser::MAX_DEPTH + 2 {
        deep.push_str("<TriggerGroup isFolder=\"yes\"><name>n</name>");
    }
    for _ in 0..parser::MAX_DEPTH + 2 {
        deep.push_str("</TriggerGroup>");
    }
    deep.push_str("</TriggerPackage></MudletPackage>");
    assert!(parser::parse(deep.as_bytes()).is_err());
    // The five built-in entities and character references are fine.
    let fine = parser::parse(
        br#"<MudletPackage><AliasPackage><Alias isActive="yes"><name>a&lt;b&amp;&#67;&#x44;</name><regex>^x$</regex></Alias></AliasPackage></MudletPackage>"#,
    )
    .unwrap();
    assert_eq!(fine.aliases[0].name, "a<b&CD");
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wandur-mudlet-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_profile_folder_gives_the_world_and_the_password_is_never_read() {
    let dir = scratch("profile");
    let profile_dir = dir.join("The Lantern Road");
    let current = profile_dir.join("current");
    std::fs::create_dir_all(&current).unwrap();
    let old = current.join("01-10-2026#09-00-00.xml");
    std::fs::write(
        &old,
        "<MudletPackage><HostPackage><Host><name>Old save</name></Host></HostPackage></MudletPackage>",
    )
    .unwrap();
    let newer = current.join("02-10-2026#21-30-00.xml");
    std::fs::write(&newer, profile_xml()).unwrap();
    let set_time = |path: &Path, seconds: u64| {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
            .unwrap();
    };
    set_time(&old, 1_790_000_000);
    set_time(&newer, 1_790_100_000);
    std::fs::write(profile_dir.join("url"), "lanternroad.example.net").unwrap();
    std::fs::write(profile_dir.join("port"), "4100").unwrap();
    std::fs::write(profile_dir.join("ssl_tsl"), "2").unwrap();
    std::fs::write(profile_dir.join("login"), "Wren").unwrap();
    let password = profile_dir.join("password");
    std::fs::write(&password, "fictional-password").unwrap();
    // An unreadable password file proves the reader never opens it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o000)).unwrap();
    }

    let read = source::read(&profile_dir).unwrap();
    assert_eq!(read.name, "The Lantern Road");
    assert!(read.is_profile);
    assert!(read.password_skipped);
    let target = resolve_world(&read, &[], None).unwrap();
    assert!(target.created());
    let world = &target.world;
    assert_eq!(
        (world.host.as_str(), world.port, world.tls, world.username.as_str()),
        ("lanternroad.example.net", 4100, true, "Wren")
    );
    assert_eq!(world.password_id, None);
    assert!(!world.auto_login);
    assert!(
        convert(&read, scripts::MAX_ENTRIES)
            .summary
            .to_text(5)
            .contains("password was not imported")
    );

    // The same world already saved is reused, not duplicated.
    let saved = SavedWorld {
        name: "Road".into(),
        host: "LanternRoad.example.net".into(),
        port: 4100,
        world_id: "a".repeat(32),
        ..SavedWorld::default()
    };
    let again = resolve_world(&read, std::slice::from_ref(&saved), None).unwrap();
    assert!(!again.created());
    assert_eq!(again.world.world_id, saved.world_id);
    assert_eq!(again.world.username, "Wren", "an empty login is filled in");
    // Not under a saved password, whose vault key includes the (empty) username.
    let locked = SavedWorld {
        password_id: Some("6f9619ff-8b86-d011-b42d-00c04fc964ff".into()),
        ..saved.clone()
    };
    let kept = resolve_world(&read, std::slice::from_ref(&locked), None).unwrap();
    assert_eq!(kept.world, locked, "the saved password stays reachable");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_round_trips_through_storage_and_a_reimport_keeps_edits_and_choices() {
    let dir = scratch("storage");
    let (db, _) = crate::db::Database::open(&dir).unwrap();
    let source = source_of(vec![profile()]);
    let starter = scripts::starter();
    db.write(|tx| scripts::ensure_started(tx, WORLD_KEY, &starter)).unwrap();
    let before = db.read(|c| scripts::load(c, WORLD_KEY)).unwrap();
    let (after, summary) = apply(&source, WORLD_KEY, "The Lantern Road", true, &before);
    db.write(|tx| scripts::save_changes(tx, WORLD_KEY, &before, &after))
        .unwrap();
    let (reopened, _) = crate::db::Database::open(&dir).unwrap();
    let loaded = reopened.read(|c| scripts::load(c, WORLD_KEY)).unwrap();
    let imported: Vec<&LibraryEntry> = loaded.iter().filter(|s| s.import.is_some()).collect();
    assert_eq!(summary.scripts_written, imported.len());
    let plan = convert(&source, scripts::MAX_ENTRIES - 1);
    for script in &plan.scripts {
        let stored = imported.iter().find(|s| s.id == script.id).expect("stored");
        assert_eq!(*stored, script);
    }
    // The starter script the library has for a new world is untouched.
    assert!(loaded.iter().any(|s| s.import.is_none() && s.id == starter.id));

    let travel = imported.iter().find(|s| s.name == "Travel (Mudlet)").unwrap();
    let idle = imported.iter().find(|s| s.name == "Idle (Mudlet, was off)").unwrap();
    let mut edited = (*travel).clone();
    edited.source.push_str("\n// Wren's change");
    let mut switched = (*idle).clone();
    switched.enabled = true;
    let mut changed = loaded.clone();
    for slot in changed.iter_mut() {
        if slot.id == edited.id {
            *slot = edited.clone();
        } else if slot.id == switched.id {
            *slot = switched.clone();
        }
    }
    reopened
        .write(|tx| scripts::save_changes(tx, WORLD_KEY, &loaded, &changed))
        .unwrap();
    let (again, second) = apply(&source, WORLD_KEY, "The Lantern Road", false, &changed);
    reopened
        .write(|tx| scripts::save_changes(tx, WORLD_KEY, &changed, &again))
        .unwrap();
    let after = reopened.read(|c| scripts::load(c, WORLD_KEY)).unwrap();
    let find = |id: &str| after.iter().find(|s| s.id == id).unwrap();
    assert!(find(&edited.id).source.trim_end().ends_with("// Wren's change"));
    assert!(find(&switched.id).enabled);
    assert!(
        second
            .left_out
            .iter()
            .any(|e| e.reason == reason::EDITED && e.path == "Travel (Mudlet)")
    );
    assert_eq!(after.iter().filter(|s| s.import.is_some()).count(), imported.len());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_library_with_little_room_merges_groups_then_leaves_out_what_needs_conversion_first() {
    let source = source_of(vec![profile()]);
    let full = convert(&source, scripts::MAX_ENTRIES);
    let tight = convert(&source, 4);
    assert!(full.scripts.len() > 4);
    assert!(tight.scripts.len() <= 4);
    assert!(
        tight.scripts.iter().any(|s| s.name.starts_with("Other groups")),
        "{:?}",
        tight.scripts.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
    for script in tight.scripts.iter().filter(|s| !s.needs_conversion() && !s.is_macro()) {
        assert_eq!(Running::new(script).event(EventKind::Line, "x").error, None);
    }
    let none = convert(&source, 0);
    assert!(none.scripts.is_empty());
    for entry in none
        .summary
        .left_out
        .iter()
        .filter(|e| e.kind.is_some() && e.reason != reason::TEMPORARY)
    {
        assert!(
            [reason::NO_ROOM, reason::EMPTY, reason::NO_PATTERN].contains(&entry.reason),
            "{entry:?}"
        );
    }
}

#[test]
fn every_generated_script_loads_and_validates() {
    for script in profile_plan().scripts {
        assert!(script.validate().is_ok(), "{}", script.name);
        if !script.needs_conversion() {
            Running::new(&script);
        }
        assert!(script.source.len() <= scripts::MAX_SOURCE_BYTES);
    }
}

#[test]
fn plain_text_patterns_escape_every_metacharacter() {
    let pattern = super::regex::literal("a.b*c(d)[e]{f}|g^h$i\\j/k+l?", true, true);
    let source = format!(
        "mud.trigger(new RegExp({}), () => mud.send('hit'));",
        serde_json::to_string(&pattern.source).unwrap()
    );
    let mut set = EngineSet::default();
    assert_eq!(set.load("s", &source, false).error, None);
    let hit = set.dispatch_one("s", &ScriptEvent::new(EventKind::Line, "a.b*c(d)[e]{f}|g^h$i\\j/k+l?"));
    assert_eq!(hit.actions.len(), 1);
    let miss = set.dispatch_one("s", &ScriptEvent::new(EventKind::Line, "aXb*c(d)[e]{f}|g^h$i\\j/k+l?"));
    assert!(miss.actions.is_empty());
}

// The C# `MudletLuaWrapperTests`: kept items run as Lua through the Mudlet layer.

fn wrapper_plan() -> ImportPlan {
    let source = Source {
        name: "The Lantern Road".into(),
        host: "lanternroad.example.net".into(),
        port: Some(4100),
        packages: vec![profile()],
        ..Source::default()
    };
    convert(&source, 64)
}

#[cfg(feature = "lua")]
#[test]
fn kept_combat_items_run_as_lua_and_what_cannot_run_is_listed() {
    let plan = wrapper_plan();
    let combat = named(&plan, "Combat (Mudlet, needs conversion)");
    let lua = lua_wrapper::build(combat.import.as_ref().unwrap(), ";;");
    assert!(lua.contains("local hp = tonumber(matches[2])"));
    assert!(lua.contains("-- Triggers: Combat / Two lines is not run here"), "{lua}");
    let (mut engine, load) = Running::lua(&lua);
    assert_eq!(load.error, None, "{lua}");
    assert_eq!(engine.line("HP: 20/90"), ["quaff tonic"]);
    assert!(engine.line("HP: 80/90").is_empty());
    // The multi-line trigger is listed, not registered, so its lines do nothing.
    assert!(
        engine
            .event(EventKind::Line, "Odo shouts about an arrow")
            .actions
            .is_empty()
    );
}

#[cfg(feature = "lua")]
#[test]
fn kept_scripts_load_and_unsupported_calls_report_themselves() {
    let plan = wrapper_plan();
    let ungrouped = named(&plan, "Ungrouped (Mudlet, needs conversion)");
    let lua = lua_wrapper::build(ungrouped.import.as_ref().unwrap(), ";;");
    let (mut engine, load) = Running::lua(&lua);
    assert_eq!(load.error, None, "{lua}");
    // The mapper script fails on its first call and says what is missing, without stopping the others.
    assert!(
        load.actions
            .iter()
            .any(|a| a.kind == ActionKind::Echo && a.text.contains("not supported in Wandur yet: createRoomID")),
        "{:?}",
        load.actions
    );
    assert_eq!(
        actions(&engine.event(EventKind::Gmcp, r#"Char.Vitals {"hp":12}"#)),
        ["echo:HP 12"]
    );
    let geyser = engine.event(EventKind::Command, "sw");
    assert!(geyser.handled);
    assert!(
        geyser
            .actions
            .iter()
            .any(|a| a.text.contains("not supported in Wandur yet: Geyser.Label"))
    );
    // The sub-second timer now runs through a chain of one-shot timers.
    assert_eq!(sent(&engine.tick(500)), ["blink"]);
    assert_eq!(sent(&engine.tick(1000)), ["blink"]);
    assert_eq!(engine.event(EventKind::Line, "still here").error, None);
}

#[test]
fn imported_kinds_and_reasons_have_labels() {
    for kind in ItemKind::ALL {
        assert_eq!(ItemKind::from_key(kind.key()), Some(kind));
        assert!(!kind.label().is_empty());
    }
    assert_eq!(super::summary::describe("unknown-reason"), "unknown-reason");
    assert_ne!(super::summary::describe(reason::LUA), reason::LUA);
}
