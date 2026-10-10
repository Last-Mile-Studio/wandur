//! The map editor's operations, built only from the tracker's editing calls
//! ([`RoomMapTracker::upsert_room`], [`RoomMapTracker::remove_room`],
//! [`RoomMapTracker::edit_link`], [`RoomMapTracker::remove_link`]) inside
//! [`RoomMapTracker::edit_group`], so every one keeps the same checks, revisions, tombstones,
//! manual-edit marks (which protect a room from automatic docking) and saving, and each is one
//! undo step however many rooms or exits it touches.

use super::model::{MapImage, MapLabel, MapLink, MapRoom, opposite};
use super::tracker::RoomMapTracker;

/// The eight compass directions, counterclockwise from east (the angle's order).
const COMPASS: [&str; 8] = [
    "east",
    "northeast",
    "north",
    "northwest",
    "west",
    "southwest",
    "south",
    "southeast",
];

/// The direction from one position to another (x east, y north, z the floor): up or down when
/// the floors differ, else the nearest of the eight compass directions. `None` for the same
/// spot.
pub fn infer_direction(from: (f64, f64, f64), to: (f64, f64, f64)) -> Option<&'static str> {
    let (dx, dy, dz) = (to.0 - from.0, to.1 - from.1, to.2 - from.2);
    if !(dx.is_finite() && dy.is_finite() && dz.is_finite()) {
        return None;
    }
    if dz.abs() > 1e-9 {
        return Some(if dz > 0.0 { "up" } else { "down" });
    }
    if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
        return None;
    }
    let degrees = dy.atan2(dx).to_degrees().rem_euclid(360.0);
    let sector = ((degrees + 22.5) / 45.0).floor() as usize % 8;
    Some(COMPASS[sector])
}

/// A room's position.
pub fn position(room: &MapRoom) -> (f64, f64, f64) {
    (room.x, room.y, room.z)
}

/// The direction of the way back for an exit `direction` from `from` to `to`: the opposite
/// compass or vertical direction, else the direction the positions give.
pub fn return_direction(direction: &str, from: &MapRoom, to: &MapRoom) -> Option<String> {
    opposite(direction)
        .or_else(|| infer_direction(position(to), position(from)))
        .map(str::to_string)
}

impl RoomMapTracker {
    /// Exits from `link`'s destination that lead back to its origin (the way back).
    pub fn return_links(&self, from: &str, direction: &str) -> Vec<&MapLink> {
        let Some(link) = self.link(from, direction) else {
            return Vec::new();
        };
        self.links()
            .filter(|l| l.from_id == link.to_id && l.to_id == link.from_id && l.from_id != l.to_id)
            .collect()
    }

    /// Connect two rooms by hand as one undoable edit: an exit from `from` to `to` by
    /// `direction`, and with `two_way` the way back (the opposite direction). An exit already
    /// leaving by that direction is replaced.
    pub fn connect_rooms(&mut self, from: &str, to: &str, direction: &str, two_way: bool) -> bool {
        if from == to {
            return false;
        }
        let (Some(a), Some(b)) = (self.room(from), self.room(to)) else {
            return false;
        };
        let back = if two_way {
            match return_direction(direction, a, b) {
                Some(d) => Some(MapLink::new(to, from, &d, true)),
                None => return false,
            }
        } else {
            None
        };
        self.edit_link(MapLink::new(from, to, direction, true), None, back)
    }

    /// Move rooms to new positions as one undoable edit. Returns how many moved.
    pub fn place_rooms(&mut self, moves: &[(String, f64, f64, f64)]) -> usize {
        self.edit_group(|t| {
            let mut moved = 0;
            for (id, x, y, z) in moves {
                let Some(room) = t.room(id) else { continue };
                if (room.x, room.y, room.z) == (*x, *y, *z) {
                    continue;
                }
                let placed = MapRoom {
                    x: *x,
                    y: *y,
                    z: *z,
                    ..room.clone()
                };
                moved += usize::from(t.upsert_room(placed));
            }
            moved
        })
    }

    /// Move rooms by an offset as one undoable edit. Returns how many moved.
    pub fn move_rooms(&mut self, ids: &[String], dx: f64, dy: f64, dz: f64) -> usize {
        let moves: Vec<(String, f64, f64, f64)> = ids
            .iter()
            .filter_map(|id| self.room(id))
            .map(|r| (r.id.clone(), r.x + dx, r.y + dy, r.z + dz))
            .collect();
        self.place_rooms(&moves)
    }

    /// Change rooms by hand as one undoable edit (`change` is applied to a copy of each; rooms
    /// it leaves as they were are not touched). Returns how many changed; a change the checks
    /// refuse (an empty name, a bad colour) changes nothing for that room.
    pub fn edit_rooms(&mut self, ids: &[String], change: impl Fn(&mut MapRoom)) -> usize {
        self.edit_group(|t| {
            let mut changed = 0;
            for id in ids {
                let Some(room) = t.room(id) else { continue };
                let mut edited = room.clone();
                change(&mut edited);
                if edited != *room && t.upsert_room(edited) {
                    changed += 1;
                }
            }
            changed
        })
    }

    /// Delete rooms (with their exits, leaving tombstones) as one undoable edit.
    pub fn remove_rooms(&mut self, ids: &[String]) -> usize {
        self.edit_group(|t| ids.iter().filter(|id| t.remove_room(id)).count())
    }

    /// Delete exits as one undoable edit.
    pub fn remove_links(&mut self, keys: &[(String, String)]) -> usize {
        self.edit_group(|t| keys.iter().filter(|(f, d)| t.remove_link(f, d)).count())
    }

    /// Change an exit by hand as one undoable edit. A new direction renames it (the old one
    /// gets a tombstone).
    pub fn edit_exit(&mut self, from: &str, direction: &str, change: impl FnOnce(&mut MapLink)) -> bool {
        let Some(link) = self.link(from, direction) else {
            return false;
        };
        let mut edited = link.clone();
        change(&mut edited);
        edited.from_id = from.to_string();
        if edited == *link {
            return true;
        }
        if edited.direction.trim().is_empty() {
            return false;
        }
        let previous = (edited.direction != direction).then_some(direction);
        if previous.is_some() && self.link(from, &edited.direction).is_some() {
            // Another exit already leaves this way.
            return false;
        }
        self.edit_link(edited, previous, None)
    }

    /// Change a label by hand as one undoable edit (`change` is applied to a copy). A change
    /// the checks refuse changes nothing.
    pub fn edit_label(&mut self, id: &str, change: impl FnOnce(&mut MapLabel)) -> bool {
        let Some(label) = self.label(id) else {
            return false;
        };
        let mut edited = label.clone();
        change(&mut edited);
        edited.id = id.to_string();
        if edited == *label {
            return true;
        }
        self.upsert_label(edited)
    }

    /// Add a picture label, or give a label a picture, as one undoable edit.
    pub fn place_picture_label(&mut self, label: MapLabel, image: MapImage) -> bool {
        if label.image.as_deref() != Some(image.hash.as_str()) {
            return false;
        }
        self.edit_group(|t| t.add_image(image) && t.upsert_label(label))
    }

    /// Make an exit one-way (its way back is removed) or two-way (a way back is added when there
    /// is none) as one undoable edit.
    pub fn set_two_way(&mut self, from: &str, direction: &str, two_way: bool) -> bool {
        let Some(link) = self.link(from, direction).cloned() else {
            return false;
        };
        let back: Vec<(String, String)> = self
            .return_links(from, direction)
            .iter()
            .map(|l| (l.from_id.clone(), l.direction.clone()))
            .collect();
        if two_way == !back.is_empty() {
            return true;
        }
        if !two_way {
            return self.remove_links(&back) > 0;
        }
        let (Some(a), Some(b)) = (self.room(&link.from_id), self.room(&link.to_id)) else {
            return false;
        };
        let Some(d) = return_direction(&link.direction, a, b) else {
            return false;
        };
        let reverse = MapLink {
            door_state: link.door_state,
            ..MapLink::new(&link.to_id, &link.from_id, &d, true)
        };
        self.upsert_link(reverse)
    }
}

#[cfg(test)]
mod tests;
