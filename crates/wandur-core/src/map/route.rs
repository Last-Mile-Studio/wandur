//! Route planning (the C# `MapRoutePlanner`): the cheapest directed path. Exits cost their own
//! weight, or the destination room's when theirs is zero. Locked exits, closed or locked doors
//! (opening them needs game-specific commands) and locked (excluded) rooms are never used;
//! inferred exits and provisional rooms only when asked for.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use super::model::{MapLink, MapRoom, MapSnapshot};
use super::tracker::RoomMapTracker;

#[derive(Clone, Debug, PartialEq)]
pub struct MapRoute {
    pub steps: Vec<MapLink>,
    pub cost: f64,
}

impl MapRoute {
    /// The commands it sends, joined with arrows ("east → east → south").
    pub fn commands(&self) -> String {
        self.steps
            .iter()
            .map(|s| s.command.as_deref().unwrap_or(&s.direction))
            .collect::<Vec<_>>()
            .join(" → ")
    }
}

#[derive(PartialEq)]
struct Entry<'a> {
    cost: f64,
    order: u64,
    id: &'a str,
}

impl Eq for Entry<'_> {}

impl Ord for Entry<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Lowest cost first; then the earliest queued, as a FIFO tie break.
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.order.cmp(&self.order))
    }
}

impl PartialOrd for Entry<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The cheapest route on a saved map.
pub fn find_route(map: &MapSnapshot, from: &str, to: &str, allow_inferred: bool) -> Option<MapRoute> {
    let rooms: HashMap<&str, &MapRoom> = map.rooms.iter().map(|r| (r.id.as_str(), r)).collect();
    plan(&|id| rooms.get(id).copied(), map.links.iter(), from, to, allow_inferred)
}

/// The cheapest route on a live map.
pub fn find_route_live(map: &RoomMapTracker, from: &str, to: &str, allow_inferred: bool) -> Option<MapRoute> {
    plan(&|id| map.room(id), map.links(), from, to, allow_inferred)
}

fn plan<'a>(
    room: &dyn Fn(&str) -> Option<&'a MapRoom>,
    links: impl Iterator<Item = &'a MapLink>,
    from: &str,
    to: &str,
    allow_inferred: bool,
) -> Option<MapRoute> {
    let (start, end) = (room(from)?, room(to)?);
    if start.is_locked || end.is_locked || (!allow_inferred && (start.provisional || end.provisional)) {
        return None;
    }
    let mut exits: HashMap<&'a str, Vec<&'a MapLink>> = HashMap::new();
    for link in links {
        exits.entry(link.from_id.as_str()).or_default().push(link);
    }
    let mut distance: HashMap<&'a str, f64> = HashMap::from([(start.id.as_str(), 0.0)]);
    let mut previous: HashMap<&'a str, &'a MapLink> = HashMap::new();
    let mut queue = BinaryHeap::new();
    let mut order = 0;
    queue.push(Entry {
        cost: 0.0,
        order,
        id: start.id.as_str(),
    });
    while let Some(Entry { cost, id, .. }) = queue.pop() {
        if cost > distance[id] {
            continue;
        }
        if id == end.id {
            let mut steps = Vec::new();
            let mut at = id;
            while at != start.id {
                let link = previous[at];
                steps.push(link.clone());
                at = &link.from_id;
            }
            steps.reverse();
            return Some(MapRoute { steps, cost });
        }
        for link in exits.get(id).map(Vec::as_slice).unwrap_or_default() {
            if link.blocked() || (!allow_inferred && !link.confirmed) {
                continue;
            }
            let Some(target) = room(&link.to_id) else {
                continue;
            };
            if target.is_locked || (!allow_inferred && target.provisional) {
                continue;
            }
            let weight = if link.weight == 0.0 { target.weight } else { link.weight };
            if !weight.is_finite() || weight <= 0.0 {
                continue;
            }
            let next = cost + weight;
            let target_id = target.id.as_str();
            if !next.is_finite() || distance.get(target_id).is_some_and(|known| next >= *known) {
                continue;
            }
            distance.insert(target_id, next);
            previous.insert(target_id, link);
            order += 1;
            queue.push(Entry {
                cost: next,
                order,
                id: target_id,
            });
        }
    }
    None
}
