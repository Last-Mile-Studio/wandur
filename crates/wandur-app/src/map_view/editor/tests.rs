//! The map editor: its actions through [`apply`] (each one undo step), and the toolbar, the
//! canvas tools, the keys, the context menu and the inspector driven with real input events
//! over a session's full map.

use super::*;
use crate::map_view::{MapContext, show};
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use wandur_core::map::{MapLink, MapSnapshot, TrackingState};

/// A 3 by 2 block in one area, and a room on the floor above:
///
/// ```text
///   d (0,1)   e (1,1)   f (2,1)
///   a (0,0) = b (1,0)   c (2,0)        g (0,0) on floor 1
/// ```
/// a and b are joined both ways; the player is in a.
fn block() -> RoomMapTracker {
    let r = |id: &str, name: &str, x: f64, y: f64, z: f64| MapRoom::new(id, name, "", Some("Keep"), x, y, z, false);
    let mut tracker = RoomMapTracker::from_snapshot(MapSnapshot::of(
        vec![
            r("a", "Atrium", 0.0, 0.0, 0.0),
            r("b", "Barracks", 1.0, 0.0, 0.0),
            r("c", "Chapel", 2.0, 0.0, 0.0),
            r("d", "Dais", 0.0, 1.0, 0.0),
            r("e", "Eyrie", 1.0, 1.0, 0.0),
            r("f", "Forge", 2.0, 1.0, 0.0),
            r("g", "Gallery", 0.0, 0.0, 1.0),
        ],
        vec![
            MapLink::new("a", "b", "east", true),
            MapLink::new("b", "a", "west", true),
        ],
    ));
    tracker.set_current_room("a");
    tracker
}

fn editing(tracker: &RoomMapTracker) -> MapViewState {
    let mut state = MapViewState::editor();
    state.refresh(tracker, true);
    state
}

fn ed(state: &mut MapViewState) -> &mut EditorState {
    state.editor.as_deref_mut().unwrap()
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

// ---- actions ------------------------------------------------------------------------------

#[test]
fn adding_a_room_puts_it_on_the_cell_selects_it_and_marks_it_manual() {
    let mut tracker = block();
    let mut state = editing(&tracker);
    apply(&mut state, &mut tracker, EditAction::AddRoomAt(3.0, -1.0));
    let id = ed(&mut state).rooms[0].clone();
    let room = tracker.room(&id).unwrap();
    assert!(id.starts_with("manual:") && id.len() == 39, "{id}");
    assert_eq!((room.x, room.y, room.z), (3.0, -1.0, 0.0));
    assert_eq!(room.area.as_deref(), Some("Keep"), "the shown area");
    assert_eq!(room.name, "New room");
    assert!(room.is_manually_edited);
    assert!(ed(&mut state).focus_name, "the name is focused for typing");
    assert_eq!(state.selected.as_deref(), Some(id.as_str()));
    assert!(tracker.undo());
    assert!(tracker.room(&id).is_none());
}

#[test]
fn inspector_edits_apply_to_the_selection_and_undo_one_at_a_time() {
    let mut tracker = block();
    let mut state = editing(&tracker);
    let one = ids(&["c"]);
    for change in [
        RoomChange::Name("Chantry".into()),
        RoomChange::Terrain(Some("forest".into())),
        RoomChange::Color(Some("#4A90C8".into())),
        RoomChange::Symbol(Some("✝".into())),
        RoomChange::X(5.0),
        RoomChange::Y(-2.0),
        RoomChange::Cost(3.0),
        RoomChange::Notes("Bells at dusk".into()),
    ] {
        apply(&mut state, &mut tracker, EditAction::EditRooms(one.clone(), change));
    }
    let c = tracker.room("c").unwrap().clone();
    assert_eq!(c.name, "Chantry");
    assert_eq!(c.environment.as_deref(), Some("forest"));
    assert_eq!(c.color.as_deref(), Some("#4A90C8"));
    assert_eq!(c.symbol.as_deref(), Some("✝"));
    assert_eq!((c.x, c.y, c.weight), (5.0, -2.0, 3.0));
    assert_eq!(c.notes, "Bells at dusk");
    // Each was its own step: undo takes back the notes, then the cost.
    assert!(tracker.undo());
    assert_eq!(tracker.room("c").unwrap().notes, "");
    assert!(tracker.undo());
    assert_eq!(tracker.room("c").unwrap().weight, 1.0);
    assert_eq!(tracker.room("c").unwrap().x, 5.0);
    // A new area moves the view with the room, keeping it selected.
    ed(&mut state).select_only(Some("c"));
    publish(&mut state);
    apply(
        &mut state,
        &mut tracker,
        EditAction::EditRooms(one.clone(), RoomChange::Area(Some("Garden".into()))),
    );
    assert_eq!(state.area, "Garden");
    assert_eq!(ed(&mut state).rooms, one);
    // Another floor too.
    apply(
        &mut state,
        &mut tracker,
        EditAction::EditRooms(one.clone(), RoomChange::Z(2.0)),
    );
    assert_eq!((state.area.as_str(), state.floor), ("Garden", 2.0));
    // A bad value is refused with a message and changes nothing.
    apply(
        &mut state,
        &mut tracker,
        EditAction::EditRooms(one.clone(), RoomChange::Name(" ".into())),
    );
    assert_eq!(tracker.room("c").unwrap().name, "Chantry");
    assert!(ed(&mut state).message.is_some());
}

#[test]
fn several_rooms_are_edited_together_in_one_step_and_locked_rooms_stay_in_place() {
    let mut tracker = block();
    let mut state = editing(&tracker);
    let three = ids(&["d", "e", "f"]);
    apply(
        &mut state,
        &mut tracker,
        EditAction::EditRooms(three.clone(), RoomChange::Terrain(Some("swamp".into()))),
    );
    assert!(
        three
            .iter()
            .all(|id| tracker.room(id).unwrap().environment.as_deref() == Some("swamp"))
    );
    assert!(tracker.undo());
    assert!(
        three.iter().all(|id| tracker.room(id).unwrap().environment.is_none()),
        "one step"
    );
    apply(
        &mut state,
        &mut tracker,
        EditAction::EditRooms(ids(&["f"]), RoomChange::Locked(true)),
    );
    apply(
        &mut state,
        &mut tracker,
        EditAction::MoveRooms(three.clone(), 0.0, 2.0, 0.0),
    );
    assert_eq!(tracker.room("d").unwrap().y, 3.0);
    assert_eq!(tracker.room("f").unwrap().y, 1.0, "locked");
    assert_eq!(
        ed(&mut state).message.as_deref(),
        Some("Locked rooms stay in place. Unlock to move them.")
    );
    assert!(tracker.undo());
    assert_eq!(tracker.room("d").unwrap().y, 1.0);
}

#[test]
fn exits_are_edited_renamed_made_one_way_and_deleted_with_undo() {
    let mut tracker = block();
    let mut state = editing(&tracker);
    let edit = |c: ExitChange| EditAction::EditExit("a".into(), "east".into(), c);
    apply(&mut state, &mut tracker, edit(ExitChange::Door(DoorState::Closed)));
    apply(&mut state, &mut tracker, edit(ExitChange::Cost(4.0)));
    apply(
        &mut state,
        &mut tracker,
        edit(ExitChange::Command(Some("unlock door".into()))),
    );
    let east = tracker.link("a", "east").unwrap();
    assert_eq!((east.door_state, east.weight), (DoorState::Closed, 4.0));
    assert_eq!(east.command.as_deref(), Some("unlock door"));
    assert!(tracker.undo());
    assert!(tracker.link("a", "east").unwrap().command.is_none());
    // Renaming keeps the exit selected under its new direction.
    ed(&mut state).select_exit("a", "east");
    apply(&mut state, &mut tracker, edit(ExitChange::Direction("gate".into())));
    assert!(tracker.link("a", "gate").is_some());
    assert_eq!(ed(&mut state).exit, Some(("a".into(), "gate".into())));
    apply(
        &mut state,
        &mut tracker,
        EditAction::SetTwoWay("a".into(), "gate".into(), false),
    );
    assert!(tracker.return_links("a", "gate").is_empty());
    apply(
        &mut state,
        &mut tracker,
        EditAction::DeleteExit("a".into(), "gate".into()),
    );
    assert!(tracker.link("a", "gate").is_none());
    assert!(ed(&mut state).exit.is_none());
    assert!(tracker.undo());
    assert!(tracker.link("a", "gate").is_some());
}

#[test]
fn context_menu_actions_set_the_position_and_merge() {
    let mut tracker = block();
    let mut state = editing(&tracker);
    apply(&mut state, &mut tracker, EditAction::SetCurrentRoom("e".into()));
    assert_eq!(tracker.current_id(), Some("e"));
    assert_eq!(tracker.state(), TrackingState::Inferred);
    apply(&mut state, &mut tracker, EditAction::Merge("f".into(), "c".into()));
    assert!(tracker.room("f").is_none());
    assert_eq!(ed(&mut state).rooms, ids(&["c"]));
    assert!(tracker.undo());
    assert!(tracker.room("f").is_some());
}

#[test]
fn export_then_import_replaces_the_map_as_one_undoable_edit() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.superpowers/test-data")
        .join(format!("map-editor-files-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut tracker = block();
    let mut state = editing(&tracker);
    let path = dir.join("wandur-map.json");
    apply(&mut state, &mut tracker, EditAction::Export(path.clone()));
    assert_eq!(ed(&mut state).message.as_deref(), Some("Map exported."));
    let text = read_map_file(&path).unwrap();
    let other = format::serialize(&MapSnapshot::of(
        vec![MapRoom::new("x", "Harbor", "", None, 0.0, 0.0, 0.0, false)],
        Vec::new(),
    ))
    .unwrap();
    ed(&mut state).select_only(Some("a"));
    let outcome = apply(&mut state, &mut tracker, EditAction::Import(other));
    assert!(outcome.stop_walk && outcome.rescan);
    assert_eq!(tracker.room_count(), 1);
    assert!(ed(&mut state).rooms.is_empty());
    apply(&mut state, &mut tracker, EditAction::Undo);
    assert_eq!(tracker.room_count(), 7);
    apply(&mut state, &mut tracker, EditAction::Import(text));
    assert_eq!(tracker.room_count(), 7);
    apply(&mut state, &mut tracker, EditAction::Import("not json".into()));
    assert!(
        ed(&mut state)
            .message
            .clone()
            .unwrap()
            .starts_with("Could not import this map")
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ---- drawn and driven with input ------------------------------------------------------------

/// A session's full map with Edit on, drawn frame by frame in one egui context.
struct Harness {
    ctx: egui::Context,
    tab: crate::session_tab::SessionTab,
    state: MapViewState,
    theme: Theme,
    time: f64,
    texts: Vec<String>,
    prefs: wandur_core::settings::MapEditorPrefs,
}

impl Harness {
    fn new() -> Self {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let mut tab = crate::session_tab::SessionTab::demo(1, &crate::session_tab::TabOptions::default());
        tab.map = wandur_core::map::MapSession::with_map(block().snapshot());
        tab.map.tracker_mut().set_current_room("a");
        let mut state = MapViewState::full();
        state.set_editing(true);
        let mut h = Self {
            ctx,
            tab,
            state,
            theme: Theme::ember(),
            time: 0.0,
            texts: Vec::new(),
            prefs: Default::default(),
        };
        h.frame(Vec::new());
        h.frame(Vec::new());
        h.state.fit(h.tab.map.tracker());
        h.frame(Vec::new());
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.frame_with(events, Modifiers::NONE);
    }

    fn frame_with(&mut self, events: Vec<Event>, modifiers: Modifiers) {
        self.time += 0.05;
        let mut all = vec![Event::ModifiersChanged(modifiers)];
        all.extend(events);
        let input = egui::RawInput {
            time: Some(self.time),
            events: all,
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 900.0))),
            ..Default::default()
        };
        let (tab, state, theme, prefs) = (&mut self.tab, &mut self.state, &self.theme, &mut self.prefs);
        let mut auto = false;
        let mut output = self.ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let mut cx = MapContext::new(theme, &mut auto);
                cx.editor_prefs = Some(&mut *prefs);
                show(ui, Some(&mut *tab), state, &mut cx);
            });
        });
        output.textures_delta.clear();
        self.texts = output
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
    }

    fn at(&self, x: f64, y: f64) -> Pos2 {
        self.state.project(self.state.canvas, x, y)
    }

    fn tracker(&self) -> &RoomMapTracker {
        self.tab.map.tracker()
    }

    fn editor(&mut self) -> &mut EditorState {
        self.state.editor.as_deref_mut().unwrap()
    }

    fn click(&mut self, p: Pos2, modifiers: Modifiers) {
        self.frame_with(vec![Event::PointerMoved(p), button(p, true, modifiers)], modifiers);
        self.frame_with(vec![button(p, false, modifiers)], modifiers);
        self.frame(Vec::new());
    }

    fn right_click(&mut self, p: Pos2) {
        let b = |pressed| Event::PointerButton {
            pos: p,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(vec![Event::PointerMoved(p), b(true)]);
        self.frame(vec![b(false)]);
        self.frame(Vec::new());
    }

    fn drag(&mut self, from: Pos2, to: Pos2, modifiers: Modifiers) {
        let middle = from + (to - from) / 2.0;
        self.frame_with(
            vec![Event::PointerMoved(from), button(from, true, modifiers)],
            modifiers,
        );
        self.frame_with(vec![Event::PointerMoved(from + (middle - from) / 4.0)], modifiers);
        self.frame_with(vec![Event::PointerMoved(middle)], modifiers);
        self.frame_with(vec![Event::PointerMoved(to)], modifiers);
        self.frame_with(vec![button(to, false, modifiers)], modifiers);
        self.frame(Vec::new());
    }

    fn key(&mut self, key: egui::Key, modifiers: Modifiers) {
        self.frame_with(
            vec![Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            modifiers,
        );
        self.frame(Vec::new());
    }

    fn has_text(&self, want: &str) -> bool {
        self.texts.iter().any(|t| t == want)
    }
}

fn button(pos: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers,
    }
}

#[test]
fn the_toolbar_and_the_inspector_show_with_edit_and_hide_without_it() {
    let mut h = Harness::new();
    assert!(h.has_text("Properties"), "{:?}", h.texts);
    assert!(h.has_text("Shortcuts"));
    let canvas = h.state.canvas;
    h.state.set_editing(false);
    h.frame(Vec::new());
    assert!(!h.has_text("Properties"));
    assert!(h.state.canvas.width() > canvas.width() && h.state.canvas.height() > canvas.height());
}

#[test]
fn clicking_selects_shift_adds_and_a_marquee_selects_an_area() {
    let mut h = Harness::new();
    let a = h.at(0.0, 0.0);
    h.click(a, Modifiers::NONE);
    assert_eq!(h.editor().rooms, ids(&["a"]));
    assert!(h.has_text("Atrium") && h.has_text("Room"));
    let c = h.at(2.0, 0.0);
    h.click(c, Modifiers::SHIFT);
    assert_eq!(h.editor().rooms, ids(&["a", "c"]));
    assert!(h.has_text("2 rooms selected"), "{:?}", h.texts);
    h.click(c, Modifiers::SHIFT);
    assert_eq!(h.editor().rooms, ids(&["a"]), "Shift-click again takes it out");
    // A marquee from empty space around the top row.
    let from = h.at(-0.5, 1.5);
    let to = h.at(2.5, 0.6);
    h.drag(from, to, Modifiers::NONE);
    let mut picked = h.editor().rooms.clone();
    picked.sort();
    assert_eq!(picked, ids(&["d", "e", "f"]));
    // Clicking empty space clears it.
    let empty = h.at(3.0, -1.0);
    h.click(empty, Modifiers::NONE);
    assert!(h.editor().rooms.is_empty());
}

#[test]
fn dragging_a_selection_moves_every_room_as_one_snapped_undo_step() {
    let mut h = Harness::new();
    h.editor().rooms = ids(&["d", "e"]);
    publish(&mut h.state);
    h.frame(Vec::new());
    let from = h.at(0.0, 1.0);
    let to = h.at(2.2, 2.9);
    h.drag(from, to, Modifiers::NONE);
    let t = h.tracker();
    assert_eq!(t.room("d").map(|r| (r.x, r.y)), Some((2.0, 3.0)));
    assert_eq!(t.room("e").map(|r| (r.x, r.y)), Some((3.0, 3.0)));
    assert!(t.room("d").unwrap().is_manually_edited);
    assert!(h.tab.map.tracker_mut().undo());
    let t = h.tracker();
    assert_eq!(t.room("d").map(|r| (r.x, r.y)), Some((0.0, 1.0)));
    assert_eq!(t.room("e").map(|r| (r.x, r.y)), Some((1.0, 1.0)));
    assert!(!t.can_undo(), "one step for both");
}

#[test]
fn the_add_room_tool_adds_on_an_empty_cell_and_selects_an_existing_room() {
    let mut h = Harness::new();
    h.key(egui::Key::R, Modifiers::NONE);
    assert_eq!(h.editor().tool, Tool::AddRoom);
    let cell = h.at(3.1, -0.9);
    h.click(cell, Modifiers::NONE);
    let id = h.editor().rooms[0].clone();
    let room = h.tracker().room(&id).unwrap();
    assert_eq!((room.x, room.y, room.z), (3.0, -1.0, 0.0));
    assert!(room.is_manually_edited);
    let count = h.tracker().room_count();
    // The new room's name has the keyboard: leave it first.
    h.key(egui::Key::Enter, Modifiers::NONE);
    let b = h.at(1.0, 0.0);
    h.click(b, Modifiers::NONE);
    assert_eq!(h.tracker().room_count(), count);
    assert_eq!(h.editor().rooms, ids(&["b"]));
    // Escape returns to Select.
    h.key(egui::Key::Escape, Modifiers::NONE);
    assert_eq!(h.editor().tool, Tool::Select);
}

#[test]
fn connecting_infers_the_direction_and_alt_makes_it_one_way() {
    let mut h = Harness::new();
    h.key(egui::Key::E, Modifiers::NONE);
    assert_eq!(h.editor().tool, Tool::Connect);
    // a (0,0) to e (1,1): northeast, both ways.
    let (a, e) = (h.at(0.0, 0.0), h.at(1.0, 1.0));
    h.drag(a, e, Modifiers::NONE);
    assert_eq!(h.tracker().link("a", "northeast").map(|l| l.to_id.as_str()), Some("e"));
    assert_eq!(h.tracker().link("e", "southwest").map(|l| l.to_id.as_str()), Some("a"));
    assert!(h.tab.map.tracker_mut().undo());
    assert!(h.tracker().link("a", "northeast").is_none() && h.tracker().link("e", "southwest").is_none());
    // c (2,0) to f (2,1) with Alt: north, one-way.
    let (c, f) = (h.at(2.0, 0.0), h.at(2.0, 1.0));
    h.drag(c, f, Modifiers::ALT);
    assert_eq!(h.tracker().link("c", "north").map(|l| l.to_id.as_str()), Some("f"));
    assert!(h.tracker().return_links("c", "north").is_empty());
    // Dropped on empty space: nothing.
    let links = h.tracker().link_count();
    let (d, nowhere) = (h.at(0.0, 1.0), h.at(-2.0, 3.0));
    h.drag(d, nowhere, Modifiers::NONE);
    assert_eq!(h.tracker().link_count(), links);
}

#[test]
fn delete_removes_at_once_up_to_five_rooms_and_asks_for_more_and_undo_restores() {
    let mut h = Harness::new();
    let a = h.at(0.0, 0.0);
    h.click(a, Modifiers::NONE);
    h.key(egui::Key::Delete, Modifiers::NONE);
    assert!(h.tracker().room("a").is_none());
    assert!(h.tracker().link("b", "west").is_none(), "its exits go with it");
    h.key(egui::Key::Z, Modifiers::COMMAND);
    assert!(h.tracker().room("a").is_some() && h.tracker().link("b", "west").is_some());
    h.editor().rooms = ids(&["a", "b", "c", "d", "e", "f"]);
    publish(&mut h.state);
    h.key(egui::Key::Backspace, Modifiers::NONE);
    assert_eq!(h.tracker().room_count(), 7, "asked first");
    assert!(h.has_text("Delete 6 rooms and their exits?"), "{:?}", h.texts);
    h.key(egui::Key::Escape, Modifiers::NONE);
    assert!(h.editor().confirm_delete.is_none());
    assert_eq!(h.tracker().room_count(), 7);
    h.key(egui::Key::Delete, Modifiers::NONE);
    let ids = h.editor().confirm_delete.clone().unwrap();
    h.editor().pending.push(EditAction::DeleteRooms(ids));
    h.frame(Vec::new());
    assert_eq!(h.tracker().room_count(), 1);
    h.key(egui::Key::Z, Modifiers::COMMAND);
    assert_eq!(h.tracker().room_count(), 7, "one step");
    h.key(egui::Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(h.tracker().room_count(), 1, "redo");
}

#[test]
fn arrow_keys_nudge_the_selection_a_cell() {
    let mut h = Harness::new();
    let c = h.at(2.0, 0.0);
    h.click(c, Modifiers::NONE);
    h.key(egui::Key::ArrowRight, Modifiers::NONE);
    h.key(egui::Key::ArrowUp, Modifiers::NONE);
    assert_eq!(h.tracker().room("c").map(|r| (r.x, r.y)), Some((3.0, 1.0)));
}

#[test]
fn typing_a_name_commits_once_on_enter_and_escape_puts_it_back() {
    let mut h = Harness::new();
    let b = h.at(1.0, 0.0);
    h.click(b, Modifiers::NONE);
    h.editor().focus_name = true;
    h.frame(Vec::new());
    h.frame(vec![Event::Text("Gate".into())]);
    h.frame(vec![Event::Text("house".into())]);
    assert_eq!(h.tracker().room("b").unwrap().name, "Barracks", "not per keystroke");
    assert!(!h.tracker().can_undo());
    h.key(egui::Key::Enter, Modifiers::NONE);
    assert_eq!(h.tracker().room("b").unwrap().name, "Gatehouse");
    assert!(h.tab.map.tracker_mut().undo());
    assert_eq!(h.tracker().room("b").unwrap().name, "Barracks");
    assert!(!h.tracker().can_undo(), "one step");
    // Escape while typing puts the value back.
    h.editor().focus_name = true;
    h.frame(Vec::new());
    h.frame(vec![Event::Text("Nope".into())]);
    h.key(egui::Key::Escape, Modifiers::NONE);
    h.frame(Vec::new());
    assert_eq!(h.tracker().room("b").unwrap().name, "Barracks");
    assert!(h.editor().drafts.is_empty());
    // An empty name is flagged and not applied.
    h.editor().focus_name = true;
    h.frame(Vec::new());
    h.key(egui::Key::Backspace, Modifiers::NONE);
    h.key(egui::Key::Enter, Modifiers::NONE);
    assert_eq!(h.tracker().room("b").unwrap().name, "Barracks");
    assert!(h.has_text("A room needs a name."), "{:?}", h.texts);
}

#[test]
fn a_multi_selection_shows_mixed_values_and_move_by() {
    let mut h = Harness::new();
    h.tab
        .map
        .tracker_mut()
        .edit_rooms(&ids(&["d"]), |r| r.environment = Some("forest".into()));
    h.editor().rooms = ids(&["d", "e", "f"]);
    publish(&mut h.state);
    h.frame(Vec::new());
    h.frame(Vec::new());
    assert!(h.has_text("3 rooms selected"));
    assert!(h.has_text("Mixed"), "the names and terrains differ: {:?}", h.texts);
    assert!(h.has_text("Move by"));
    assert!(
        !h.texts.iter().any(|t| t.starts_with("Exits")),
        "no exits for several rooms"
    );
}

#[test]
fn double_clicking_a_room_focuses_its_name_and_walks_nowhere() {
    let mut h = Harness::new();
    let e = h.at(1.0, 1.0);
    h.frame(vec![Event::PointerMoved(e), button(e, true, Modifiers::NONE)]);
    h.frame(vec![button(e, false, Modifiers::NONE)]);
    h.frame(vec![button(e, true, Modifiers::NONE)]);
    h.frame(vec![button(e, false, Modifiers::NONE)]);
    h.frame(Vec::new());
    assert_eq!(h.editor().rooms, ids(&["e"]));
    assert!(
        h.ctx.memory(|m| m.focused()).is_some(),
        "the name field has the keyboard"
    );
    assert_eq!(h.state.walk_requested, 0);
}

#[test]
fn the_context_menu_opens_on_a_room_and_its_items_act() {
    let mut h = Harness::new();
    let f = h.at(2.0, 1.0);
    h.right_click(f);
    assert_eq!(h.editor().rooms, ids(&["f"]));
    for want in [
        "Rename",
        "Connect from here",
        "Set current position here",
        "Lock",
        "Merge into…",
        "Delete room",
    ] {
        assert!(h.has_text(want), "{want}: {:?}", h.texts);
    }
    let menu = h.editor().menu.clone().unwrap();
    // "Lock" is the fourth row: 2 points off the pointer, 5 inside, 29 a row.
    let lock = menu.at + vec2(40.0, 2.0 + 5.0 + 3.5 * 29.0);
    h.click(lock, Modifiers::NONE);
    assert!(h.tracker().room("f").unwrap().is_locked);
    assert!(h.editor().menu.is_none());
    // On an exit: one-way and delete.
    let (a, b) = (h.at(0.0, 0.0), h.at(1.0, 0.0));
    let near_a = a + (b - a) * 0.3;
    h.right_click(near_a);
    assert_eq!(h.editor().exit, Some(("a".into(), "east".into())));
    assert!(h.has_text("Make one-way") && h.has_text("Delete exit"), "{:?}", h.texts);
    let menu = h.editor().menu.clone().unwrap();
    h.click(menu.at + vec2(40.0, 2.0 + 5.0 + 14.0), Modifiers::NONE);
    assert!(h.tracker().return_links("a", "east").is_empty(), "now one-way");
    // A click outside closes it.
    let empty = h.at(-1.0, 2.0);
    h.right_click(empty);
    assert!(h.has_text("Add room here"));
    let outside = h.at(3.0, -1.0);
    h.click(outside, Modifiers::NONE);
    assert!(h.editor().menu.is_none());
}

#[test]
fn the_edit_menus_undo_reaches_the_map_through_pending() {
    let mut h = Harness::new();
    h.tab
        .map
        .tracker_mut()
        .edit_rooms(&ids(&["a"]), |r| r.name = "Hall".into());
    h.editor().pending.push(EditAction::Undo);
    h.frame(Vec::new());
    assert_eq!(h.tracker().room("a").unwrap().name, "Atrium");
    h.editor().pending.push(EditAction::Redo);
    h.frame(Vec::new());
    assert_eq!(h.tracker().room("a").unwrap().name, "Hall");
}

#[test]
fn the_editor_keeps_escape_while_it_has_something_to_step_back_from() {
    let mut h = Harness::new();
    let keeps = |h: &Harness| {
        h.ctx
            .data(|d| d.get_temp::<bool>(crate::map_view::map_keeps_escape_id()))
            .unwrap_or(false)
    };
    assert!(!keeps(&h), "nothing selected: Escape goes back to Play");
    h.key(egui::Key::E, Modifiers::NONE);
    assert!(keeps(&h));
    h.key(egui::Key::Escape, Modifiers::NONE);
    assert!(!keeps(&h));
}

#[test]
fn the_inspector_width_and_closed_sections_come_from_and_go_to_the_settings() {
    let mut h = Harness::new();
    h.prefs = wandur_core::settings::MapEditorPrefs {
        inspector_width: 400.0,
        closed_sections: vec!["keys".into()],
        snap: false,
    };
    h.frame(Vec::new());
    h.frame(Vec::new());
    assert!(
        !h.texts.iter().any(|t| t.starts_with("V  ·")),
        "Shortcuts closed: {:?}",
        h.texts
    );
    assert_eq!(h.editor().prefs.inspector_width, 400.0);
    assert!(!h.editor().prefs.snap);
    // Dragging the splitter 50 points left widens the inspector, and the settings follow.
    let canvas = h.state.canvas;
    let grip = egui::pos2(canvas.right() + 2.5, canvas.center().y);
    h.drag(grip, grip - vec2(50.0, 0.0), Modifiers::NONE);
    assert!(
        (h.prefs.inspector_width - 450.0).abs() < 1.0,
        "{}",
        h.prefs.inspector_width
    );
}

#[test]
fn the_overview_search_finds_ids_server_ids_and_notes_and_shows_the_floor() {
    let mut tracker = block();
    tracker.edit_rooms(&ids(&["g"]), |r| {
        r.notes = "A hidden brass key".into();
        r.server_id = Some("server-gallery".into());
    });
    let mut state = editing(&tracker);
    let found = |q: &str| -> Vec<String> { search_results(&tracker, q).iter().map(|r| r.id.clone()).collect() };
    assert_eq!(found("brass key"), ["g"]);
    assert_eq!(found("server-gallery"), ["g"]);
    assert_eq!(found("chapel"), ["c"]);
    assert!(found("nothing like it").is_empty());
    apply(&mut state, &mut tracker, EditAction::Select("g".into()));
    assert_eq!(state.floor, 1.0, "its floor is shown");
    assert_eq!(ed(&mut state).rooms, ids(&["g"]));
}

// ---- labels ---------------------------------------------------------------------------------

fn picture(w: u32, h: u32) -> wandur_core::map::MapImage {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([x as u8, y as u8, 200, 255]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    wandur_core::map::images::prepare(&out).unwrap()
}

#[test]
fn labels_are_added_edited_moved_resized_and_deleted_each_one_undo_step() {
    let mut tracker = block();
    let mut state = editing(&tracker);
    apply(&mut state, &mut tracker, EditAction::AddLabelAt(0.5, 2.0));
    let id = ed(&mut state).label.clone().expect("the new label is selected");
    assert!(ed(&mut state).rooms.is_empty() && ed(&mut state).focus_name);
    let label = tracker.label(&id).unwrap();
    assert_eq!((label.x, label.y, label.z), (0.5, 2.0, 0.0));
    assert_eq!(label.area.as_deref(), Some("Keep"));
    assert_eq!(label.text, "Label");
    for change in [
        LabelChange::Text("North wing".into()),
        LabelChange::FontSize(20.0),
        LabelChange::Color(Some("#FFFFFF".into())),
        LabelChange::Background(Some("#3B4148".into())),
        LabelChange::Opacity(0.5),
        LabelChange::AboveRooms(true),
    ] {
        apply(&mut state, &mut tracker, EditAction::EditLabel(id.clone(), change));
    }
    apply(&mut state, &mut tracker, EditAction::MoveLabel(id.clone(), 1.0, -0.5));
    apply(&mut state, &mut tracker, EditAction::ResizeLabel(id.clone(), 4.0, 1.5));
    let label = tracker.label(&id).unwrap();
    assert_eq!(label.text, "North wing");
    assert_eq!((label.font_size, label.opacity, label.above_rooms), (20.0, 0.5, true));
    assert_eq!(label.background.as_deref(), Some("#3B4148"));
    assert_eq!((label.x, label.y, label.width, label.height), (1.5, 1.5, 4.0, 1.5));
    // A refused change says so and changes nothing.
    apply(
        &mut state,
        &mut tracker,
        EditAction::EditLabel(id.clone(), LabelChange::Text("  ".into())),
    );
    assert!(ed(&mut state).message.is_some());
    assert_eq!(tracker.label(&id).unwrap().text, "North wing");
    // Delete: the outcome offers the toast; undo brings it back.
    let outcome = apply(&mut state, &mut tracker, EditAction::DeleteLabel(id.clone()));
    let deleted = outcome.deleted.expect("the undo toast");
    assert_eq!((deleted.labels, deleted.rooms, deleted.exits), (1, 0, 0));
    assert_eq!(tracker.last_edit(), Some(deleted.edit));
    assert!(tracker.label(&id).is_none() && ed(&mut state).label.is_none());
    assert!(tracker.undo());
    assert_eq!(tracker.label(&id).unwrap().width, 4.0);
    // Nine changes, nine steps back to nothing.
    for _ in 0..9 {
        assert!(tracker.undo());
    }
    assert!(tracker.label(&id).is_none());
}

#[test]
fn picture_labels_take_the_picture_and_go_back_to_text() {
    let mut tracker = block();
    let mut state = editing(&tracker);
    let image = picture(192, 96);
    apply(
        &mut state,
        &mut tracker,
        EditAction::AddPictureLabelAt(3.0, 1.0, image.clone()),
    );
    let id = ed(&mut state).label.clone().unwrap();
    let label = tracker.label(&id).unwrap();
    assert_eq!(label.image.as_deref(), Some(image.hash.as_str()));
    assert_eq!((label.width, label.height), (2.0, 1.0), "the picture's size at zoom 1");
    assert_eq!(tracker.image(&image.hash), Some(&image));
    // Another picture keeps the label's size; Use text drops the picture.
    let other = picture(10, 10);
    apply(
        &mut state,
        &mut tracker,
        EditAction::SetLabelPicture(id.clone(), other.clone()),
    );
    assert_eq!(tracker.label(&id).unwrap().image.as_deref(), Some(other.hash.as_str()));
    assert!(tracker.image(&image.hash).is_none());
    apply(
        &mut state,
        &mut tracker,
        EditAction::EditLabel(id.clone(), LabelChange::UseText),
    );
    let label = tracker.label(&id).unwrap();
    assert!(label.image.is_none());
    assert_eq!(label.text, "Label");
    assert!(tracker.undo() && tracker.undo());
    assert_eq!(tracker.label(&id).unwrap().image.as_deref(), Some(image.hash.as_str()));
    assert_eq!(tracker.image(&image.hash), Some(&image), "undo brings the picture back");
    // A file that is not a picture says why.
    let dir = std::env::temp_dir().join(format!("wandur-picture-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("x.png"), b"not a picture").unwrap();
    assert!(read_picture(&dir.join("x.png")).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_label_tool_adds_and_labels_are_dragged_resized_and_deleted_on_the_canvas() {
    let mut h = Harness::new();
    // Zoomed out, so the label and its grip stay on the canvas.
    h.state.zoom = 0.4;
    h.state.fit_floor = false;
    h.frame(Vec::new());
    h.key(egui::Key::L, Modifiers::NONE);
    assert_eq!(h.editor().tool, Tool::AddLabel);
    let spot = h.at(0.5, -1.0);
    h.click(spot, Modifiers::NONE);
    let id = h.editor().label.clone().expect("added and selected");
    assert_eq!(h.editor().tool, Tool::Select, "back to Select for moving it");
    let label = h.tracker().label(&id).unwrap().clone();
    assert_eq!((label.x, label.y), (0.5, -1.0));
    // The inspector shows the label's fields.
    h.frame(Vec::new());
    assert!(
        h.has_text("Show above rooms") && h.has_text("Text size"),
        "{:?}",
        h.texts
    );
    // Drag its middle: moved by whole cells (snap is on).
    let middle = h.at(label.x + label.width / 2.0, label.y - label.height / 2.0);
    let to = middle + (h.at(2.0, 0.0) - h.at(0.0, 0.0));
    h.drag(middle, to, Modifiers::NONE);
    let moved = h.tracker().label(&id).unwrap().clone();
    assert_eq!((moved.x, moved.y), (2.5, -1.0));
    // Drag its grip: resized.
    let rect = crate::map_view::labels::label_rect(&h.state, h.state.canvas, &moved);
    let grip = crate::map_view::labels::grip_rect(rect).center();
    let wider = grip + (h.at(1.0, -1.0) - h.at(0.0, 0.0));
    h.drag(grip, wider, Modifiers::NONE);
    let sized = h.tracker().label(&id).unwrap().clone();
    assert_eq!((sized.width, sized.height), (moved.width + 1.0, moved.height + 1.0));
    // Delete removes it (once its text field, focused for typing when it was added, lets go).
    h.key(egui::Key::Escape, Modifiers::NONE);
    assert_eq!(h.editor().label.as_deref(), Some(id.as_str()));
    h.key(egui::Key::Delete, Modifiers::NONE);
    assert!(h.tracker().label(&id).is_none());
    assert!(h.tab.map.tracker_mut().undo());
    assert!(h.tracker().label(&id).is_some());
}
