//! The versioned, bounded JSON map file (the C# `MapFileFormat`): `{"Version":1,"Map":{...}}`,
//! or a bare map (the C# client's older cache files). Version 2 adds labels and their pictures
//! (base64); a map without labels is still written as version 1, which the C# client reads.
//! Everything is validated before use: text lengths and control characters, finite
//! coordinates, limits on rooms, exits, labels, pictures and records, and references between
//! them.

use std::collections::HashSet;

use super::images::{self, MAX_IMAGE_BYTES, MAX_IMAGES_BYTES, MAX_LABELS};
use super::model::{MapImage, MapLabel, MapLink, MapRoom, MapSnapshot};

/// Largest map file (64 MiB: room for 20 MiB of pictures in base64 besides the map).
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
/// Largest map without its pictures' text (16 MiB, the C# client's file limit).
pub const MAX_MAP_BYTES: usize = 16 * 1024 * 1024;
/// The newest map file version.
pub const VERSION: i64 = 2;
/// Most rooms in a map.
pub const MAX_ROOMS: usize = 10_000;
/// Most exits in a map.
pub const MAX_LINKS: usize = 60_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError(pub String);

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FormatError {}

fn bad(message: &str) -> FormatError {
    FormatError(message.into())
}

/// How a text field is checked.
#[derive(Clone, Copy, Default)]
pub struct TextRule {
    /// Not blank.
    pub required: bool,
    /// May be absent.
    pub optional: bool,
    /// Line breaks and tabs allowed.
    pub multiline: bool,
}

pub const REQUIRED: TextRule = TextRule {
    required: true,
    optional: false,
    multiline: false,
};
pub const OPTIONAL: TextRule = TextRule {
    required: false,
    optional: true,
    multiline: false,
};
pub const PLAIN: TextRule = TextRule {
    required: false,
    optional: false,
    multiline: false,
};
pub const MULTILINE: TextRule = TextRule {
    required: false,
    optional: false,
    multiline: true,
};
pub const OPTIONAL_MULTILINE: TextRule = TextRule {
    required: false,
    optional: true,
    multiline: true,
};

/// The C# `MapFileFormat.Text`: present unless optional, at most `max` UTF-16 units, not blank
/// when required, no control characters (line breaks and tabs only when multiline).
pub fn text(value: Option<&str>, max: usize, rule: TextRule) -> bool {
    let Some(value) = value else {
        return rule.optional;
    };
    value.encode_utf16().count() <= max
        && (!rule.required || !value.trim().is_empty())
        && !value
            .chars()
            .any(|c| c.is_control() && !(rule.multiline && matches!(c, '\r' | '\n' | '\t')))
}

/// A coordinate the map accepts: finite, within a billion of the origin.
pub fn coordinate(value: f64) -> bool {
    value.is_finite() && value.abs() <= 1e9
}

pub fn valid_room(r: &MapRoom) -> bool {
    text(Some(&r.id), 256, REQUIRED)
        && text(Some(&r.name), 512, REQUIRED)
        && text(r.observed_name.as_deref(), 512, OPTIONAL)
        && text(r.observed_description.as_deref(), 16_000, OPTIONAL_MULTILINE)
        && text(r.observed_area.as_deref(), 512, OPTIONAL)
        && text(Some(&r.description), 16_000, MULTILINE)
        && text(r.area.as_deref(), 512, OPTIONAL)
        && coordinate(r.x)
        && coordinate(r.y)
        && coordinate(r.z)
        && text(r.server_id.as_deref(), 128, OPTIONAL)
        && text(r.environment.as_deref(), 128, OPTIONAL)
        && text(r.symbol.as_deref(), 32, OPTIONAL)
        && text(Some(&r.notes), 16_000, MULTILINE)
        && r.weight.is_finite()
        && r.weight > 0.0
        && r.weight <= 1e9
        && r.revision >= 0
        && r.color.as_deref().is_none_or(crate::settings::is_color)
        && text(r.inferred_environment.as_deref(), 128, OPTIONAL)
        && text(r.inferred_key.as_deref(), 128, OPTIONAL)
        && r.inferred_confidence
            .is_none_or(|c| c.is_finite() && (0.0..=1.0).contains(&c))
        && r.known_exits.len() <= 64
        && r.known_exits.iter().all(|e| text(Some(e), 64, REQUIRED))
}

pub fn valid_link(l: &MapLink) -> bool {
    text(Some(&l.from_id), 256, REQUIRED)
        && text(Some(&l.to_id), 256, REQUIRED)
        && text(Some(&l.direction), 64, REQUIRED)
        && text(l.command.as_deref(), 512, OPTIONAL)
        && l.weight.is_finite()
        && l.weight >= 0.0
        && l.weight <= 1e9
        && l.revision >= 0
        && l.line_points.len() <= 128
        && l.line_points.iter().all(|p| coordinate(p.x) && coordinate(p.y))
}

/// A colour the map takes (`#RRGGBB`).
fn color(value: Option<&str>) -> bool {
    value.is_none_or(crate::settings::is_color)
}

pub fn valid_label(l: &MapLabel) -> bool {
    text(Some(&l.id), 256, REQUIRED)
        && text(l.area.as_deref(), 512, OPTIONAL)
        && coordinate(l.x)
        && coordinate(l.y)
        && coordinate(l.z)
        && l.width.is_finite()
        && l.width > 0.0
        && l.width <= 10_000.0
        && l.height.is_finite()
        && l.height > 0.0
        && l.height <= 10_000.0
        && text(Some(&l.text), 4_000, MULTILINE)
        && l.font_size.is_finite()
        && (4.0..=400.0).contains(&l.font_size)
        && color(l.color.as_deref())
        && color(l.background.as_deref())
        && l.image.as_deref().is_none_or(images::is_hash)
        && (l.image.is_some() || !l.text.trim().is_empty())
        && l.opacity.is_finite()
        && (0.05..=1.0).contains(&l.opacity)
        && l.revision >= 0
}

/// A picture as a map holds it: a known kind within the size limit and a well-formed hash (the
/// hash itself is checked when a file is read, [`deserialize`]).
pub fn valid_image(i: &MapImage) -> bool {
    !i.data.is_empty()
        && i.data.len() <= MAX_IMAGE_BYTES
        && images::ImageKind::of(&i.data).is_some()
        && images::is_hash(&i.hash)
}

/// Labels and pictures: limits, unique ids and hashes, every picture a label names present,
/// the pictures' total size.
fn validate_labels(map: &MapSnapshot) -> bool {
    if map.labels.len() > MAX_LABELS
        || map.deleted_labels.len() > 100_000
        || map.images.len() > MAX_LABELS
        || !map.labels.iter().all(valid_label)
        || !map.images.iter().all(valid_image)
    {
        return false;
    }
    let hashes: HashSet<&str> = map.images.iter().map(|i| i.hash.as_str()).collect();
    let ids: HashSet<&str> = map.labels.iter().map(|l| l.id.as_str()).collect();
    hashes.len() == map.images.len()
        && ids.len() == map.labels.len()
        && map.images.iter().map(|i| i.data.len()).sum::<usize>() <= MAX_IMAGES_BYTES
        && map
            .labels
            .iter()
            .all(|l| l.image.as_deref().is_none_or(|h| hashes.contains(h)))
        && map
            .deleted_labels
            .iter()
            .all(|d| text(Some(&d.id), 256, REQUIRED) && d.revision > 0)
}

/// Check a whole map: limits, every room and exit, unique ids, references.
pub fn validate(map: &MapSnapshot) -> Result<(), FormatError> {
    if !validate_labels(map) {
        return Err(bad("Invalid map labels or pictures, or limits exceeded."));
    }
    if map.rooms.len() > MAX_ROOMS
        || map.links.len() > MAX_LINKS
        || map.area_settings.len() > MAX_ROOMS
        || map.room_aliases.len() > 100_000
        || map.deleted_rooms.len() > 100_000
        || map.deleted_links.len() > 100_000
        || map.candidate_room_ids.len() > 256
        || map.observation_count < 0
        || !map.rooms.iter().all(valid_room)
        || !map.links.iter().all(valid_link)
    {
        return Err(bad("Invalid map data or limits exceeded."));
    }
    let ids: HashSet<&str> = map.rooms.iter().map(|r| r.id.as_str()).collect();
    let link_keys: HashSet<(&str, &str)> = map
        .links
        .iter()
        .map(|l| (l.from_id.as_str(), l.direction.as_str()))
        .collect();
    let alias_sources: HashSet<&str> = map.room_aliases.iter().map(|a| a.source_id.as_str()).collect();
    let areas: HashSet<&str> = map.area_settings.iter().map(|a| a.area.as_str()).collect();
    let ok = ids.len() == map.rooms.len()
        && link_keys.len() == map.links.len()
        && map.links.iter().all(|l| ids.contains(l.from_id.as_str()))
        && map.candidate_room_ids.iter().all(|id| ids.contains(id.as_str()))
        && map.current_room_id.as_deref().is_none_or(|id| ids.contains(id))
        && map.room_aliases.iter().all(|a| {
            text(Some(&a.source_id), 256, REQUIRED) && text(Some(&a.target_id), 256, REQUIRED) && a.revision >= 0
        })
        && alias_sources.len() == map.room_aliases.len()
        && map
            .area_settings
            .iter()
            .all(|a| text(Some(&a.area), 512, PLAIN) && a.revision >= 0)
        && areas.len() == map.area_settings.len()
        && map
            .deleted_rooms
            .iter()
            .all(|d| text(Some(&d.id), 256, REQUIRED) && d.revision > 0)
        && map
            .deleted_links
            .iter()
            .all(|d| text(Some(&d.from_id), 256, REQUIRED) && text(Some(&d.direction), 64, REQUIRED) && d.revision > 0);
    if ok {
        Ok(())
    } else {
        Err(bad("Invalid map references or metadata."))
    }
}

/// The map as a file: version 1 when it has no labels (the C# client reads it), else version 2.
/// Fails when it is not valid or too large.
pub fn serialize(map: &MapSnapshot) -> Result<String, FormatError> {
    validate(map)?;
    #[derive(serde::Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct Document<'a> {
        version: i32,
        map: &'a MapSnapshot,
    }
    let version = if map.labels.is_empty() && map.images.is_empty() && map.deleted_labels.is_empty() {
        1
    } else {
        VERSION as i32
    };
    let json = serde_json::to_string(&Document { version, map }).map_err(|e| FormatError(e.to_string()))?;
    let pictures: usize = map.images.iter().map(|i| i.data.len().div_ceil(3) * 4).sum();
    if json.len() > MAX_BYTES || json.len() - pictures.min(json.len()) > MAX_MAP_BYTES {
        return Err(bad("Map file exceeds the size limit."));
    }
    Ok(json)
}

/// Read a map file (version 1 or 2, or a bare map) and validate it, pictures' hashes included.
pub fn deserialize(json: &str) -> Result<MapSnapshot, FormatError> {
    if json.len() > MAX_BYTES {
        return Err(bad("Map file exceeds the size limit."));
    }
    let root: serde_json::Value = serde_json::from_str(json).map_err(|_| bad("Invalid map file."))?;
    if depth(&root) > 32 {
        return Err(bad("Invalid map file."));
    }
    let serde_json::Value::Object(object) = &root else {
        return Err(bad("Expected a map object."));
    };
    let data = match object.get("Version") {
        Some(version) => {
            if !matches!(version.as_i64(), Some(1..=VERSION)) {
                return Err(bad("Unsupported map version."));
            }
            object
                .get("Map")
                .ok_or_else(|| bad("Unsupported map version."))?
                .clone()
        }
        None => root.clone(),
    };
    let map: MapSnapshot = serde_json::from_value(data).map_err(|_| bad("Invalid map file."))?;
    validate(&map)?;
    if !map.images.iter().all(images::valid_image) {
        return Err(bad("Invalid map labels or pictures, or limits exceeded."));
    }
    Ok(map)
}

fn depth(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        serde_json::Value::Object(map) => 1 + map.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}
