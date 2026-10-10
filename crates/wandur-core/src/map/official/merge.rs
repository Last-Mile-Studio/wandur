//! Bringing a game's official map into the world's map, as one undoable step.
//!
//! The first time (no earlier file) it is a plain import, merged as File > Import map merges a
//! Mudlet map ([`RoomMapTracker::import_map`]). After that, a new version is merged three ways:
//! the base is the file imported last time, theirs is the new file, ours is the world's map now.
//! Rooms, exits and labels are compared one by one:
//!
//! - unchanged here since the base: the new version is taken (or, gone from the new file, it is
//!   removed);
//! - changed here (moved, renamed, recoloured, an exit's door or destination, a label's text):
//!   the person's version stays, even when the new file changed or dropped it;
//! - deleted here: it stays deleted;
//! - new in the file: added. Something the map already has under the same id that the base did
//!   not have (a room the tracker learned by walking) takes the file's version, as a plain
//!   import would.
//!
//! What the tracker records by itself (the server's words for a room, its known exits, a
//! terrain guess, an exit seen from both ends) is not an edit, so it never holds a room back.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::map::format::FormatError;
use crate::map::model::{MapLabel, MapLink, MapRoom, MapSnapshot};
use crate::map::tracker::RoomMapTracker;

/// What a merge did, counted over rooms, exits and labels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MergeReport {
    pub added: usize,
    pub updated: usize,
    /// Changed here, and kept although the new file changed or dropped them.
    pub kept_local: usize,
    pub removed: usize,
}

impl MergeReport {
    pub fn changed(&self) -> bool {
        self.added + self.updated + self.removed > 0
    }
}

/// A room as the file gives it: what the tracker learns by itself left out. The description
/// counts only when the base has one (a file without descriptions keeps the observed ones).
fn room_content(room: &MapRoom, description: bool) -> MapRoom {
    MapRoom {
        description: if description {
            room.description.clone()
        } else {
            String::new()
        },
        provisional: false,
        observed_name: None,
        observed_description: None,
        observed_area: None,
        known_exits: Vec::new(),
        inferred_environment: None,
        inferred_confidence: None,
        inferred_key: None,
        is_manually_edited: false,
        revision: 0,
        ..room.clone()
    }
}

fn same_room(a: &MapRoom, b: &MapRoom, base: Option<&MapRoom>) -> bool {
    let description = base.is_some_and(|r| !r.description.is_empty());
    room_content(a, description) == room_content(b, description)
}

fn same_link(a: &MapLink, b: &MapLink) -> bool {
    let plain = |l: &MapLink| MapLink {
        confirmed: false,
        is_manually_edited: false,
        revision: 0,
        ..l.clone()
    };
    plain(a) == plain(b)
}

fn same_label(a: &MapLabel, b: &MapLabel) -> bool {
    let plain = |l: &MapLabel| MapLabel {
        is_manually_edited: false,
        revision: 0,
        ..l.clone()
    };
    plain(a) == plain(b)
}

/// What one kind of item does in a three-way merge.
enum Step {
    Take,
    Remove,
    Keep,
}

/// One item: `base`, `theirs` and `ours` (each may be missing) to what to do, counting it.
fn decide<T>(
    base: Option<&T>,
    theirs: Option<&T>,
    ours: Option<&T>,
    same: impl Fn(&T, &T) -> bool,
    report: &mut MergeReport,
) -> Step {
    match (base, theirs, ours) {
        (Some(b), Some(t), Some(o)) => {
            if same(o, b) {
                if same(o, t) {
                    Step::Keep
                } else {
                    report.updated += 1;
                    Step::Take
                }
            } else {
                if !same(o, t) {
                    report.kept_local += 1;
                }
                Step::Keep
            }
        }
        // Deleted here: stays deleted.
        (Some(b), Some(t), None) => {
            if !same(t, b) {
                report.kept_local += 1;
            }
            Step::Keep
        }
        (Some(b), None, Some(o)) => {
            if same(o, b) {
                report.removed += 1;
                Step::Remove
            } else {
                report.kept_local += 1;
                Step::Keep
            }
        }
        (None, Some(t), Some(o)) => {
            if same(o, t) {
                Step::Keep
            } else {
                report.updated += 1;
                Step::Take
            }
        }
        (None, Some(_), None) => {
            report.added += 1;
            Step::Take
        }
        (_, None, _) => Step::Keep,
    }
}

/// Bring `theirs` into the tracker over `base` (the file imported last time, if any), as one
/// undoable step. Fails, changing nothing, when the result would pass the map's limits.
pub fn merge(
    tracker: &mut RoomMapTracker,
    base: Option<&MapSnapshot>,
    theirs: &MapSnapshot,
) -> Result<MergeReport, FormatError> {
    let Some(base) = base else {
        return import(tracker, theirs);
    };
    let mut report = MergeReport::default();
    // ---- rooms ----
    let mut take_rooms: Vec<MapRoom> = Vec::new();
    let mut remove_rooms: Vec<String> = Vec::new();
    for t in &theirs.rooms {
        let b = base.room(&t.id);
        let same = |x: &MapRoom, y: &MapRoom| same_room(x, y, b);
        if let Step::Take = decide(b, Some(t), tracker.room(&t.id), same, &mut report) {
            take_rooms.push(t.clone());
        }
    }
    let their_rooms: HashSet<&str> = theirs.rooms.iter().map(|r| r.id.as_str()).collect();
    for b in base.rooms.iter().filter(|r| !their_rooms.contains(r.id.as_str())) {
        let same = |x: &MapRoom, y: &MapRoom| same_room(x, y, Some(b));
        if let Step::Remove = decide(Some(b), None, tracker.room(&b.id), same, &mut report) {
            remove_rooms.push(b.id.clone());
        }
    }
    // Which rooms there will be, for the exits.
    let removed: HashSet<&str> = remove_rooms.iter().map(String::as_str).collect();
    let taken: HashSet<&str> = take_rooms.iter().map(|r| r.id.as_str()).collect();
    let will_exist = |id: &str| !removed.contains(id) && (taken.contains(id) || tracker.room(id).is_some());
    // ---- exits ----
    let mut take_links: Vec<MapLink> = Vec::new();
    let mut remove_links: Vec<(String, String)> = Vec::new();
    let link_in = |map: &MapSnapshot, from: &str, direction: &str| {
        map.links
            .iter()
            .find(|l| l.from_id == from && l.direction == direction)
            .cloned()
    };
    for t in &theirs.links {
        let b = link_in(base, &t.from_id, &t.direction);
        let o = tracker.link(&t.from_id, &t.direction).cloned();
        let mut counted = MergeReport::default();
        if let Step::Take = decide(b.as_ref(), Some(t), o.as_ref(), same_link, &mut counted) {
            // An exit of a room that is gone (deleted here) or to one stays out.
            if !will_exist(&t.from_id) || !will_exist(&t.to_id) {
                continue;
            }
            take_links.push(t.clone());
        }
        report.added += counted.added;
        report.updated += counted.updated;
        report.kept_local += counted.kept_local;
    }
    for b in &base.links {
        if link_in(theirs, &b.from_id, &b.direction).is_some() || removed.contains(b.from_id.as_str()) {
            continue;
        }
        let o = tracker.link(&b.from_id, &b.direction).cloned();
        if let Step::Remove = decide(Some(b), None, o.as_ref(), same_link, &mut report) {
            remove_links.push((b.from_id.clone(), b.direction.clone()));
        }
    }
    // ---- labels ----
    let mut take_labels: Vec<MapLabel> = Vec::new();
    let mut remove_labels: Vec<String> = Vec::new();
    let label_in = |map: &MapSnapshot, id: &str| map.labels.iter().find(|l| l.id == id).cloned();
    for t in &theirs.labels {
        let b = label_in(base, &t.id);
        if let Step::Take = decide(b.as_ref(), Some(t), tracker.label(&t.id), same_label, &mut report) {
            take_labels.push(t.clone());
        }
    }
    for b in &base.labels {
        if label_in(theirs, &b.id).is_some() {
            continue;
        }
        if let Step::Remove = decide(Some(b), None, tracker.label(&b.id), same_label, &mut report) {
            remove_labels.push(b.id.clone());
        }
    }
    // Grid mode for areas the map has no setting for yet.
    let areas: Vec<_> = theirs
        .area_settings
        .iter()
        .filter(|a| tracker.area_settings().all(|o| o.area != a.area))
        .cloned()
        .collect();
    // ---- apply, as one undo step ----
    tracker.edit_group(|t| -> Result<(), FormatError> {
        if !take_rooms.is_empty() || !areas.is_empty() {
            let rooms = MapSnapshot {
                area_settings: areas,
                ..MapSnapshot::of(take_rooms, Vec::new())
            };
            t.import_map(&rooms)?;
        }
        for link in take_links {
            t.upsert_link(link);
        }
        for label in take_labels {
            if let Some(hash) = &label.image
                && let Some(image) = theirs.images.iter().find(|i| &i.hash == hash)
            {
                t.add_image(image.clone());
            }
            t.upsert_label(label);
        }
        for (from, direction) in &remove_links {
            t.remove_link(from, direction);
        }
        for id in &remove_labels {
            t.remove_label(id);
        }
        for id in &remove_rooms {
            t.remove_room(id);
        }
        Ok(())
    })?;
    Ok(report)
}

/// The first import: everything in the file is added or replaces the map's item of the same
/// id (as File > Import map).
fn import(tracker: &mut RoomMapTracker, theirs: &MapSnapshot) -> Result<MergeReport, FormatError> {
    let mut report = MergeReport::default();
    for r in &theirs.rooms {
        match tracker.room(&r.id) {
            None => report.added += 1,
            Some(o) if !same_room(o, r, None) => report.updated += 1,
            Some(_) => {}
        }
    }
    for l in &theirs.links {
        match tracker.link(&l.from_id, &l.direction) {
            None => report.added += 1,
            Some(o) if !same_link(o, l) => report.updated += 1,
            Some(_) => {}
        }
    }
    for l in &theirs.labels {
        match tracker.label(&l.id) {
            None => report.added += 1,
            Some(o) if !same_label(o, l) => report.updated += 1,
            Some(_) => {}
        }
    }
    tracker.import_map(theirs)?;
    Ok(report)
}

#[cfg(test)]
mod tests;
