//! Script packs: the scripts a world's directory listing supplies (the C#
//! `WorldScriptLibrary.ApplyPack`). When a session opens a listed world, its library takes the
//! listing's supported scripts:
//!
//! - A new pack script is added enabled, with its send policy in force: it may send commands only
//!   from an alias or a panel button until the person allows it (the engine refuses the rest).
//! - A pack script whose version changed takes the new name and source and keeps the person's
//!   enable and send choices. The same version changes nothing.
//! - The listing is the whole pack: a pack script it no longer names is removed. Hand-written
//!   scripts and macros are never touched. An empty listing changes nothing.
//! - Pack scripts are read only in the editor; Duplicate makes an ordinary copy.

use crate::db::scripts::{self, LibraryEntry, PackInfo};
use crate::directory::listing::WorldScriptListing;

/// The library after applying `supplied` to `library` of world `world`, or `None` when nothing
/// changes. New scripts that would pass the 64-entry limit are left out.
pub fn apply(library: &[LibraryEntry], world: &str, supplied: &[WorldScriptListing]) -> Option<Vec<LibraryEntry>> {
    if supplied.is_empty() {
        return None;
    }
    let wanted: Vec<String> = supplied.iter().map(|s| scripts::pack_entry_id(world, &s.id)).collect();
    let mut next: Vec<LibraryEntry> = library
        .iter()
        .filter(|e| !e.is_pack() || wanted.contains(&e.id))
        .cloned()
        .collect();
    for (listing, id) in supplied.iter().zip(&wanted) {
        let info = PackInfo {
            pack_id: listing.id.clone(),
            provenance: listing.provenance.clone(),
            version: listing.version,
            description: listing.description.clone(),
        };
        let name = listing.name.trim().to_string();
        match next.iter_mut().find(|e| &e.id == id) {
            None => {
                if next.len() >= scripts::MAX_ENTRIES {
                    continue;
                }
                let mut entry = LibraryEntry {
                    id: id.clone(),
                    name,
                    source: listing.source.clone(),
                    enabled: true,
                    ..LibraryEntry::default()
                };
                entry.set_pack(&info, false);
                if entry.validate().is_ok() {
                    next.push(entry);
                }
            }
            Some(existing) => {
                let Some(previous) = existing.pack() else { continue };
                if previous.version == listing.version {
                    continue;
                }
                let mut updated = existing.clone();
                updated.name = name;
                updated.source = listing.source.clone();
                let allow = existing.allow_send();
                updated.set_pack(&info, allow);
                if updated.validate().is_ok() {
                    *existing = updated;
                }
            }
        }
    }
    (next != library).then_some(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macros::{MacroDefinition, MacroKind};

    const FIRST: &str = "mud.panel('ship', { title: 'Ship' }).label('a', { text: 'v1' });";
    const SECOND: &str = "mud.panel('ship', { title: 'Ship' }).label('a', { text: 'v2' });";

    fn listing(id: &str, name: &str, source: &str, provenance: &str, version: i64) -> WorldScriptListing {
        WorldScriptListing {
            id: id.into(),
            name: name.into(),
            description: "Ship telemetry.".into(),
            source: source.into(),
            provenance: provenance.into(),
            version,
        }
    }

    fn hand_written() -> Vec<LibraryEntry> {
        vec![
            scripts::starter(),
            LibraryEntry::new_macro("Look", MacroDefinition::new(MacroKind::Shortcut, "F2", "look")),
        ]
    }

    /// C# `ASuppliedScriptIsAttachedOnOpenMarkedAsAPackAndRefreshedWhenItsVersionChanges`, on
    /// the library: attached enabled with the policy in force, refreshed by version with the
    /// person's choices kept, retired when the pack renames it; hand-written entries stay.
    #[test]
    fn a_supplied_script_is_attached_marked_as_a_pack_and_refreshed_when_its_version_changes() {
        let world = "3f2504e0-4f89-11d3-9a0c-0305e82c3301";
        let mine = hand_written();
        let pack = [listing("ship-panel", " Ship panel ", FIRST, "generated", 1)];
        let installed = apply(&mine, world, &pack).expect("installed");
        assert_eq!(installed.len(), 3);
        assert_eq!(&installed[..2], &mine[..]);
        let entry = &installed[2];
        assert_eq!(entry.id, scripts::pack_entry_id(world, "ship-panel"));
        assert_eq!(entry.name, "Ship panel");
        assert_eq!(entry.source, FIRST);
        assert!(entry.enabled && entry.is_pack() && !entry.allow_send() && entry.restricted_send());
        let info = entry.pack().unwrap();
        assert_eq!((info.provenance.as_str(), info.version), ("generated", 1));
        assert_eq!(info.description, "Ship telemetry.");
        assert!(scripts::validate_library(&installed).is_ok());

        // The same version again: nothing to do.
        assert_eq!(apply(&installed, world, &pack), None);

        // The person turns it off and allows sending; version 2 keeps both choices.
        let mut chosen = installed.clone();
        chosen[2].enabled = false;
        chosen[2].set_allow_send(true);
        let upgraded = apply(
            &chosen,
            world,
            &[listing("ship-panel", "Ship panel", SECOND, "generated", 2)],
        )
        .expect("upgraded");
        assert_eq!(upgraded.len(), 3);
        assert_eq!(upgraded[2].id, chosen[2].id);
        assert_eq!(upgraded[2].source, SECOND);
        assert_eq!(upgraded[2].pack().unwrap().version, 2);
        assert!(!upgraded[2].enabled);
        assert!(upgraded[2].allow_send() && !upgraded[2].restricted_send());

        // A regenerated pack that renames the script retires the old one.
        let renamed = apply(
            &upgraded,
            world,
            &[listing("cockpit", "Cockpit", SECOND, "reviewed", 1)],
        )
        .expect("renamed");
        assert_eq!(renamed.len(), 3);
        assert!(renamed.iter().all(|e| e.id != upgraded[2].id));
        assert_eq!(renamed[2].id, scripts::pack_entry_id(world, "cockpit"));
        assert_eq!(&renamed[..2], &mine[..]);

        // An empty listing changes nothing; another world gets other ids.
        assert_eq!(apply(&renamed, world, &[]), None);
        assert_ne!(
            scripts::pack_entry_id(world, "cockpit"),
            scripts::pack_entry_id("another", "cockpit")
        );
    }

    #[test]
    fn the_pack_id_reads_as_the_csharp_guid() {
        // C#: new Guid(SHA256("wandur-script-pack\nworld\nship")[..16]).ToString().
        use sha2::Digest;
        let hash = sha2::Sha256::digest(b"wandur-script-pack\nworld\nship");
        let id = scripts::pack_entry_id("world", "ship");
        let hex: String = hash[..16].iter().map(|b| format!("{b:02x}")).collect();
        // The first three fields are little-endian in a .NET Guid.
        let expected = format!(
            "{}{}{}{}-{}{}-{}{}-{}-{}",
            &hex[6..8],
            &hex[4..6],
            &hex[2..4],
            &hex[0..2],
            &hex[10..12],
            &hex[8..10],
            &hex[14..16],
            &hex[12..14],
            &hex[16..20],
            &hex[20..32]
        );
        assert_eq!(id, expected);
    }

    #[test]
    fn the_listing_keeps_only_scripts_it_supports() {
        let json = serde_json::json!({
            "id": "lotj", "name": "Legends", "host": "127.0.0.1", "port": 4000,
            "scripts": [
                {"id": "ship-panel", "name": "Ship panel", "source": FIRST, "provenance": "generated", "version": 1},
                {"id": "community", "name": "Community", "source": "mud.echo('x');", "provenance": "community", "version": 1},
                {"id": "ship-panel", "name": "Twice", "source": FIRST, "provenance": "generated", "version": 1},
                {"id": "", "name": "No id", "source": FIRST, "provenance": "generated", "version": 1},
                {"id": "neg", "name": "Negative", "source": FIRST, "provenance": "reviewed", "version": -1},
                {"id": "bad", "name": 7},
                "not an object"
            ]
        });
        let listing: crate::directory::listing::WorldListing = serde_json::from_value(json).unwrap();
        assert_eq!(listing.scripts.len(), 5, "entries of the wrong shape are dropped");
        let supported = listing.supported_scripts();
        assert_eq!(supported.len(), 1);
        assert_eq!(supported[0].name, "Ship panel");
        let none: crate::directory::listing::WorldListing =
            serde_json::from_value(serde_json::json!({"id": "x", "scripts": null})).unwrap();
        assert!(none.scripts.is_empty());
    }

    #[test]
    fn a_full_library_takes_no_new_pack_scripts() {
        let mut full: Vec<LibraryEntry> = (0..scripts::MAX_ENTRIES).map(|_| scripts::starter()).collect();
        assert_eq!(apply(&full, "w", &[listing("a", "A", FIRST, "generated", 1)]), None);
        full.pop();
        let next = apply(&full, "w", &[listing("a", "A", FIRST, "generated", 1)]).unwrap();
        assert_eq!(next.len(), scripts::MAX_ENTRIES);
    }
}
