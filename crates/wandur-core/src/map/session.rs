//! One session's map (the non-UI half of the C# `WorkspaceController.Mapping` and
//! `.Navigation`): it feeds the tracker from GMCP and MSDP rooms, from room blocks in the text
//! (while the world sends no structured rooms) and from the directions the player sends; it
//! keeps what this connection proved about the protocols; and it walks verified routes.
//!
//! A GMCP or MSDP room without a description (Legends of the Jedi sends none) takes the one
//! the text shows under its title: the room block just before it or just after it (until the
//! next command, within [`DESCRIPTION_WINDOW`]). The protocol keeps the room's identity, exits
//! and position; the text gives only the description, kept stable across visits.
//!
//! Walking sends one direction, then waits for the server to report the expected room id
//! before sending the next. A wrong room, a blocked move, a timeout, a disconnect, private
//! input or a login, any other command, or a change to the map stops it. A command already
//! sent cannot be taken back.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use super::model::*;
use super::route::MapRoute;
use super::text::{TextBlock, TextRoomObserver};
use super::tracker::{RoomMapTracker, same_description, similar_descriptions};
use crate::l10n::{S, t, tf};

/// A direction sent this long ago no longer explains the next room.
pub const DIRECTION_WINDOW: Duration = Duration::from_secs(10);
/// A text room guess may be refined by structured data this soon after it.
pub const REFINE_WINDOW: Duration = Duration::from_secs(2);
/// A room block and the protocol room it shows may arrive this far apart (either first).
pub const DESCRIPTION_WINDOW: Duration = Duration::from_secs(1);
/// A different description replaces the stored one once this many visits showed it.
const DESCRIPTION_VISITS: u32 = 2;
/// Room blocks held for a protocol room still to come.
const RECENT_BLOCKS: usize = 4;
/// Rooms with a different description waiting for another visit.
const DESCRIPTION_CANDIDATES: usize = 256;
/// Saves at most this often (forced saves aside).
pub const SAVE_INTERVAL: Duration = Duration::from_secs(2);
/// How long a walk waits for each expected room.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(10);
/// Rooms held back while the saved map loads.
const DEFERRED_LIMIT: usize = 512;
/// Most steps in a walk.
const MAX_WALK_STEPS: usize = 10_000;

/// A telnet option as this connection has seen it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OptionState {
    /// No answer seen: not proof that the server lacks it.
    #[default]
    Unknown,
    Enabled,
    Disabled,
}

/// What this connection proved about structured room data (cached or imported rooms do not
/// count).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProtocolEvidence {
    pub gmcp: OptionState,
    pub msdp: OptionState,
    pub last_room_source: Option<RoomSource>,
    pub received_room_id: bool,
    pub received_exits: bool,
    pub received_terrain: bool,
    pub received_coordinates: bool,
    pub received_name: bool,
    pub received_description: bool,
    pub received_area: bool,
    pub received_symbol: bool,
}

impl ProtocolEvidence {
    /// "GMCP: Supported · MSDP: Not negotiated".
    pub fn summary(&self) -> String {
        let state = |s: OptionState| {
            t(match s {
                OptionState::Enabled => S::MapProtocolEnabled,
                OptionState::Disabled => S::MapProtocolDisabled,
                OptionState::Unknown => S::MapProtocolUnknown,
            })
        };
        tf(S::MapProtocolSummary, &[&state(self.gmcp), &state(self.msdp)])
    }

    /// The protocols in use, for the map's status line ("GMCP", "GMCP · MSDP", or nothing); the
    /// full summary goes in its tooltip (C# UI review, item 15).
    pub fn short(&self) -> String {
        [(self.gmcp, "GMCP"), (self.msdp, "MSDP")]
            .into_iter()
            .filter(|(s, _)| *s == OptionState::Enabled)
            .map(|(_, name)| name)
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// "Room fields received: ID, name, ..." or "No structured room fields received yet."
    pub fn room_fields(&self) -> String {
        let fields: Vec<&str> = [
            (self.received_room_id, S::MapFieldId),
            (self.received_name, S::MapFieldName),
            (self.received_description, S::MapFieldDescription),
            (self.received_area, S::MapFieldArea),
            (self.received_symbol, S::MapFieldSymbol),
            (self.received_exits, S::MapFieldExits),
            (self.received_terrain, S::MapFieldEnvironment),
            (self.received_coordinates, S::MapFieldCoordinates),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, key)| t(key))
        .collect();
        if fields.is_empty() {
            t(S::MapRoomFieldsNone).into()
        } else {
            tf(S::MapRoomFields, &[&fields.join(", ")])
        }
    }

    fn note(&mut self, room: &RoomObservation) {
        let filled = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
        self.last_room_source = Some(room.source);
        self.received_room_id |= room.server_id.is_some();
        self.received_exits |= room.exits_provided || !room.exits.is_empty();
        self.received_terrain |= filled(&room.environment);
        self.received_coordinates |= room.x.is_some() || room.y.is_some() || room.z.is_some();
        self.received_name |= !room.name.trim().is_empty();
        self.received_description |= !room.description.trim().is_empty();
        self.received_area |= filled(&room.area);
        self.received_symbol |= filled(&room.symbol);
    }
}

/// Where walking stands (the C# `MapWalk*` messages).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WalkStatus {
    #[default]
    Ready,
    Unavailable,
    CustomCommand,
    InvalidRoute,
    Complete,
    /// Step `step` of `of` sent, waiting for its room.
    Progress {
        step: usize,
        of: usize,
    },
    WrongRoom,
    Private,
    SendFailed,
    Stopped,
    Timeout,
    Disconnected,
    Blocked,
    GraphChanged,
    ManualCommand,
    OutputLost,
}

impl WalkStatus {
    pub fn text(self) -> String {
        let key = match self {
            WalkStatus::Progress { step, of } => return tf(S::MapWalkProgress, &[&step, &of]),
            WalkStatus::Ready => S::MapWalkReady,
            WalkStatus::Unavailable => S::MapWalkUnavailable,
            WalkStatus::CustomCommand => S::MapWalkCustomCommand,
            WalkStatus::InvalidRoute => S::MapWalkInvalidRoute,
            WalkStatus::Complete => S::MapWalkComplete,
            WalkStatus::WrongRoom => S::MapWalkWrongRoom,
            WalkStatus::Private => S::MapWalkPrivate,
            WalkStatus::SendFailed => S::MapWalkSendFailed,
            WalkStatus::Stopped => S::MapWalkStopped,
            WalkStatus::Timeout => S::MapWalkTimeout,
            WalkStatus::Disconnected => S::MapWalkDisconnected,
            WalkStatus::Blocked => S::MapWalkBlocked,
            WalkStatus::GraphChanged => S::MapWalkGraphChanged,
            WalkStatus::ManualCommand => S::MapWalkManualCommand,
            WalkStatus::OutputLost => S::MapWalkOutputLost,
        };
        t(key).into()
    }
}

/// What the session is doing, as walking cares.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WalkGate {
    pub connected: bool,
    /// Input is private (a password prompt, the server echoing, Private input on).
    pub private: bool,
    /// Automatic login is running.
    pub login: bool,
    /// The server echoes (it hides what is typed).
    pub remote_echo: bool,
}

#[derive(Clone, Debug)]
struct Walk {
    steps: Vec<MapLink>,
    index: usize,
    signature: u64,
    checked_version: u64,
    generation: u64,
    expected_server: Option<String>,
    expected_room: Option<String>,
    arrived: bool,
    sent_at: Instant,
}

#[derive(Debug)]
pub struct MapSession {
    tracker: RoomMapTracker,
    /// Goes up when the tracker is replaced (a loaded map): a walk on the old one ends.
    generation: u64,
    /// Versions of replaced trackers, so [`Self::version`] never goes back.
    version_base: u64,
    text: TextRoomObserver,
    pending_direction: Option<&'static str>,
    command_at: Option<Instant>,
    has_structured: bool,
    tentative: Option<(String, i32, Instant)>,
    pub evidence: ProtocolEvidence,
    loaded: bool,
    deferred: Vec<RoomObservation>,
    saved_version: u64,
    saved_at: Option<Instant>,
    walk: Option<Walk>,
    walk_status: WalkStatus,
    /// How long a walk waits for each room (tests shorten it).
    pub step_timeout: Duration,
    /// Room blocks read since the last command, for a protocol room arriving after its text.
    recent_blocks: VecDeque<(TextBlock, Instant)>,
    /// A protocol room without a description waiting for its text: (room id, name, exits, when).
    awaiting_text: Option<(String, String, Vec<String>, Instant)>,
    /// A description different from a room's stored one, and how many visits showed it.
    description_candidates: HashMap<String, (String, u32)>,
    /// Descriptions read during a walk, applied when it ends (a room's new revision would stop
    /// it as a changed map).
    held_descriptions: Vec<(String, String)>,
}

impl Default for MapSession {
    fn default() -> Self {
        Self {
            tracker: RoomMapTracker::new(),
            generation: 0,
            version_base: 0,
            text: TextRoomObserver::new(),
            pending_direction: None,
            command_at: None,
            has_structured: false,
            tentative: None,
            evidence: ProtocolEvidence::default(),
            loaded: true,
            deferred: Vec::new(),
            saved_version: 0,
            saved_at: None,
            walk: None,
            walk_status: WalkStatus::Ready,
            step_timeout: STEP_TIMEOUT,
            recent_blocks: VecDeque::new(),
            awaiting_text: None,
            description_candidates: HashMap::new(),
            held_descriptions: Vec::new(),
        }
    }
}

impl MapSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// A session over a saved map.
    pub fn with_map(map: MapSnapshot) -> Self {
        Self {
            tracker: RoomMapTracker::from_snapshot(map),
            ..Self::default()
        }
    }

    pub fn tracker(&self) -> &RoomMapTracker {
        &self.tracker
    }

    /// Change the map by hand (the editing calls). A walk notices and stops.
    pub fn tracker_mut(&mut self) -> &mut RoomMapTracker {
        &mut self.tracker
    }

    /// Goes up on every change, also across a loaded map.
    pub fn version(&self) -> u64 {
        self.version_base + self.tracker.version()
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Goes up when the tracker is replaced by a loaded map.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The stored map is being read: hold rooms back until it arrives (saving before it
    /// arrives would write an empty map over it).
    pub fn begin_load(&mut self) {
        self.loaded = false;
    }

    /// The stored map arrived (or there is none): use it, then the rooms held back.
    pub fn finish_load(&mut self, map: Option<MapSnapshot>, gate: WalkGate, now: Instant) {
        if let Some(map) = map {
            self.version_base = self.version() + 1;
            self.tracker = RoomMapTracker::from_snapshot(map);
            self.generation += 1;
        }
        self.saved_version = self.version();
        self.loaded = true;
        for room in std::mem::take(&mut self.deferred) {
            self.observe_room(room, gate, now);
        }
    }

    /// A new connection: walking and per-connection evidence start over and the position is
    /// found again from what the world sends.
    pub fn connection_started(&mut self) {
        self.stop_walk(WalkStatus::Disconnected);
        self.walk_status = WalkStatus::Ready;
        self.evidence = ProtocolEvidence::default();
        self.text.reset();
        self.forget_text_blocks();
        self.pending_direction = None;
        self.has_structured = false;
        self.tentative = None;
        if self.tracker.current_id().is_some() {
            self.tracker.lose_position();
        }
    }

    pub fn set_gmcp(&mut self, state: OptionState) {
        self.evidence.gmcp = state;
    }

    pub fn set_msdp(&mut self, state: OptionState) {
        self.evidence.msdp = state;
    }

    /// A room from GMCP, MSDP or the text.
    pub fn observe_room(&mut self, room: RoomObservation, gate: WalkGate, now: Instant) {
        if !self.loaded {
            if self.deferred.len() < DEFERRED_LIMIT {
                self.deferred.push(room);
            }
            return;
        }
        let direction = self.pending_direction.filter(|_| {
            self.command_at
                .is_some_and(|at| now.duration_since(at) < DIRECTION_WINDOW)
        });
        let origin_server = self.tracker.current().and_then(|r| r.server_id.clone());
        // GMCP and MSDP may repeat the origin while a move is in flight: that refreshes it, it
        // neither acknowledges the arrival nor uses up the move.
        let repeats_origin = direction.is_some() && origin_server.is_some() && room.server_id == origin_server;
        let observations = self.tracker.observation_count();
        let room_count = self.tracker.room_count();
        // A server may send the text and the structured part of one answer in separate reads:
        // only a guess just made, with no command or observation since, is revised.
        let tentative = self
            .tentative
            .clone()
            .filter(|(_, _, at)| now.duration_since(*at) < REFINE_WINDOW);
        let refined = !self.has_structured
            && room.source != RoomSource::Text
            && tentative.is_some_and(|(id, count, _)| self.tracker.refine_provisional_room(&id, count, &room));
        if !refined {
            self.tracker.observe(&room, direction);
        }
        if self.tracker.observation_count() == observations {
            return;
        }
        self.tentative = match self.tracker.current_id() {
            Some(id) if room.source == RoomSource::Text && self.tracker.room_count() > room_count => {
                Some((id.to_string(), self.tracker.observation_count(), now))
            }
            _ => None,
        };
        // Structured names and exits stay better evidence than titles guessed from login
        // banners or tutorial prose, even without room ids.
        if room.source != RoomSource::Text {
            self.has_structured = true;
            self.evidence.note(&room);
            self.describe_protocol_room(&room, now);
        }
        if !repeats_origin {
            self.pending_direction = None;
            self.observe_walk_arrival(&room, gate);
        }
    }

    /// A command is being sent (`private`: it is a password or other private input).
    pub fn track_command(&mut self, command: &str, private: bool, now: Instant) {
        self.tentative = None;
        if private {
            self.pending_direction = None;
            return;
        }
        match normalize_direction(command) {
            Some(direction) => {
                // Two moves without an answer cannot be told apart.
                if self.pending_direction.is_some() {
                    self.tracker.lose_position();
                    self.pending_direction = None;
                } else {
                    self.pending_direction = Some(direction);
                }
                self.command_at = Some(now);
            }
            None => {
                // `enter portal` and the like cannot inherit an older direction. The next room
                // still sets the position.
                self.pending_direction = None;
                if command.trim().eq_ignore_ascii_case("recall") {
                    self.tracker.lose_position();
                }
            }
        }
        self.text.reset();
        self.forget_text_blocks();
    }

    /// Server text: rooms read from it (while no structured rooms came) and failed moves.
    pub fn track_output(&mut self, text: &str, gate: WalkGate, now: Instant) {
        // A failed move matters while a move or a walk waits for its answer (C# looks at every
        // line; this saves the search on a flood).
        self.text.watch_failures = self.pending_direction.is_some() || self.walk.is_some();
        // Room blocks matter once the world sends structured rooms (or may: the first room's
        // text can come before its `Room.Info`).
        self.text.keep_blocks = self.has_structured
            || self.evidence.gmcp == OptionState::Enabled
            || self.evidence.msdp == OptionState::Enabled;
        let rooms = self.text.feed(text);
        self.take_text_blocks(now);
        if self.text.movement_failed {
            self.stop_walk(WalkStatus::Blocked);
            self.pending_direction = None;
            self.tracker.movement_failed();
        }
        if self.has_structured {
            return;
        }
        for room in rooms {
            self.observe_room(room, gate, now);
        }
    }

    /// The server marked a prompt: the room block being read is finished (its description may
    /// belong to a protocol room that came first).
    pub fn prompt_seen(&mut self, now: Instant) {
        if self.has_structured {
            self.text.flush();
            self.take_text_blocks(now);
        }
    }

    // ---- descriptions from text --------------------------------------------------------

    fn forget_text_blocks(&mut self) {
        self.recent_blocks.clear();
        self.awaiting_text = None;
        self.text.blocks.clear();
    }

    /// Pair the room blocks the observer finished with the protocol room waiting for its text,
    /// or hold them for one still to come.
    fn take_text_blocks(&mut self, now: Instant) {
        if self.text.blocks.is_empty() {
            return;
        }
        for block in std::mem::take(&mut self.text.blocks) {
            let awaiting = self
                .awaiting_text
                .take()
                .filter(|(_, _, _, at)| now.duration_since(*at) < DESCRIPTION_WINDOW);
            match awaiting {
                Some((id, name, exits, _)) if shows_room(&block, &name, &exits) => {
                    if let Some(description) = block.description_for(&name) {
                        self.offer_description(&id, description);
                    }
                }
                other => {
                    self.awaiting_text = other;
                    self.recent_blocks
                        .retain(|(_, at)| now.duration_since(*at) < DESCRIPTION_WINDOW);
                    if self.recent_blocks.len() >= RECENT_BLOCKS {
                        self.recent_blocks.pop_front();
                    }
                    self.recent_blocks.push_back((block, now));
                }
            }
        }
    }

    /// A protocol room was observed: when it has no description, take one from the room block
    /// just read, or wait for the block to come.
    fn describe_protocol_room(&mut self, room: &RoomObservation, now: Instant) {
        // A room change ends the block being read (it belongs to the room before, or to this
        // one when the text came first).
        self.text.flush();
        self.take_text_blocks(now);
        self.awaiting_text = None;
        if !room.description.trim().is_empty() {
            return;
        }
        let Some(id) = self.tracker.current_id().map(str::to_string) else {
            return;
        };
        let exits: Vec<String> = room.exits.keys().cloned().collect();
        let found = self.recent_blocks.iter().rposition(|(block, at)| {
            now.duration_since(*at) < DESCRIPTION_WINDOW && shows_room(block, &room.name, &exits)
        });
        match found {
            Some(index) => {
                // That block and every one before it are used up.
                let block = self.recent_blocks.drain(..=index).next_back().map(|(b, _)| b);
                if let Some(description) = block.and_then(|b| b.description_for(&room.name)) {
                    self.offer_description(&id, description);
                }
            }
            None => self.awaiting_text = Some((id, room.name.clone(), exits, now)),
        }
    }

    /// A description read for room `id`: the first one is kept; a different one replaces it
    /// only once [`DESCRIPTION_VISITS`] visits in a row showed it (a weather or time-of-day
    /// line changes nothing); a room edited by hand keeps its own.
    fn offer_description(&mut self, id: &str, description: String) {
        if self.walk.is_some() {
            if self.held_descriptions.len() < DEFERRED_LIMIT {
                self.held_descriptions.push((id.to_string(), description));
            }
            return;
        }
        let Some(room) = self.tracker.room(id) else {
            return;
        };
        if room.is_manually_edited {
            return;
        }
        if room.description.trim().is_empty() {
            self.description_candidates.remove(id);
            self.tracker.set_text_description(id, &description);
            return;
        }
        if similar_descriptions(&room.description, &description) || same_core(&room.description, &description) {
            self.description_candidates.remove(id);
            return;
        }
        let seen = match self.description_candidates.get(id) {
            Some((candidate, visits)) if same_description(candidate, &description) => visits + 1,
            _ => 1,
        };
        if seen >= DESCRIPTION_VISITS {
            self.description_candidates.remove(id);
            self.tracker.set_text_description(id, &description);
        } else {
            if self.description_candidates.len() >= DESCRIPTION_CANDIDATES {
                self.description_candidates.clear();
            }
            self.description_candidates.insert(id.to_string(), (description, seen));
        }
    }

    fn apply_held_descriptions(&mut self) {
        for (id, description) in std::mem::take(&mut self.held_descriptions) {
            self.offer_description(&id, description);
        }
    }

    // ---- saving ------------------------------------------------------------------------

    /// The map to save now, if it changed (at most every two seconds unless `force`).
    pub fn take_save(&mut self, now: Instant, force: bool) -> Option<MapSnapshot> {
        if !self.loaded || self.version() == self.saved_version {
            return None;
        }
        if !force && self.saved_at.is_some_and(|at| now.duration_since(at) < SAVE_INTERVAL) {
            return None;
        }
        self.saved_at = Some(now);
        self.saved_version = self.version();
        Some(self.tracker.snapshot())
    }

    /// When a save is due (for the frame scheduler).
    pub fn save_deadline(&self) -> Option<Instant> {
        if !self.loaded || self.version() == self.saved_version {
            return None;
        }
        Some(self.saved_at.map_or_else(Instant::now, |at| at + SAVE_INTERVAL))
    }

    // ---- walking -----------------------------------------------------------------------

    pub fn is_walking(&self) -> bool {
        self.walk.is_some()
    }

    pub fn walk_status(&self) -> WalkStatus {
        self.walk_status
    }

    /// Stop a walk, saying why. Nothing happens when none runs.
    pub fn stop_walk(&mut self, reason: WalkStatus) {
        if self.walk.take().is_some() {
            self.walk_status = reason;
            self.apply_held_descriptions();
        }
    }

    /// Start walking `route`. Returns the first command to send, or `None` with the reason in
    /// [`Self::walk_status`].
    pub fn start_walk(&mut self, route: &MapRoute, gate: WalkGate, now: Instant) -> Option<String> {
        self.stop_walk(WalkStatus::Stopped);
        if !gate.connected
            || gate.private
            || gate.login
            || gate.remote_echo
            || self.pending_direction.is_some()
            || self.tracker.state() != TrackingState::Confirmed
            || self.tracker.current_id().is_none()
        {
            self.walk_status = WalkStatus::Unavailable;
            return None;
        }
        // Every step is checked before the first is sent; custom commands need a person.
        if route.steps.iter().any(|s| {
            let command = s.command.as_deref().unwrap_or(&s.direction);
            command.chars().any(char::is_control) || normalize_direction(command).is_none()
        }) {
            self.walk_status = WalkStatus::CustomCommand;
            return None;
        }
        if !self.valid_route(&route.steps) {
            self.walk_status = WalkStatus::InvalidRoute;
            return None;
        }
        if route.steps.is_empty() {
            self.walk_status = WalkStatus::Complete;
            return None;
        }
        self.walk = Some(Walk {
            steps: route.steps.clone(),
            index: 0,
            signature: self.tracker.walk_signature(),
            checked_version: self.tracker.version(),
            generation: self.generation,
            expected_server: None,
            expected_room: None,
            arrived: false,
            sent_at: now,
        });
        self.next_step(gate, now)
    }

    /// After a batch of output was applied: the next command once the expected room arrived,
    /// the end of the walk, or a stop (privacy, a changed map, a timeout).
    pub fn advance_walk(&mut self, gate: WalkGate, now: Instant) -> Option<String> {
        if self.walk.is_none() || !self.can_continue(gate) {
            return None;
        }
        let walk = self.walk.as_mut()?;
        if walk.arrived {
            walk.index += 1;
            if walk.index >= walk.steps.len() {
                self.walk = None;
                self.walk_status = WalkStatus::Complete;
                self.apply_held_descriptions();
                return None;
            }
            return self.next_step(gate, now);
        }
        if now.duration_since(walk.sent_at) >= self.step_timeout {
            self.stop_walk(WalkStatus::Timeout);
        }
        None
    }

    /// When the current step times out.
    pub fn walk_deadline(&self) -> Option<Instant> {
        self.walk
            .as_ref()
            .filter(|w| !w.arrived)
            .map(|w| w.sent_at + self.step_timeout)
    }

    fn next_step(&mut self, gate: WalkGate, now: Instant) -> Option<String> {
        if !self.can_continue(gate) {
            return None;
        }
        let walk = self.walk.as_ref()?;
        let step = walk.steps[walk.index].clone();
        if self.tracker.current_id() != Some(step.from_id.as_str()) || self.tracker.state() != TrackingState::Confirmed
        {
            self.stop_walk(WalkStatus::WrongRoom);
            return None;
        }
        let expected_server = self.tracker.room(&step.to_id).and_then(|r| r.server_id.clone());
        let walk = self.walk.as_mut()?;
        walk.expected_server = expected_server;
        walk.expected_room = Some(step.to_id.clone());
        walk.arrived = false;
        walk.sent_at = now;
        self.walk_status = WalkStatus::Progress {
            step: walk.index + 1,
            of: walk.steps.len(),
        };
        normalize_direction(step.command.as_deref().unwrap_or(&step.direction)).map(str::to_string)
    }

    fn can_continue(&mut self, gate: WalkGate) -> bool {
        let Some(walk) = &self.walk else {
            return false;
        };
        if walk.generation != self.generation || !gate.connected {
            self.stop_walk(WalkStatus::Disconnected);
            return false;
        }
        if gate.private || gate.login || gate.remote_echo {
            self.stop_walk(WalkStatus::Private);
            return false;
        }
        if walk.checked_version != self.tracker.version() {
            let signature = self.tracker.walk_signature();
            if signature != walk.signature {
                self.stop_walk(WalkStatus::GraphChanged);
                return false;
            }
            let version = self.tracker.version();
            if let Some(walk) = &mut self.walk {
                walk.checked_version = version;
            }
        }
        if self.tracker.state() != TrackingState::Confirmed {
            self.stop_walk(WalkStatus::WrongRoom);
            return false;
        }
        true
    }

    fn observe_walk_arrival(&mut self, room: &RoomObservation, gate: WalkGate) {
        if self.walk.is_none() || !self.can_continue(gate) {
            return;
        }
        // Text matches, names and provisional ids never acknowledge a move.
        if room.source == RoomSource::Text || room.server_id.as_deref().is_none_or(|s| s.trim().is_empty()) {
            return;
        }
        let current = self.tracker.current_id().map(str::to_string);
        let confirmed = self.tracker.state() == TrackingState::Confirmed;
        let Some(walk) = self.walk.as_mut() else {
            return;
        };
        if walk.expected_server.is_none()
            || room.server_id != walk.expected_server
            || !confirmed
            || current != walk.expected_room
        {
            self.stop_walk(WalkStatus::WrongRoom);
            return;
        }
        walk.arrived = true;
    }

    fn valid_route(&self, steps: &[MapLink]) -> bool {
        if steps.len() > MAX_WALK_STEPS {
            return false;
        }
        let walkable = |room: &MapRoom| {
            !room.provisional
                && !room.is_locked
                && room
                    .server_id
                    .as_deref()
                    .is_some_and(|s| !s.trim().is_empty() && s != "0" && s != "-1")
        };
        let Some(mut current) = self.tracker.current_id().map(str::to_string) else {
            return false;
        };
        if !self.tracker.room(&current).is_some_and(walkable) {
            return false;
        }
        for step in steps {
            let Some(actual) = self.tracker.link(&step.from_id, &step.direction) else {
                return false;
            };
            let same = actual.to_id == step.to_id
                && actual.confirmed == step.confirmed
                && actual.command == step.command
                && actual.is_locked == step.is_locked
                && actual.door_state == step.door_state
                && actual.weight == step.weight
                && actual.revision == step.revision;
            let Some(destination) = self.tracker.room(&step.to_id) else {
                return false;
            };
            if step.from_id != current || !same || !step.confirmed || step.blocked() || !walkable(destination) {
                return false;
            }
            let weight = if step.weight == 0.0 {
                destination.weight
            } else {
                step.weight
            };
            if !weight.is_finite() || weight <= 0.0 {
                return false;
            }
            current = step.to_id.clone();
        }
        true
    }
}

#[cfg(test)]
mod tests;

/// Whether two descriptions are one room's text with a line that changes (weather, the time of
/// day): sentences found in both make up most of each.
fn same_core(first: &str, second: &str) -> bool {
    fn sentences(text: &str) -> Vec<String> {
        let text = super::text::collapse_whitespace(text).to_lowercase();
        let mut out = Vec::new();
        let mut start = 0;
        for (i, c) in text.char_indices() {
            if matches!(c, '.' | '!' | '?') && text[i + 1..].starts_with(' ') {
                out.push(text[start..=i].trim().to_string());
                start = i + 1;
            }
        }
        let rest = text[start..].trim();
        if !rest.is_empty() {
            out.push(rest.to_string());
        }
        out
    }
    let (a, b) = (sentences(first), sentences(second));
    let length = |list: &[String]| list.iter().map(String::len).sum::<usize>();
    let shared = |from: &[String], other: &[String]| -> usize {
        from.iter().filter(|s| other.contains(s)).map(String::len).sum()
    };
    let (la, lb) = (length(&a), length(&b));
    la > 0 && lb > 0 && shared(&a, &b) * 5 >= la * 3 && shared(&b, &a) * 5 >= lb * 3
}

/// Whether `block` shows the room `name` (its title matches) and its exits do not contradict
/// the protocol's (they share one, or one side lists none).
fn shows_room(block: &TextBlock, name: &str, exits: &[String]) -> bool {
    block.shows(name)
        && (exits.is_empty()
            || block.exits.is_empty()
            || block
                .exits
                .iter()
                .any(|e| exits.iter().any(|x| normalize_direction(x).unwrap_or(x.as_str()) == e)))
}
