//! Revision-aware merge of two saves of one world's map (the C# `MapSnapshotMerge`): for each
//! room, exit, label, area and alias the newer revision wins (a manual edit wins a tie);
//! tombstones keep the newest deletion, and a deleted room, exit or label stays deleted unless it
//! was changed after. Pictures are kept when a merged label shows them.

use std::collections::HashSet;

use indexmap::IndexMap;

use super::format::{MAX_LINKS, MAX_ROOMS};
use super::images::{MAX_IMAGES_BYTES, MAX_LABELS};
use super::model::*;

/// Merge `snapshot` (newer) onto `previous` (stored).
pub fn combine(previous: &MapSnapshot, snapshot: &MapSnapshot) -> MapSnapshot {
    let mut room_deletes: IndexMap<String, i64> = IndexMap::new();
    for d in previous.deleted_rooms.iter().chain(&snapshot.deleted_rooms) {
        let entry = room_deletes.entry(d.id.clone()).or_insert(d.revision);
        if d.revision > *entry {
            *entry = d.revision;
        }
    }
    let mut link_deletes: IndexMap<(String, String), i64> = IndexMap::new();
    for d in previous.deleted_links.iter().chain(&snapshot.deleted_links) {
        let entry = link_deletes
            .entry((d.from_id.clone(), d.direction.clone()))
            .or_insert(d.revision);
        if d.revision > *entry {
            *entry = d.revision;
        }
    }
    // Later entries win ties, so the newer save's copy wins when nothing else decides.
    let newer = |rev: i64, manual: bool, best: (i64, bool)| (rev, manual) >= best;
    let mut rooms: IndexMap<&str, &MapRoom> = IndexMap::new();
    for r in previous.rooms.iter().chain(&snapshot.rooms) {
        match rooms.get(r.id.as_str()) {
            Some(best)
                if !newer(
                    r.revision,
                    r.is_manually_edited,
                    (best.revision, best.is_manually_edited),
                ) => {}
            _ => {
                rooms.insert(&r.id, r);
            }
        }
    }
    let mut rooms: Vec<MapRoom> = rooms
        .into_values()
        .filter(|r| room_deletes.get(&r.id).is_none_or(|d| r.revision > *d))
        .cloned()
        .collect();
    if rooms.len() > MAX_ROOMS {
        rooms.drain(..rooms.len() - MAX_ROOMS);
    }
    let ids: HashSet<&str> = rooms.iter().map(|r| r.id.as_str()).collect();
    let mut links: IndexMap<(&str, &str), &MapLink> = IndexMap::new();
    for l in previous.links.iter().chain(&snapshot.links) {
        let key = (l.from_id.as_str(), l.direction.as_str());
        match links.get(&key) {
            Some(best)
                if !newer(
                    l.revision,
                    l.is_manually_edited,
                    (best.revision, best.is_manually_edited),
                ) => {}
            _ => {
                links.insert(key, l);
            }
        }
    }
    let mut links: Vec<MapLink> = links
        .into_values()
        .filter(|l| {
            ids.contains(l.from_id.as_str())
                && (!room_deletes.contains_key(&l.to_id) || ids.contains(l.to_id.as_str()))
                && link_deletes
                    .get(&(l.from_id.clone(), l.direction.clone()))
                    .is_none_or(|d| l.revision > *d)
        })
        .cloned()
        .collect();
    if links.len() > MAX_LINKS {
        links.drain(..links.len() - MAX_LINKS);
    }
    let mut label_deletes: IndexMap<String, i64> = IndexMap::new();
    for d in previous.deleted_labels.iter().chain(&snapshot.deleted_labels) {
        let entry = label_deletes.entry(d.id.clone()).or_insert(d.revision);
        if d.revision > *entry {
            *entry = d.revision;
        }
    }
    let mut labels: IndexMap<&str, &MapLabel> = IndexMap::new();
    for l in previous.labels.iter().chain(&snapshot.labels) {
        match labels.get(l.id.as_str()) {
            Some(best)
                if !newer(
                    l.revision,
                    l.is_manually_edited,
                    (best.revision, best.is_manually_edited),
                ) => {}
            _ => {
                labels.insert(&l.id, l);
            }
        }
    }
    let mut labels: Vec<MapLabel> = labels
        .into_values()
        .filter(|l| label_deletes.get(&l.id).is_none_or(|d| l.revision > *d))
        .cloned()
        .collect();
    if labels.len() > MAX_LABELS {
        labels.drain(..labels.len() - MAX_LABELS);
    }
    // Pictures the merged labels show (either save may hold one); labels whose picture is
    // missing, or past the pictures' limit (oldest first), are dropped.
    let mut images: IndexMap<&str, &MapImage> = IndexMap::new();
    let mut bytes = 0;
    labels.retain(|l| {
        let Some(hash) = l.image.as_deref() else {
            return true;
        };
        if images.contains_key(hash) {
            return true;
        }
        let Some(image) = snapshot.images.iter().chain(&previous.images).find(|i| i.hash == hash) else {
            return false;
        };
        if bytes + image.data.len() > MAX_IMAGES_BYTES {
            return false;
        }
        bytes += image.data.len();
        images.insert(&image.hash, image);
        true
    });
    let mut areas: IndexMap<&str, &MapAreaSettings> = IndexMap::new();
    for a in previous.area_settings.iter().chain(&snapshot.area_settings) {
        if areas
            .get(a.area.as_str())
            .is_none_or(|best| a.revision >= best.revision)
        {
            areas.insert(&a.area, a);
        }
    }
    let mut aliases: IndexMap<&str, &MapRoomAlias> = IndexMap::new();
    for a in previous.room_aliases.iter().chain(&snapshot.room_aliases) {
        if aliases
            .get(a.source_id.as_str())
            .is_none_or(|best| a.revision >= best.revision)
        {
            aliases.insert(&a.source_id, a);
        }
    }
    MapSnapshot {
        rooms,
        links,
        area_settings: areas.into_values().cloned().collect(),
        room_aliases: aliases.into_values().cloned().collect(),
        deleted_rooms: room_deletes
            .into_iter()
            .map(|(id, revision)| MapRoomDeletion { id, revision })
            .collect(),
        deleted_links: link_deletes
            .into_iter()
            .map(|((from_id, direction), revision)| MapLinkDeletion {
                from_id,
                direction,
                revision,
            })
            .collect(),
        labels,
        deleted_labels: label_deletes
            .into_iter()
            .map(|(id, revision)| MapLabelDeletion { id, revision })
            .collect(),
        images: images.into_values().cloned().collect(),
        ..snapshot.clone()
    }
}
