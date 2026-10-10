//! The directory snapshot: the wire and disk format (`format: "wandur.directory"`,
//! `schema_version: 2`). The C# client's legacy version 1 (raw MUDVerse) is not read.

use serde::Deserialize;

use crate::l10n::{S, t, tf};

use super::listing::WorldListing;
use super::time::parse_rfc3339;

pub const FORMAT: &str = "wandur.directory";
pub const SCHEMA_VERSION: i64 = 2;

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// The snapshot's own timestamp (when the directory built it), seconds since 1970.
    pub fetched_at: Option<i64>,
    /// Sorted by name without case, as the C# client keeps them.
    pub worlds: Vec<WorldListing>,
    /// Entries left out because they had no id or name, or repeated an id.
    pub skipped: usize,
}

#[derive(Deserialize)]
struct Wire {
    format: Option<String>,
    schema_version: Option<i64>,
    fetched_at: Option<String>,
    #[serde(default)]
    worlds: Option<Vec<serde_json::Value>>,
}

/// Parse a snapshot. The format and version must match; a world entry that cannot be read is
/// skipped (the C# client rejects the whole snapshot instead), so one bad listing never hides the
/// directory.
pub fn parse(json: &str) -> Result<Snapshot, String> {
    let wire: Wire = serde_json::from_str(json).map_err(|e| tf(S::DirectoryUnreadableReason, &[&e]))?;
    if wire.format.as_deref() != Some(FORMAT) || wire.schema_version != Some(SCHEMA_VERSION) {
        return Err(t(S::DirectoryFormatUnsupported).into());
    }
    let entries = wire.worlds.unwrap_or_default();
    let mut worlds = Vec::with_capacity(entries.len());
    let mut seen = std::collections::HashSet::new();
    let mut skipped = 0;
    for entry in entries {
        match serde_json::from_value::<WorldListing>(entry) {
            Ok(w) if !w.id.trim().is_empty() && !w.name.trim().is_empty() && seen.insert(w.id.clone()) => {
                worlds.push(w);
            }
            _ => skipped += 1,
        }
    }
    worlds.sort_by_cached_key(|w| w.name.to_lowercase());
    Ok(Snapshot {
        fetched_at: wire.fetched_at.as_deref().and_then(parse_rfc3339),
        worlds,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const FIXTURE: &str = r#"{
      "format": "wandur.directory", "schema_version": 2, "fetched_at": "2026-09-15T20:00:00+00:00",
      "worlds": [
        {"id": "mudverse:7", "name": "Lantern & Forest", "summary": "A quiet world",
         "description": "A wizard's forest.\n\nVisit & explore.", "host": "mud.example.org", "port": 4000,
         "tls_port": 4001, "web_only": false, "established_at": null,
         "community": {"rating": null, "rating_count": null, "review_count": null, "rank": null, "monthly_votes": 123},
         "source": {"provider": "mudverse", "name": "MUDVerse", "record_id": "7",
                    "listing_url": "https://www.mudverse.com/game/7", "updated_at": "2026-09-14T20:00:00+00:00"},
         "availability": {"online": true, "archived": false, "archive_reason": null},
         "population": {"latest_count": 0, "observed_at": "2026-09-14T12:00:00+00:00", "reported_range": "75-100"},
         "features": {"theme": "Fantasy", "kind": "MUD", "language": "English", "codebase": "Custom", "roleplaying": null},
         "tags": ["Exploration", null], "banner_url": null, "generated_artwork_path": "games/7/art",
         "protocol_mapping": {"ignored": true}},
        {"id": "", "name": "No id"},
        {"id": "b", "name": "Aardvark", "port": 70000, "web_only": true},
        {"id": "b", "name": "Duplicate"},
        {"id": "c", "name": "Bad types", "port": "four thousand"}
      ]}"#;

    #[test]
    fn reads_the_wire_format_and_tolerates_nulls() {
        let s = parse(FIXTURE).unwrap();
        assert_eq!(s.worlds.len(), 2);
        assert_eq!(s.skipped, 3);
        assert_eq!(s.worlds[0].name, "Aardvark", "sorted by name");
        assert_eq!(s.worlds[0].port, None, "out of range port dropped");
        assert!(!s.worlds[0].can_connect());
        let w = &s.worlds[1];
        assert!(
            w.protocol_mapping.is_none(),
            "an invalid mapping is dropped, the listing kept"
        );
        assert_eq!(w.tags, ["Exploration"]);
        assert_eq!(w.features.roleplaying, "");
        assert_eq!(w.banner_url, "");
        assert!(w.can_connect() && w.is_online() && w.has_generated_artwork());
        assert_eq!(w.address(), "mud.example.org:4000");
        assert_eq!(w.source.updated_at, Some(1_789_416_000));
        assert_eq!(w.live_players(1_789_416_000), None, "not counted by Wandur");
        assert_eq!(w.online_text(0).as_deref(), Some("Online"));
        assert_eq!(s.fetched_at, Some(1_789_416_000 + 86_400));
    }

    #[test]
    fn refuses_other_formats() {
        assert!(parse(r#"{"schema_version": 1, "games": []}"#).is_err());
        assert!(parse("not json").is_err());
        assert!(
            parse(r#"{"format": "wandur.directory", "schema_version": 2}"#)
                .unwrap()
                .worlds
                .is_empty()
        );
    }

    #[test]
    fn live_counts_need_a_fresh_wandur_observation() {
        let mut w = WorldListing {
            availability: crate::directory::WorldAvailability {
                online: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        w.population.latest_count = Some(143);
        w.population.observed_at = Some(1000);
        w.population.source = Some("wandur".into());
        assert_eq!(w.live_players(1000 + 3600), Some(143));
        assert_eq!(w.online_text(1000 + 3600).as_deref(), Some("143 online"));
        assert_eq!(w.live_players(1000 + 3 * 3600), None);
        assert_eq!(
            w.population_history(1000 + 3 * 86_400).as_deref(),
            Some("Wandur counted 143, 3 days ago")
        );
    }

    #[test]
    fn row_pills_follow_the_site() {
        let w = WorldListing {
            features: crate::directory::WorldFeatures {
                theme: "Fantasy".into(),
                ..Default::default()
            },
            tags: vec![
                "fantasy".into(),
                "PvP".into(),
                "Quests".into(),
                "pvp".into(),
                "Crafting".into(),
            ],
            ..Default::default()
        };
        let (top, bottom) = w.row_pills();
        assert_eq!(top, ["Fantasy", "PvP", "Quests"]);
        assert_eq!(bottom, Some("Crafting"));
    }
}
