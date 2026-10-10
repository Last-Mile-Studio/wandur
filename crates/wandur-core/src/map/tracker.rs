//! The room tracker (the C# `RoomMapTracker`): directed room topology learned from observations,
//! independent of its illustrative coordinates.
//!
//! - A server room id (`s:<id>`) is authoritative: the room is placed, its exits with
//!   destination ids become confirmed links, and a move from the previous room in a walked
//!   direction is a confirmed link. A room change without a walked direction (a teleport) moves
//!   the player but invents no link.
//! - Without ids, rooms are recognized by name, description, area and exits. Several matching
//!   rooms are candidate paths, advanced together by later moves until one remains; a new link
//!   needs a distinctive sequence first. Nothing matching makes a provisional room (`t:<id>`).
//! - A provisional room that later shows up with a server id is upgraded in place when exactly
//!   one compatible room has the same complete exit set.
//! - Manual edits win over later observations; deletions leave tombstones so an older save
//!   cannot bring them back; the last 50 edits can be undone.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use super::format::{self, OPTIONAL, REQUIRED, coordinate, text};
use super::model::*;

mod placement;
#[cfg(test)]
mod tests;

/// Most rooms a tracker holds.
pub const MAX_ROOMS: usize = format::MAX_ROOMS;
/// Most exits a tracker holds.
pub const MAX_LINKS: usize = format::MAX_LINKS;
/// Most labels a tracker holds.
pub const MAX_LABELS: usize = super::images::MAX_LABELS;
/// Manual edits kept for undo.
pub const UNDO_LIMIT: usize = 50;

type LinkKey = (String, String);

#[derive(Clone, Debug, Default)]
pub struct RoomMapTracker {
    rooms: IndexMap<String, MapRoom>,
    links: IndexMap<LinkKey, MapLink>,
    aliases: IndexMap<String, MapRoomAlias>,
    areas: IndexMap<String, MapAreaSettings>,
    deleted_rooms: IndexMap<String, i64>,
    deleted_links: IndexMap<LinkKey, i64>,
    labels: IndexMap<String, MapLabel>,
    deleted_labels: IndexMap<String, i64>,
    /// Pictures by hash: those the labels show (and, briefly, one just added for a label).
    images: IndexMap<String, MapImage>,
    /// Each step: the map before, the map after, and the step's number ([`Self::last_edit`]).
    undo: Vec<(MapSnapshot, MapSnapshot, u64)>,
    redo: Vec<(MapSnapshot, MapSnapshot, u64)>,
    /// The number the last recorded undo step got (counts up from 1).
    edit_serial: u64,
    revision: i64,
    paths: Vec<Vec<String>>,
    current: Option<String>,
    last_traversal: Option<(String, String, String)>,
    pending_origin: Option<String>,
    pending_direction: Option<String>,
    first_fingerprint: Option<String>,
    distinct_observation: bool,
    steps: u32,
    observations: i32,
    source: RoomSource,
    state: TrackingState,
    /// Goes up on every change (the C# `Changed` event): views and the walker compare it.
    version: u64,
    /// Set once the world reports room coordinates: its layout is then the server's, never ours
    /// to shift (docking stays off).
    server_coordinates: bool,
    /// Edit groups open ([`RoomMapTracker::edit_group`]); their edits are one undo step.
    group_depth: u32,
    /// The map before the outermost open group.
    group_before: Option<MapSnapshot>,
    /// An edit was made inside the open group.
    group_changed: bool,
}

/// .NET ticks (100 ns since 0001-01-01) now, the unit of C# map revisions, so maps merge the
/// same in both clients.
pub fn now_ticks() -> i64 {
    const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    UNIX_EPOCH_TICKS + (since.as_nanos() / 100) as i64
}

fn normalize(value: &str) -> String {
    let lower = value.trim().to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut space = false;
    for c in lower.chars() {
        if c.is_whitespace() {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
    }
    out
}

/// A title without LOTJ's trailing room flags (`[Hotel]`, `[Hospital]`, `[Engine]`), which its
/// text adds and its `Room.Info` leaves out. Other bracketed names stay: they may tell rooms or
/// instances apart.
fn normalize_title(value: &str) -> String {
    let mut s = value.trim_end();
    loop {
        let lower = s.to_ascii_lowercase();
        let stripped = ["[hotel]", "[hospital]", "[engine]"]
            .iter()
            .find(|flag| lower.ends_with(*flag))
            .map(|flag| s[..s.len() - flag.len()].trim_end());
        match stripped {
            Some(rest) => s = rest,
            None => break,
        }
    }
    normalize(s)
}

fn fingerprint(room: &RoomObservation) -> String {
    format!("{}|{}", normalize_title(&room.name), normalize(&room.description))
}

fn exit_names(observation: &RoomObservation) -> Vec<String> {
    observation
        .exits
        .keys()
        .map(|e| normalize_direction(e).map_or_else(|| e.clone(), str::to_string))
        .collect()
}

/// Whether `room` may be the room `observation` describes.
fn matches(room: &MapRoom, observation: &RoomObservation) -> bool {
    let name = room.observed_name.as_deref().unwrap_or(&room.name);
    let description = room.observed_description.as_deref().unwrap_or(&room.description);
    let area = if room.observed_name.is_some() {
        room.observed_area.as_deref()
    } else {
        room.area.as_deref()
    };
    if normalize_title(name) != normalize_title(&observation.name) {
        return false;
    }
    // Missing exits may be hidden or closed; wholly contradictory exit sets reject the match.
    let exits = exit_names(observation);
    if !room.known_exits.is_empty() && !exits.is_empty() && !room.known_exits.iter().any(|e| exits.contains(e)) {
        return false;
    }
    if let (Some(a), Some(b)) = (area, observation.area.as_deref())
        && !a.trim().is_empty()
        && !b.trim().is_empty()
        && normalize(a) != normalize(b)
    {
        return false;
    }
    if description.trim().is_empty() || observation.description.trim().is_empty() {
        return true;
    }
    similar_descriptions(description, &observation.description)
}

/// Whether two descriptions are the same text but for small wording changes (a weather or
/// time-of-day line): equal once normalized, or sharing 80% of their longer words.
pub(crate) fn similar_descriptions(first: &str, second: &str) -> bool {
    let first = normalize(first);
    let second = normalize(second);
    if first == second {
        return true;
    }
    // Small wording changes are evidence, not a new identity.
    let words = |s: &str| -> HashSet<String> {
        s.split(' ')
            .filter(|w| w.encode_utf16().count() > 2)
            .map(str::to_string)
            .collect()
    };
    let (a, b) = (words(&first), words(&second));
    let union = a.union(&b).count();
    !a.is_empty() && a.intersection(&b).count() as f64 / union as f64 >= 0.8
}

/// Whether two descriptions are the same text, ignoring case and spacing.
pub(crate) fn same_description(first: &str, second: &str) -> bool {
    normalize(first) == normalize(second)
}

fn valid_observation(observation: &RoomObservation) -> bool {
    text(Some(&observation.name), 512, REQUIRED) && text(Some(&observation.description), 16_000, format::MULTILINE)
}

impl RoomMapTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// A tracker over a saved map. The player's position is not assumed.
    pub fn from_snapshot(saved: MapSnapshot) -> Self {
        let mut t = Self::default();
        for alias in saved.room_aliases {
            t.aliases.insert(alias.source_id.clone(), alias);
        }
        for area in saved.area_settings {
            t.areas.insert(area.area.clone(), area);
        }
        for d in saved.deleted_rooms {
            t.deleted_rooms.insert(d.id, d.revision);
        }
        for d in saved.deleted_links {
            t.deleted_links.insert((d.from_id, d.direction), d.revision);
        }
        for d in saved.deleted_labels {
            t.deleted_labels.insert(d.id, d.revision);
        }
        for image in saved.images {
            t.images.insert(image.hash.clone(), image);
        }
        let revisions = saved
            .rooms
            .iter()
            .map(|r| r.revision)
            .chain(saved.links.iter().map(|l| l.revision))
            .chain(t.aliases.values().map(|a| a.revision))
            .chain(t.areas.values().map(|a| a.revision))
            .chain(t.deleted_rooms.values().copied())
            .chain(t.deleted_links.values().copied())
            .chain(saved.labels.iter().map(|l| l.revision))
            .chain(t.deleted_labels.values().copied());
        t.revision = revisions.max().unwrap_or(0);
        for room in saved.rooms.into_iter().take(MAX_ROOMS) {
            t.rooms.entry(room.id.clone()).or_insert(room);
        }
        for link in saved.links.into_iter().take(MAX_LINKS) {
            if t.rooms.contains_key(&link.from_id) {
                t.links.insert((link.from_id.clone(), link.direction.clone()), link);
            }
        }
        for label in saved.labels.into_iter().take(MAX_LABELS) {
            t.labels.entry(label.id.clone()).or_insert(label);
        }
        t.prune_images();
        t.state = if t.rooms.is_empty() {
            TrackingState::Waiting
        } else {
            TrackingState::Unknown
        };
        t
    }

    // ---- reading -------------------------------------------------------------------------

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn rooms(&self) -> impl Iterator<Item = &MapRoom> {
        self.rooms.values()
    }

    pub fn room(&self, id: &str) -> Option<&MapRoom> {
        self.rooms.get(id)
    }

    /// The room at `index` in the tracker's order (indices change when rooms are removed;
    /// views key what they keep by [`Self::version`]).
    pub fn room_at(&self, index: usize) -> Option<&MapRoom> {
        self.rooms.get_index(index).map(|(_, r)| r)
    }

    pub fn room_index(&self, id: &str) -> Option<usize> {
        self.rooms.get_index_of(id)
    }

    /// The exit at `index` in the tracker's order.
    pub fn link_at(&self, index: usize) -> Option<&MapLink> {
        self.links.get_index(index).map(|(_, l)| l)
    }

    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    pub fn links(&self) -> impl Iterator<Item = &MapLink> {
        self.links.values()
    }

    pub fn link(&self, from: &str, direction: &str) -> Option<&MapLink> {
        self.links.get(&(from.to_string(), direction.to_string()))
    }

    pub fn link_count(&self) -> usize {
        self.links.len()
    }

    pub fn labels(&self) -> impl Iterator<Item = &MapLabel> {
        self.labels.values()
    }

    pub fn label(&self, id: &str) -> Option<&MapLabel> {
        self.labels.get(id)
    }

    pub fn label_count(&self) -> usize {
        self.labels.len()
    }

    /// A picture the map holds, by hash.
    pub fn image(&self, hash: &str) -> Option<&MapImage> {
        self.images.get(hash)
    }

    /// The bytes of all the pictures the labels show.
    pub fn image_bytes(&self) -> usize {
        self.images.values().map(|i| i.data.len()).sum()
    }

    pub fn area_settings(&self) -> impl Iterator<Item = &MapAreaSettings> {
        self.areas.values()
    }

    pub fn grid_mode(&self, area: &str) -> bool {
        self.areas.get(area).is_some_and(|a| a.grid_mode)
    }

    pub fn current_id(&self) -> Option<&str> {
        self.current.as_deref()
    }

    pub fn current(&self) -> Option<&MapRoom> {
        self.current.as_ref().and_then(|id| self.rooms.get(id))
    }

    pub fn state(&self) -> TrackingState {
        self.state
    }

    pub fn source(&self) -> RoomSource {
        self.source
    }

    pub fn observation_count(&self) -> i32 {
        self.observations
    }

    /// Rooms the player may be in while the position is ambiguous.
    pub fn candidates(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.paths
            .iter()
            .filter_map(|p| p.last())
            .filter(|id| seen.insert(id.as_str()))
            .cloned()
            .collect()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The number of the edit Undo would take back now (each recorded edit gets the next
    /// number), or `None` when there is nothing to undo. The undo toast checks it is still its
    /// own edit before undoing.
    pub fn last_edit(&self) -> Option<u64> {
        self.undo.last().map(|e| e.2)
    }

    /// Everything, as a map value.
    pub fn snapshot(&self) -> MapSnapshot {
        MapSnapshot {
            rooms: self.rooms.values().cloned().collect(),
            links: self.links.values().cloned().collect(),
            candidate_room_ids: self.candidates(),
            current_room_id: self.current.clone(),
            state: self.state,
            source: self.source,
            observation_count: self.observations,
            room_aliases: self.aliases.values().cloned().collect(),
            area_settings: self.areas.values().cloned().collect(),
            deleted_rooms: self
                .deleted_rooms
                .iter()
                .map(|(id, r)| MapRoomDeletion {
                    id: id.clone(),
                    revision: *r,
                })
                .collect(),
            deleted_links: self
                .deleted_links
                .iter()
                .map(|((from, direction), r)| MapLinkDeletion {
                    from_id: from.clone(),
                    direction: direction.clone(),
                    revision: *r,
                })
                .collect(),
            labels: self.labels.values().cloned().collect(),
            deleted_labels: self
                .deleted_labels
                .iter()
                .map(|(id, r)| MapLabelDeletion {
                    id: id.clone(),
                    revision: *r,
                })
                .collect(),
            images: self.used_images().cloned().collect(),
        }
    }

    /// The pictures the labels show, each once, in the labels' order.
    fn used_images(&self) -> impl Iterator<Item = &MapImage> {
        let mut seen = HashSet::new();
        self.labels
            .values()
            .filter_map(|l| l.image.as_deref())
            .filter(move |h| seen.insert(*h))
            .filter_map(|h| self.images.get(h))
    }

    /// Forget pictures no label shows.
    fn prune_images(&mut self) {
        let used: HashSet<&str> = self.labels.values().filter_map(|l| l.image.as_deref()).collect();
        if used.len() == self.images.len() {
            return;
        }
        let keep: HashSet<String> = used.into_iter().map(str::to_string).collect();
        self.images.retain(|h, _| keep.contains(h));
    }

    /// The pictures' bytes once `label` replaces the label with its id.
    fn image_bytes_with(&self, label: &MapLabel) -> usize {
        let mut used: HashSet<&str> = self
            .labels
            .values()
            .filter(|l| l.id != label.id)
            .filter_map(|l| l.image.as_deref())
            .collect();
        used.extend(label.image.as_deref());
        used.iter()
            .filter_map(|h| self.images.get(*h))
            .map(|i| i.data.len())
            .sum()
    }

    /// A fingerprint of everything a walk relies on (the C# `SameWalkGraph`): each room's id,
    /// server id, provisional and locked flags, weight and revision; each exit's ends,
    /// direction, evidence, command, lock, door, weight and revision. Order does not matter.
    pub fn walk_signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let hash = |f: &dyn Fn(&mut std::collections::hash_map::DefaultHasher)| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            f(&mut h);
            h.finish()
        };
        let mut total = (self.rooms.len() as u64).wrapping_mul(1_000_003) ^ self.links.len() as u64;
        for r in self.rooms.values() {
            total = total.wrapping_add(hash(&|h| {
                (
                    &r.id,
                    &r.server_id,
                    r.provisional,
                    r.is_locked,
                    r.weight.to_bits(),
                    r.revision,
                )
                    .hash(h)
            }));
        }
        for l in self.links.values() {
            total = total.wrapping_add(hash(&|h| {
                (
                    &l.from_id,
                    &l.to_id,
                    &l.direction,
                    l.confirmed,
                    &l.command,
                    l.is_locked,
                    l.door_state,
                    l.weight.to_bits(),
                    l.revision,
                )
                    .hash(h)
            }));
        }
        total
    }

    fn changed(&mut self) {
        self.version += 1;
    }

    fn next_revision(&mut self) -> i64 {
        self.revision = now_ticks().max(self.revision + 1);
        self.revision
    }

    // ---- tracking ------------------------------------------------------------------------

    /// Forget where the player is (the map stays).
    pub fn lose_position(&mut self) {
        self.last_traversal = None;
        self.current = None;
        self.clear_candidates();
        self.state = if self.rooms.is_empty() {
            TrackingState::Waiting
        } else {
            TrackingState::Unknown
        };
        self.changed();
    }

    /// A move failed ("You can't go that way"): nothing moves.
    pub fn movement_failed(&mut self) {
        self.changed();
    }

    /// Replace a just-created text guess with structured data from the same command's answer
    /// (no id): rename it, or relocalize against a saved room that matches.
    pub fn refine_provisional_room(&mut self, id: &str, observation_count: i32, observation: &RoomObservation) -> bool {
        let Some(room) = self.rooms.get(id) else {
            return false;
        };
        if self.current.as_deref() != Some(id)
            || self.observations != observation_count
            || observation.source == RoomSource::Text
            || observation.server_id.is_some()
            || !room.provisional
            || room.is_manually_edited
            || room.server_id.is_some()
            || !valid_observation(observation)
        {
            return false;
        }
        if self.rooms.values().any(|c| c.id != id && matches(c, observation)) {
            // Relocalize against the saved graph rather than rename the guess into a duplicate.
            // Its short-lived incoming exit goes with it; the tombstone keeps an older saved
            // copy of the guess from coming back.
            let traversal = self.last_traversal.clone().filter(|t| t.1 == id);
            self.rooms.shift_remove(id);
            self.links.retain(|_, l| l.from_id != id && l.to_id != id);
            let revision = self.next_revision();
            self.deleted_rooms.insert(id.to_string(), revision);
            self.clear_candidates();
            self.current = traversal.as_ref().map(|t| t.0.clone());
            self.observe(observation, traversal.as_ref().map(|t| t.2.as_str()));
            return true;
        }
        // Observe normally once the guessed title and description are no longer evidence; the
        // coordinates and the witnessed move stay with this room.
        let room = self.rooms.get_mut(id).expect("checked above");
        room.name = observation.name.clone();
        room.description = observation.description.clone();
        room.area = None;
        room.known_exits.clear();
        self.observe(observation, None);
        true
    }

    /// Give a room the description read from the text around its protocol room (whose data
    /// had none). An observation, not a manual edit, and not undoable; a room edited by hand is
    /// left alone. The room's revision goes up, so the terrain classifier looks at it again and
    /// a merge with an older copy keeps the description.
    pub fn set_text_description(&mut self, id: &str, description: &str) -> bool {
        if !text(Some(description), 16_000, format::MULTILINE) || description.trim().is_empty() {
            return false;
        }
        match self.rooms.get(id) {
            Some(room) if !room.is_manually_edited && room.description != description => {}
            _ => return false,
        }
        let revision = self.next_revision();
        let room = self.rooms.get_mut(id).expect("checked above");
        room.description = description.to_string();
        room.revision = revision;
        self.changed();
        true
    }

    /// Take an observation. `movement` is the direction the player just walked, if any.
    pub fn observe(&mut self, observation: &RoomObservation, movement: Option<&str>) {
        if !valid_observation(observation) {
            return;
        }
        let mut observation = observation.clone();
        if !text(observation.server_id.as_deref(), 128, OPTIONAL) {
            observation.server_id = None;
        }
        if !text(observation.area.as_deref(), 512, OPTIONAL) {
            observation.area = None;
        }
        if !text(observation.environment.as_deref(), 128, OPTIONAL) {
            observation.environment = None;
        }
        if !text(observation.symbol.as_deref(), 32, OPTIONAL) {
            observation.symbol = None;
        }
        observation.exits = observation
            .exits
            .into_iter()
            .filter(|(k, _)| text(Some(k), 64, REQUIRED))
            .take(64)
            .map(|(k, v)| {
                let v = v.filter(|v| text(Some(v), 128, OPTIONAL));
                (k, v)
            })
            .collect();
        if let Some(server) = &observation.server_id {
            let id = self.resolve_alias(&format!("s:{server}"));
            if self.deleted_rooms.contains_key(&id) && !self.rooms.contains_key(&id) {
                self.lose_position();
                return;
            }
        }
        let direction = movement.and_then(normalize_direction);
        if observation.x.is_some() || observation.y.is_some() || observation.z.is_some() {
            self.server_coordinates = true;
        }
        self.observations += 1;
        self.source = observation.source;
        if observation
            .server_id
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty() && s != "-1" && s != "0")
        {
            self.observe_identified(&observation, direction);
            self.changed();
            return;
        }

        if !self.paths.is_empty() {
            // Advance every plausible path, not only the first matching room.
            let next: Vec<Vec<String>> = match direction {
                None => self
                    .paths
                    .iter()
                    .filter(|p| matches(&self.rooms[p.last().unwrap()], &observation))
                    .cloned()
                    .collect(),
                Some(direction) => {
                    let mut next = Vec::new();
                    'paths: for p in &self.paths {
                        let last = p.last().unwrap();
                        for l in self.links.values() {
                            if l.from_id == *last
                                && l.direction == direction
                                && let Some(to) = self.rooms.get(&l.to_id)
                                && matches(to, &observation)
                            {
                                let mut path = p.clone();
                                path.push(l.to_id.clone());
                                next.push(path);
                                if next.len() > 256 {
                                    break 'paths;
                                }
                            }
                        }
                    }
                    next
                }
            };
            if next.len() > 256 {
                self.lose_position();
                return;
            }
            if !next.is_empty() {
                self.paths = next;
                if direction.is_some() {
                    self.steps += 1;
                }
                self.distinct_observation |= Some(fingerprint(&observation)) != self.first_fingerprint;
                self.resolve_candidates();
                self.changed();
                return;
            }
            // Contradictory evidence ends the old hypotheses; no guessed exit survives.
            self.clear_candidates();
            self.current = None;
        }

        if let Some(current) = self.current.clone() {
            if direction.is_none() && matches(&self.rooms[&current], &observation) {
                self.update_room(&current, &observation);
                self.changed();
                return;
            }
            if let Some(direction) = direction
                && let Some(link) = self.links.get(&(current.clone(), direction.to_string()))
                && let Some(target) = self.rooms.get(&link.to_id)
                && matches(target, &observation)
            {
                let target = target.id.clone();
                // Walking a saved link also repairs a room left parked away from it.
                self.dock(&current, &target, direction);
                self.last_traversal = Some((current, target.clone(), direction.to_string()));
                self.current = Some(target.clone());
                self.update_room(&target, &observation);
                self.state = TrackingState::Inferred;
                self.changed();
                return;
            }
        }

        let origin = if direction.is_some() {
            self.current.clone()
        } else {
            None
        };
        let mut candidates = Vec::new();
        for r in self.rooms.values() {
            if Some(&r.id) != origin.as_ref() && matches(r, &observation) {
                candidates.push(r.id.clone());
                if candidates.len() > 256 {
                    break;
                }
            }
        }
        if candidates.len() > 256 {
            self.lose_position();
            return;
        }
        // A witnessed out-and-back between distinctive rooms is itself a sequence. A reverse
        // exit is never made up just because the outward one exists.
        if candidates.len() == 1
            && let (Some(origin), Some(direction)) = (&origin, direction)
            && let Some((from, to, walked)) = &self.last_traversal
            && to == origin
            && *from == candidates[0]
            && opposite(walked) == Some(direction)
            && !matches(&self.rooms[origin], &observation)
        {
            let target = candidates[0].clone();
            self.current = Some(target.clone());
            self.add_link(origin, &target, direction, false);
            self.last_traversal = Some((origin.clone(), target.clone(), direction.to_string()));
            self.update_room(&target, &observation);
            self.state = TrackingState::Inferred;
            self.changed();
            return;
        }
        self.last_traversal = None;
        if !candidates.is_empty() {
            self.paths = candidates.into_iter().map(|id| vec![id]).collect();
            self.pending_origin = origin;
            self.pending_direction = direction.map(str::to_string);
            self.first_fingerprint = Some(fingerprint(&observation));
            self.steps = 0;
            self.distinct_observation = false;
            self.current = None;
            self.resolve_candidates();
        } else {
            if self.rooms.len() >= MAX_ROOMS {
                self.lose_position();
                return;
            }
            let id = format!("t:{}", uuid::Uuid::new_v4().simple());
            let (x, y, z) = self.position(origin.as_deref(), direction, observation.area.as_deref());
            let mut room = MapRoom::new(
                &id,
                &observation.name,
                &observation.description,
                observation.area.as_deref(),
                x,
                y,
                z,
                true,
            );
            room.known_exits = exit_names(&observation);
            self.rooms.insert(id.clone(), room);
            self.update_room(&id, &observation);
            if let (Some(origin), Some(direction)) = (&origin, direction) {
                self.add_link(origin, &id, direction, false);
                self.last_traversal = Some((origin.clone(), id.clone(), direction.to_string()));
            }
            self.current = Some(id);
            self.state = TrackingState::Inferred;
        }
        self.changed();
    }

    fn resolve_candidates(&mut self) {
        let locations = self.candidates();
        // A new exit needs a distinctive sequence; plain relocalization can settle sooner.
        if locations.len() == 1 && (self.pending_origin.is_none() || (self.steps >= 2 && self.distinct_observation)) {
            let firsts: HashSet<&String> = self.paths.iter().filter_map(|p| p.first()).collect();
            if let (Some(origin), Some(direction)) = (self.pending_origin.clone(), self.pending_direction.clone())
                && firsts.len() == 1
            {
                let to = self.paths[0][0].clone();
                self.add_link(&origin, &to, &direction, false);
            }
            self.current = Some(locations[0].clone());
            self.clear_candidates();
            self.state = TrackingState::Inferred;
        } else {
            self.state = TrackingState::Ambiguous;
        }
    }

    fn observe_identified(&mut self, observation: &RoomObservation, direction: Option<&str>) {
        let server = observation.server_id.clone().unwrap_or_default();
        let id = self.resolve_alias(&format!("s:{server}"));
        self.reconcile_provisional_identity(&id, observation, direction);
        let previous = self.current.clone();
        // A move the server's own exit data contradicts was a transport (a shuttle, a recall, a
        // script, death): the previous room's confirmed exit that way leads somewhere else. It
        // is not an exit, so it adds no link, overwrites no real exit and places nothing beside
        // the previous room. A missing exit proves nothing (doors can be hidden or closed), so
        // only a contradicting link counts.
        let direction = match (&previous, direction) {
            (Some(previous), Some(walked))
                if *previous != id
                    && self
                        .links
                        .get(&(previous.clone(), walked.to_string()))
                        .is_some_and(|l| l.confirmed && l.to_id != id) =>
            {
                None
            }
            _ => direction,
        };
        if !self.rooms.contains_key(&id) {
            if self.rooms.len() >= MAX_ROOMS {
                self.lose_position();
                return;
            }
            let incoming = self
                .links
                .values()
                .find(|l| l.to_id == id && self.rooms.contains_key(&l.from_id))
                .map(|l| (l.from_id.clone(), l.direction.clone()));
            // An unsolicited room change is evidence of where the player is, not of an exit
            // from the previous room.
            let (x, y, z) = match &incoming {
                Some((from, dir)) => self.position(Some(from), Some(dir), observation.area.as_deref()),
                None => self.position(
                    if direction.is_some() { previous.as_deref() } else { None },
                    direction,
                    observation.area.as_deref(),
                ),
            };
            let room = MapRoom::new(
                &id,
                &observation.name,
                &observation.description,
                observation.area.as_deref(),
                x,
                y,
                z,
                false,
            )
            .with_server_id(&server);
            self.rooms.insert(id.clone(), room);
        }
        self.update_room(&id, observation);
        if let (Some(previous), Some(direction)) = (&previous, direction)
            && *previous != id
        {
            self.add_link(previous, &id, direction, true);
        }
        let exits: Vec<(String, Option<String>)> = observation
            .exits
            .iter()
            .take(32)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (exit, to) in exits {
            let name = normalize_direction(&exit).map_or_else(|| exit.trim().to_lowercase(), str::to_string);
            if let Some(to) = to.filter(|t| !t.trim().is_empty() && t != "-1" && t != "0") {
                let to = self.resolve_alias(&format!("s:{to}"));
                self.add_link(&id, &to, &name, true);
            }
        }
        self.current = Some(id);
        self.clear_candidates();
        self.state = TrackingState::Confirmed;
    }

    fn reconcile_provisional_identity(&mut self, id: &str, observation: &RoomObservation, direction: Option<&str>) {
        // A room id the server starts sending must not leave a second, disconnected copy of an
        // old text room: upgrade a unique compatible room with the same complete exit set.
        // Ambiguous corridors stay separate.
        let exits: HashSet<String> = exit_names(observation).into_iter().collect();
        if exits.is_empty() {
            return;
        }
        let found: Vec<&MapRoom> = self
            .rooms
            .values()
            .filter(|r| {
                r.id != id && matches(r, observation) && r.known_exits.iter().cloned().collect::<HashSet<_>>() == exits
            })
            .take(2)
            .collect();
        if found.len() != 1 || !found[0].provisional || found[0].server_id.is_some() {
            return;
        }
        let legacy = found[0].clone();
        if direction.is_some() && self.current.as_deref() == Some(legacy.id.as_str()) {
            return;
        }
        // Labels, coordinates, locks and notes stay. Protocol identity is evidence, not a
        // manual edit, so the upgrade is not an undo step.
        if self.rooms.get(id).is_some_and(|r| r.is_manually_edited) {
            return;
        }
        let revision = self.next_revision();
        let upgraded = MapRoom {
            id: id.to_string(),
            server_id: observation.server_id.clone(),
            provisional: false,
            revision,
            ..legacy.clone()
        };
        self.rooms.insert(id.to_string(), upgraded);
        self.merge_rooms_inner(&legacy.id, id, false);
    }

    fn update_room(&mut self, id: &str, observation: &RoomObservation) {
        let exits = exit_names(observation);
        let Some(room) = self.rooms.get_mut(id) else {
            return;
        };
        if room.is_manually_edited {
            room.observed_name = Some(observation.name.clone());
            if !observation.description.trim().is_empty() {
                room.observed_description = Some(observation.description.clone());
            }
            if observation.area.is_some() {
                room.observed_area = observation.area.clone();
            }
            room.known_exits = exits;
            return;
        }
        room.name = observation.name.clone();
        if !observation.description.trim().is_empty() {
            room.description = observation.description.clone();
        }
        if observation.area.is_some() {
            room.area = observation.area.clone();
        }
        if observation.environment.is_some() {
            room.environment = observation.environment.clone();
        }
        if observation.symbol.is_some() {
            room.symbol = observation.symbol.clone();
        }
        if let Some(x) = observation.x.filter(|v| coordinate(*v)) {
            room.x = x;
        }
        if let Some(y) = observation.y.filter(|v| coordinate(*v)) {
            room.y = y;
        }
        if let Some(z) = observation.z.filter(|v| coordinate(*v)) {
            room.z = z;
        }
        room.known_exits = exits;
        room.provisional = observation.server_id.is_none() && room.provisional;
    }

    fn add_link(&mut self, from: &str, to: &str, direction: &str, confirmed: bool) {
        let key = (from.to_string(), direction.to_string());
        let existing = self.links.get(&key);
        if let Some(deleted) = self.deleted_links.get(&key)
            && existing.is_none_or(|l| l.revision <= *deleted)
        {
            return;
        }
        if self.deleted_rooms.contains_key(to) && !self.rooms.contains_key(to) {
            return;
        }
        if let Some(edited) = existing.filter(|l| l.is_manually_edited) {
            // Seeing this exact exit strengthens its evidence; the person's destination,
            // command, locks and drawing stay.
            if confirmed && !edited.confirmed && edited.to_id == to {
                let revision = self.next_revision();
                let link = self.links.get_mut(&key).expect("present");
                link.confirmed = true;
                link.revision = revision;
            }
            self.dock(from, to, direction);
            return;
        }
        let revision = existing.map_or(0, |l| l.revision);
        let value = MapLink {
            revision,
            ..MapLink::new(from, to, direction, confirmed)
        };
        if self.links.contains_key(&key) || self.links.len() < MAX_LINKS {
            self.links.insert(key, value);
        }
        self.dock(from, to, direction);
    }

    fn clear_candidates(&mut self) {
        self.last_traversal = None;
        self.paths.clear();
        self.pending_origin = None;
        self.pending_direction = None;
        self.first_fingerprint = None;
        self.steps = 0;
        self.distinct_observation = false;
    }

    fn resolve_alias(&self, id: &str) -> String {
        let mut id = id.to_string();
        let mut seen = HashSet::new();
        while let Some(alias) = self.aliases.get(&id) {
            if alias.target_id == id || !seen.insert(id.clone()) {
                break;
            }
            id = alias.target_id.clone();
        }
        id
    }

    // ---- editing (the C# RoomMapTracker.Editing) -----------------------------------------

    /// Replace the whole map as one undoable edit (an import). The position is cleared.
    pub fn replace_map(&mut self, map: &MapSnapshot) -> bool {
        if format::validate(map).is_err() {
            return false;
        }
        let before = self.checkpoint();
        let incoming: HashSet<&str> = map.rooms.iter().map(|r| r.id.as_str()).collect();
        let gone: Vec<String> = self
            .rooms
            .keys()
            .filter(|id| !incoming.contains(id.as_str()))
            .cloned()
            .collect();
        for id in gone {
            self.delete_room(&id);
        }
        let incoming_links: HashSet<(&str, &str)> = map
            .links
            .iter()
            .map(|l| (l.from_id.as_str(), l.direction.as_str()))
            .collect();
        let gone: Vec<LinkKey> = self
            .links
            .keys()
            .filter(|(f, d)| !incoming_links.contains(&(f.as_str(), d.as_str())))
            .cloned()
            .collect();
        for key in gone {
            let revision = self.next_revision();
            self.deleted_links.insert(key, revision);
        }
        self.rooms.clear();
        for room in &map.rooms {
            let revision = self.next_revision();
            self.rooms.insert(
                room.id.clone(),
                MapRoom {
                    is_manually_edited: true,
                    revision,
                    ..room.clone()
                },
            );
        }
        self.links.clear();
        for link in &map.links {
            let revision = self.next_revision();
            self.links.insert(
                (link.from_id.clone(), link.direction.clone()),
                MapLink {
                    is_manually_edited: true,
                    revision,
                    ..link.clone()
                },
            );
        }
        // Areas no longer in the map go back to their defaults, newer than stale saves.
        let areas: Vec<String> = self.areas.keys().cloned().collect();
        for area in areas {
            let revision = self.next_revision();
            self.areas.insert(
                area.clone(),
                MapAreaSettings {
                    revision,
                    ..MapAreaSettings::new(&area, false)
                },
            );
        }
        for area in &map.area_settings {
            let revision = self.next_revision();
            self.areas.insert(
                area.area.clone(),
                MapAreaSettings {
                    revision,
                    ..area.clone()
                },
            );
        }
        let sources: Vec<String> = self.aliases.keys().cloned().collect();
        for source in sources {
            let revision = self.next_revision();
            self.aliases.insert(
                source.clone(),
                MapRoomAlias {
                    source_id: source.clone(),
                    target_id: source,
                    revision,
                },
            );
        }
        for alias in &map.room_aliases {
            let revision = self.next_revision();
            self.aliases.insert(
                alias.source_id.clone(),
                MapRoomAlias {
                    revision,
                    ..alias.clone()
                },
            );
        }
        let incoming_labels: HashSet<&str> = map.labels.iter().map(|l| l.id.as_str()).collect();
        let gone: Vec<String> = self
            .labels
            .keys()
            .filter(|id| !incoming_labels.contains(id.as_str()))
            .cloned()
            .collect();
        for id in gone {
            let revision = self.next_revision();
            self.deleted_labels.insert(id, revision);
        }
        self.labels.clear();
        for image in &map.images {
            self.images.insert(image.hash.clone(), image.clone());
        }
        for label in &map.labels {
            let revision = self.next_revision();
            self.labels.insert(
                label.id.clone(),
                MapLabel {
                    is_manually_edited: true,
                    revision,
                    ..label.clone()
                },
            );
        }
        self.prune_images();
        self.current = None;
        self.state = TrackingState::Unknown;
        self.record_edit(before);
        true
    }

    /// Add or change a room by hand. Its server-observed words are kept for recognition.
    pub fn upsert_room(&mut self, room: MapRoom) -> bool {
        if !format::valid_room(&room) || (!self.rooms.contains_key(&room.id) && self.rooms.len() >= MAX_ROOMS) {
            return false;
        }
        let before = self.checkpoint();
        let existing = self.rooms.get(&room.id).cloned();
        let revision = self.next_revision();
        let observed_name = room
            .observed_name
            .clone()
            .or_else(|| existing.as_ref().and_then(|e| e.observed_name.clone()))
            .or_else(|| existing.as_ref().map(|e| e.name.clone()));
        let observed_description = room
            .observed_description
            .clone()
            .or_else(|| existing.as_ref().and_then(|e| e.observed_description.clone()))
            .or_else(|| existing.as_ref().map(|e| e.description.clone()));
        let observed_area = if room.observed_name.is_some() {
            room.observed_area.clone()
        } else {
            match &existing {
                Some(e) if e.observed_name.is_some() => e.observed_area.clone(),
                Some(e) => e.area.clone(),
                None => None,
            }
        };
        let id = room.id.clone();
        self.rooms.insert(
            id,
            MapRoom {
                is_manually_edited: true,
                revision,
                observed_name,
                observed_description,
                observed_area,
                ..room
            },
        );
        self.record_edit(before);
        true
    }

    pub fn remove_room(&mut self, id: &str) -> bool {
        if !self.rooms.contains_key(id) {
            return false;
        }
        let before = self.checkpoint();
        self.delete_room(id);
        self.record_edit(before);
        true
    }

    /// Merge `source` into `target` by hand: its exits move to the target (the target's own
    /// exits win), and its id becomes an alias of the target.
    pub fn merge_rooms(&mut self, source: &str, target: &str) -> bool {
        self.merge_rooms_inner(source, target, true)
    }

    fn merge_rooms_inner(&mut self, source: &str, target: &str, manual: bool) -> bool {
        if source == target || !self.rooms.contains_key(source) || !self.rooms.contains_key(target) {
            return false;
        }
        let before = self.checkpoint();
        let was_current = self.current.as_deref() == Some(source);
        let affected: Vec<MapRoomAlias> = self
            .aliases
            .values()
            .filter(|a| a.target_id == source)
            .cloned()
            .collect();
        let rewired: Vec<MapLink> = self
            .links
            .values()
            .filter(|l| l.from_id == source || l.to_id == source)
            .map(|l| {
                let mut l = l.clone();
                if l.from_id == source {
                    l.from_id = target.to_string();
                }
                if l.to_id == source {
                    l.to_id = target.to_string();
                }
                l
            })
            .collect();
        self.delete_room(source);
        for link in rewired {
            if link.from_id == link.to_id {
                continue;
            }
            let key = (link.from_id.clone(), link.direction.clone());
            if let Some(existing) = self.links.get(&key).cloned() {
                // A manual merge keeps the target's exits; an automatic identity upgrade keeps
                // edited exits over automatically discovered duplicates.
                if !manual && link.is_manually_edited && !existing.is_manually_edited {
                    let revision = self.next_revision();
                    let confirmed = link.confirmed || (existing.to_id == link.to_id && existing.confirmed);
                    self.links.insert(
                        key,
                        MapLink {
                            confirmed,
                            revision,
                            ..link
                        },
                    );
                }
                continue;
            }
            let revision = self.next_revision();
            let edited = manual || link.is_manually_edited;
            self.links.insert(
                key,
                MapLink {
                    is_manually_edited: edited,
                    revision,
                    ..link
                },
            );
        }
        for alias in affected {
            let revision = self.next_revision();
            self.aliases.insert(
                alias.source_id.clone(),
                MapRoomAlias {
                    target_id: target.to_string(),
                    revision,
                    ..alias
                },
            );
        }
        let revision = self.next_revision();
        self.aliases.insert(
            source.to_string(),
            MapRoomAlias {
                source_id: source.to_string(),
                target_id: target.to_string(),
                revision,
            },
        );
        if was_current {
            self.current = Some(target.to_string());
        }
        if manual {
            self.record_edit(before);
        }
        true
    }

    /// Add or change an exit by hand.
    pub fn upsert_link(&mut self, link: MapLink) -> bool {
        if !format::valid_link(&link) || !self.rooms.contains_key(&link.from_id) {
            return false;
        }
        let key = (link.from_id.clone(), link.direction.clone());
        if !self.links.contains_key(&key) && self.links.len() >= MAX_LINKS {
            return false;
        }
        let before = self.checkpoint();
        let revision = self.next_revision();
        self.links.insert(
            key,
            MapLink {
                is_manually_edited: true,
                revision,
                ..link
            },
        );
        self.record_edit(before);
        true
    }

    /// Rename or change an exit, and optionally add its return exit, as one undoable edit.
    pub fn edit_link(&mut self, link: MapLink, previous_direction: Option<&str>, return_link: Option<MapLink>) -> bool {
        if !format::valid_link(&link)
            || !self.rooms.contains_key(&link.from_id)
            || return_link
                .as_ref()
                .is_some_and(|r| !format::valid_link(r) || !self.rooms.contains_key(&r.from_id))
        {
            return false;
        }
        let previous_key = (
            link.from_id.clone(),
            previous_direction.unwrap_or(&link.direction).to_string(),
        );
        let new_key = (link.from_id.clone(), link.direction.clone());
        let mut count = self.links.len();
        if self.links.contains_key(&previous_key) {
            count -= 1;
        }
        let adds =
            |k: &LinkKey, links: &IndexMap<LinkKey, MapLink>| usize::from(!links.contains_key(k) || *k == previous_key);
        count += adds(&new_key, &self.links);
        if let Some(r) = &return_link {
            let rk = (r.from_id.clone(), r.direction.clone());
            if rk != new_key {
                count += adds(&rk, &self.links);
            }
        }
        if count > MAX_LINKS {
            return false;
        }
        let before = self.checkpoint();
        if previous_key != new_key {
            self.links.shift_remove(&previous_key);
            let revision = self.next_revision();
            self.deleted_links.insert(previous_key, revision);
        }
        let revision = self.next_revision();
        self.links.insert(
            new_key,
            MapLink {
                is_manually_edited: true,
                revision,
                ..link
            },
        );
        if let Some(r) = return_link {
            let revision = self.next_revision();
            self.links.insert(
                (r.from_id.clone(), r.direction.clone()),
                MapLink {
                    is_manually_edited: true,
                    revision,
                    ..r
                },
            );
        }
        self.record_edit(before);
        true
    }

    pub fn remove_link(&mut self, from: &str, direction: &str) -> bool {
        let direction = normalize_direction(direction).map_or_else(|| direction.trim().to_lowercase(), str::to_string);
        let key = (from.to_string(), direction);
        if !self.links.contains_key(&key) {
            return false;
        }
        let before = self.checkpoint();
        self.links.shift_remove(&key);
        let revision = self.next_revision();
        self.deleted_links.insert(key, revision);
        self.record_edit(before);
        true
    }

    /// Hold a picture for a label about to show it ([`Self::upsert_label`]). Not an edit by
    /// itself: a picture no label shows is forgotten with the next edit.
    pub fn add_image(&mut self, image: MapImage) -> bool {
        if !format::valid_image(&image) {
            return false;
        }
        self.images.entry(image.hash.clone()).or_insert(image);
        true
    }

    /// Add or change a label by hand. A picture label's picture must be held
    /// ([`Self::add_image`]), and all the pictures together stay within the map's limit.
    pub fn upsert_label(&mut self, label: MapLabel) -> bool {
        if !format::valid_label(&label)
            || label.image.as_deref().is_some_and(|h| !self.images.contains_key(h))
            || (!self.labels.contains_key(&label.id) && self.labels.len() >= MAX_LABELS)
            || self.image_bytes_with(&label) > super::images::MAX_IMAGES_BYTES
        {
            return false;
        }
        let before = self.checkpoint();
        let revision = self.next_revision();
        self.labels.insert(
            label.id.clone(),
            MapLabel {
                is_manually_edited: true,
                revision,
                ..label
            },
        );
        self.record_edit(before);
        true
    }

    pub fn remove_label(&mut self, id: &str) -> bool {
        if !self.labels.contains_key(id) {
            return false;
        }
        let before = self.checkpoint();
        self.labels.shift_remove(id);
        let revision = self.next_revision();
        self.deleted_labels.insert(id.to_string(), revision);
        self.record_edit(before);
        true
    }

    /// Bring an imported map into this one as one undoable edit (File > Import map): its rooms,
    /// exits, labels and area settings are added, or replace those with the same ids; what the
    /// map already had otherwise stays, and the player's position is kept. An imported room
    /// keeps the server's words the tracker observed for it (and its description when the
    /// import has none), so recognition keeps working. Items already exactly as imported are
    /// left alone, so importing the same file again changes nothing. Returns how many items
    /// changed, or why nothing was done (the map's limits).
    pub fn import_map(&mut self, map: &MapSnapshot) -> Result<usize, format::FormatError> {
        format::validate(map)?;
        let rooms = self.rooms.len() + map.rooms.iter().filter(|r| !self.rooms.contains_key(&r.id)).count();
        let links = self.links.len()
            + map
                .links
                .iter()
                .filter(|l| !self.links.contains_key(&(l.from_id.clone(), l.direction.clone())))
                .count();
        let labels = self.labels.len() + map.labels.iter().filter(|l| !self.labels.contains_key(&l.id)).count();
        if rooms > MAX_ROOMS || links > MAX_LINKS || labels > MAX_LABELS {
            return Err(format::FormatError(crate::l10n::tf(
                crate::l10n::S::MapImportTooBig,
                &[&rooms, &MAX_ROOMS, &links, &MAX_LINKS],
            )));
        }
        let mut used: HashSet<&str> = self
            .labels
            .values()
            .filter(|l| !map.labels.iter().any(|m| m.id == l.id))
            .filter_map(|l| l.image.as_deref())
            .collect();
        used.extend(map.labels.iter().filter_map(|l| l.image.as_deref()));
        let bytes: usize = used
            .iter()
            .map(|h| {
                map.images
                    .iter()
                    .find(|i| i.hash == *h)
                    .or_else(|| self.images.get(*h))
                    .map_or(0, |i| i.data.len())
            })
            .sum();
        if bytes > super::images::MAX_IMAGES_BYTES {
            return Err(format::FormatError(
                crate::l10n::t(crate::l10n::S::MapImageTooLarge).to_string(),
            ));
        }
        let before = self.checkpoint();
        let mut changed = 0;
        let same_room = |a: &MapRoom, b: &MapRoom| {
            MapRoom {
                revision: 0,
                is_manually_edited: false,
                ..a.clone()
            } == MapRoom {
                revision: 0,
                is_manually_edited: false,
                ..b.clone()
            }
        };
        for room in &map.rooms {
            let mut merged = room.clone();
            if let Some(existing) = self.rooms.get(&room.id) {
                if merged.description.is_empty() {
                    merged.description = existing.description.clone();
                }
                merged.observed_name = existing
                    .observed_name
                    .clone()
                    .or_else(|| (!existing.is_manually_edited && !existing.provisional).then(|| existing.name.clone()));
                merged.observed_description = existing.observed_description.clone();
                merged.observed_area = existing.observed_area.clone();
                if merged.inferred_key.is_none() {
                    merged.inferred_environment = existing.inferred_environment.clone();
                    merged.inferred_confidence = existing.inferred_confidence;
                    merged.inferred_key = existing.inferred_key.clone();
                }
                for exit in &existing.known_exits {
                    if !merged.known_exits.contains(exit) && merged.known_exits.len() < 64 {
                        merged.known_exits.push(exit.clone());
                    }
                }
                if same_room(&merged, existing) {
                    continue;
                }
            }
            let revision = self.next_revision();
            self.rooms.insert(
                room.id.clone(),
                MapRoom {
                    is_manually_edited: true,
                    revision,
                    ..merged
                },
            );
            changed += 1;
        }
        for link in &map.links {
            let key = (link.from_id.clone(), link.direction.clone());
            let same = self.links.get(&key).is_some_and(|l| {
                MapLink {
                    revision: 0,
                    is_manually_edited: false,
                    ..l.clone()
                } == MapLink {
                    revision: 0,
                    is_manually_edited: false,
                    ..link.clone()
                }
            });
            if same {
                continue;
            }
            let revision = self.next_revision();
            self.links.insert(
                key,
                MapLink {
                    is_manually_edited: true,
                    revision,
                    ..link.clone()
                },
            );
            changed += 1;
        }
        for image in &map.images {
            self.images.entry(image.hash.clone()).or_insert_with(|| image.clone());
        }
        for label in &map.labels {
            let same = self.labels.get(&label.id).is_some_and(|l| {
                MapLabel {
                    revision: 0,
                    is_manually_edited: false,
                    ..l.clone()
                } == MapLabel {
                    revision: 0,
                    is_manually_edited: false,
                    ..label.clone()
                }
            });
            if same {
                continue;
            }
            let revision = self.next_revision();
            self.labels.insert(
                label.id.clone(),
                MapLabel {
                    is_manually_edited: true,
                    revision,
                    ..label.clone()
                },
            );
            changed += 1;
        }
        for area in &map.area_settings {
            if self
                .areas
                .get(&area.area)
                .is_some_and(|a| a.grid_mode == area.grid_mode)
                || (!self.areas.contains_key(&area.area) && !area.grid_mode)
            {
                continue;
            }
            let revision = self.next_revision();
            self.areas.insert(
                area.area.clone(),
                MapAreaSettings {
                    revision,
                    ..area.clone()
                },
            );
            changed += 1;
        }
        if changed > 0 {
            self.record_edit(before);
        } else {
            self.prune_images();
        }
        Ok(changed)
    }

    /// Grid mode for an area (an undoable edit).
    pub fn set_area_settings(&mut self, settings: MapAreaSettings) {
        if !text(Some(&settings.area), 512, format::PLAIN) {
            return;
        }
        if self
            .areas
            .get(&settings.area)
            .is_some_and(|a| a.grid_mode == settings.grid_mode)
        {
            return;
        }
        if !self.areas.contains_key(&settings.area) && self.areas.len() >= MAX_ROOMS {
            return;
        }
        let before = self.checkpoint();
        let revision = self.next_revision();
        self.areas
            .insert(settings.area.clone(), MapAreaSettings { revision, ..settings });
        self.record_edit(before);
    }

    /// Say where the player is, by hand: useful for navigation, but not server evidence.
    pub fn set_current_room(&mut self, id: &str) -> bool {
        if !self.rooms.contains_key(id) {
            return false;
        }
        self.current = Some(id.to_string());
        self.clear_candidates();
        self.state = TrackingState::Inferred;
        self.changed();
        true
    }

    // ---- terrain inference (the C# RoomMapTracker.Inference) ----------------------------

    /// Rooms without terrain of their own, the player's room first (UI thread: no hashing).
    pub fn inference_candidates(&self) -> impl Iterator<Item = &MapRoom> {
        let current = self.current();
        current
            .into_iter()
            .chain(
                self.rooms
                    .values()
                    .filter(move |r| Some(r.id.as_str()) != current.map(|c| c.id.as_str())),
            )
            .filter(|r| r.environment.as_deref().is_none_or(|e| e.trim().is_empty()))
    }

    /// Rooms without terrain whose inference is missing or stale for `model_version`, the
    /// player's room first. Hashes every candidate: worker threads and tests only.
    pub fn rooms_needing_inference(&self, model_version: &str) -> Vec<&MapRoom> {
        self.inference_candidates()
            .filter(|r| {
                r.inferred_key.as_deref()
                    != Some(crate::classify::preprocess::inference_key(&r.name, &r.description, model_version).as_str())
            })
            .collect()
    }

    /// Store a classifier result (the C# `ApplyInference`): ignored when the room is gone or
    /// `key` no longer matches its text. An abstention clears the guess and keeps the key, so it
    /// is not asked again. Not a manual edit and not undoable. Hashes: worker threads and tests.
    pub fn apply_inference(
        &mut self,
        id: &str,
        key: &str,
        model_version: &str,
        prediction: Option<&crate::classify::RoomEnvironmentPrediction>,
    ) -> bool {
        let Some(room) = self.rooms.get(id) else {
            return false;
        };
        if key != crate::classify::preprocess::inference_key(&room.name, &room.description, model_version) {
            return false;
        }
        self.store_inference(id, key, prediction);
        true
    }

    /// Store a worker's result when the room still has the text it was classified on (UI
    /// thread: compares text, no hashing).
    pub fn apply_inference_result(&mut self, result: &crate::classify::InferenceResult) -> bool {
        let Some(room) = self.rooms.get(&result.id) else {
            return false;
        };
        if room.name != result.name || room.description != result.description {
            return false;
        }
        if room.inferred_key.as_deref() == Some(result.key.as_str())
            && room.inferred_environment.as_deref() == result.prediction.as_ref().map(|p| p.environment.as_str())
            && room.inferred_confidence == result.prediction.as_ref().map(|p| p.confidence)
        {
            return false;
        }
        let key = result.key.clone();
        self.store_inference(&result.id, &key, result.prediction.as_ref());
        true
    }

    fn store_inference(
        &mut self,
        id: &str,
        key: &str,
        prediction: Option<&crate::classify::RoomEnvironmentPrediction>,
    ) {
        let revision = self.next_revision();
        if let Some(room) = self.rooms.get_mut(id) {
            room.inferred_environment = prediction.map(|p| p.environment.clone());
            room.inferred_confidence = prediction.map(|p| p.confidence);
            room.inferred_key = Some(key.to_string());
            room.revision = revision;
        }
        self.changed();
    }

    pub fn undo(&mut self) -> bool {
        let Some(edit) = self.undo.pop() else {
            return false;
        };
        self.apply_history(&edit.1, &edit.0);
        self.redo.push(edit);
        self.changed();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(edit) = self.redo.pop() else {
            return false;
        };
        self.apply_history(&edit.0, &edit.1);
        self.undo.push(edit);
        self.changed();
        true
    }

    fn delete_room(&mut self, id: &str) {
        let pointing: Vec<String> = self
            .aliases
            .values()
            .filter(|a| a.target_id == id)
            .map(|a| a.source_id.clone())
            .collect();
        for source in pointing {
            let revision = self.next_revision();
            self.aliases.insert(
                source.clone(),
                MapRoomAlias {
                    source_id: source.clone(),
                    target_id: source,
                    revision,
                },
            );
        }
        self.rooms.shift_remove(id);
        let revision = self.next_revision();
        self.deleted_rooms.insert(id.to_string(), revision);
        let gone: Vec<LinkKey> = self
            .links
            .iter()
            .filter(|(_, l)| l.from_id == id || l.to_id == id)
            .map(|(k, _)| k.clone())
            .collect();
        for key in gone {
            self.links.shift_remove(&key);
            let revision = self.next_revision();
            self.deleted_links.insert(key, revision);
        }
        if self.current.as_deref() == Some(id) {
            self.current = None;
            self.state = TrackingState::Unknown;
        }
        self.clear_candidates();
    }

    /// The map before an edit, for undo; inside a group the group's own copy is used instead.
    fn checkpoint(&self) -> MapSnapshot {
        if self.group_depth > 0 {
            MapSnapshot::default()
        } else {
            self.snapshot()
        }
    }

    /// Run several editing calls as one undoable edit (a multi-room move, a delete of a
    /// selection, an exit with its way back, a field set on several rooms). Every call keeps its
    /// own checks, revisions and tombstones; only the undo history sees one step. Groups nest
    /// (the outermost one records). Nothing is recorded when no call changed the map.
    pub fn edit_group<R>(&mut self, edits: impl FnOnce(&mut Self) -> R) -> R {
        if self.group_depth == 0 {
            self.group_before = Some(self.snapshot());
            self.group_changed = false;
        }
        self.group_depth += 1;
        let result = edits(self);
        self.group_depth -= 1;
        if self.group_depth == 0
            && let Some(before) = self.group_before.take()
            && std::mem::take(&mut self.group_changed)
        {
            self.record_edit(before);
        }
        result
    }

    fn record_edit(&mut self, before: MapSnapshot) {
        self.prune_images();
        if self.group_depth > 0 {
            self.group_changed = true;
            self.clear_candidates();
            self.changed();
            return;
        }
        let after = self.snapshot();
        self.edit_serial += 1;
        self.undo.push((before, after, self.edit_serial));
        // Only manual edits, bounded apart from observations.
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.clear_candidates();
        self.changed();
    }

    fn apply_history(&mut self, from: &MapSnapshot, to: &MapSnapshot) {
        // Replay only what the edit changed, so observations made since are kept.
        let old_rooms: HashMap<&str, &MapRoom> = from.rooms.iter().map(|r| (r.id.as_str(), r)).collect();
        let new_rooms: HashMap<&str, &MapRoom> = to.rooms.iter().map(|r| (r.id.as_str(), r)).collect();
        let mut ids: Vec<&str> = from.rooms.iter().map(|r| r.id.as_str()).collect();
        ids.extend(
            to.rooms
                .iter()
                .map(|r| r.id.as_str())
                .filter(|id| !old_rooms.contains_key(id)),
        );
        for id in ids {
            let (old, desired) = (old_rooms.get(id), new_rooms.get(id));
            if old == desired {
                continue;
            }
            match desired {
                None => self.delete_room(id),
                Some(room) => {
                    let revision = self.next_revision();
                    self.rooms.insert(
                        id.to_string(),
                        MapRoom {
                            revision,
                            ..(*room).clone()
                        },
                    );
                }
            }
        }
        let key = |l: &MapLink| (l.from_id.clone(), l.direction.clone());
        let old_links: HashMap<LinkKey, &MapLink> = from.links.iter().map(|l| (key(l), l)).collect();
        let new_links: HashMap<LinkKey, &MapLink> = to.links.iter().map(|l| (key(l), l)).collect();
        let mut keys: Vec<LinkKey> = from.links.iter().map(key).collect();
        keys.extend(to.links.iter().map(key).filter(|k| !old_links.contains_key(k)));
        for k in keys {
            let (old, desired) = (old_links.get(&k), new_links.get(&k));
            if old == desired {
                continue;
            }
            self.links.shift_remove(&k);
            match desired {
                None => {
                    let revision = self.next_revision();
                    self.deleted_links.insert(k, revision);
                }
                Some(link) if self.rooms.contains_key(&link.from_id) => {
                    let revision = self.next_revision();
                    self.links.insert(
                        k,
                        MapLink {
                            revision,
                            ..(*link).clone()
                        },
                    );
                }
                Some(_) => {}
            }
        }
        let old_areas: HashMap<&str, &MapAreaSettings> =
            from.area_settings.iter().map(|a| (a.area.as_str(), a)).collect();
        let new_areas: HashMap<&str, &MapAreaSettings> =
            to.area_settings.iter().map(|a| (a.area.as_str(), a)).collect();
        let mut names: Vec<&str> = old_areas.keys().copied().collect();
        names.extend(new_areas.keys().copied().filter(|a| !old_areas.contains_key(a)));
        for area in names {
            if old_areas.get(area) != new_areas.get(area) {
                let revision = self.next_revision();
                let value = new_areas
                    .get(area)
                    .map_or_else(|| MapAreaSettings::new(area, false), |a| (*a).clone());
                self.areas
                    .insert(area.to_string(), MapAreaSettings { revision, ..value });
            }
        }
        let old_aliases: HashMap<&str, &MapRoomAlias> =
            from.room_aliases.iter().map(|a| (a.source_id.as_str(), a)).collect();
        let new_aliases: HashMap<&str, &MapRoomAlias> =
            to.room_aliases.iter().map(|a| (a.source_id.as_str(), a)).collect();
        let mut sources: Vec<&str> = old_aliases.keys().copied().collect();
        sources.extend(new_aliases.keys().copied().filter(|s| !old_aliases.contains_key(s)));
        for source in sources {
            if old_aliases.get(source) != new_aliases.get(source) {
                let revision = self.next_revision();
                let value = new_aliases.get(source).map_or_else(
                    || MapRoomAlias {
                        source_id: source.to_string(),
                        target_id: source.to_string(),
                        revision: 0,
                    },
                    |a| (*a).clone(),
                );
                self.aliases
                    .insert(source.to_string(), MapRoomAlias { revision, ..value });
            }
        }
        for image in &to.images {
            self.images.entry(image.hash.clone()).or_insert_with(|| image.clone());
        }
        let old_labels: HashMap<&str, &MapLabel> = from.labels.iter().map(|l| (l.id.as_str(), l)).collect();
        let new_labels: HashMap<&str, &MapLabel> = to.labels.iter().map(|l| (l.id.as_str(), l)).collect();
        let mut label_ids: Vec<&str> = from.labels.iter().map(|l| l.id.as_str()).collect();
        label_ids.extend(
            to.labels
                .iter()
                .map(|l| l.id.as_str())
                .filter(|id| !old_labels.contains_key(id)),
        );
        for id in label_ids {
            let (old, desired) = (old_labels.get(id), new_labels.get(id));
            if old == desired {
                continue;
            }
            let revision = self.next_revision();
            match desired {
                None => {
                    self.labels.shift_remove(id);
                    self.deleted_labels.insert(id.to_string(), revision);
                }
                Some(label) => {
                    self.labels.insert(
                        id.to_string(),
                        MapLabel {
                            revision,
                            ..(*label).clone()
                        },
                    );
                }
            }
        }
        self.prune_images();
        if from.current_room_id != to.current_room_id {
            self.current = to.current_room_id.clone().filter(|id| self.rooms.contains_key(id));
        }
        self.clear_candidates();
        self.state = if self.current.is_none() {
            TrackingState::Unknown
        } else {
            TrackingState::Inferred
        };
    }
}
