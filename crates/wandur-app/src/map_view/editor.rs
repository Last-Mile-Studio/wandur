//! The full map's editor (its Edit toggle): a tool toolbar over the canvas ([`toolbar`]), the
//! tools on the canvas ([`canvas`]: select, marquee, move, nudge, add room, connect, delete, a
//! context menu) and a docked property inspector on the right ([`inspector`]: Room, Position,
//! Exits, Notes and description; an exit's own fields; shared fields of a multi-selection).
//!
//! Every change is an [`EditAction`] applied once drawing is done ([`apply`]), through the map
//! core's editing operations (`wandur_core::map::editing`), so saving, merging, tombstones,
//! manual-edit marks and undo stay the tracker's, and each action is one undo step.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{Pos2, Rect};
use wandur_core::l10n::{S, t, tf};
use wandur_core::map::format::{self, MAX_BYTES};
use wandur_core::map::{DoorState, MapImage, MapLabel, MapRoom, RoomMapTracker};
use wandur_core::settings::MapEditorPrefs;

use super::MapViewState;
use crate::theme::Theme;

pub mod canvas;
pub mod inspector;
pub mod toolbar;

/// What a press on the canvas does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    /// Click to select, drag to move or to draw a marquee (V).
    #[default]
    Select,
    /// Click an empty cell to add a room there (R).
    AddRoom,
    /// Drag from a room to another to connect them (E).
    Connect,
    /// Click to add a text label there (L).
    AddLabel,
}

/// Something under the pointer or a context menu's subject.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Room(String),
    Exit(String, String),
    Label(String),
    /// Empty canvas at these map coordinates.
    Empty(f64, f64),
}

/// A pointer gesture in progress on the canvas.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Gesture {
    #[default]
    None,
    /// Moving the selected rooms: the map point pressed and the offset so far (map units).
    Move { anchor: (f64, f64), offset: (f64, f64) },
    /// A selection rectangle in screen points; `additive` keeps what was selected.
    Marquee { from: Pos2, to: Pos2, additive: bool },
    /// A rubber band from a room to the pointer, over `target` when it is on a room.
    Connect {
        from: String,
        to: Pos2,
        target: Option<String>,
    },
    /// Moving a label: the map point pressed and the offset so far.
    MoveLabel {
        id: String,
        anchor: (f64, f64),
        offset: (f64, f64),
    },
    /// Resizing a label by its grip: the map point pressed and the offset so far.
    ResizeLabel {
        id: String,
        anchor: (f64, f64),
        offset: (f64, f64),
    },
    /// Panning the view.
    Pan,
}

/// The context menu: where it opened and what for.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextMenu {
    pub at: Pos2,
    pub target: Target,
    /// Frames shown (the opening click is not a click outside).
    pub age: u32,
}

/// The editor's state, kept with the session's full map.
#[derive(Debug)]
pub struct EditorState {
    /// Edit is on: the toolbar and the inspector are shown and the tools act.
    pub open: bool,
    pub tool: Tool,
    /// Selected rooms, in the order picked (the last is the primary one).
    pub rooms: Vec<String>,
    /// The selected exit (from, direction); rooms and an exit are never selected together.
    pub exit: Option<(String, String)>,
    /// The selected label (never with rooms or an exit).
    pub label: Option<String>,
    /// What the pointer is over (drawn as a hover highlight).
    pub hover: Option<Target>,
    pub gesture: Gesture,
    /// The inspector's width, closed sections and grid snap (remembered in the settings).
    pub prefs: MapEditorPrefs,
    /// Text typed into inspector fields and not committed yet, by field key.
    pub drafts: HashMap<String, String>,
    /// Inline problems of fields, by field key.
    pub errors: HashMap<String, String>,
    /// The selection the drafts were typed for.
    drafts_for: (Vec<String>, Option<(String, String)>, Option<String>),
    /// Focus the Name field next frame (a new room, a double click).
    pub focus_name: bool,
    pub menu: Option<ContextMenu>,
    /// Rooms waiting for a confirmation before they are deleted (more than five).
    pub confirm_delete: Option<Vec<String>>,
    /// Merge mode: the next room clicked is the one this room merges into.
    pub merge_source: Option<String>,
    /// The last edit's outcome (an import, a refusal).
    pub message: Option<String>,
    /// Edits asked from outside the map (the Edit menu's Undo and Redo).
    pub pending: Vec<EditAction>,
    /// The symbol being typed, shown on the selected rooms before it is committed.
    pub symbol_preview: Option<String>,
    /// The open room picker: its field key and the search typed.
    pub picker: Option<(String, String)>,
    /// The room the last click selected, with the time, for double clicks.
    last_click: Option<(String, f64)>,
    /// A text field had the keyboard at the end of the last frame.
    pub typing: bool,
    /// The overview's room search (names, ids, server ids, descriptions, notes).
    pub find: String,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            open: false,
            tool: Tool::Select,
            rooms: Vec::new(),
            exit: None,
            label: None,
            hover: None,
            gesture: Gesture::None,
            prefs: MapEditorPrefs::default(),
            drafts: HashMap::new(),
            errors: HashMap::new(),
            drafts_for: (Vec::new(), None, None),
            focus_name: false,
            menu: None,
            confirm_delete: None,
            merge_source: None,
            message: None,
            pending: Vec::new(),
            symbol_preview: None,
            picker: None,
            last_click: None,
            typing: false,
            find: String::new(),
        }
    }
}

impl EditorState {
    /// The primary selected room.
    pub fn primary(&self) -> Option<&str> {
        self.rooms.last().map(String::as_str)
    }

    pub fn is_selected(&self, id: &str) -> bool {
        self.rooms.iter().any(|r| r == id)
    }

    /// Select only this room (or nothing).
    pub fn select_only(&mut self, id: Option<&str>) {
        self.rooms = id.map(|i| vec![i.to_string()]).unwrap_or_default();
        self.exit = None;
        self.label = None;
    }

    /// Select only this label.
    pub fn select_label(&mut self, id: &str) {
        self.rooms.clear();
        self.exit = None;
        self.label = Some(id.to_string());
    }

    /// Add a room to the selection, or take it out when it is in.
    pub fn toggle(&mut self, id: &str) {
        self.exit = None;
        self.label = None;
        if let Some(i) = self.rooms.iter().position(|r| r == id) {
            self.rooms.remove(i);
        } else {
            self.rooms.push(id.to_string());
        }
    }

    pub fn select_exit(&mut self, from: &str, direction: &str) {
        self.rooms.clear();
        self.label = None;
        self.exit = Some((from.to_string(), direction.to_string()));
    }

    /// The editor wants Escape for itself (a tool, a gesture, a menu, a selection), so the
    /// Map page does not go back to Play.
    pub fn wants_escape(&self) -> bool {
        self.open
            && (self.tool != Tool::Select
                || self.gesture != Gesture::None
                || self.menu.is_some()
                || self.confirm_delete.is_some()
                || self.merge_source.is_some()
                || self.picker.is_some()
                || !self.rooms.is_empty()
                || self.exit.is_some()
                || self.label.is_some())
    }

    /// Forget the drafts and problems when the selection changed.
    fn keep_drafts_for_selection(&mut self) {
        let now = (self.rooms.clone(), self.exit.clone(), self.label.clone());
        if self.drafts_for != now {
            self.drafts.clear();
            self.errors.clear();
            self.symbol_preview = None;
            self.picker = None;
            self.drafts_for = now;
        }
    }
}

/// A change to rooms from the inspector or a menu.
#[derive(Clone, Debug, PartialEq)]
pub enum RoomChange {
    Name(String),
    Area(Option<String>),
    Terrain(Option<String>),
    Color(Option<String>),
    Symbol(Option<String>),
    Locked(bool),
    X(f64),
    Y(f64),
    Z(f64),
    Cost(f64),
    Notes(String),
    Description(String),
}

impl RoomChange {
    fn apply(&self, room: &mut MapRoom) {
        match self {
            Self::Name(v) => room.name = v.trim().to_string(),
            Self::Area(v) => room.area = v.clone(),
            Self::Terrain(v) => room.environment = v.clone(),
            Self::Color(v) => room.color = v.clone(),
            Self::Symbol(v) => room.symbol = v.clone(),
            Self::Locked(v) => room.is_locked = *v,
            Self::X(v) => room.x = *v,
            Self::Y(v) => room.y = *v,
            Self::Z(v) => room.z = *v,
            Self::Cost(v) => room.weight = *v,
            Self::Notes(v) => room.notes = v.clone(),
            Self::Description(v) => room.description = v.clone(),
        }
    }

    /// The change moves a room (refused for a locked one).
    fn moves(&self) -> bool {
        matches!(self, Self::X(_) | Self::Y(_) | Self::Z(_))
    }
}

/// A change to one exit.
#[derive(Clone, Debug, PartialEq)]
pub enum ExitChange {
    Direction(String),
    Destination(String),
    Door(DoorState),
    Cost(f64),
    Excluded(bool),
    Command(Option<String>),
}

/// A change to one label from the inspector or a menu.
#[derive(Clone, Debug, PartialEq)]
pub enum LabelChange {
    Text(String),
    FontSize(f64),
    Color(Option<String>),
    Background(Option<String>),
    Opacity(f64),
    AboveRooms(bool),
    X(f64),
    Y(f64),
    Z(f64),
    Width(f64),
    Height(f64),
    Area(Option<String>),
    /// Back to a text label (the picture goes; empty text becomes "Label").
    UseText,
}

impl LabelChange {
    fn apply(&self, label: &mut MapLabel) {
        match self {
            Self::Text(v) => label.text = v.clone(),
            Self::FontSize(v) => label.font_size = *v,
            Self::Color(v) => label.color = v.clone(),
            Self::Background(v) => label.background = v.clone(),
            Self::Opacity(v) => label.opacity = *v,
            Self::AboveRooms(v) => label.above_rooms = *v,
            Self::X(v) => label.x = *v,
            Self::Y(v) => label.y = *v,
            Self::Z(v) => label.z = *v,
            Self::Width(v) => label.width = *v,
            Self::Height(v) => label.height = *v,
            Self::Area(v) => label.area = v.clone(),
            Self::UseText => {
                label.image = None;
                if label.text.trim().is_empty() {
                    label.text = new_label_text().to_string();
                }
            }
        }
    }
}

/// Something the toolbar, the canvas, the inspector or a menu asks of the map.
#[derive(Clone, Debug, PartialEq)]
pub enum EditAction {
    /// A new room at these map coordinates on the shown area and floor, selected, its name
    /// focused.
    AddRoomAt(f64, f64),
    /// An exit between two rooms, with the way back unless one-way.
    Connect {
        from: String,
        to: String,
        direction: String,
        two_way: bool,
    },
    /// Move rooms by an offset (locked rooms stay).
    MoveRooms(Vec<String>, f64, f64, f64),
    DeleteRooms(Vec<String>),
    DeleteExit(String, String),
    EditRooms(Vec<String>, RoomChange),
    EditExit(String, String, ExitChange),
    /// Make an exit two-way (true) or one-way.
    SetTwoWay(String, String, bool),
    SetCurrentRoom(String),
    /// Select a room found by the overview's search, showing its area and floor.
    Select(String),
    /// Grid mode for an area.
    GridMode(String, bool),
    /// Merge the first room into the second.
    Merge(String, String),
    Undo,
    Redo,
    /// A map file's text to import.
    Import(String),
    /// A map file that could not be read (the reason).
    ImportFailed(String),
    /// Export to this file.
    Export(PathBuf),
    /// A new text label with its top left corner here (shown area and floor), selected, its
    /// text focused.
    AddLabelAt(f64, f64),
    /// A new picture label here, sized to the picture.
    AddPictureLabelAt(f64, f64, MapImage),
    /// Give a label this picture.
    SetLabelPicture(String, MapImage),
    EditLabel(String, LabelChange),
    /// Move a label by an offset.
    MoveLabel(String, f64, f64),
    /// Set a label's size.
    ResizeLabel(String, f64, f64),
    DeleteLabel(String),
    /// A picture that could not be used (the reason).
    PictureFailed(String),
    /// Open File > Import map for this map (the overview's Import).
    OpenImport,
}

/// What an edit needs from the session besides the map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// An import replaced the map: stop a walk.
    pub stop_walk: bool,
    /// Terrain or text may have changed: scan the map for inference.
    pub rescan: bool,
    /// Rooms, exits or a label were deleted (the app offers the undo toast).
    pub deleted: Option<Deleted>,
    /// The overview's Import was pressed (the app opens File > Import map for this map).
    pub open_import: bool,
}

/// What a delete removed, and its undo step ([`RoomMapTracker::last_edit`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Deleted {
    pub rooms: usize,
    pub exits: usize,
    pub labels: usize,
    pub edit: u64,
}

/// Keep the editor's selection in step with the view's (a search result, a floor badge, a
/// floor change that cleared it) and drop what no longer exists.
pub fn sync(state: &mut MapViewState, tracker: &RoomMapTracker) {
    let selected = state.selected.clone();
    let Some(editor) = state.editor.as_deref_mut() else {
        return;
    };
    editor.rooms.retain(|id| tracker.room(id).is_some());
    if editor.exit.as_ref().is_some_and(|(f, d)| tracker.link(f, d).is_none()) {
        editor.exit = None;
    }
    if editor.label.as_deref().is_some_and(|id| tracker.label(id).is_none()) {
        editor.label = None;
    }
    if selected.as_deref() != editor.primary()
        && !(selected.is_none() && (editor.exit.is_some() || editor.label.is_some()))
    {
        editor.select_only(selected.as_deref());
    }
    editor.keep_drafts_for_selection();
}

/// After the editor changed its selection: the view's single selection is the primary room.
pub fn publish(state: &mut MapViewState) {
    if let Some(editor) = state.editor.as_deref_mut() {
        editor.keep_drafts_for_selection();
        state.selected = editor.primary().map(str::to_string);
    }
}

/// Show the primary room's area and floor when an edit moved it elsewhere, keeping the
/// selection.
fn follow(state: &mut MapViewState, tracker: &RoomMapTracker) {
    let Some(editor) = state.editor.as_deref() else {
        return;
    };
    let rooms = editor.rooms.clone();
    let Some(room) = editor.primary().and_then(|id| tracker.room(id)) else {
        return;
    };
    if room.area_key() != state.area || room.z != state.floor {
        let (area, z, x, y) = (room.area_key().to_string(), room.z, room.x, room.y);
        state.set_area(tracker, &area);
        state.set_floor(z);
        state.fit_floor = false;
        state.center_on_floor(tracker, Some((x, y)));
        // Rooms left on another floor or area drop out of the selection.
        let kept: Vec<String> = rooms
            .into_iter()
            .filter(|id| tracker.room(id).is_some_and(|r| r.area_key() == area && r.z == z))
            .collect();
        if let Some(e) = state.editor.as_deref_mut() {
            e.rooms = kept;
        }
    }
    publish(state);
}

/// A new room's name.
pub fn new_room_name() -> &'static str {
    t(S::MapNewRoomName)
}

/// A new label's text.
pub fn new_label_text() -> &'static str {
    t(S::MapNewLabelText)
}

/// A picture label's size in map cells: the picture at zoom 1 (96 points a cell), at most 6
/// cells on its longer side.
pub fn picture_size(image: &MapImage) -> (f64, f64) {
    let (w, h) = wandur_core::map::images::dimensions(&image.data).unwrap_or((96, 96));
    let (w, h) = (f64::from(w.max(1)) / 96.0, f64::from(h.max(1)) / 96.0);
    let scale = (6.0 / w.max(h)).min(1.0);
    (
        ((w * scale) * 10.0).round().max(1.0) / 10.0,
        ((h * scale) * 10.0).round().max(1.0) / 10.0,
    )
}

/// Apply an edit to the live map.
pub fn apply(state: &mut MapViewState, tracker: &mut RoomMapTracker, action: EditAction) -> Outcome {
    let mut outcome = Outcome::default();
    if state.editor.is_none() {
        return outcome;
    }
    let say = |state: &mut MapViewState, text: String| {
        if let Some(e) = state.editor.as_deref_mut() {
            e.message = Some(text);
        }
    };
    match action {
        EditAction::AddRoomAt(x, y) => {
            if !(x.is_finite() && y.is_finite()) {
                return outcome;
            }
            let id = format!("manual:{}", uuid::Uuid::new_v4().simple());
            let area = (!state.area.is_empty()).then(|| state.area.clone());
            let mut room = MapRoom::new(&id, new_room_name(), "", area.as_deref(), x, y, state.floor, false);
            room.is_manually_edited = true;
            if tracker.upsert_room(room) {
                if let Some(e) = state.editor.as_deref_mut() {
                    e.select_only(Some(&id));
                    e.focus_name = true;
                    e.message = None;
                }
                publish(state);
                outcome.rescan = true;
            } else {
                say(state, t(S::MapEditRejected).into());
            }
        }
        EditAction::Connect {
            from,
            to,
            direction,
            two_way,
        } => {
            if tracker.connect_rooms(&from, &to, &direction, two_way) {
                if let Some(e) = state.editor.as_deref_mut() {
                    e.message = None;
                }
            } else {
                say(state, t(S::MapEditRejected).into());
            }
        }
        EditAction::MoveRooms(ids, dx, dy, dz) => {
            let movable: Vec<String> = ids
                .iter()
                .filter(|id| tracker.room(id).is_some_and(|r| !r.is_locked))
                .cloned()
                .collect();
            if movable.len() < ids.len() {
                say(state, t(S::MapRoomIsLocked).into());
            }
            if tracker.move_rooms(&movable, dx, dy, dz) > 0 && dz != 0.0 {
                follow(state, tracker);
            }
        }
        EditAction::DeleteRooms(ids) => {
            let rooms = tracker.remove_rooms(&ids);
            if rooms > 0
                && let Some(edit) = tracker.last_edit()
            {
                outcome.deleted = Some(Deleted {
                    rooms,
                    edit,
                    ..Deleted::default()
                });
            }
            if let Some(e) = state.editor.as_deref_mut() {
                e.rooms.retain(|id| !ids.contains(id));
                e.confirm_delete = None;
            }
            publish(state);
        }
        EditAction::DeleteExit(from, direction) => {
            if tracker.remove_link(&from, &direction)
                && let Some(edit) = tracker.last_edit()
            {
                outcome.deleted = Some(Deleted {
                    exits: 1,
                    edit,
                    ..Deleted::default()
                });
            }
            if let Some(e) = state.editor.as_deref_mut()
                && e.exit.as_ref() == Some(&(from, direction))
            {
                e.exit = None;
            }
        }
        EditAction::EditRooms(ids, change) => {
            let ids: Vec<String> = if change.moves() {
                let movable: Vec<String> = ids
                    .iter()
                    .filter(|id| tracker.room(id).is_some_and(|r| !r.is_locked))
                    .cloned()
                    .collect();
                if movable.len() < ids.len() {
                    say(state, t(S::MapRoomIsLocked).into());
                }
                movable
            } else {
                ids
            };
            if ids.is_empty() {
                return outcome;
            }
            let changed = tracker.edit_rooms(&ids, |room| change.apply(room));
            let no_op = ids.iter().all(|id| {
                tracker.room(id).is_some_and(|r| {
                    let mut same = r.clone();
                    change.apply(&mut same);
                    same == *r
                })
            });
            if changed == 0 && !no_op {
                say(state, t(S::MapEditRejected).into());
            }
            outcome.rescan = matches!(
                change,
                RoomChange::Name(_) | RoomChange::Description(_) | RoomChange::Terrain(_)
            );
            if matches!(change, RoomChange::Area(_) | RoomChange::Z(_)) {
                follow(state, tracker);
            }
        }
        EditAction::EditExit(from, direction, change) => {
            let renamed = match &change {
                ExitChange::Direction(d) => Some(d.trim().to_string()),
                _ => None,
            };
            let done = tracker.edit_exit(&from, &direction, |link| match change {
                ExitChange::Direction(d) => link.direction = d.trim().to_string(),
                ExitChange::Destination(to) => link.to_id = to,
                ExitChange::Door(door) => link.door_state = door,
                ExitChange::Cost(cost) => link.weight = cost,
                ExitChange::Excluded(on) => link.is_locked = on,
                ExitChange::Command(c) => link.command = c,
            });
            if !done {
                say(state, t(S::MapEditRejected).into());
            } else if let (Some(d), Some(e)) = (renamed, state.editor.as_deref_mut())
                && e.exit.as_ref() == Some(&(from.clone(), direction.clone()))
            {
                e.exit = Some((from, d));
                e.keep_drafts_for_selection();
            }
        }
        EditAction::SetTwoWay(from, direction, two_way) => {
            if !tracker.set_two_way(&from, &direction, two_way) {
                say(state, t(S::MapEditRejected).into());
            }
        }
        EditAction::SetCurrentRoom(id) => {
            tracker.set_current_room(&id);
        }
        EditAction::Select(id) => {
            state.select_room(tracker, &id);
            sync(state, tracker);
        }
        EditAction::GridMode(area, on) => {
            tracker.set_area_settings(wandur_core::map::MapAreaSettings::new(&area, on));
            state.fit_floor = true;
        }
        EditAction::Merge(source, target) => {
            if let Some(e) = state.editor.as_deref_mut() {
                e.merge_source = None;
            }
            if tracker.merge_rooms(&source, &target) {
                if let Some(e) = state.editor.as_deref_mut() {
                    e.select_only(Some(&target));
                }
                publish(state);
            } else {
                say(state, t(S::MapEditRejected).into());
            }
        }
        EditAction::Undo | EditAction::Redo => {
            let done = if action == EditAction::Undo {
                tracker.undo()
            } else {
                tracker.redo()
            };
            if done {
                outcome.rescan = true;
                if let Some(e) = state.editor.as_deref_mut() {
                    e.drafts.clear();
                    e.errors.clear();
                }
                sync(state, tracker);
                publish(state);
            }
        }
        EditAction::Import(json) => match format::deserialize(&json) {
            Err(e) => say(state, tf(S::MapImportFailed, &[&e])),
            Ok(map) => {
                // The C# `ImportMap`: walking stops, the selection goes, one undoable replace.
                outcome.stop_walk = true;
                outcome.rescan = true;
                reset_after_import(state);
                let text = if tracker.replace_map(&map) {
                    t(S::MapImportComplete)
                } else {
                    t(S::MapEditRejected)
                };
                say(state, text.into());
            }
        },
        EditAction::ImportFailed(reason) => say(state, tf(S::MapImportFailed, &[&reason])),
        EditAction::AddLabelAt(x, y) => {
            let id = format!("label:{}", uuid::Uuid::new_v4().simple());
            let area = (!state.area.is_empty()).then(|| state.area.clone());
            let label = MapLabel::text(&id, area.as_deref(), x, y, state.floor, new_label_text());
            if tracker.upsert_label(label) {
                if let Some(e) = state.editor.as_deref_mut() {
                    e.select_label(&id);
                    e.focus_name = true;
                    e.message = None;
                    e.tool = Tool::Select;
                }
                publish(state);
            } else {
                say(state, t(S::MapEditRejected).into());
            }
        }
        EditAction::AddPictureLabelAt(x, y, image) => {
            let id = format!("label:{}", uuid::Uuid::new_v4().simple());
            let area = (!state.area.is_empty()).then(|| state.area.clone());
            let (w, h) = picture_size(&image);
            let label = MapLabel {
                width: w,
                height: h,
                image: Some(image.hash.clone()),
                ..MapLabel::text(&id, area.as_deref(), x, y, state.floor, "")
            };
            if tracker.place_picture_label(label, image) {
                if let Some(e) = state.editor.as_deref_mut() {
                    e.select_label(&id);
                    e.message = None;
                    e.tool = Tool::Select;
                }
                publish(state);
            } else {
                say(state, t(S::MapImageTooLarge).into());
            }
        }
        EditAction::SetLabelPicture(id, image) => {
            let Some(label) = tracker.label(&id).cloned() else {
                return outcome;
            };
            let (w, h) = picture_size(&image);
            let pictured = MapLabel {
                image: Some(image.hash.clone()),
                width: if label.image.is_some() { label.width } else { w },
                height: if label.image.is_some() { label.height } else { h },
                ..label
            };
            if !tracker.place_picture_label(pictured, image) {
                say(state, t(S::MapImageTooLarge).into());
            }
        }
        EditAction::EditLabel(id, change) => {
            let follows = matches!(change, LabelChange::Area(_) | LabelChange::Z(_));
            if !tracker.edit_label(&id, |label| change.apply(label)) {
                say(state, t(S::MapEditRejected).into());
            } else if follows && let Some(label) = tracker.label(&id) {
                let (area, z, x, y) = (label.area_key().to_string(), label.z, label.x, label.y);
                state.set_area(tracker, &area);
                state.set_floor(z);
                state.fit_floor = false;
                state.center_on_floor(tracker, Some((x, y)));
                if let Some(e) = state.editor.as_deref_mut() {
                    e.select_label(&id);
                }
            }
        }
        EditAction::MoveLabel(id, dx, dy) => {
            if !tracker.edit_label(&id, |label| {
                label.x += dx;
                label.y += dy;
            }) {
                say(state, t(S::MapEditRejected).into());
            }
        }
        EditAction::ResizeLabel(id, w, h) => {
            if !tracker.edit_label(&id, |label| {
                label.width = w;
                label.height = h;
            }) {
                say(state, t(S::MapEditRejected).into());
            }
        }
        EditAction::DeleteLabel(id) => {
            if tracker.remove_label(&id)
                && let Some(edit) = tracker.last_edit()
            {
                outcome.deleted = Some(Deleted {
                    labels: 1,
                    edit,
                    ..Deleted::default()
                });
            }
            if let Some(e) = state.editor.as_deref_mut()
                && e.label.as_deref() == Some(id.as_str())
            {
                e.label = None;
            }
        }
        EditAction::PictureFailed(reason) => say(state, reason),
        EditAction::OpenImport => outcome.open_import = true,
        EditAction::Export(path) => {
            let text = match export_to(tracker, &path) {
                Ok(()) => t(S::MapExportComplete).into(),
                Err(e) => tf(S::MapExportFailed, &[&e]),
            };
            say(state, text);
        }
    }
    outcome
}

/// After an import: the selection and the planned route go, and the view finds the map again.
pub fn reset_after_import(state: &mut MapViewState) {
    if let Some(e) = state.editor.as_deref_mut() {
        e.select_only(None);
        e.tool = Tool::Select;
        e.gesture = Gesture::None;
    }
    state.selected = None;
    state.clear_route();
    state.has_centered = false;
    state.last_floor = None;
    state.last_area = None;
}

/// The map as a file at `path` (version 1, or 2 with labels).
pub fn export_to(tracker: &RoomMapTracker, path: &Path) -> Result<(), String> {
    let json = format::serialize(&tracker.snapshot()).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

/// A picture file for a label: read (at most 64 MiB) and checked, scaled down when too large.
pub fn read_picture(path: &Path) -> Result<MapImage, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(t(S::MapImageTooLarge).into());
    }
    wandur_core::map::images::prepare(&bytes).map_err(|e| e.to_string())
}

/// The picture file picker.
#[cfg(feature = "native-dialogs")]
pub fn pick_picture() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title(t(S::MapChoosePicture).trim_end_matches('…'))
        .add_filter(t(S::MapPictureFileType), &["png", "jpg", "jpeg"])
        .pick_file()
}

#[cfg(not(feature = "native-dialogs"))]
pub fn pick_picture() -> Option<PathBuf> {
    None
}

/// Pick a picture and read it: the action to take (none when nothing was picked).
pub fn picture_action(make: impl FnOnce(MapImage) -> EditAction) -> Option<EditAction> {
    let path = pick_picture()?;
    Some(match read_picture(&path) {
        Ok(image) => make(image),
        Err(e) => EditAction::PictureFailed(e),
    })
}

/// A map file's text: at most 64 MiB, UTF-8.
pub fn read_map_file(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err(t(S::MapFileType).into());
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

#[cfg(feature = "native-dialogs")]
fn pick_export() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title(t(S::MapExport).trim_end_matches('…'))
        .set_file_name("wandur-map.json")
        .add_filter(t(S::MapFileType), &["json"])
        .save_file()
}

#[cfg(not(feature = "native-dialogs"))]
fn pick_export() -> Option<PathBuf> {
    None
}

/// A bordered section with a header that opens and closes it (the C# `Expander`; Map tools'
/// Room terrain inference uses it).
pub fn expander(ui: &mut egui::Ui, theme: &Theme, title: &str, open: &mut bool, body: impl FnOnce(&mut egui::Ui)) {
    use egui::{CornerRadius, RichText, Sense, Stroke, vec2};
    egui::Frame::new()
        .stroke(Stroke::new(1.0, theme.border))
        .fill(theme.panel)
        .corner_radius(CornerRadius::same(4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let header = ui
                .horizontal(|ui| {
                    ui.set_min_height(44.0);
                    ui.add_space(16.0);
                    ui.label(RichText::new(title).size(13.0).color(theme.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(12.0);
                        let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                        crate::widgets::paint_icon(
                            ui,
                            if *open {
                                crate::widgets::Icon::ChevronUp
                            } else {
                                crate::widgets::Icon::ChevronDown
                            },
                            rect,
                            theme.text,
                        );
                    });
                })
                .response
                .interact(Sense::click());
            crate::a11y::toggle(&header, egui::accesskit::Role::Button, title, *open);
            if header.clicked() {
                *open = !*open;
            }
            if !*open {
                return;
            }
            ui.painter().hline(
                ui.min_rect().x_range(),
                ui.min_rect().bottom(),
                Stroke::new(1.0, theme.border),
            );
            egui::Frame::new().inner_margin(egui::Margin::same(16)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 7.0;
                body(ui);
            });
        });
}

/// The overview's room search: rooms whose name, id, server id, description or notes hold
/// the text, by name, the first 50 (the C# editor's `SearchResults`).
pub fn search_results<'a>(tracker: &'a RoomMapTracker, query: &str) -> Vec<&'a MapRoom> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<&MapRoom> = tracker
        .rooms()
        .filter(|r| {
            [
                Some(r.name.as_str()),
                Some(r.id.as_str()),
                r.server_id.as_deref(),
                Some(r.description.as_str()),
                Some(r.notes.as_str()),
            ]
            .into_iter()
            .flatten()
            .any(|text| text.to_lowercase().contains(&q))
        })
        .collect();
    found.sort_by_cached_key(|r| r.name.to_lowercase());
    found.truncate(50);
    found
}

/// Rooms whose centres lie inside a screen rectangle (a marquee), from the rooms drawn.
pub fn rooms_in(hits: &[(String, Rect)], area: Rect) -> Vec<String> {
    hits.iter()
        .filter(|(_, r)| area.contains(r.center()))
        .map(|(id, _)| id.clone())
        .collect()
}

#[cfg(test)]
mod tests;
