//! The Map panel (the C# `MapView`, `MapViewModel` and `RoomMapControl`): the active session's
//! rooms on one floor of one area, north up.
//!
//! - Toolbar: room search, auto-center, fit the floor, Map tools, and Stop while walking.
//! - Search: rooms whose observed name or description holds every term are ringed, the rest
//!   fade; Enter steps through them (changing floor when needed), Escape clears; matches on
//!   other floors are listed over the map.
//! - Canvas: terrain-coloured rooms and directed exits (dashed when inferred, door marks,
//!   arrows), exit lights on the room edges, floor badges, labels where they fit; grid mode
//!   (saved per area) fills each room's cell instead. Drag to pan, scroll or pinch to zoom at the
//!   pointer, click to select, double-click to walk a verified route there.
//! - Map tools: the tracking status, area and grid mode, floors, the selection, legends,
//!   Recheck position, the local exercise, the protocol evidence, and Route planning.
//! - Status bar: walking progress, GMCP and MSDP negotiation, the zoom slider.
//!
//! The view comes in two sizes. The docked Map panel is a mini map ([`MapViewState::mini`]):
//! the current room and its neighbourhood, centred and following, with minimal chrome (follow,
//! Open full map, and a menu with Edit map, Fit floor and the remaining tools; no search, no
//! route planning, no status line except while walking). Each session's Map page is the full map
//! ([`MapViewState::full`]): every tool above, and an Edit toggle that puts the map editor's
//! inspector ([`editor`]) beside the canvas instead of the Map tools overlay. Both draw the
//! session's one tracker, so a move, an edit or an undo shows in both at once.

use std::collections::{HashMap, HashSet};

use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, Vec2, pos2,
    vec2,
};
use wandur_core::l10n::{S, t, tf};
use wandur_core::map::model::RoomObservation;
use wandur_core::map::{
    MapAreaSettings, MapLink, MapRoom, MapRoute, MapSnapshot, RoomMapTracker, RoomSource, TrackingState, WalkStatus,
    find_route_live,
};

pub mod editor;
pub mod labels;

use crate::session_tab::SessionTab;
use crate::theme::{Theme, is_light, mix};
use crate::widgets::{self, Icon};

/// Points between room centres at zoom 1 (the C# 96).
const SCALE: f64 = 96.0;
/// The mini map's starting zoom: the current room and a ring or two of neighbours.
const MINI_ZOOM: f64 = 0.6;
const MIN_ZOOM: f64 = 0.05;
const MAX_ZOOM: f64 = 4.0;

/// A screen set up for a capture (the C# reference scenes).
#[derive(Clone, Debug, PartialEq)]
pub enum MapScene {
    /// The search box open with this text.
    Search(String),
    /// Grid mode on for the current area, the floor fitted.
    Grid,
    /// Map tools open.
    Tools,
    /// A route planned to this room, Map tools open on Route planning.
    Route(String),
    /// This room selected, the floor fitted.
    Full(String),
    /// The floor fitted (the full map at a glance).
    Fit,
    /// This room's (by id) area and floor fitted, nothing selected.
    Area(String),
    /// This zoom, centred on the current room (the mini map).
    Zoom(f64),
    /// Editing with this label selected.
    EditorLabel(String),
    /// Editing: the floor fitted, edit mode on.
    Editor,
    /// Editing with this room selected, text typed into its description and notes.
    EditorRoom {
        name: String,
        description: String,
        notes: String,
    },
    /// Editing with these rooms selected (the inspector's shared fields).
    EditorRooms(Vec<String>),
    /// Editing with this room's exit selected.
    EditorExit { from: String, direction: String },
    /// A Connect drag from one room on its way to another.
    EditorConnect { from: String, toward: String },
    /// The context menu open on this room.
    EditorMenu(String),
}

/// The local recognition exercise: a small fixed map tracked from text observations only.
#[derive(Debug)]
struct Exercise {
    tracker: RoomMapTracker,
    step: usize,
}

#[derive(Debug)]
pub struct MapViewState {
    center: (f64, f64),
    pan: Vec2,
    pub zoom: f64,
    pub area: String,
    pub floor: f64,
    pub selected: Option<String>,
    has_centered: bool,
    last_centered: Option<String>,
    last_floor: Option<f64>,
    last_area: Option<String>,
    fit_floor: bool,
    viewport: Vec2,
    seen_version: Option<u64>,
    pub search_visible: bool,
    pub search_query: String,
    search_index: Option<usize>,
    focus_search: bool,
    search_cache: Option<(u64, String, Vec<String>)>,
    pub tools_open: bool,
    /// A header action clicked last frame, run at the toolbar's turn this frame.
    pub header_click: Option<&'static str>,
    pub route_open: bool,
    /// Map tools' Room terrain inference section is open.
    pub inference_open: bool,
    scroll_tools_to_end: bool,
    pub allow_inferred: bool,
    route_destination: Option<String>,
    pub planned: Option<MapRoute>,
    walk_feedback: bool,
    route_unavailable: bool,
    exercise: Option<Exercise>,
    last_clicked: Option<String>,
    scene: Option<MapScene>,
    /// Rooms drawn in the last frame.
    pub painted: usize,
    /// Where the canvas was last drawn.
    canvas: Rect,
    floor_index: FloorIndex,
    /// The walk the last double click asked for (tests).
    pub walk_requested: u64,
    /// The map editor's inspector and forms (`Some` for the full map, shown while editing).
    pub editor: Option<Box<editor::EditorState>>,
    /// The docked panel's mini map: minimal chrome, no search or route planning.
    pub mini: bool,
    /// The full map follows the current room (its own toggle; the mini map follows the
    /// auto-center setting).
    pub follow: bool,
    /// Frames this view was drawn (a hidden full map is not drawn at all).
    pub shown: u64,
}

impl Default for MapViewState {
    fn default() -> Self {
        Self {
            center: (0.0, 0.0),
            pan: Vec2::ZERO,
            zoom: 1.0,
            area: String::new(),
            floor: 0.0,
            selected: None,
            has_centered: false,
            last_centered: None,
            last_floor: None,
            last_area: None,
            fit_floor: false,
            viewport: vec2(320.0, 300.0),
            seen_version: None,
            search_visible: false,
            search_query: String::new(),
            search_index: None,
            focus_search: false,
            search_cache: None,
            tools_open: false,
            header_click: None,
            route_open: false,
            inference_open: false,
            scroll_tools_to_end: false,
            allow_inferred: false,
            route_destination: None,
            planned: None,
            walk_feedback: false,
            route_unavailable: false,
            exercise: None,
            last_clicked: None,
            scene: None,
            painted: 0,
            canvas: Rect::NOTHING,
            floor_index: FloorIndex::default(),
            walk_requested: 0,
            editor: None,
            mini: false,
            follow: true,
            shown: 0,
        }
    }
}

/// What the panel needs besides the session.
pub struct MapContext<'a> {
    pub theme: &'a Theme,
    /// Settings > the map's auto-center (the toolbar toggle); the app saves a change.
    pub auto_center: &'a mut bool,
    /// Set when the mini map's Edit map is pressed (the app opens the session's Map page with
    /// Edit on).
    pub open_editor: bool,
    /// Set when the mini map's Open full map is pressed (the app shows the session's Map page).
    pub open_full: bool,
    /// Room terrain inference in Map tools, when the app has a classifier.
    pub inference: Option<InferenceControls<'a>>,
    /// Where the toolbar's buttons go: the dock header (the app passes clicks in
    /// `MapViewState::header_click`), a row under it, or a plain bar (the map editor document).
    pub actions_place: crate::panel_header::ActionsPlace,
    /// The map editor's remembered inspector width, sections and snap (the settings'); `None`
    /// keeps the editor's own.
    pub editor_prefs: Option<&'a mut wandur_core::settings::MapEditorPrefs>,
    /// Set when an edit deleted rooms, exits or a label (the app shows the undo toast).
    pub deleted: Option<editor::Deleted>,
    /// Set when the overview's Import was pressed (the app opens File > Import map).
    pub open_import: bool,
}

impl<'a> MapContext<'a> {
    pub fn new(theme: &'a Theme, auto_center: &'a mut bool) -> Self {
        Self {
            theme,
            auto_center,
            open_editor: false,
            open_full: false,
            inference: None,
            actions_place: crate::panel_header::ActionsPlace::Bar,
            editor_prefs: None,
            deleted: None,
            open_import: false,
        }
    }
}

/// Map tools' Room terrain inference section (the C# `MapInferenceSection`).
pub struct InferenceControls<'a> {
    pub service: &'a wandur_core::classify::RoomClassificationService,
    /// Settings: colour rooms with the local model.
    pub enabled: &'a mut bool,
    /// Settings: the minimum confidence, 0.5 to 0.99.
    pub threshold: &'a mut f64,
    /// Set when the person picked a package to install.
    pub install: Option<std::path::PathBuf>,
}

/// Something to do to the session once drawing is done.
enum Intent {
    Walk(MapRoute),
    StopWalk,
    Recheck,
    Grid(String, bool),
    ToggleExercise,
    NextExercise,
    Edit(editor::EditAction),
}

/// Colours of the map, after the C# brushes; a light map gets darker inks.
#[derive(Clone, Copy)]
struct Ink {
    ground: Color32,
    route: Color32,
    mint: Color32,
    glow: Color32,
    amber: Color32,
    gray: Color32,
    red: Color32,
    tile: Color32,
    select: Color32,
    grid: Color32,
    search: Color32,
    /// The North badge: the panel's colour, line and text.
    badge: (Color32, Color32, Color32),
}

impl Ink {
    fn of(theme: &Theme) -> Self {
        let badge = (theme.panel, theme.border, theme.text);
        Self {
            badge,
            ..Self::base(theme)
        }
    }

    fn base(theme: &Theme) -> Self {
        let ground = theme.map_background;
        if is_light(ground) {
            Self {
                ground,
                route: Color32::from_rgb(0x3E, 0x55, 0x63),
                mint: Color32::from_rgb(0x1F, 0x8A, 0x6D),
                glow: Color32::from_rgba_unmultiplied(0x1F, 0x8A, 0x6D, 0x40),
                amber: Color32::from_rgb(0xB7, 0x79, 0x1F),
                gray: Color32::from_rgb(0x6B, 0x7C, 0x87),
                red: Color32::from_rgb(0xB5, 0x47, 0x5A),
                tile: mix(ground, Color32::from_rgb(0x25, 0x34, 0x3E), 0.12),
                select: Color32::from_rgb(0x1B, 0x2A, 0x33),
                grid: theme.map_grid,
                search: theme.accent,
                badge: (Color32::TRANSPARENT, Color32::TRANSPARENT, Color32::TRANSPARENT),
            }
        } else {
            Self {
                ground,
                route: Color32::from_rgb(0xC3, 0xD6, 0xE0),
                mint: Color32::from_rgb(0x81, 0xD9, 0xBE),
                glow: Color32::from_rgba_unmultiplied(0x81, 0xD9, 0xBE, 0x40),
                amber: Color32::from_rgb(0xF2, 0xBF, 0x6A),
                gray: Color32::from_rgb(0x84, 0x97, 0xA3),
                red: Color32::from_rgb(0xDE, 0x7D, 0x88),
                tile: Color32::from_rgb(0x25, 0x34, 0x3E),
                select: Color32::WHITE,
                grid: theme.map_grid,
                search: theme.accent,
                badge: (Color32::TRANSPARENT, Color32::TRANSPARENT, Color32::TRANSPARENT),
            }
        }
    }
}

/// "0.##": whole numbers without decimals.
fn number(v: f64) -> String {
    let rounded = (v * 100.0).round() / 100.0;
    if rounded == rounded.trunc() {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

impl MapViewState {
    /// The full map with its inspector open (the old map editor document's view).
    pub fn editor() -> Self {
        let mut state = Self::full();
        if let Some(e) = state.editor.as_deref_mut() {
            e.open = true;
        }
        state
    }

    /// A session's full map (its Map page): every tool, and the inspector behind Edit.
    pub fn full() -> Self {
        Self {
            editor: Some(Box::default()),
            ..Self::default()
        }
    }

    /// The docked panel's mini map.
    pub fn mini() -> Self {
        Self {
            mini: true,
            zoom: MINI_ZOOM,
            ..Self::default()
        }
    }

    /// The map editor's inspector is shown (the full map's Edit is on).
    pub fn is_editor(&self) -> bool {
        self.editor.as_deref().is_some_and(|e| e.open)
    }

    /// Turn the full map's Edit on or off: the inspector and the editor's edit mode together.
    pub fn set_editing(&mut self, on: bool) {
        if let Some(e) = self.editor.as_deref_mut() {
            e.open = on;
            e.tool = editor::Tool::Select;
            e.gesture = editor::Gesture::None;
            e.menu = None;
            e.confirm_delete = None;
            e.merge_source = None;
            if on {
                self.tools_open = false;
            }
        }
    }

    /// Start on a room (the map editor opening on the selection): its area and floor shown,
    /// centred on it, kept there by the first refresh.
    pub fn start_on(&mut self, tracker: &RoomMapTracker, id: &str) {
        self.select_room(tracker, id);
        self.has_centered = true;
        let current = tracker.current();
        self.last_centered = current.map(|c| c.id.clone());
        self.last_floor = current.map(|c| c.z);
        self.last_area = current.map(|c| c.area_key().to_string());
    }

    /// Set up a capture's screen once the map has its rooms.
    pub fn set_scene(&mut self, scene: MapScene) {
        self.scene = Some(scene);
    }

    /// Fit the floor now (the toolbar's Fit floor), for benches.
    pub fn fit_now(&mut self, tracker: &RoomMapTracker) {
        self.fit(tracker);
    }

    pub fn scene_pending(&self) -> bool {
        self.scene.is_some()
    }

    fn scale(&self) -> f64 {
        SCALE * self.zoom
    }

    fn project(&self, rect: Rect, x: f64, y: f64) -> Pos2 {
        let s = self.scale();
        pos2(
            rect.left() + rect.width() / 2.0 + self.pan.x + ((x - self.center.0) * s) as f32,
            rect.top() + rect.height() / 2.0 + self.pan.y - ((y - self.center.1) * s) as f32,
        )
    }

    fn unproject(&self, rect: Rect, p: Pos2) -> (f64, f64) {
        let s = self.scale();
        (
            self.center.0 + f64::from(p.x - rect.left() - rect.width() / 2.0 - self.pan.x) / s,
            self.center.1 - f64::from(p.y - rect.top() - rect.height() / 2.0 - self.pan.y) / s,
        )
    }

    fn zoom_at(&mut self, rect: Rect, zoom: f64, anchor: Pos2) {
        if !zoom.is_finite() || zoom <= 0.0 {
            return;
        }
        let at = self.unproject(rect, anchor);
        self.fit_floor = false;
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let projected = self.project(rect, at.0, at.1);
        self.pan += anchor - projected;
    }

    fn set_area(&mut self, tracker: &RoomMapTracker, area: &str) {
        if self.area == area {
            return;
        }
        self.area = area.to_string();
        self.selected = None;
        let floors = floors(tracker, &self.area);
        if !floors.contains(&self.floor) {
            self.set_floor(floors.first().copied().unwrap_or(0.0));
        }
        self.fit(tracker);
    }

    fn set_floor(&mut self, floor: f64) {
        if self.floor != floor {
            self.floor = floor;
            self.selected = None;
        }
    }

    fn visible<'a>(&self, tracker: &'a RoomMapTracker) -> impl Iterator<Item = &'a MapRoom> {
        let (area, floor) = (self.area.clone(), self.floor);
        tracker.rooms().filter(move |r| r.area_key() == area && r.z == floor)
    }

    fn center_on_floor(&mut self, tracker: &RoomMapTracker, room: Option<(f64, f64)>) {
        self.center = room.unwrap_or_else(|| {
            let mut bounds: Option<(f64, f64, f64, f64)> = None;
            for r in self.visible(tracker) {
                bounds = Some(match bounds {
                    None => (r.x, r.x, r.y, r.y),
                    Some((a, b, c, d)) => (a.min(r.x), b.max(r.x), c.min(r.y), d.max(r.y)),
                });
            }
            bounds.map_or((0.0, 0.0), |(a, b, c, d)| ((a + b) / 2.0, (c + d) / 2.0))
        });
        self.pan = Vec2::ZERO;
    }

    /// Fit the floor (the C# `FitFloor`): centred, zoomed to show every room, at most 1.
    fn fit(&mut self, tracker: &RoomMapTracker) {
        self.fit_floor = true;
        self.center_on_floor(tracker, None);
        let grid = f64::from(u8::from(tracker.grid_mode(&self.area)));
        let mut bounds: Option<(f64, f64, f64, f64)> = None;
        for r in self.visible(tracker) {
            bounds = Some(match bounds {
                None => (r.x, r.x, r.y, r.y),
                Some((a, b, c, d)) => (a.min(r.x), b.max(r.x), c.min(r.y), d.max(r.y)),
            });
        }
        let Some((a, b, c, d)) = bounds else {
            self.zoom = 1.0;
            return;
        };
        let x = (b - a + grid).max(1.0);
        let y = (d - c + grid).max(1.0);
        let (w, h) = (f64::from(self.viewport.x), f64::from(self.viewport.y));
        self.zoom = ((w - 72.0) / (SCALE * x))
            .min((h - 72.0) / (SCALE * y))
            .clamp(MIN_ZOOM, 1.0);
    }

    /// Select a room, showing its area and floor (search results, badges, scenes).
    pub fn select_room(&mut self, tracker: &RoomMapTracker, id: &str) {
        let Some(room) = tracker.room(id) else {
            return;
        };
        let (area, z, x, y) = (room.area_key().to_string(), room.z, room.x, room.y);
        self.set_area(tracker, &area);
        self.set_floor(z);
        self.selected = Some(id.to_string());
        self.fit_floor = false;
        self.center_on_floor(tracker, Some((x, y)));
    }

    /// The C# `Refresh`: follow the current room's area and floor, keep the view centred on it
    /// while auto-center is on, drop a selection whose room is gone.
    fn refresh(&mut self, tracker: &RoomMapTracker, auto_center: bool) {
        let current = tracker
            .current()
            .map(|r| (r.id.clone(), r.area_key().to_string(), r.z, r.x, r.y));
        if !self.has_centered && tracker.room_count() > 0 {
            let area = match &current {
                Some(c) => c.1.clone(),
                None => tracker
                    .rooms()
                    .next()
                    .map(|r| r.area_key().to_string())
                    .unwrap_or_default(),
            };
            self.set_area(tracker, &area);
            let floor = current
                .as_ref()
                .map(|c| c.2)
                .or_else(|| floors(tracker, &self.area).first().copied())
                .unwrap_or(0.0);
            self.set_floor(floor);
            self.center_on_floor(tracker, current.as_ref().map(|c| (c.3, c.4)));
            self.has_centered = true;
            if current.is_none() {
                self.fit(tracker);
            }
        } else if let Some(c) = &current
            && (Some(c.2) != self.last_floor || Some(&c.1) != self.last_area.as_ref())
        {
            self.set_area(tracker, &c.1.clone());
            self.set_floor(c.2);
            self.center_on_floor(tracker, Some((c.3, c.4)));
        } else {
            let floors = floors(tracker, &self.area);
            if !floors.is_empty() && !floors.contains(&self.floor) {
                self.set_floor(floors[0]);
                self.center_on_floor(tracker, None);
            }
        }
        if auto_center
            && let Some(c) = &current
            && Some(&c.0) != self.last_centered.as_ref()
        {
            self.fit_floor = false;
            self.center_on_floor(tracker, Some((c.3, c.4)));
        }
        self.last_centered = current.as_ref().map(|c| c.0.clone());
        self.last_floor = current.as_ref().map(|c| c.2);
        self.last_area = current.as_ref().map(|c| c.1.clone());
        if self.fit_floor {
            self.fit(tracker);
        }
        if self.selected.as_deref().is_some_and(|id| tracker.room(id).is_none()) {
            self.selected = None;
        }
        self.update_route(tracker);
    }

    fn update_route(&mut self, tracker: &RoomMapTracker) {
        if let Some(to) = &self.route_destination {
            self.planned = tracker
                .current_id()
                .and_then(|from| find_route_live(tracker, from, to, self.allow_inferred));
        }
    }

    fn plan_route(&mut self, tracker: &RoomMapTracker) {
        self.route_destination = self.selected.clone();
        self.update_route(tracker);
    }

    fn clear_route(&mut self) {
        self.route_destination = None;
        self.planned = None;
    }

    /// The search's matches, ordered by name (cached per map version and query).
    fn search_matches(&mut self, tracker: &RoomMapTracker, version: u64) -> Vec<String> {
        if self.search_query.trim().chars().count() < 2 {
            return Vec::new();
        }
        if let Some((v, q, ids)) = &self.search_cache
            && *v == version
            && *q == self.search_query
        {
            return ids.clone();
        }
        let mut found = wandur_core::map::search::search(tracker.rooms(), &self.search_query);
        found.sort_by_cached_key(|r| r.name.to_lowercase());
        let ids: Vec<String> = found.into_iter().map(|r| r.id.clone()).collect();
        self.search_cache = Some((version, self.search_query.clone(), ids.clone()));
        ids
    }

    fn next_match(&mut self, tracker: &RoomMapTracker, version: u64) {
        let matches = self.search_matches(tracker, version);
        if matches.is_empty() {
            return;
        }
        let index = self.search_index.map_or(0, |i| (i + 1) % matches.len());
        self.search_index = Some(index);
        self.select_room(tracker, &matches[index]);
    }

    fn clear_search(&mut self) {
        self.search_query.clear();
        self.search_index = None;
    }

    fn reset_live(&mut self) {
        self.floor_index = FloorIndex::default();
        self.has_centered = false;
        self.last_centered = None;
        self.last_floor = None;
        self.last_area = None;
        self.selected = None;
        self.clear_route();
        self.seen_version = None;
    }
}

/// The floors of an area, lowest first.
fn floors(tracker: &RoomMapTracker, area: &str) -> Vec<f64> {
    let mut floors: Vec<f64> = tracker.rooms().filter(|r| r.area_key() == area).map(|r| r.z).collect();
    floors.sort_by(f64::total_cmp);
    floors.dedup();
    floors
}

/// The areas to choose from: every room's and every area setting's, plus no area.
fn areas(tracker: &RoomMapTracker) -> Vec<String> {
    let mut areas: Vec<String> = tracker
        .rooms()
        .map(|r| r.area_key().to_string())
        .chain(tracker.area_settings().map(|a| a.area.clone()))
        .chain(std::iter::once(String::new()))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    areas.sort_by_key(|a| a.to_lowercase());
    areas
}

fn area_label(area: &str) -> String {
    if area.is_empty() {
        t(S::MapUnassignedArea).into()
    } else {
        area.to_string()
    }
}

/// Room ids of the exercise map (as C#).
const LANDMARK: &str = "landmark";
const STAIRS: &str = "stairs";

/// The exercise's fixed map: three identical corridors north of each other, one apart, a
/// beacon beyond them and stairs elsewhere (the C# `ToggleExercise`).
fn exercise() -> Exercise {
    let corridor = |id: &str, x: f64, y: f64| {
        MapRoom::new(
            id,
            t(S::MapExerciseCorridor),
            t(S::MapExerciseCorridorDescription),
            None,
            x,
            y,
            0.0,
            false,
        )
    };
    let rooms = vec![
        corridor("a", 0.0, 0.0),
        corridor("b", 0.0, 1.0),
        corridor("c", 0.0, 2.0),
        corridor("d", 2.0, 0.0),
        MapRoom::new(
            LANDMARK,
            t(S::MapExerciseBeacon),
            t(S::MapExerciseBeaconDescription),
            None,
            0.0,
            3.0,
            0.0,
            false,
        ),
        MapRoom::new(
            STAIRS,
            t(S::MapExerciseStairs),
            t(S::MapExerciseStairsDescription),
            None,
            2.0,
            -1.0,
            1.0,
            false,
        ),
    ];
    let link = |a: &str, b: &str, d: &str| MapLink::new(a, b, d, true);
    let links = vec![
        link("a", "b", "north"),
        link("b", "a", "south"),
        link("b", "c", "north"),
        link("c", "b", "south"),
        link("c", LANDMARK, "north"),
        link(LANDMARK, "c", "south"),
        link("d", STAIRS, "south"),
        link(STAIRS, "d", "north"),
    ];
    let mut tracker = RoomMapTracker::from_snapshot(MapSnapshot {
        source: RoomSource::Text,
        ..MapSnapshot::of(rooms, links)
    });
    tracker.lose_position();
    tracker.observe(&exercise_corridor(), None);
    Exercise { tracker, step: 0 }
}

fn exercise_corridor() -> RoomObservation {
    RoomObservation::new(
        None,
        t(S::MapExerciseCorridor),
        t(S::MapExerciseCorridorDescription),
        &[],
    )
}

/// A small square icon button that can show as pressed (the C# toolbar toggles).
fn icon_button(ui: &mut Ui, icon: Icon, on: bool, theme: &Theme, tip: S) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(26.0, 24.0), Sense::click());
    crate::a11y::toggle(&response, egui::accesskit::Role::Button, t(tip), on);
    let painter = ui.painter();
    if on {
        painter.rect_filled(rect, 4.0, mix(theme.panel, theme.accent, 0.16));
        painter.rect_stroke(
            rect,
            4.0,
            Stroke::new(1.0, mix(theme.panel, theme.accent, 0.6)),
            StrokeKind::Inside,
        );
    } else if response.hovered() {
        painter.rect_filled(rect, 4.0, theme.hover_fill());
    }
    widgets::paint_icon(ui, icon, rect, theme.text);
    response.on_hover_text(t(tip))
}

pub const ACTION_SEARCH: &str = "MapSearch";
pub const ACTION_CENTER: &str = "MapAutoCenter";
pub const ACTION_FIT: &str = "MapFitFloor";
pub const ACTION_TOOLS: &str = "MapTools";
pub const ACTION_STOP: &str = "MapStopWalk";
pub const ACTION_EDIT: &str = "MapEdit";
pub const ACTION_OPEN_FULL: &str = "MapOpenFull";

/// The map's actions. The full map, as the C# header has them: room search, auto-center (a
/// toggle), fit the floor, Edit (a toggle), Map tools (not while editing), and Stop while
/// walking. The mini map: follow, Open full map, the menu, and Stop while walking.
pub fn header_actions(
    state: &MapViewState,
    auto_center: bool,
    walking: bool,
) -> Vec<crate::panel_header::HeaderAction> {
    use crate::panel_header::HeaderAction as A;
    if state.mini {
        let mut actions = vec![
            A::toggle(ACTION_CENTER, Icon::Crosshair, S::MapAutoCenter, auto_center),
            A::button(ACTION_OPEN_FULL, Icon::Expand, S::MapOpenFull),
            A::toggle(ACTION_TOOLS, Icon::Menu, S::MapToolsToggle, state.tools_open),
        ];
        if walking {
            actions.push(A::button(ACTION_STOP, Icon::Stop, S::MapStopWalk));
        }
        return actions;
    }
    let mut actions = vec![
        A::toggle(
            ACTION_SEARCH,
            Icon::Search,
            S::MapRoomSearchToggle,
            state.search_visible,
        ),
        A::toggle(ACTION_CENTER, Icon::Crosshair, S::MapAutoCenter, auto_center),
        A::button(ACTION_FIT, Icon::Fit, S::MapFitFloor),
    ];
    if state.editor.is_some() {
        actions.push(A::toggle(ACTION_EDIT, Icon::Edit, S::MapEditMode, state.is_editor()));
    }
    if !state.is_editor() {
        actions.push(A::toggle(ACTION_TOOLS, Icon::Menu, S::MapToolsToggle, state.tools_open));
    }
    if walking {
        actions.push(A::button(ACTION_STOP, Icon::Stop, S::MapStopWalk));
    }
    actions
}

/// The panel. `tab` is the active session, if any.
pub fn show(ui: &mut Ui, mut tab: Option<&mut SessionTab>, state: &mut MapViewState, cx: &mut MapContext) {
    let theme = cx.theme;
    let ink = Ink::of(theme);
    state.shown += 1;
    let mut intents: Vec<Intent> = Vec::new();
    if let (Some(prefs), Some(e)) = (cx.editor_prefs.as_deref(), state.editor.as_deref_mut())
        && e.prefs != *prefs
    {
        e.prefs = prefs.clone();
    }
    if let Some(e) = state.editor.as_deref_mut() {
        intents.extend(e.pending.drain(..).map(Intent::Edit));
    }
    let exercise_on = state.exercise.is_some();
    let exercise_taken = state.exercise.take();
    {
        let live = tab.as_deref();
        let tracker: Option<&RoomMapTracker> = match &exercise_taken {
            Some(e) => Some(&e.tracker),
            None => live.map(|t| t.map.tracker()),
        };
        let version = match (&exercise_taken, live) {
            (Some(e), _) => e.tracker.version(),
            (None, Some(t)) => t.map.version(),
            (None, None) => 0,
        };
        let walking = live.is_some_and(|t| t.map.is_walking());
        if let Some(tracker) = tracker
            && state.seen_version != Some(version)
        {
            state.seen_version = Some(version);
            state.refresh(tracker, *cx.auto_center);
        }
        if let (Some(tracker), Some(scene)) = (tracker, state.scene.clone())
            && tracker.room_count() > 0
            && state.viewport.x > 100.0
            && state.canvas != Rect::NOTHING
        {
            apply_scene(state, tracker, &scene, &mut intents);
            state.scene = None;
        }

        if let Some(tracker) = tracker {
            editor::sync(state, tracker);
        }
        // ---- toolbar (the panel's actions: in its header, or a row of their own) ----
        let actions = header_actions(state, *cx.auto_center, walking);
        let clicked = match crate::panel_header::place_actions(ui, &cx.actions_place, &actions, theme) {
            Some(id) => Some(id),
            None => state.header_click.take(),
        };
        if matches!(cx.actions_place, crate::panel_header::ActionsPlace::Bar) {
            ui.add_space(2.0);
        }
        match clicked {
            Some(ACTION_SEARCH) => {
                state.search_visible = !state.search_visible;
                if state.search_visible {
                    state.focus_search = true;
                } else {
                    state.clear_search();
                }
            }
            Some(ACTION_CENTER) => {
                *cx.auto_center = !*cx.auto_center;
                if *cx.auto_center
                    && let Some(tracker) = tracker
                    && let Some(c) = tracker.current()
                {
                    state.fit_floor = false;
                    state.center_on_floor(tracker, Some((c.x, c.y)));
                    state.last_centered = Some(c.id.clone());
                }
            }
            Some(ACTION_FIT) => {
                if let Some(tracker) = tracker {
                    state.fit(tracker);
                }
            }
            Some(ACTION_TOOLS) => state.tools_open = !state.tools_open,
            Some(ACTION_STOP) => intents.push(Intent::StopWalk),
            Some(ACTION_EDIT) => {
                let on = !state.is_editor();
                state.set_editing(on);
            }
            Some(ACTION_OPEN_FULL) => cx.open_full = true,
            _ => {}
        }
        // ---- search row ----
        let mut matches: Vec<String> = Vec::new();
        if state.search_visible {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let before = state.search_query.clone();
                let edit = egui::TextEdit::singleline(&mut state.search_query)
                    .hint_text(t(S::MapRoomSearchPlaceholder))
                    .font(FontId::proportional(11.5))
                    .desired_width(ui.available_width() - 28.0);
                let response = ui.add(edit);
                if state.focus_search {
                    response.request_focus();
                    state.focus_search = false;
                }
                if state.search_query != before {
                    state.search_index = None;
                }
                let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
                if let Some(tracker) = tracker {
                    if response.lost_focus() && enter {
                        state.next_match(tracker, version);
                        response.request_focus();
                    }
                    if (response.has_focus() || response.lost_focus()) && escape {
                        state.clear_search();
                    }
                }
                if icon_button(ui, Icon::ChevronRight, false, theme, S::MapRoomSearchNext).clicked()
                    && let Some(tracker) = tracker
                {
                    state.next_match(tracker, version);
                }
            });
            if let Some(tracker) = tracker {
                matches = state.search_matches(tracker, version);
            }
            if state.search_query.trim().chars().count() >= 2 {
                ui.label(
                    egui::RichText::new(tf(S::MapRoomSearchCount, &[&matches.len()]))
                        .size(10.0)
                        .color(theme.muted),
                );
            }
        }
        ui.add_space(2.0);

        // ---- canvas, status bar ----
        let feedback = state.walk_feedback || walking;
        let rest = ui.available_rect_before_wrap();
        // An empty map is a message on the panel, without the status line (C# UI review, item 8).
        let empty = tracker.is_none_or(|t| t.room_count() == 0);
        let status_height = if state.mini {
            // The mini map: a line of walking progress, nothing otherwise.
            if feedback { 22.0 } else { 0.0 }
        } else if empty && !feedback {
            0.0
        } else if feedback {
            40.0
        } else {
            24.0
        };
        let body = Rect::from_min_max(
            rest.min,
            pos2(rest.max.x, (rest.max.y - status_height).max(rest.min.y + 60.0)),
        );
        let status_rect = Rect::from_min_max(pos2(rest.min.x, body.max.y), rest.max);
        // Editing: the toolbar over the canvas, a 5-point splitter, the inspector.
        let (canvas, inspector) = match state.editor.as_deref().filter(|e| e.open) {
            Some(e) => {
                let width = inspector_width(e.prefs.inspector_width, body.width());
                let split = body.right() - width - 5.0;
                (
                    Rect::from_min_max(body.min, pos2(split, body.max.y)),
                    Some(Rect::from_min_max(pos2(split + 5.0, body.min.y), body.max)),
                )
            }
            None => (body, None),
        };
        let (toolbar_rect, canvas) = if inspector.is_some() {
            let height = editor::toolbar::HEIGHT.min(canvas.height() / 3.0);
            (
                Some(Rect::from_min_max(
                    canvas.min,
                    pos2(canvas.right(), canvas.top() + height),
                )),
                Rect::from_min_max(pos2(canvas.left(), canvas.top() + height), canvas.max),
            )
        } else {
            (None, canvas)
        };
        if let (Some(rect), Some(tracker)) = (toolbar_rect, tracker) {
            let out = editor::toolbar::show(ui, rect, tracker, state, theme);
            intents.extend(out.actions.into_iter().map(Intent::Edit));
            if out.delete
                && let Some(action) = editor::canvas::request_delete(state)
            {
                intents.push(Intent::Edit(action));
            }
            if out.fit {
                state.fit(tracker);
            }
            editor::publish(state);
        }
        state.viewport = canvas.size();
        state.canvas = canvas;
        let response = ui.allocate_rect(canvas, Sense::click_and_drag());
        crate::a11y::control(&response, egui::accesskit::Role::Canvas, t(S::Map));
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, CornerRadius::ZERO, ink.ground);
        match tracker {
            Some(tracker) if tracker.room_count() > 0 || (state.is_editor() && !exercise_on) => {
                let match_set: HashSet<&str> = matches.iter().map(String::as_str).collect();
                let search_active = state.search_visible && state.search_query.trim().chars().count() >= 2;
                let hits = paint(
                    &painter,
                    canvas,
                    tracker,
                    version,
                    state,
                    &ink,
                    search_active.then_some(&match_set),
                );
                interact(
                    ui,
                    &response,
                    canvas,
                    tracker,
                    state,
                    &hits,
                    exercise_on,
                    walking,
                    &mut intents,
                );
                if state.is_editor() && !exercise_on {
                    editor::canvas::paint_overlay(ui, tracker, state, &hits, theme);
                    let hint = editor::canvas::hint(state, tracker);
                    editor::toolbar::hint(ui, canvas, &hint, theme);
                    let mut edits = editor::canvas::keys(ui, state);
                    edits.extend(editor::canvas::context_menu(ui, tracker, state, theme));
                    edits.extend(editor::canvas::confirm_delete(ui, state, theme));
                    intents.extend(edits.into_iter().map(Intent::Edit));
                }
                // Matches on other floors, reachable without stepping through every match.
                if search_active {
                    let other: Vec<&MapRoom> = matches
                        .iter()
                        .filter_map(|id| tracker.room(id))
                        .filter(|r| r.area_key() != state.area || r.z != state.floor)
                        .collect();
                    if !other.is_empty() {
                        let ids: Vec<String> = other.iter().map(|r| r.id.clone()).collect();
                        let labels: Vec<String> = other
                            .iter()
                            .map(|r| {
                                format!(
                                    "{} · {} · {}",
                                    r.name,
                                    area_label(r.area_key()),
                                    tf(S::MapFloor, &[&number(r.z)])
                                )
                            })
                            .collect();
                        let panel = Rect::from_min_size(
                            canvas.min + vec2(8.0, 4.0),
                            vec2(260.0f32.min(canvas.width() - 16.0), 0.0),
                        );
                        let mut child = ui.new_child(
                            UiBuilder::new().max_rect(Rect::from_min_size(panel.min, vec2(panel.width(), 200.0))),
                        );
                        egui::Frame::new()
                            .fill(theme.panel)
                            .stroke(Stroke::new(1.0, theme.border))
                            .corner_radius(CornerRadius::same(4))
                            .inner_margin(egui::Margin::same(8))
                            .show(&mut child, |ui| {
                                ui.label(
                                    egui::RichText::new(t(S::MapRoomSearchOtherFloors))
                                        .size(10.0)
                                        .color(theme.muted),
                                );
                                egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                                    for (id, label) in ids.iter().zip(&labels) {
                                        if ui
                                            .add(
                                                egui::Button::selectable(false, egui::RichText::new(label).size(11.0))
                                                    .truncate(),
                                            )
                                            .clicked()
                                        {
                                            state.select_room(tracker, id);
                                        }
                                    }
                                });
                            });
                    }
                }
            }
            _ => {
                state.painted = 0;
                let color = mix(
                    ink.ground,
                    if is_light(ink.ground) {
                        Color32::BLACK
                    } else {
                        Color32::WHITE
                    },
                    0.72,
                );
                let galley = widgets::clipped(
                    ui,
                    t(S::MapEmpty),
                    FontId::proportional(13.0),
                    color,
                    canvas.width() - 36.0,
                    4,
                );
                painter.galley(canvas.center() - galley.size() / 2.0, galley, color);
            }
        }
        // ---- Map tools ----
        if state.tools_open {
            let width = 360.0f32.min(canvas.width() - 12.0).max(120.0);
            let area = Rect::from_min_max(
                pos2(canvas.right() - 6.0 - width, canvas.top() + 6.0),
                pos2(canvas.right() - 6.0, canvas.bottom() - 6.0),
            );
            let mut child = ui.new_child(UiBuilder::new().max_rect(area));
            egui::Frame::new()
                .fill(theme.panel)
                .stroke(Stroke::new(1.0, theme.border))
                .corner_radius(CornerRadius::same(4))
                .show(&mut child, |ui| {
                    ui.set_min_size(area.size());
                    let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
                    if state.scroll_tools_to_end {
                        scroll = scroll.vertical_scroll_offset(10_000.0);
                    }
                    scroll.show(ui, |ui| {
                        egui::Frame::new().inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 8.0;
                            tools(
                                ui,
                                tracker,
                                live,
                                state,
                                cx,
                                exercise_taken.as_ref(),
                                false,
                                &mut intents,
                            );
                        });
                    });
                    state.scroll_tools_to_end = false;
                });
        }
        // ---- the editor's inspector ----
        if let Some(inspector) = inspector {
            let splitter = Rect::from_min_max(pos2(canvas.right(), body.top()), pos2(inspector.left(), body.bottom()));
            let drag = ui.interact(splitter, ui.id().with("map-editor-splitter"), Sense::drag());
            crate::a11y::control(&drag, egui::accesskit::Role::Splitter, t(S::MapEditorTitle));
            if drag.hovered() || drag.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            if drag.dragged()
                && let Some(e) = state.editor.as_deref_mut()
            {
                let width = inspector_width(e.prefs.inspector_width, body.width()) - drag.drag_delta().x;
                e.prefs.inspector_width = inspector_width(width, body.width());
            }
            ui.painter().rect_filled(splitter, CornerRadius::ZERO, theme.shell);
            if let Some(tracker) = tracker.filter(|_| !exercise_on) {
                let edits = editor::inspector::show(ui, inspector, tracker, state, theme);
                intents.extend(edits.into_iter().map(Intent::Edit));
                editor::publish(state);
            } else {
                ui.painter().rect_filled(inspector, CornerRadius::ZERO, theme.panel);
            }
            ui.painter()
                .vline(inspector.left(), inspector.y_range(), Stroke::new(1.0, theme.border));
        }
        // The Map page's Escape goes back to Play unless the editor wants it (read next frame).
        if !state.mini {
            let keeps = state.editor.as_deref().is_some_and(editor::EditorState::wants_escape);
            ui.ctx().data_mut(|d| d.insert_temp(map_keeps_escape_id(), keeps));
        }
        if let (Some(prefs), Some(e)) = (cx.editor_prefs.as_deref_mut(), state.editor.as_deref())
            && *prefs != e.prefs
        {
            *prefs = e.prefs.clone();
        }
        // ---- status bar ----
        let mut status = ui.new_child(UiBuilder::new().max_rect(status_rect));
        if status_rect.height() > 0.0 {
            status
                .painter()
                .rect_filled(status_rect, CornerRadius::ZERO, theme.shell);
            status
                .painter()
                .hline(status_rect.x_range(), status_rect.top(), Stroke::new(1.0, theme.border));
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(8, 3))
                .show(&mut status, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    if feedback {
                        let text = if state.route_unavailable {
                            t(S::MapRouteUnavailable).to_string()
                        } else {
                            live.map_or_else(|| t(S::MapWalkUnavailable).to_string(), |t| t.map.walk_status().text())
                        };
                        ui.add(egui::Label::new(egui::RichText::new(text).size(10.0).color(theme.muted)).truncate());
                    }
                    if state.mini {
                        return;
                    }
                    ui.horizontal(|ui| {
                        let summary = live.map_or_else(
                            || wandur_core::map::ProtocolEvidence::default().summary(),
                            |t| t.map.evidence.summary(),
                        );
                        let short = live.map_or_else(String::new, |t| t.map.evidence.short());
                        let fields = live.map_or_else(
                            || wandur_core::map::ProtocolEvidence::default().room_fields(),
                            |t| t.map.evidence.room_fields(),
                        );
                        let zoom_width = 128.0;
                        let label_width = (ui.available_width() - zoom_width).max(40.0);
                        let status_label = ui
                            .allocate_ui_with_layout(
                                vec2(label_width, 16.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_width(label_width);
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(short).size(10.0).color(theme.muted))
                                            .truncate()
                                            .sense(Sense::hover()),
                                    )
                                },
                            )
                            .inner;
                        // Named by the full summary (the short line can be empty).
                        let label = summary.clone();
                        status_label.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &label));
                        status_label.on_hover_ui(|ui| {
                            ui.label(egui::RichText::new(&summary).size(11.0));
                            ui.label(egui::RichText::new(&fields).size(11.0));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.label(egui::RichText::new("+").size(11.0).color(theme.muted));
                            ui.spacing_mut().slider_width = 92.0;
                            let mut zoom = state.zoom;
                            let slider = ui
                                .add(egui::Slider::new(&mut zoom, MIN_ZOOM..=MAX_ZOOM).show_value(false))
                                .on_hover_text(t(S::MapZoom));
                            crate::a11y::label(&slider, t(S::MapZoom));
                            if slider.changed() {
                                let anchor = canvas.center();
                                state.zoom_at(canvas, zoom, anchor);
                            }
                            ui.label(egui::RichText::new("-").size(11.0).color(theme.muted));
                        });
                    });
                });
        }
    }
    state.exercise = exercise_taken;
    // ---- what the clicks asked for ----
    for intent in intents {
        match intent {
            Intent::Walk(route) => {
                state.walk_requested += 1;
                if let Some(tab) = tab.as_deref_mut()
                    && state.exercise.is_none()
                {
                    tab.walk_route(&route);
                }
            }
            Intent::StopWalk => {
                if let Some(tab) = tab.as_deref_mut() {
                    tab.stop_walk();
                }
            }
            Intent::Recheck => {
                if let Some(tab) = tab.as_deref_mut() {
                    tab.map.tracker_mut().lose_position();
                }
            }
            Intent::Grid(area, on) => match &mut state.exercise {
                Some(e) => e.tracker.set_area_settings(MapAreaSettings::new(&area, on)),
                None => {
                    if let Some(tab) = tab.as_deref_mut() {
                        tab.map.tracker_mut().set_area_settings(MapAreaSettings::new(&area, on));
                    }
                }
            },
            Intent::ToggleExercise => {
                if let Some(tab) = tab.as_deref_mut() {
                    tab.stop_walk();
                }
                state.exercise = if state.exercise.is_some() {
                    None
                } else {
                    Some(exercise())
                };
                state.reset_live();
            }
            Intent::Edit(action) => {
                if let Some(tab) = tab.as_deref_mut()
                    && state.exercise.is_none()
                {
                    if matches!(action, editor::EditAction::Import(_)) {
                        tab.stop_walk();
                    }
                    let outcome = editor::apply(state, tab.map.tracker_mut(), action);
                    if outcome.deleted.is_some() {
                        cx.deleted = outcome.deleted;
                    }
                    cx.open_import |= outcome.open_import;
                    if outcome.stop_walk {
                        tab.stop_walk();
                    }
                    if outcome.rescan
                        && let Some(inference) = &mut tab.inference
                    {
                        inference.rescan();
                    }
                }
            }
            Intent::NextExercise => {
                if let Some(e) = &mut state.exercise
                    && e.step < 3
                {
                    e.step += 1;
                    let observation = if e.step == 3 {
                        RoomObservation::new(None, t(S::MapExerciseBeacon), t(S::MapExerciseBeaconDescription), &[])
                    } else {
                        exercise_corridor()
                    };
                    e.tracker.observe(&observation, Some("north"));
                }
            }
        }
    }
}

/// The inspector's width for a page `available` wide: the remembered width, leaving the canvas
/// at least 240 points.
fn inspector_width(width: f32, available: f32) -> f32 {
    let (lo, hi) = (
        *wandur_core::settings::MAP_INSPECTOR_WIDTHS.start(),
        *wandur_core::settings::MAP_INSPECTOR_WIDTHS.end(),
    );
    width.clamp(lo, hi.min((available - 245.0).max(lo)))
}

/// Where the full map says whether its editor keeps Escape (a temporary egui value).
pub fn map_keeps_escape_id() -> egui::Id {
    egui::Id::new("wandur-map-keeps-escape")
}

/// A direction as the editor shows it (the command word itself).
pub fn direction_label(direction: &str) -> String {
    direction.to_string()
}

/// Set up a capture's screen (the C# `MapScenes`).
fn apply_scene(state: &mut MapViewState, tracker: &RoomMapTracker, scene: &MapScene, intents: &mut Vec<Intent>) {
    let named = |name: &str| tracker.rooms().find(|r| r.name == name).map(|r| r.id.clone());
    match scene {
        MapScene::Search(text) => {
            state.search_visible = true;
            state.search_query = text.clone();
            state.focus_search = true;
        }
        MapScene::Grid => {
            intents.push(Intent::Grid(state.area.clone(), true));
            state.fit_floor = true;
        }
        MapScene::Tools => state.tools_open = true,
        MapScene::Route(name) => {
            if let Some(id) = named(name) {
                state.select_room(tracker, &id);
                state.plan_route(tracker);
            }
            state.tools_open = true;
            state.route_open = true;
            state.scroll_tools_to_end = true;
            state.fit(tracker);
            // Keep the rooms clear of the tools panel, as the C# capture does.
            let covered = 360.0f32.min(state.viewport.x - 12.0) + 12.0;
            let rect = Rect::from_min_size(Pos2::ZERO, state.viewport);
            let zoom = state.zoom * f64::from((state.viewport.x - covered) / state.viewport.x);
            state.zoom_at(rect, zoom, rect.center());
            state.pan.x -= covered / 2.0;
        }
        MapScene::Full(name) => {
            if let Some(id) = named(name) {
                state.select_room(tracker, &id);
            }
            state.fit(tracker);
        }
        MapScene::Fit => state.fit(tracker),
        MapScene::Area(id) => {
            state.select_room(tracker, id);
            state.selected = None;
            state.fit(tracker);
        }
        MapScene::Zoom(zoom) => {
            state.fit_floor = false;
            state.zoom = *zoom;
            let at = tracker.current().map(|r| (r.x, r.y));
            state.center_on_floor(tracker, at);
        }
        MapScene::EditorLabel(id) => {
            if let Some(label) = tracker.label(id) {
                let (area, z) = (label.area_key().to_string(), label.z);
                state.set_area(tracker, &area);
                state.set_floor(z);
            }
            state.fit(tracker);
            state.selected = None;
            editor::sync(state, tracker);
            if let Some(e) = state.editor.as_deref_mut() {
                e.select_label(id);
            }
            editor::publish(state);
        }
        MapScene::Editor => {
            state.fit(tracker);
            state.selected = None;
            editor::sync(state, tracker);
        }
        MapScene::EditorRoom {
            name,
            description,
            notes,
        } => {
            // The floor fitted, then the room selected with text typed into its notes.
            state.fit(tracker);
            if let Some(id) = named(name) {
                state.selected = Some(id);
            }
            editor::sync(state, tracker);
            if let Some(e) = state.editor.as_deref_mut() {
                if !description.is_empty() {
                    e.drafts.insert("description".into(), description.clone());
                }
                if !notes.is_empty() {
                    e.drafts.insert("notes".into(), notes.clone());
                }
            }
        }
        MapScene::EditorRooms(names) => {
            state.fit(tracker);
            let ids: Vec<String> = names.iter().filter_map(|n| named(n)).collect();
            if let Some(e) = state.editor.as_deref_mut() {
                e.rooms = ids;
                e.exit = None;
            }
            editor::publish(state);
        }
        MapScene::EditorExit { from, direction } => {
            state.fit(tracker);
            if let Some(id) = named(from)
                && let Some(e) = state.editor.as_deref_mut()
            {
                e.select_exit(&id, direction);
            }
            editor::publish(state);
        }
        MapScene::EditorConnect { from, toward } => {
            state.fit(tracker);
            if let (Some(a), Some(b)) = (named(from), named(toward))
                && let Some(room) = tracker.room(&b)
            {
                // Short of the target, as a drag would be on its way there.
                let to = state.project(state.canvas, room.x - 0.08, room.y + 0.12);
                if let Some(e) = state.editor.as_deref_mut() {
                    e.tool = editor::Tool::Connect;
                    e.select_only(Some(&a));
                    e.gesture = editor::Gesture::Connect {
                        from: a,
                        to,
                        target: Some(b),
                    };
                }
            }
            editor::publish(state);
        }
        MapScene::EditorMenu(name) => {
            state.fit(tracker);
            if let Some(id) = named(name)
                && let Some(room) = tracker.room(&id)
            {
                let at = state.project(state.canvas, room.x, room.y) + vec2(6.0, 6.0);
                if let Some(e) = state.editor.as_deref_mut() {
                    e.select_only(Some(&id));
                    e.menu = Some(editor::ContextMenu {
                        at,
                        target: editor::Target::Room(id.clone()),
                        age: 0,
                    });
                }
            }
            editor::publish(state);
        }
    }
}

/// Where a room was drawn, for clicks and the tooltip.
pub struct Hits {
    pub rooms: Vec<(String, Rect)>,
    pub badges: Vec<(String, Rect)>,
    /// Exits drawn while editing: from, direction and the line's ends.
    pub links: Vec<(String, String, Pos2, Pos2)>,
    /// Labels of the floor, under the rooms first, then those above them.
    pub labels: Vec<(String, Rect)>,
}

#[allow(clippy::too_many_arguments)]
fn interact(
    ui: &Ui,
    response: &egui::Response,
    canvas: Rect,
    tracker: &RoomMapTracker,
    state: &mut MapViewState,
    hits: &Hits,
    exercise: bool,
    walking: bool,
    intents: &mut Vec<Intent>,
) {
    let editing = state.is_editor() && !exercise;
    if editing {
        // The editor's tools: selection, moves, adding, connecting, the context menu.
        let edits = editor::canvas::interact(ui, response, tracker, state, hits);
        intents.extend(edits.into_iter().map(Intent::Edit));
    } else if response.dragged() {
        state.fit_floor = false;
        state.pan += response.drag_delta();
    }
    if response.hovered() {
        let (scroll, pinch, pointer) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.pointer.hover_pos()));
        let anchor = pointer.unwrap_or(canvas.center());
        if scroll != 0.0 {
            // A wheel notch is about 50 points; each zooms by 1.12 (the C# step).
            let steps = f64::from(scroll / 50.0).clamp(-100.0, 100.0);
            state.zoom_at(canvas, state.zoom * 1.12f64.powf(steps), anchor);
        }
        if (pinch - 1.0).abs() > 1e-4 {
            state.zoom_at(canvas, state.zoom * f64::from(pinch), anchor);
        }
    }
    let pointer = response.interact_pointer_pos().or(response.hover_pos());
    if editing {
        return;
    }
    if response.clicked()
        && let Some(at) = pointer
    {
        if let Some((id, _)) = hits.badges.iter().rev().find(|(_, r)| r.contains(at)) {
            // A floor badge shows the other floor; it never sends a command.
            state.last_clicked = None;
            state.select_room(tracker, id);
        } else {
            let hit = hits
                .rooms
                .iter()
                .rev()
                .find(|(_, r)| r.contains(at))
                .map(|(id, _)| id.clone());
            state.selected = hit.clone();
            if response.double_clicked() && hit.is_some() && hit == state.last_clicked {
                walk_to(
                    state,
                    tracker,
                    hit.as_deref().unwrap_or_default(),
                    exercise,
                    walking,
                    intents,
                );
            }
            state.last_clicked = hit;
        }
    }
    if let Some(at) = response.hover_pos()
        && let Some((id, _)) = hits.rooms.iter().rev().find(|(_, r)| r.contains(at))
        && let Some(room) = tracker.room(id)
    {
        let mut text = format!(
            "{}\n{} · ({}, {}, {})",
            room.name,
            crate::map_palette::describe(room),
            number(room.x),
            number(room.y),
            number(room.z)
        );
        if !room.notes.is_empty() {
            text.push('\n');
            text.push_str(&room.notes);
        }
        response.clone().on_hover_text(text);
    }
}

/// A double click on a room: plan the verified route there and walk it.
fn walk_to(
    state: &mut MapViewState,
    tracker: &RoomMapTracker,
    id: &str,
    exercise: bool,
    walking: bool,
    intents: &mut Vec<Intent>,
) {
    if exercise || walking || tracker.room(id).is_none() {
        return;
    }
    state.selected = Some(id.to_string());
    state.route_destination = Some(id.to_string());
    state.planned = tracker
        .current_id()
        .and_then(|from| find_route_live(tracker, from, id, false));
    state.walk_feedback = true;
    state.route_unavailable = state.planned.is_none();
    if let Some(route) = &state.planned {
        intents.push(Intent::Walk(route.clone()));
    }
}

fn dashed_rect(painter: &egui::Painter, r: Rect, stroke: Stroke) {
    let corners = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    painter.extend(Shape::dashed_line(&corners, stroke, 3.0, 2.0));
}

fn arrow(shapes: &mut Vec<Shape>, tip: Pos2, unit: Vec2, color: Color32) {
    let side = vec2(-unit.y, unit.x);
    shapes.push(Shape::line(
        vec![tip - unit * 6.0 + side * 3.0, tip, tip - unit * 6.0 - side * 3.0],
        Stroke::new(1.8, color),
    ));
}

fn direction_vector(direction: &str) -> Option<Vec2> {
    Some(match direction {
        "north" => vec2(0.0, -1.0),
        "south" => vec2(0.0, 1.0),
        "east" => vec2(1.0, 0.0),
        "west" => vec2(-1.0, 0.0),
        "northeast" => vec2(0.707, -0.707),
        "northwest" => vec2(-0.707, -0.707),
        "southeast" => vec2(0.707, 0.707),
        "southwest" => vec2(-0.707, 0.707),
        _ => return None,
    })
}

/// What one floor needs drawn, by tracker index, kept while the map, area and floor stay the
/// same (rebuilt when the player moves, not every frame).
#[derive(Debug, Default)]
struct FloorIndex {
    key: Option<(u64, String, u64)>,
    /// Rooms on the floor (tracker indices).
    rooms: Vec<usize>,
    /// Exits to draw, each pair of rooms once: the exit, its ends (positions in `rooms`) and
    /// the way back if there is one.
    links: Vec<(usize, usize, usize, Option<usize>)>,
    /// Each floor room's exits (tracker link indices).
    outgoing: Vec<Vec<usize>>,
    /// Each floor room's destinations on another floor or in another area, lowest floor first.
    elsewhere: Vec<Vec<usize>>,
    /// Each floor room's up and down exits that lead to a mapped room.
    vertical: Vec<(bool, bool)>,
}

impl FloorIndex {
    fn build(tracker: &RoomMapTracker, area: &str, floor: f64, key: (u64, String, u64)) -> Self {
        let mut index = FloorIndex {
            key: Some(key),
            ..Default::default()
        };
        let mut position: HashMap<&str, usize> = HashMap::new();
        for (i, room) in tracker.rooms().enumerate() {
            if room.area_key() == area && room.z == floor {
                position.insert(&room.id, index.rooms.len());
                index.rooms.push(i);
            }
        }
        let n = index.rooms.len();
        index.outgoing = vec![Vec::new(); n];
        index.elsewhere = vec![Vec::new(); n];
        index.vertical = vec![(false, false); n];
        let mut by_ends: HashMap<(&str, &str), usize> = HashMap::new();
        for (i, link) in tracker.links().enumerate() {
            by_ends.entry((link.from_id.as_str(), link.to_id.as_str())).or_insert(i);
        }
        let mut drawn: HashSet<(usize, usize)> = HashSet::new();
        for (i, link) in tracker.links().enumerate() {
            let Some(&from) = position.get(link.from_id.as_str()) else {
                continue;
            };
            index.outgoing[from].push(i);
            let target = tracker
                .room_index(&link.to_id)
                .and_then(|t| tracker.room_at(t).map(|r| (t, r)));
            if let Some((t, target_room)) = target {
                let room = tracker.room_at(index.rooms[from]).expect("indexed");
                if target_room.z != room.z || target_room.area != room.area {
                    index.elsewhere[from].push(t);
                }
                match link.direction.as_str() {
                    "up" => index.vertical[from].0 = true,
                    "down" => index.vertical[from].1 = true,
                    _ => {}
                }
            }
            if let Some(&to) = position.get(link.to_id.as_str())
                && !drawn.contains(&(to, from))
                && drawn.insert((from, to))
            {
                let reverse = by_ends.get(&(link.to_id.as_str(), link.from_id.as_str())).copied();
                index.links.push((i, from, to, reverse));
            }
        }
        for list in &mut index.elsewhere {
            list.sort_by(|a, b| {
                let z = |i: &usize| tracker.room_at(*i).map_or(0.0, |r| r.z);
                z(a).total_cmp(&z(b))
            });
            list.dedup();
        }
        index
    }
}

/// Draw the floor (the C# `RoomMapControl.Render`).
fn paint(
    painter: &egui::Painter,
    canvas: Rect,
    tracker: &RoomMapTracker,
    version: u64,
    state: &mut MapViewState,
    ink: &Ink,
    search: Option<&HashSet<&str>>,
) -> Hits {
    let mut hits = Hits {
        rooms: Vec::new(),
        badges: Vec::new(),
        links: Vec::new(),
        labels: Vec::new(),
    };
    // The editor's look: selection, hover, rooms being moved, a symbol being typed.
    let edit = state.editor.as_deref().filter(|e| e.open);
    let edit_selected: HashSet<&str> = edit
        .map(|e| e.rooms.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let edit_hover = edit.and_then(|e| match &e.hover {
        Some(editor::Target::Room(id)) => Some(id.as_str()),
        _ => None,
    });
    let edit_offset = editor::canvas::move_preview(state);
    let edit_symbol = edit.and_then(|e| e.symbol_preview.as_deref());
    let key = (version, state.area.clone(), state.floor.to_bits());
    let stale = state.floor_index.rooms.iter().any(|&i| i >= tracker.room_count())
        || state.floor_index.links.iter().any(|l| l.0 >= tracker.link_count());
    if state.floor_index.key.as_ref() != Some(&key) || stale {
        state.floor_index = FloorIndex::build(tracker, &state.area, state.floor, key);
    }
    let index = std::mem::take(&mut state.floor_index);
    let scale = state.scale() as f32;
    let grid_mode = tracker.grid_mode(&state.area);
    let half = if grid_mode {
        scale / 2.0
    } else {
        (scale * 0.16).clamp(4.0, 26.0)
    };
    // The grid: integer coordinates are room centres, half coordinates cell edges.
    let origin = state.project(canvas, -0.5, 0.5);
    let grid = Stroke::new(1.0, ink.grid);
    if scale >= 3.0 {
        let mut x = canvas.left() + (origin.x - canvas.left()).rem_euclid(scale);
        while x < canvas.right() {
            painter.vline(x, canvas.y_range(), grid);
            x += scale;
        }
        let mut y = canvas.top() + (origin.y - canvas.top()).rem_euclid(scale);
        while y < canvas.bottom() {
            painter.hline(canvas.x_range(), y, grid);
            y += scale;
        }
    }
    // Labels under the rooms (and their exits).
    let label_preview = editor::canvas::label_preview(state, tracker);
    let label_ink = if is_light(ink.ground) {
        Color32::from_rgb(0x1B, 0x2A, 0x33)
    } else {
        Color32::from_rgb(0xE6, 0xEE, 0xF2)
    };
    if tracker.label_count() > 0 {
        labels::paint(
            painter,
            canvas,
            tracker,
            state,
            false,
            label_ink,
            label_preview.as_ref().map(|(id, l)| (id.as_str(), l.clone())),
            &mut hits.labels,
        );
    }
    let room_at = |i: usize| tracker.room_at(index.rooms[i]).expect("indexed room");
    let positions: Vec<Pos2> = (0..index.rooms.len())
        .map(|i| {
            let r = room_at(i);
            match edit_offset {
                // Rooms being moved follow the pointer (snapped) until they are dropped.
                Some((dx, dy)) if edit_selected.contains(r.id.as_str()) && !r.is_locked => {
                    state.project(canvas, r.x + dx, r.y + dy)
                }
                _ => state.project(canvas, r.x, r.y),
            }
        })
        .collect();
    let planned: HashSet<(&str, &str)> = state
        .planned
        .as_ref()
        .map(|r| r.steps.iter().map(|s| (s.from_id.as_str(), s.to_id.as_str())).collect())
        .unwrap_or_default();
    let candidates: HashSet<String> = tracker.candidates().into_iter().collect();
    let current = tracker.current_id();
    let confirmed = tracker.state() == TrackingState::Confirmed;
    let margin = canvas.expand2(vec2(half + 120.0, half + 80.0));
    let mut reserved: Vec<Rect> = Vec::new();
    let mut shapes: Vec<Shape> = Vec::new();
    let labels = !grid_mode && state.zoom >= 0.65;
    if labels || state.selected.is_some() || current.is_some() {
        for &p in &positions {
            if canvas.contains(p) {
                reserved.push(Rect::from_center_size(p, Vec2::splat(half * 2.0 + 4.0)));
            }
        }
    }
    // Exits between rooms of this floor, each pair once.
    for &(li, from, to, reverse) in &index.links {
        if grid_mode {
            break;
        }
        let link = tracker.link_at(li).expect("indexed link");
        let (start0, end0) = (positions[from], positions[to]);
        if !margin.contains(start0) && !margin.contains(end0) && !canvas.intersects(Rect::from_two_pos(start0, end0)) {
            continue;
        }
        let delta = end0 - start0;
        if delta.length() < 1.0 {
            continue;
        }
        let reverse_index = reverse;
        let reverse = reverse.and_then(|r| tracker.link_at(r));
        let unit = delta / delta.length();
        let trim = (delta.length() / 3.0).min(half / unit.x.abs().max(unit.y.abs()) + 3.0);
        let (start, end) = (start0 + unit * trim, end0 - unit * trim);
        if edit.is_some() {
            // Each half of a two-way line picks its own exit: the half nearer its origin.
            let middle = start + (end - start) / 2.0;
            match reverse {
                Some(back) if reverse_index != Some(li) => {
                    hits.links
                        .push((link.from_id.clone(), link.direction.clone(), start, middle));
                    hits.links
                        .push((back.from_id.clone(), back.direction.clone(), end, middle));
                }
                _ => hits
                    .links
                    .push((link.from_id.clone(), link.direction.clone(), start, end)),
            }
        }
        let on_route = planned.contains(&(link.from_id.as_str(), link.to_id.as_str()))
            || planned.contains(&(link.to_id.as_str(), link.from_id.as_str()));
        let locked = link.is_locked || link.door_state == wandur_core::map::DoorState::Locked;
        let color = if on_route {
            ink.amber
        } else if locked {
            ink.red
        } else {
            ink.route
        };
        if !link.line_points.is_empty() {
            let mut points = vec![start0];
            points.extend(link.line_points.iter().map(|p| state.project(canvas, p.x, p.y)));
            points.push(end0);
            let stroke = Stroke::new(if on_route { 3.0 } else { 1.8 }, color);
            for pair in points.windows(2) {
                if link.confirmed {
                    shapes.push(Shape::line_segment([pair[0], pair[1]], stroke));
                } else {
                    shapes.extend(Shape::dashed_line(&[pair[0], pair[1]], stroke, 4.0, 3.0));
                }
                reserved.push(Rect::from_two_pos(pair[0], pair[1]).expand(3.0));
            }
            let n = points.len();
            let last = points[n - 1] - points[n - 2];
            if last.length() > 1.0 {
                let u = last / last.length();
                arrow(&mut shapes, points[n - 1] - u * (half + 3.0), u, color);
            }
            let first = points[1] - points[0];
            if reverse.is_some() && first.length() > 1.0 {
                let u = first / first.length();
                arrow(&mut shapes, points[0] + u * (half + 3.0), -u, color);
            }
            continue;
        }
        // Each half keeps its own evidence when the way back is only inferred.
        let middle = start + (end - start) / 2.0;
        let stroke = Stroke::new(1.8, color);
        let first_confirmed = reverse.map_or(link.confirmed, |r| r.confirmed);
        if first_confirmed && link.confirmed {
            shapes.push(Shape::line_segment([start, end], stroke));
        } else {
            for (a, b, solid) in [(start, middle, first_confirmed), (middle, end, link.confirmed)] {
                if solid {
                    shapes.push(Shape::line_segment([a, b], stroke));
                } else {
                    shapes.extend(Shape::dashed_line(&[a, b], stroke, 4.0, 3.0));
                }
            }
        }
        // Arrowheads only where the line is long enough to carry them (zoomed far out they
        // would be bigger than the line).
        if (end - start).length() >= 10.0 {
            arrow(&mut shapes, end, unit, color);
            if reverse.is_some() {
                arrow(&mut shapes, start, -unit, color);
            }
        }
        let door = |exit: Option<&MapLink>, at: Pos2, shapes: &mut Vec<Shape>| {
            let Some(exit) = exit else { return };
            if exit.door_state == wandur_core::map::DoorState::None && !exit.is_locked {
                return;
            }
            let side = vec2(-unit.y, unit.x);
            let locked = exit.is_locked || exit.door_state == wandur_core::map::DoorState::Locked;
            let open = exit.door_state == wandur_core::map::DoorState::Open && !exit.is_locked;
            let c = if locked { ink.red } else { color };
            shapes.push(Shape::line_segment(
                [at - side * 5.0, at + side * 5.0],
                Stroke::new(if open { 1.0 } else { 4.0 }, c),
            ));
        };
        door(Some(link), start + (end - start) * 0.65, &mut shapes);
        door(reverse, start + (end - start) * 0.35, &mut shapes);
        if labels {
            reserved.push(Rect::from_two_pos(start, end).expand(4.0));
        }
    }
    painter.extend(std::mem::take(&mut shapes));

    // Rooms.
    let mut painted = 0;
    let label_font = FontId::proportional(10.0);
    let selected = state.selected.clone();
    for (i, &p) in positions.iter().enumerate() {
        if !margin.contains(p) {
            continue;
        }
        let room = room_at(i);
        painted += 1;
        let is_current = current == Some(room.id.as_str());
        let candidate = !candidates.is_empty() && candidates.contains(&room.id);
        let is_selected = if edit.is_some() {
            edit_selected.contains(room.id.as_str())
        } else {
            selected.as_deref() == Some(room.id.as_str())
        };
        let accent = if is_current {
            ink.mint
        } else if candidate {
            ink.amber
        } else {
            ink.gray
        };
        let bounds = Rect::from_center_size(p, Vec2::splat(half * 2.0));
        let (fill, symbol) = crate::map_palette::resolve(room);
        let inferred = room.provisional || (is_current && !confirmed);
        let is_match = search.is_some_and(|m| m.contains(room.id.as_str()));
        let fade = search.is_some() && !is_match;
        let f = |c: Color32| if fade { c.gamma_multiply(0.35) } else { c };
        if grid_mode {
            painter.rect(
                bounds,
                0.0,
                f(fill),
                Stroke::new(0.5, f(ink.ground)),
                StrokeKind::Middle,
            );
        } else if inferred {
            painter.rect_filled(bounds, 0.0, f(fill));
            dashed_rect(painter, bounds, Stroke::new(1.3, f(accent)));
        } else {
            painter.rect(bounds, 0.0, f(fill), Stroke::new(1.3, f(accent)), StrokeKind::Middle);
        }
        if is_match {
            let ring = if grid_mode {
                bounds.expand(1.0)
            } else {
                bounds.expand(if is_selected { 8.0 } else { 4.0 })
            };
            painter.rect_stroke(ring, 0.0, Stroke::new(2.5, ink.search), StrokeKind::Middle);
        }
        if edit.is_some() {
            // Editing: the selection in the accent, the hovered room lightly.
            let ring = if grid_mode {
                bounds.shrink(2.0)
            } else {
                bounds.expand(4.0)
            };
            if is_selected {
                painter.rect_stroke(
                    ring.expand(2.5),
                    5.0,
                    Stroke::new(4.0, ink.search.gamma_multiply(0.22)),
                    StrokeKind::Middle,
                );
                painter.rect_stroke(ring, 3.0, Stroke::new(2.0, ink.search), StrokeKind::Middle);
            } else if edit_hover == Some(room.id.as_str()) {
                painter.rect_stroke(
                    ring,
                    3.0,
                    Stroke::new(1.5, ink.search.gamma_multiply(0.55)),
                    StrokeKind::Middle,
                );
            }
        } else if is_selected {
            let ring = if grid_mode {
                bounds.shrink(2.0)
            } else {
                bounds.expand(4.0)
            };
            painter.rect_stroke(ring, 0.0, Stroke::new(2.0, ink.select), StrokeKind::Middle);
        }
        let symbol = match edit_symbol {
            Some(preview) if is_selected => std::borrow::Cow::Owned(preview.to_string()),
            _ => symbol,
        };
        if !symbol.trim().is_empty() && half >= 8.0 && !is_current {
            let size = half.clamp(10.0, 18.0);
            if symbol == "△" {
                // Drawn, so it never depends on a font having the glyph.
                let r = size * 0.42;
                let tri = vec![
                    p + vec2(0.0, -r),
                    p + vec2(r * 0.95, r * 0.7),
                    p + vec2(-r * 0.95, r * 0.7),
                ];
                painter.add(Shape::closed_line(tri, Stroke::new(1.2, f(ink.ground))));
            } else {
                painter.text(
                    p,
                    Align2::CENTER_CENTER,
                    &symbol,
                    FontId::proportional(size),
                    f(ink.ground),
                );
            }
        }
        if room.is_locked && half >= 8.0 {
            painter.text(
                pos2(bounds.right() - 10.0, bounds.top()),
                Align2::LEFT_TOP,
                "×",
                FontId::proportional(12.0),
                f(ink.ground),
            );
        }
        if !room.notes.is_empty() && half >= 8.0 {
            painter.circle(
                bounds.left_top() + vec2(4.0, 4.0),
                2.0,
                Color32::WHITE,
                Stroke::new(1.0, ink.ground),
            );
        }
        let exits: Vec<&MapLink> = index.outgoing[i].iter().filter_map(|&l| tracker.link_at(l)).collect();
        exit_lights(painter, room, &exits, p, half, ink, fade);
        if is_current {
            let r = (half * 0.55).clamp(4.0, 9.0);
            painter.circle(p, r + 2.0, ink.ground, Stroke::new(1.5, ink.select));
            painter.circle(
                p,
                r,
                if confirmed { ink.mint } else { Color32::TRANSPARENT },
                Stroke::new(2.0, ink.mint),
            );
        } else if candidate {
            painter.text(p, Align2::CENTER_CENTER, "?", FontId::proportional(13.0), ink.amber);
        }
        // Labels where they fit: below, right, then left.
        if !grid_mode && (labels || is_selected || is_current) {
            let width = (scale - 10.0).clamp(55.0, 130.0);
            let mut job = egui::text::LayoutJob::simple(room.name.clone(), label_font.clone(), f(ink.route), width);
            job.halign = egui::Align::Center;
            job.wrap.max_rows = 2;
            let galley = painter.layout_job(job);
            let size = vec2(width, galley.size().y);
            for at in [
                pos2(p.x - width / 2.0, p.y + half + 5.0),
                pos2(p.x + half + 7.0, p.y - size.y / 2.0),
                pos2(p.x - half - width - 7.0, p.y - size.y / 2.0),
            ] {
                let r = Rect::from_min_size(at, size);
                if !canvas.contains_rect(r) || reserved.iter().any(|q| q.intersects(r)) {
                    continue;
                }
                painter.rect_filled(r, 0.0, ink.ground);
                painter.galley(pos2(at.x + width / 2.0, at.y), galley, f(ink.route));
                reserved.push(r);
                break;
            }
        }
        // Badges for exits to other floors or areas: navigation of the map only.
        let mut badge_y = p.y - half - 20.0;
        for d in index.elsewhere[i].iter().filter_map(|&t| tracker.room_at(t)) {
            let caption = if d.area != room.area {
                format!("↗ {}", area_label(d.area_key()))
            } else {
                format!("{} {}", if d.z > room.z { "↑" } else { "↓" }, number(d.z))
            };
            let galley = painter.layout_no_wrap(caption, FontId::proportional(10.0), ink.route);
            let badge = Rect::from_min_size(pos2(p.x + half + 7.0, badge_y), vec2(galley.size().x + 8.0, 17.0));
            painter.rect(badge, 3.0, ink.tile, Stroke::new(1.0, ink.gray), StrokeKind::Inside);
            painter.galley(badge.min + vec2(4.0, 1.0), galley, ink.route);
            hits.badges.push((d.id.clone(), badge));
            badge_y -= 21.0;
        }
        // Known up and down exits with no mapped destination stay visibly unexplored.
        if !grid_mode {
            let (up, down) = index.vertical[i];
            let unexplored: Vec<&str> = room
                .known_exits
                .iter()
                .filter_map(|d| match d.as_str() {
                    "up" if !up => Some("↑?"),
                    "down" if !down => Some("↓?"),
                    _ => None,
                })
                .collect();
            if !unexplored.is_empty() {
                painter.text(
                    pos2(p.x + half + 6.0, p.y - 7.0),
                    Align2::LEFT_TOP,
                    unexplored.join(" "),
                    FontId::proportional(10.0),
                    accent,
                );
            }
        }
        hits.rooms
            .push((room.id.clone(), if grid_mode { bounds } else { bounds.expand(4.0) }));
    }
    // In grid mode the planned route is drawn over the tiles.
    if grid_mode && let Some(route) = &state.planned {
        let at = |id: &str| {
            index
                .rooms
                .iter()
                .position(|&r| tracker.room_at(r).is_some_and(|room| room.id == id))
                .map(|i| positions[i])
        };
        for step in &route.steps {
            if let (Some(a), Some(b)) = (at(&step.from_id), at(&step.to_id)) {
                painter.extend(Shape::dashed_line(&[a, b], Stroke::new(3.0, ink.amber), 5.0, 3.0));
            }
        }
    }
    // Labels over the rooms.
    if tracker.label_count() > 0 {
        labels::paint(
            painter,
            canvas,
            tracker,
            state,
            true,
            label_ink,
            label_preview.as_ref().map(|(id, l)| (id.as_str(), l.clone())),
            &mut hits.labels,
        );
    }
    // North: a badge in the panel's colour, not a dark pill (C# UI review, item 15).
    let north = painter.layout_no_wrap(format!("↑ {}", t(S::MapNorth)), FontId::proportional(11.0), ink.badge.2);
    let plate = Rect::from_min_size(canvas.min + vec2(8.0, 8.0), vec2(north.size().x + 16.0, 24.0));
    painter.rect_filled(plate, 4.0, ink.badge.0);
    painter.rect_stroke(plate, 4.0, Stroke::new(1.0, ink.badge.1), StrokeKind::Inside);
    painter.galley(plate.min + vec2(8.0, (24.0 - north.size().y) / 2.0), north, ink.badge.2);
    state.painted = painted;
    state.floor_index = index;
    hits
}

/// Small lights for known exits on the room's edges and corners (amber when closed or locked),
/// up and down off-centre on the top and bottom edges.
fn exit_lights(
    painter: &egui::Painter,
    room: &MapRoom,
    exits: &[&MapLink],
    center: Pos2,
    half: f32,
    ink: &Ink,
    fade: bool,
) {
    let radius = (half * 0.15).clamp(0.6, 3.2);
    let f = |c: Color32| if fade { c.gamma_multiply(0.35) } else { c };
    let mut seen: Vec<&str> = Vec::new();
    for direction in room
        .known_exits
        .iter()
        .map(String::as_str)
        .chain(exits.iter().map(|l| l.direction.as_str()))
    {
        if seen.contains(&direction) {
            continue;
        }
        seen.push(direction);
        let point = match direction_vector(direction) {
            Some(v) => center + v / v.x.abs().max(v.y.abs()) * (half - radius * 0.5),
            None if direction == "up" => center + vec2(-half * 0.55, -(half - radius * 0.5)),
            None if direction == "down" => center + vec2(half * 0.55, half - radius * 0.5),
            None => continue,
        };
        let blocked = exits
            .iter()
            .find(|l| l.direction == direction)
            .is_some_and(|l| l.blocked());
        let fill = f(if blocked { ink.amber } else { ink.mint });
        if half < 8.0 {
            // Zoomed far out the light is a dot of a pixel or two: a square costs less to draw.
            painter.rect_filled(
                Rect::from_center_size(point, Vec2::splat((radius * 2.0).max(1.2))),
                0.0,
                fill,
            );
            continue;
        }
        if half >= 6.0 {
            painter.circle_filled(point, radius + 2.0, f(ink.glow));
        }
        painter.circle(
            point,
            radius,
            f(if blocked { ink.amber } else { ink.mint }),
            Stroke::new(0.8, f(ink.ground)),
        );
        if matches!(direction, "up" | "down") && half >= 12.0 {
            let sign = if direction == "up" { -1.0 } else { 1.0 };
            let s = Stroke::new(1.0, f(ink.ground));
            painter.line_segment([point + vec2(-1.4, -sign * 0.6), point + vec2(0.0, sign * 0.9)], s);
            painter.line_segment([point + vec2(0.0, sign * 0.9), point + vec2(1.4, -sign * 0.6)], s);
        }
    }
}

/// Map tools' Room terrain inference section: the package's status, the switch, the minimum
/// confidence, and installing a package from a local file or folder.
fn inference_section(ui: &mut Ui, controls: &mut InferenceControls, state: &mut MapViewState, theme: &Theme) {
    use wandur_core::classify::ClassificationState;
    let status = controls.service.status();
    if status.state == ClassificationState::Unavailable {
        return;
    }
    let mut open = state.inference_open;
    editor::expander(ui, theme, t(S::MapInferenceSection), &mut open, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        let text = match status.state {
            ClassificationState::Ready => tf(S::MapInferenceReady, &[&status.version.clone().unwrap_or_default()]),
            ClassificationState::Installing => t(S::MapInferenceInstalling).to_string(),
            ClassificationState::Failed => tf(S::MapInferenceFailed, &[&status.message.clone().unwrap_or_default()]),
            _ => t(S::MapInferenceNotInstalled).to_string(),
        };
        ui.add(egui::Label::new(egui::RichText::new(text).size(11.0).color(theme.muted)).wrap());
        ui.checkbox(
            controls.enabled,
            egui::RichText::new(t(S::MapInferenceEnable)).size(11.0),
        );
        let mut percent = (*controls.threshold * 100.0).round();
        ui.label(egui::RichText::new(tf(S::MapInferenceThreshold, &[&percent])).size(11.0));
        let threshold = ui.add(
            egui::Slider::new(&mut percent, 50.0..=99.0)
                .step_by(1.0)
                .show_value(false),
        );
        crate::a11y::label(&threshold, &tf(S::MapInferenceThreshold, &[&percent]));
        if threshold.changed() {
            *controls.threshold = percent / 100.0;
        }
        let busy = status.state == ClassificationState::Installing;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(egui::RichText::new(t(S::MapInferenceInstallFile)).size(11.0)),
                )
                .clicked()
            {
                controls.install = pick_package(false);
            }
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(egui::RichText::new(t(S::MapInferenceInstallFolder)).size(11.0)),
                )
                .clicked()
            {
                controls.install = pick_package(true);
            }
        });
        ui.label(
            egui::RichText::new(t(S::MapInferenceLocalOnly))
                .size(10.0)
                .color(theme.muted),
        );
    });
    state.inference_open = open;
}

/// A classifier package: a `.zip`, or an extracted package folder.
#[cfg(feature = "native-dialogs")]
fn pick_package(folder: bool) -> Option<std::path::PathBuf> {
    let dialog = rfd::FileDialog::new().set_title(t(S::MapInferenceSection));
    if folder {
        dialog.pick_folder()
    } else {
        dialog.add_filter("zip", &["zip"]).pick_file()
    }
}

#[cfg(not(feature = "native-dialogs"))]
fn pick_package(_: bool) -> Option<std::path::PathBuf> {
    None
}

/// The Map tools panel's content, or the map editor's inspector (`inspector`): the editor keeps
/// the inspector on editing, so the mode, status, counts and the selection's prose give way to
/// Search and edit and the file buttons, as in C#. The mini map's menu starts with Edit map, Fit
/// floor and Open full map, and has no Route planning (that is on the full map).
#[allow(clippy::too_many_arguments)]
fn tools(
    ui: &mut Ui,
    tracker: Option<&RoomMapTracker>,
    live: Option<&SessionTab>,
    state: &mut MapViewState,
    cx: &mut MapContext,
    exercise: Option<&Exercise>,
    inspector: bool,
    intents: &mut Vec<Intent>,
) {
    let theme = cx.theme;
    let editing = inspector;
    let small = |text: String, size: f32| egui::RichText::new(text).size(size);
    let muted = |text: String, size: f32| egui::RichText::new(text).size(size).color(theme.muted);
    let is_live = exercise.is_none();
    if !editing {
        if state.mini {
            // The C# `CanOpenEditor`: the live map of a session.
            let can_open = is_live && live.is_some();
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if ui
                    .add_enabled(can_open, egui::Button::new(small(t(S::MapEditMode).into(), 11.0)))
                    .clicked()
                {
                    cx.open_editor = true;
                    state.tools_open = false;
                }
                if ui.button(small(t(S::MapFitFloor).into(), 11.0)).clicked()
                    && let Some(tracker) = tracker
                {
                    state.fit(tracker);
                    state.tools_open = false;
                }
                if ui
                    .add_enabled(can_open, egui::Button::new(small(t(S::MapOpenFull).into(), 11.0)))
                    .clicked()
                {
                    cx.open_full = true;
                    state.tools_open = false;
                }
            });
        }
        let mode = if is_live {
            t(S::MapLiveSession)
        } else {
            t(S::MapLocalExercise)
        };
        ui.label(muted(mode.to_string(), 10.0).extra_letter_spacing(1.0));
    }
    let Some(tracker) = tracker else {
        ui.label(small(t(S::MapWaiting).into(), 13.0).strong());
        return;
    };
    if !editing {
        let status = match tracker.state() {
            TrackingState::Confirmed => t(S::MapConfirmed).to_string(),
            TrackingState::Inferred => t(S::MapInferred).to_string(),
            TrackingState::Ambiguous => tf(S::MapAmbiguous, &[&tracker.candidates().len()]),
            TrackingState::Unknown => t(S::MapUnknown).to_string(),
            TrackingState::Waiting => t(S::MapWaiting).to_string(),
        };
        ui.add_space(-4.0);
        ui.label(small(status, 13.0).strong());
        ui.add_space(-4.0);
        ui.label(muted(
            tf(S::MapCounts, &[&tracker.room_count(), &tracker.link_count()]),
            10.0,
        ));
    }
    // Area.
    ui.label(muted(t(S::MapArea).into(), 11.0));
    ui.add_space(-4.0);
    let areas = areas(tracker);
    let mut chosen = state.area.clone();
    let area_box = egui::ComboBox::from_id_salt("map-area")
        .width(ui.available_width() - 8.0)
        .selected_text(area_label(&state.area))
        .show_ui(ui, |ui| {
            for a in &areas {
                ui.selectable_value(&mut chosen, a.clone(), area_label(a));
            }
        });
    crate::a11y::label_combo(&area_box.response, t(S::MapArea));
    if chosen != state.area {
        state.set_area(tracker, &chosen);
    }
    let mut grid = tracker.grid_mode(&state.area);
    if ui.checkbox(&mut grid, small(t(S::MapGridMode).into(), 11.0)).changed() {
        intents.push(Intent::Grid(state.area.clone(), grid));
        state.fit_floor = true;
    }
    // Floors.
    let floor_list = floors(tracker, &state.area);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let lower = floor_list.iter().rev().find(|f| **f < state.floor).copied();
        let higher = floor_list.iter().find(|f| **f > state.floor).copied();
        let down = ui.add_enabled_ui(lower.is_some(), |ui| {
            icon_button(ui, Icon::ArrowDown, false, theme, S::MapFloorDown)
        });
        if down.inner.clicked()
            && let Some(f) = lower
        {
            state.set_floor(f);
            state.fit(tracker);
        }
        ui.label(small(tf(S::MapFloor, &[&number(state.floor)]), 12.0).color(theme.text));
        let up = ui.add_enabled_ui(higher.is_some(), |ui| {
            icon_button(ui, Icon::ArrowUp, false, theme, S::MapFloorUp)
        });
        if up.inner.clicked()
            && let Some(f) = higher
        {
            state.set_floor(f);
            state.fit(tracker);
        }
    });
    // Selection.
    let selected = state.selected.as_deref().and_then(|id| tracker.room(id));
    let selection = selected
        .or(tracker.current())
        .map_or_else(|| t(S::MapSelectRoom).to_string(), |r| r.name.clone());
    ui.add(egui::Label::new(small(selection, 12.0)).truncate());
    if let Some(room) = selected
        && !room.description.is_empty()
    {
        ui.add(egui::Label::new(muted(room.description.clone(), 11.0)).wrap());
    }
    let legend = if tracker.grid_mode(&state.area) {
        t(S::MapGridLegend)
    } else {
        t(S::MapNodeLegend)
    };
    ui.label(muted(legend.into(), 10.0));
    ui.label(muted(t(S::MapLinksLegend).into(), 10.0));
    ui.label(muted(t(S::MapGestures).into(), 10.0));
    ui.horizontal_wrapped(|ui| {
        if is_live
            && ui
                .button(small(t(S::MapRecheckPosition).into(), 11.0))
                .on_hover_text(t(S::MapRecheckHint))
                .clicked()
        {
            intents.push(Intent::Recheck);
        }
        let label = if is_live {
            t(S::MapTryExercise)
        } else {
            t(S::MapReturnLive)
        };
        if ui.button(small(label.into(), 11.0)).clicked() {
            intents.push(Intent::ToggleExercise);
        }
    });
    if let Some(e) = exercise {
        ui.label(muted(tf(S::MapExerciseProgress, &[&(e.step + 1), &4]), 10.0));
        let instruction = match e.step {
            0 => S::MapExerciseStart,
            1 => S::MapExerciseNorthOne,
            2 => S::MapExerciseNorthTwo,
            _ => S::MapExerciseLandmark,
        };
        ui.label(small(t(instruction).into(), 11.0));
        if ui
            .add_enabled(
                e.step < 3,
                egui::Button::new(small(t(S::MapNextObservation).into(), 11.0)),
            )
            .clicked()
        {
            intents.push(Intent::NextExercise);
        }
    }
    if is_live {
        let evidence = match tracker.source() {
            RoomSource::Gmcp => S::MapSourceGmcp,
            RoomSource::Msdp => S::MapSourceMsdp,
            RoomSource::Text => S::MapSourceText,
        };
        ui.label(muted(t(evidence).into(), 10.0));
    }
    let evidence = live.map(|t| t.map.evidence.clone()).unwrap_or_default();
    ui.label(muted(evidence.summary(), 10.0));
    ui.label(muted(evidence.room_fields(), 10.0));
    if !editing
        && is_live
        && let Some(controls) = cx.inference.as_mut()
    {
        inference_section(ui, controls, state, theme);
    }
    if state.mini {
        return;
    }
    // Route planning.
    egui::Frame::new()
        .stroke(Stroke::new(1.0, theme.border))
        .corner_radius(CornerRadius::same(4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let header = ui
                .horizontal(|ui| {
                    ui.set_min_height(44.0);
                    ui.add_space(16.0);
                    ui.label(small(t(S::MapRouteTools).into(), 13.0).color(theme.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(12.0);
                        let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                        widgets::paint_icon(
                            ui,
                            if state.route_open {
                                Icon::ChevronUp
                            } else {
                                Icon::ChevronDown
                            },
                            rect,
                            theme.text,
                        );
                    });
                })
                .response
                .interact(Sense::click());
            crate::a11y::toggle(
                &header,
                egui::accesskit::Role::Button,
                t(S::MapRouteTools),
                state.route_open,
            );
            if header.clicked() {
                state.route_open = !state.route_open;
            }
            if !state.route_open {
                return;
            }
            ui.painter().hline(
                ui.min_rect().x_range(),
                ui.min_rect().bottom(),
                Stroke::new(1.0, theme.border),
            );
            egui::Frame::new().inner_margin(egui::Margin::same(16)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                let before = state.allow_inferred;
                ui.checkbox(&mut state.allow_inferred, small(t(S::MapAllowInferred).into(), 11.0));
                if state.allow_inferred != before {
                    state.update_route(tracker);
                }
                ui.horizontal_wrapped(|ui| {
                    let can_plan = state.selected.is_some() && tracker.current_id().is_some();
                    if ui
                        .add_enabled(can_plan, egui::Button::new(small(t(S::MapPlanRoute).into(), 11.0)))
                        .clicked()
                    {
                        state.plan_route(tracker);
                    }
                    if ui
                        .add_enabled(
                            state.route_destination.is_some(),
                            egui::Button::new(small(t(S::MapClearRoute).into(), 11.0)),
                        )
                        .clicked()
                    {
                        state.clear_route();
                    }
                });
                let route_status = match (&state.planned, &state.route_destination) {
                    (Some(route), _) => tf(S::MapRouteReady, &[&route.steps.len(), &number(route.cost)]),
                    (None, None) => t(S::MapRouteSelectRoom).to_string(),
                    (None, Some(_)) => t(S::MapRouteUnavailable).to_string(),
                };
                ui.label(muted(route_status, 11.0));
                if let Some(route) = &state.planned {
                    ui.add(egui::Label::new(small(route.commands(), 11.0)).wrap());
                }
                let walking = live.is_some_and(|t| t.map.is_walking());
                let connected = live.is_some_and(|t| t.is_connected());
                ui.horizontal_wrapped(|ui| {
                    let can_walk =
                        is_live && connected && !walking && state.planned.as_ref().is_some_and(|r| !r.steps.is_empty());
                    if ui
                        .add_enabled(can_walk, egui::Button::new(small(t(S::MapWalk).into(), 11.0)))
                        .clicked()
                        && let Some(route) = state.planned.clone()
                    {
                        state.walk_feedback = true;
                        state.route_unavailable = false;
                        intents.push(Intent::Walk(route));
                    }
                    if ui
                        .add_enabled(walking, egui::Button::new(small(t(S::MapStopWalk).into(), 11.0)))
                        .clicked()
                    {
                        intents.push(Intent::StopWalk);
                    }
                });
                let walk_status = live.map_or(WalkStatus::Unavailable, |t| t.map.walk_status());
                ui.label(muted(walk_status.text(), 11.0));
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use wandur_core::map::MapSnapshot;

    fn frame(tab: Option<&mut SessionTab>, state: &mut MapViewState, input: egui::RawInput) -> Vec<String> {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::ember();
        let mut auto = true;
        let mut tab = tab;
        let mut texts = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(input.clone(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let mut cx = MapContext::new(&theme, &mut auto);
                    show(ui, tab.as_deref_mut(), state, &mut cx);
                });
            });
            output.textures_delta.clear();
            texts = output
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect();
        }
        texts
    }

    fn screen(w: f32, h: f32) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(w, h))),
            ..Default::default()
        }
    }

    /// The C# `MapRoomSearchTests` map: six rooms on two floors, three about a temple.
    pub fn six_rooms() -> MapSnapshot {
        let r = |id: &str, name: &str, desc: &str, x: f64, z: f64| MapRoom::new(id, name, desc, None, x, 0.0, z, false);
        MapSnapshot {
            current_room_id: Some("alpha".into()),
            state: TrackingState::Confirmed,
            ..MapSnapshot::of(
                vec![
                    r("alpha", "Temple Alpha", "A dusty temple hall.", 0.0, 0.0),
                    r("market", "Market Row", "Stalls line the street.", 1.0, 0.0),
                    r("gamma", "Temple Gamma", "Incense fills a quiet temple room.", 2.0, 0.0),
                    r("guard", "Guard Post", "A watchful guard stands here.", 0.0, 1.0),
                    r("beta", "Temple Beta", "A temple shrine on the upper floor.", 1.0, 1.0),
                    r("tower", "Watchtower", "Wind howls around the tower.", 2.0, 1.0),
                ],
                Vec::new(),
            )
        }
    }

    #[test]
    fn empty_map_shows_the_csharp_text_alone() {
        let mut state = MapViewState::default();
        let texts = frame(None, &mut state, screen(400.0, 300.0));
        assert!(
            texts.iter().any(|t| t.contains("Explore a world to build its map")),
            "{texts:?}"
        );
        // No protocol line on an empty map: the message alone (C# UI review, item 8).
        assert!(!texts.iter().any(|t| t.contains("GMCP")), "{texts:?}");
        assert_eq!(state.painted, 0);
    }

    #[test]
    fn search_steps_through_matches_across_floors_and_escape_clears() {
        let tracker = RoomMapTracker::from_snapshot(six_rooms());
        let mut state = MapViewState::default();
        let version = tracker.version();
        state.refresh(&tracker, true);
        assert_eq!(state.floor, 0.0);
        state.search_visible = true;
        state.search_query = "t".into();
        assert!(
            state.search_matches(&tracker, version).is_empty(),
            "one letter does not filter"
        );
        state.search_query = "temple".into();
        let matches = state.search_matches(&tracker, version);
        let names: Vec<&str> = matches
            .iter()
            .map(|id| tracker.room(id).unwrap().name.as_str())
            .collect();
        assert_eq!(names, ["Temple Alpha", "Temple Beta", "Temple Gamma"]);
        state.next_match(&tracker, version);
        assert_eq!(state.selected.as_deref(), Some("alpha"));
        assert_eq!((state.floor, state.center), (0.0, (0.0, 0.0)));
        state.next_match(&tracker, version);
        assert_eq!(state.selected.as_deref(), Some("beta"));
        assert_eq!((state.floor, state.center), (1.0, (1.0, 0.0)));
        state.clear_search();
        assert!(state.search_matches(&tracker, version).is_empty());
    }

    #[test]
    fn rooms_draw_on_their_floor_with_labels_and_the_status_line() {
        let snapshot = six_rooms();
        let mut tab = SessionTab::demo(1, &crate::session_tab::TabOptions::default());
        tab.map = wandur_core::map::MapSession::with_map(snapshot);
        tab.map.tracker_mut().set_current_room("alpha");
        let mut state = MapViewState::default();
        let texts = frame(Some(&mut tab), &mut state, screen(600.0, 400.0));
        assert_eq!(state.painted, 3, "three rooms on floor 0");
        assert!(texts.iter().any(|t| t == "Temple Alpha"), "{texts:?}");
        assert!(!texts.iter().any(|t| t == "Temple Beta"), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("North")), "{texts:?}");
        // Other-floor matches are listed over the map.
        state.search_visible = true;
        state.search_query = "temple".into();
        let texts = frame(Some(&mut tab), &mut state, screen(600.0, 400.0));
        assert!(texts.iter().any(|t| t.contains("Temple Beta ·")), "{texts:?}");
        assert!(texts.iter().any(|t| t == "3 rooms"), "{texts:?}");
    }

    #[test]
    fn tools_show_status_area_route_and_evidence() {
        let mut tab = SessionTab::demo(1, &crate::session_tab::TabOptions::default());
        let mut snapshot = six_rooms();
        snapshot.links = vec![
            MapLink::new("alpha", "market", "east", true),
            MapLink::new("market", "gamma", "east", true),
        ];
        tab.map = wandur_core::map::MapSession::with_map(snapshot);
        tab.map.tracker_mut().set_current_room("alpha");
        let mut state = MapViewState {
            tools_open: true,
            route_open: true,
            ..Default::default()
        };
        frame(Some(&mut tab), &mut state, screen(700.0, 900.0));
        state.selected = Some("gamma".into());
        state.plan_route(tab.map.tracker());
        let texts = frame(Some(&mut tab), &mut state, screen(700.0, 900.0));
        for want in [
            "SESSION MAP",
            "Position inferred",
            "6 rooms · 2 connections",
            "Route planning",
            "2 steps · cost 2",
            "east → east",
            "Room evidence: text",
        ] {
            assert!(texts.iter().any(|t| t.contains(want)), "{want}: {texts:?}");
        }
    }

    fn walkable_tab() -> SessionTab {
        let mut tab = SessionTab::demo(1, &crate::session_tab::TabOptions::default());
        let rooms = vec![
            MapRoom::new("s:1", "Room 1", "", None, 0.0, 0.0, 0.0, false).with_server_id("1"),
            MapRoom::new("s:2", "Room 2", "", None, 0.0, 1.0, 0.0, false).with_server_id("2"),
            MapRoom::new("s:3", "Room 3", "", None, 1.0, 1.0, 0.0, false).with_server_id("3"),
        ];
        let links = vec![
            MapLink::new("s:1", "s:2", "north", true),
            MapLink::new("s:2", "s:3", "east", true),
        ];
        tab.map = wandur_core::map::MapSession::with_map(MapSnapshot::of(rooms, links));
        let gate = tab.walk_gate();
        let room = RoomObservation::new(Some("1"), "Room 1", "", &[]).with_source(RoomSource::Gmcp);
        tab.map.observe_room(room, gate, std::time::Instant::now());
        tab
    }

    /// Run frames of the panel, clicking at these map coordinates (one click a frame, 0.1 s
    /// apart).
    fn click_rooms(tab: &mut SessionTab, state: &mut MapViewState, clicks: &[(f64, f64)]) {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::ember();
        let mut auto = true;
        let mut time = 0.0;
        let mut run = |events: Vec<egui::Event>, time: f64, state: &mut MapViewState, tab: &mut SessionTab| {
            let input = egui::RawInput {
                time: Some(time),
                events,
                ..screen(550.0, 600.0)
            };
            let mut output = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let mut cx = MapContext::new(&theme, &mut auto);
                    show(ui, Some(&mut *tab), state, &mut cx);
                });
            });
            output.textures_delta.clear();
        };
        run(Vec::new(), time, state, tab);
        run(Vec::new(), time, state, tab);
        for &(x, y) in clicks {
            time += 0.1;
            let pos = state.project(state.canvas, x, y);
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            run(vec![egui::Event::PointerMoved(pos), button(true)], time, state, tab);
            run(vec![button(false)], time + 0.01, state, tab);
        }
    }

    #[test]
    fn double_clicking_a_room_walks_there_and_the_toolbar_can_stop() {
        let mut tab = walkable_tab();
        let mut state = MapViewState::default();
        click_rooms(&mut tab, &mut state, &[(1.0, 1.0), (1.0, 1.0)]);
        assert_eq!(state.selected.as_deref(), Some("s:3"));
        assert_eq!(state.walk_requested, 1);
        assert!(tab.map.is_walking());
        assert_eq!(state.planned.as_ref().map(|r| r.steps.len()), Some(2));
        assert_eq!(tab.history().len(), 0, "walk steps stay out of the command history");
        tab.stop_walk();
        assert_eq!(tab.map.walk_status(), WalkStatus::Stopped);
    }

    #[test]
    fn two_quick_clicks_on_different_rooms_do_not_walk() {
        let mut tab = walkable_tab();
        let mut state = MapViewState::default();
        click_rooms(&mut tab, &mut state, &[(0.0, 1.0), (1.0, 1.0)]);
        assert_eq!(state.selected.as_deref(), Some("s:3"));
        assert_eq!(state.walk_requested, 0);
        assert!(!tab.map.is_walking());
    }

    #[test]
    fn a_double_click_without_a_known_route_says_so_and_sends_nothing() {
        let mut tab = walkable_tab();
        assert!(tab.map.tracker_mut().remove_link("s:2", "east"));
        let mut state = MapViewState::default();
        click_rooms(&mut tab, &mut state, &[(1.0, 1.0), (1.0, 1.0)]);
        assert!(state.planned.is_none() && state.route_unavailable && state.walk_feedback);
        assert!(!tab.map.is_walking());
    }

    #[test]
    fn rendering_two_thousand_rooms_stays_cheap() {
        // A 50 by 40 block of rooms, every one linked east and north, fitted on screen.
        let mut rooms = Vec::new();
        let mut links = Vec::new();
        for i in 0..2000 {
            let (x, y) = (f64::from(i % 50), f64::from(i / 50));
            rooms.push(MapRoom::new(
                &format!("r{i}"),
                &format!("Room {i}"),
                "",
                Some("Big"),
                x,
                y,
                0.0,
                false,
            ));
            if i % 50 != 49 {
                links.push(MapLink::new(&format!("r{i}"), &format!("r{}", i + 1), "east", true));
            }
            if i < 1950 {
                links.push(MapLink::new(&format!("r{i}"), &format!("r{}", i + 50), "north", true));
            }
        }
        let tracker = RoomMapTracker::from_snapshot(MapSnapshot::of(rooms, links));
        let ctx = egui::Context::default();
        let ink = Ink::of(&Theme::ember());
        let mut state = MapViewState {
            viewport: vec2(800.0, 600.0),
            ..Default::default()
        };
        state.refresh(&tracker, true);
        state.fit(&tracker);
        let mut total = std::time::Duration::ZERO;
        for _ in 0..5 {
            let mut output = ctx.run_ui(screen(800.0, 600.0), |ui| {
                let canvas = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
                let start = std::time::Instant::now();
                let painter = ui.painter_at(canvas);
                paint(&painter, canvas, &tracker, tracker.version(), &mut state, &ink, None);
                total += start.elapsed();
            });
            output.textures_delta.clear();
        }
        assert_eq!(state.painted, 2000);
        // Debug builds are slow; the release measurement is in docs/measurements.md.
        assert!(total.as_millis() < 5 * 200, "{total:?}");
    }

    /// One frame of a view over `tab` (the mini map follows `follow`), returning the texts.
    fn frame_of(tab: &mut SessionTab, state: &mut MapViewState, follow: bool) -> (Vec<String>, MapOutcome) {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::ember();
        let mut auto = follow;
        let mut texts = Vec::new();
        let mut outcome = MapOutcome::default();
        for _ in 0..2 {
            let mut output = ctx.run_ui(screen(700.0, 700.0), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let mut cx = MapContext::new(&theme, &mut auto);
                    show(ui, Some(&mut *tab), state, &mut cx);
                    outcome.open_editor |= cx.open_editor;
                    outcome.open_full |= cx.open_full;
                });
            });
            output.textures_delta.clear();
            texts = output
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect();
        }
        (texts, outcome)
    }

    /// Labels draw on the full map and the mini map alike: text laid out at the map's zoom,
    /// a picture from a texture decoded once for both views; labels of another floor are not
    /// drawn.
    #[test]
    fn labels_draw_on_the_full_map_and_the_mini_map_from_one_decoded_picture() {
        use wandur_core::map::{MapImage, MapLabel};
        let img = image::RgbaImage::from_fn(24, 12, |x, _| image::Rgba([x as u8 * 10, 90, 40, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let picture: MapImage = wandur_core::map::images::prepare(&png).unwrap();
        let map = MapSnapshot {
            labels: vec![
                MapLabel {
                    above_rooms: true,
                    ..MapLabel::text("t", None, -0.5, 1.2, 0.0, "Temple Quarter")
                },
                MapLabel {
                    image: Some(picture.hash.clone()),
                    width: 1.0,
                    height: 0.5,
                    ..MapLabel::text("p", None, 0.6, -0.3, 0.0, "")
                },
                MapLabel::text("upstairs", None, 0.0, 0.0, 1.0, "Upper Floor Note"),
            ],
            images: vec![picture],
            ..six_rooms()
        };
        let mut tab = SessionTab::demo(1, &crate::session_tab::TabOptions::default());
        tab.map = wandur_core::map::MapSession::with_map(map);
        tab.map.tracker_mut().set_current_room("alpha");
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::ember();
        let mut full = MapViewState::full();
        let mut mini = MapViewState::mini();
        let mut auto = true;
        let mut run = |full: &mut MapViewState, mini: &mut MapViewState, tab: &mut SessionTab| {
            let mut output = ctx.run_ui(screen(1000.0, 600.0), |ui| {
                let mut right =
                    ui.new_child(UiBuilder::new().max_rect(Rect::from_min_max(pos2(680.0, 0.0), pos2(1000.0, 600.0))));
                let mut cx = MapContext::new(&theme, &mut auto);
                show(&mut right, Some(&mut *tab), mini, &mut cx);
                let mut left =
                    ui.new_child(UiBuilder::new().max_rect(Rect::from_min_max(pos2(0.0, 0.0), pos2(670.0, 600.0))));
                let mut cx = MapContext::new(&theme, &mut auto);
                show(&mut left, Some(&mut *tab), full, &mut cx);
            });
            output.textures_delta.clear();
            output.shapes
        };
        run(&mut full, &mut mini, &mut tab);
        full.zoom = 1.0;
        full.fit_floor = false;
        mini.zoom = 1.0;
        run(&mut full, &mut mini, &mut tab);
        labels::LabelImages::shared(&ctx).lock().unwrap().settle(&ctx);
        let shapes = run(&mut full, &mut mini, &mut tab);
        let texts: Vec<(String, Rect)> = shapes
            .iter()
            .filter_map(|c| match &c.shape {
                Shape::Text(t) => Some((t.galley.text().to_string(), c.clip_rect)),
                _ => None,
            })
            .collect();
        let in_view = |rect: Rect, name: &str| texts.iter().any(|(t, clip)| t == name && rect.intersects(*clip));
        assert!(in_view(full.canvas, "Temple Quarter"), "{texts:?}");
        assert!(in_view(mini.canvas, "Temple Quarter"), "the mini map too");
        assert!(!texts.iter().any(|(t, _)| t == "Upper Floor Note"), "another floor");
        let pictures: Vec<Rect> = shapes
            .iter()
            .filter_map(|c| match &c.shape {
                Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => Some(m.calc_bounds()),
                _ => None,
            })
            .collect();
        assert!(pictures.iter().any(|r| full.canvas.contains_rect(*r)), "{pictures:?}");
        assert!(pictures.iter().any(|r| mini.canvas.contains_rect(*r)), "{pictures:?}");
        // One decode serves both views and every frame after.
        run(&mut full, &mut mini, &mut tab);
        assert_eq!(labels::LabelImages::shared(&ctx).lock().unwrap().decodes, 1);
    }

    #[derive(Debug, Default, PartialEq)]
    struct MapOutcome {
        open_editor: bool,
        open_full: bool,
    }

    /// The mini map and the full map draw the session's one tracker: a move of the player, an
    /// edit made in the full map's editor and its undo all show in the mini map's next frame,
    /// which follows the current room.
    #[test]
    fn the_mini_map_and_the_full_map_share_position_edits_and_undo() {
        let mut tab = walkable_tab();
        let mut mini = MapViewState::mini();
        let mut full = MapViewState::full();
        frame_of(&mut tab, &mut mini, true);
        frame_of(&mut tab, &mut full, true);
        assert_eq!(mini.center, (0.0, 0.0), "centred on Room 1");
        assert_eq!(full.center, (0.0, 0.0));
        // The player moves north: both follow.
        let gate = tab.walk_gate();
        let room = RoomObservation::new(Some("2"), "Room 2", "", &[]).with_source(RoomSource::Gmcp);
        tab.map.observe_room(room, gate, std::time::Instant::now());
        frame_of(&mut tab, &mut mini, true);
        frame_of(&mut tab, &mut full, true);
        assert_eq!(mini.center, (0.0, 1.0));
        assert_eq!(full.center, (0.0, 1.0));
        // Edit in the full map: Room 2 moves east; the mini map, following it, recentres there.
        full.set_editing(true);
        full.selected = Some("s:2".into());
        let moved = editor::apply(
            &mut full,
            tab.map.tracker_mut(),
            editor::EditAction::MoveRooms(vec!["s:2".into()], 4.0, 0.0, 0.0),
        );
        assert_eq!(moved, editor::Outcome::default());
        assert_eq!(tab.map.tracker().room("s:2").map(|r| r.x), Some(4.0));
        let before = mini.shown;
        frame_of(&mut tab, &mut mini, true);
        assert!(mini.shown > before);
        assert_eq!(mini.painted, 3, "the mini map draws the edited floor");
        assert_eq!(tab.map.tracker().current().map(|r| (r.x, r.y)), Some((4.0, 1.0)));
        // Undo in the full map puts it back for both.
        editor::apply(&mut full, tab.map.tracker_mut(), editor::EditAction::Undo);
        assert_eq!(tab.map.tracker().room("s:2").map(|r| r.x), Some(0.0));
        assert!(tab.map.tracker().can_redo());
        frame_of(&mut tab, &mut mini, true);
        frame_of(&mut tab, &mut full, true);
        assert_eq!(
            full.selected.as_deref(),
            Some("s:2"),
            "the full map keeps its own selection"
        );
        assert!(mini.selected.is_none(), "the mini map has its own");
    }

    /// The mini map's chrome: follow, Open full map and the menu in its header (no search, no
    /// Edit, no Fit); its menu has Edit map, Fit floor and Open full map, the remaining tools,
    /// and no Route planning; no status line until it walks. The full map's header has Edit.
    #[test]
    fn the_mini_map_keeps_minimal_chrome_and_its_menu_opens_the_full_map() {
        let mini = MapViewState::mini();
        let ids: Vec<&str> = header_actions(&mini, true, false).iter().map(|a| a.id).collect();
        assert_eq!(ids, [ACTION_CENTER, ACTION_OPEN_FULL, ACTION_TOOLS]);
        let full = MapViewState::full();
        let ids: Vec<&str> = header_actions(&full, true, true).iter().map(|a| a.id).collect();
        assert_eq!(
            ids,
            [
                ACTION_SEARCH,
                ACTION_CENTER,
                ACTION_FIT,
                ACTION_EDIT,
                ACTION_TOOLS,
                ACTION_STOP
            ]
        );
        let mut editing = MapViewState::full();
        editing.set_editing(true);
        let ids: Vec<&str> = header_actions(&editing, true, false).iter().map(|a| a.id).collect();
        assert_eq!(
            ids,
            [ACTION_SEARCH, ACTION_CENTER, ACTION_FIT, ACTION_EDIT],
            "no Map tools while editing"
        );

        let mut tab = walkable_tab();
        let mut mini = MapViewState::mini();
        let (texts, _) = frame_of(&mut tab, &mut mini, true);
        assert!(!texts.iter().any(|t| t.contains("GMCP")), "no status line: {texts:?}");
        assert_eq!(mini.zoom, MINI_ZOOM);
        mini.tools_open = true;
        let (texts, _) = frame_of(&mut tab, &mut mini, true);
        for want in ["Edit map", "Fit floor", "Open full map", "SESSION MAP", "Area"] {
            assert!(texts.iter().any(|t| t == want), "{want}: {texts:?}");
        }
        assert!(!texts.iter().any(|t| t == "Route planning"), "{texts:?}");
        // The header's Open full map and the menu's Edit map ask the app.
        mini.header_click = Some(ACTION_OPEN_FULL);
        let (_, outcome) = frame_of(&mut tab, &mut mini, true);
        assert!(outcome.open_full);
        mini.header_click = Some(ACTION_EDIT);
        let (_, outcome) = frame_of(&mut tab, &mut mini, true);
        assert!(!outcome.open_editor, "Edit is not a mini map header action");
    }

    /// The full map's Edit toggle shows the inspector beside the canvas and turns edit mode on;
    /// off again hides it.
    #[test]
    fn the_full_maps_edit_toggle_shows_the_inspector() {
        let mut tab = walkable_tab();
        let mut full = MapViewState::full();
        let (texts, _) = frame_of(&mut tab, &mut full, true);
        assert!(!texts.iter().any(|t| t == "Properties"), "{texts:?}");
        let wide = full.canvas.width();
        full.header_click = Some(ACTION_EDIT);
        let (texts, _) = frame_of(&mut tab, &mut full, true);
        assert!(full.is_editor() && full.editor.as_deref().is_some_and(|e| e.open));
        assert!(texts.iter().any(|t| t == "Properties"), "{texts:?}");
        assert!(full.canvas.width() < wide, "the inspector takes its side");
        full.header_click = Some(ACTION_EDIT);
        frame_of(&mut tab, &mut full, true);
        assert!(!full.is_editor() && !full.editor.as_deref().is_some_and(|e| e.open));
    }

    /// Map tools' Room terrain inference section (the C# `MapInferenceSection`): closed at
    /// first, then the package's status, the switch, the minimum confidence and the install
    /// buttons; the switch and the slider write the settings the app saves.
    #[test]
    fn map_tools_show_the_inference_section_with_its_status_and_switch() {
        let service = wandur_core::classify::RoomClassificationService::for_testing(std::sync::Arc::new(
            crate::scene::KeywordClassifier,
        ));
        let mut tab = walkable_tab();
        let mut state = MapViewState {
            tools_open: true,
            ..MapViewState::default()
        };
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::ember();
        let (mut auto, mut enabled, mut threshold) = (true, true, 0.8);
        let mut texts = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(screen(500.0, 2000.0), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let mut cx = MapContext::new(&theme, &mut auto);
                    cx.inference = Some(InferenceControls {
                        service: &service,
                        enabled: &mut enabled,
                        threshold: &mut threshold,
                        install: None,
                    });
                    show(ui, Some(&mut tab), &mut state, &mut cx);
                });
            });
            output.textures_delta.clear();
            texts = output
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect();
            state.inference_open = true;
        }
        for want in [
            "Room terrain inference",
            "Local model fixture-1 ready.",
            "Color rooms with the local model",
            "Minimum confidence: 80%",
            "Install from file",
            "Install from folder",
        ] {
            assert!(texts.iter().any(|t| t == want), "{want}: {texts:?}");
        }
    }

    /// The C# `InferredRoomPaintsPaletteColorWhileServerTerrainWins`: an inferred forest room
    /// is filled with the forest colour, a room with server water terrain (and an inferred
    /// forest guess) with water.
    #[test]
    fn an_inferred_room_paints_its_palette_colour_while_server_terrain_wins() {
        let mut inferred = MapRoom::new("inferred", "inferred", "", None, -2.0, 0.0, 0.0, false);
        inferred.inferred_environment = Some("forest".into());
        inferred.inferred_confidence = Some(0.87);
        let mut server = MapRoom::new("server", "server", "", None, 2.0, 0.0, 0.0, false);
        server.environment = Some("water".into());
        server.inferred_environment = Some("forest".into());
        server.inferred_confidence = Some(0.87);
        let mut tab = walkable_tab();
        tab.map = wandur_core::map::MapSession::with_map(MapSnapshot::of(vec![inferred, server], Vec::new()));
        tab.map.tracker_mut().set_area_settings(MapAreaSettings::new("", true));
        let mut state = MapViewState::default();
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::ember();
        let mut auto = true;
        let mut fills = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(screen(500.0, 400.0), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let mut cx = MapContext::new(&theme, &mut auto);
                    show(ui, Some(&mut tab), &mut state, &mut cx);
                });
            });
            output.textures_delta.clear();
            fills = output
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    Shape::Rect(r) => Some(r.fill),
                    _ => None,
                })
                .collect();
        }
        let forest = Color32::from_rgb(0x2E, 0x80, 0x3F);
        let water = Color32::from_rgb(0x16, 0xA7, 0xD5);
        assert_eq!(fills.iter().filter(|f| **f == forest).count(), 1, "{fills:?}");
        assert_eq!(fills.iter().filter(|f| **f == water).count(), 1, "{fills:?}");
    }
}
