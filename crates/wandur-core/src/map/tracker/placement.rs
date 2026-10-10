//! Where new rooms go, and docking (the C# `RoomMapTracker.Placement`): a room with no known
//! route is parked just beside the map, near the current area; when a move or the server's exit
//! data later connects a misplaced part of the map, that part slides into place as a unit so the
//! new exit is one step long.

use std::collections::{HashSet, VecDeque};

use super::RoomMapTracker;
use crate::map::model::{MapLink, step};

impl RoomMapTracker {
    /// The spot for a new room: one step from `origin` in `direction`, else beside the map near
    /// `area`; nudged east until no room holds it.
    pub(super) fn position(
        &self,
        origin: Option<&str>,
        direction: Option<&str>,
        area: Option<&str>,
    ) -> (f64, f64, f64) {
        let position = match origin.and_then(|o| self.rooms.get(o)) {
            Some(room) => {
                let (dx, dy, dz) = step(direction);
                (room.x + dx, room.y + dy, room.z + dz)
            }
            None => self.unplaced(area),
        };
        self.free(position, None)
    }

    /// A room with no known route goes just beside the map, near the current area, until a
    /// move connects it.
    fn unplaced(&self, area: Option<&str>) -> (f64, f64, f64) {
        if self.rooms.is_empty() {
            return (0.0, 0.0, 0.0);
        }
        let key = area.unwrap_or("");
        let mut near: Vec<&crate::map::model::MapRoom> = self
            .rooms
            .values()
            .filter(|r| r.area.as_deref().unwrap_or("") == key)
            .collect();
        if near.is_empty() {
            near = self.rooms.values().collect();
        }
        let current = self
            .current
            .as_ref()
            .and_then(|c| self.rooms.get(c))
            .filter(|c| near.iter().any(|r| r.id == c.id));
        // The first room with the largest x, as the C# `MaxBy`.
        let widest = near
            .iter()
            .copied()
            .fold(None::<&crate::map::model::MapRoom>, |best, r| match best {
                Some(b) if b.x >= r.x => Some(b),
                _ => Some(r),
            })
            .expect("near is not empty");
        let reference = current.unwrap_or(widest);
        let max_x = near.iter().map(|r| r.x).fold(f64::NEG_INFINITY, f64::max);
        (max_x.floor() + 2.0, reference.y, reference.z)
    }

    /// Display overlap is not proof that two rooms are the same: move east until the spot is free
    /// of every room but `own`.
    fn free(&self, mut position: (f64, f64, f64), own: Option<&str>) -> (f64, f64, f64) {
        while self
            .rooms
            .values()
            .any(|r| Some(r.id.as_str()) != own && r.x == position.0 && r.y == position.1 && r.z == position.2)
        {
            position.0 += 0.35;
        }
        position
    }

    /// Evidence that `from_id` leads `direction` to `to_id` pulls a misplaced part of the map
    /// into place. A part is the rooms joined to one end by links that already fit the grid. It
    /// moves only when the other end is not inside it, when every compass link leaving it fits
    /// once it has moved (several long links from one parked island all fit after the same
    /// shift; links that would still not fit are real, non-grid geometry and stay as drawn), and
    /// when it holds no manual edit, lock, drawn exit line or server coordinates. If both ends
    /// could move, the smaller part does. Not an undoable edit.
    pub(super) fn dock(&mut self, from_id: &str, to_id: &str, direction: &str) {
        if self.server_coordinates || from_id == to_id {
            return;
        }
        let (Some(from), Some(to)) = (self.rooms.get(from_id), self.rooms.get(to_id)) else {
            return;
        };
        if !self
            .links
            .get(&(from_id.to_string(), direction.to_string()))
            .is_some_and(|l| l.to_id == to_id)
        {
            return;
        }
        let (ox, oy, oz) = step(Some(direction));
        let dx = from.x + ox - to.x;
        let dy = from.y + oy - to.y;
        let dz = from.z + oz - to.z;
        if dx.abs() <= 1.0 && dy.abs() <= 1.0 && dz.abs() < 0.5 {
            return;
        }
        let to_side = self.movable(to_id, from_id, (dx, dy, dz));
        let from_side = self.movable(from_id, to_id, (-dx, -dy, -dz));
        match (to_side, from_side) {
            (Some(to_side), from_side) if from_side.as_ref().is_none_or(|f| to_side.len() <= f.len()) => {
                self.translate(&to_side, to_id, (dx, dy, dz));
            }
            (_, Some(from_side)) => self.translate(&from_side, from_id, (-dx, -dy, -dz)),
            _ => {}
        }
    }

    /// The part around `start` that can shift by `shift` to meet `other`, or `None` when it
    /// contains `other`, is anchored, or would leave a compass link that still does not fit.
    fn movable(&self, start: &str, other: &str, shift: (f64, f64, f64)) -> Option<HashSet<String>> {
        let mut part = HashSet::from([start.to_string()]);
        let mut queue = VecDeque::from([start.to_string()]);
        while let Some(id) = queue.pop_front() {
            for link in self.links.values() {
                let next = if link.from_id == id {
                    &link.to_id
                } else if link.to_id == id {
                    &link.from_id
                } else {
                    continue;
                };
                if !self.rooms.contains_key(next)
                    || !compass(&link.direction)
                    || !self.fits(link, (0.0, 0.0, 0.0), None)
                {
                    continue;
                }
                if next == other {
                    return None;
                }
                if part.insert(next.clone()) {
                    queue.push_back(next.clone());
                }
            }
        }
        if self.anchored(&part) {
            return None;
        }
        for link in self.links.values() {
            if !compass(&link.direction)
                || !self.rooms.contains_key(&link.from_id)
                || !self.rooms.contains_key(&link.to_id)
                || part.contains(&link.from_id) == part.contains(&link.to_id)
            {
                continue;
            }
            if !self.fits(link, shift, Some(&part)) {
                return None;
            }
        }
        Some(part)
    }

    /// Whether a link matches its direction on the grid, with the rooms in `moved` shifted.
    fn fits(&self, link: &MapLink, shift: (f64, f64, f64), moved: Option<&HashSet<String>>) -> bool {
        let (Some(a), Some(b)) = (self.rooms.get(&link.from_id), self.rooms.get(&link.to_id)) else {
            return false;
        };
        let at = |id: &str, x: f64, y: f64, z: f64| {
            if moved.is_some_and(|m| m.contains(id)) {
                (x + shift.0, y + shift.1, z + shift.2)
            } else {
                (x, y, z)
            }
        };
        let (ax, ay, az) = at(&a.id, a.x, a.y, a.z);
        let (bx, by, bz) = at(&b.id, b.x, b.y, b.z);
        let (ox, oy, oz) = step(Some(&link.direction));
        (ax + ox - bx).abs() <= 1.0 && (ay + oy - by).abs() <= 1.0 && (az + oz - bz).abs() < 0.5
    }

    fn anchored(&self, part: &HashSet<String>) -> bool {
        part.iter()
            .any(|id| self.rooms.get(id).is_some_and(|r| r.is_manually_edited || r.is_locked))
            || self
                .links
                .values()
                .any(|l| !l.line_points.is_empty() && (part.contains(&l.from_id) || part.contains(&l.to_id)))
    }

    fn translate(&mut self, part: &HashSet<String>, anchor: &str, shift: (f64, f64, f64)) {
        for id in part {
            if let Some(room) = self.rooms.get_mut(id) {
                room.x += shift.0;
                room.y += shift.1;
                room.z += shift.2;
            }
        }
        // The joining room first, so it takes the exact spot when that is free.
        let mut order: Vec<&String> = part.iter().collect();
        order.sort_by_key(|id| id.as_str() != anchor);
        for id in order {
            let Some(room) = self.rooms.get(id.as_str()) else {
                continue;
            };
            let spot = self.free((room.x, room.y, room.z), Some(id));
            // A new revision so stores keep the new position over the saved one.
            let revision = self.next_revision();
            if let Some(room) = self.rooms.get_mut(id.as_str()) {
                (room.x, room.y, room.z) = spot;
                room.revision = revision;
            }
        }
    }
}

fn compass(direction: &str) -> bool {
    matches!(
        direction,
        "north" | "south" | "east" | "west" | "northeast" | "northwest" | "southeast" | "southwest" | "up" | "down"
    )
}
