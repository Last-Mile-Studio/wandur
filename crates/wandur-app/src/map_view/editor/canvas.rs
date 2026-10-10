//! The editor's tools on the canvas: hover, select (click, Shift or Cmd click, marquee), move
//! (drag, snapped to whole cells when snap is on, one undo step), nudge with the arrow keys, add
//! a room on a click, connect by dragging from room to room (the direction inferred, Alt or
//! Option for one-way), delete (asking first for more than five rooms), labels (add with a
//! click, select, move by dragging, resize by the corner grip, nudge, delete), the context
//! menu, the tool shortcuts and the overlays (marquee, rubber band, the selected exit and label).

use egui::{
    Align2, Color32, CursorIcon, FontId, Id, Key, Modifiers, PointerButton, Pos2, Rect, Sense, Shape, Stroke,
    StrokeKind, Ui, pos2, vec2,
};
use wandur_core::l10n::{S, t, tf};
use wandur_core::map::{MapLabel, RoomMapTracker, infer_direction};

use super::{ContextMenu, EditAction, Gesture, Target, Tool, publish, rooms_in};
use crate::map_view::{Hits, MapViewState};
use crate::theme::{Theme, mix};

/// How near an exit's line a press must be to pick it, in points.
const EXIT_REACH: f32 = 6.0;
/// The context menu's rows.
const MENU_WIDTH: f32 = 220.0;

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len = ab.length_sq();
    if len < 1e-6 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// The room, exit or label under a point (a label last: it may lie under rooms).
pub fn hit_at(hits: &Hits, p: Pos2) -> Option<Target> {
    if let Some((id, _)) = hits.rooms.iter().rev().find(|(_, r)| r.contains(p)) {
        return Some(Target::Room(id.clone()));
    }
    let exit = hits
        .links
        .iter()
        .map(|(from, direction, a, b)| (distance_to_segment(p, *a, *b), from, direction))
        .filter(|(d, _, _)| *d <= EXIT_REACH)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, from, direction)| Target::Exit(from.clone(), direction.clone()));
    exit.or_else(|| {
        hits.labels
            .iter()
            .rev()
            .find(|(_, r)| r.contains(p))
            .map(|(id, _)| Target::Label(id.clone()))
    })
}

/// The selected label's resize grip under a point.
fn on_grip(hits: &Hits, label: Option<&str>, p: Pos2) -> Option<String> {
    let id = label?;
    hits.labels
        .iter()
        .find(|(l, r)| l == id && crate::map_view::labels::grip_rect(*r).expand(3.0).contains(p))
        .map(|(l, _)| l.clone())
}

/// The label being moved or resized, as it is drawn until it is dropped.
pub fn label_preview(state: &MapViewState, tracker: &RoomMapTracker) -> Option<(String, MapLabel)> {
    let editor = state.editor.as_deref()?;
    match &editor.gesture {
        Gesture::MoveLabel { id, offset, .. } => {
            let (dx, dy) = snapped(*offset, editor.prefs.snap);
            let label = tracker.label(id)?;
            Some((
                id.clone(),
                MapLabel {
                    x: label.x + dx,
                    y: label.y + dy,
                    ..label.clone()
                },
            ))
        }
        Gesture::ResizeLabel { id, offset, .. } => {
            let label = tracker.label(id)?;
            let (w, h) = resized(label, *offset, editor.prefs.snap);
            Some((
                id.clone(),
                MapLabel {
                    width: w,
                    height: h,
                    ..label.clone()
                },
            ))
        }
        _ => None,
    }
}

/// A label's size after dragging its grip by `offset` (map units: right and up).
fn resized(label: &MapLabel, offset: (f64, f64), snap: bool) -> (f64, f64) {
    let step = if snap { 0.5 } else { 0.1 };
    let round = |v: f64| ((v / step).round() * step).max(step);
    (round(label.width + offset.0), round(label.height - offset.1))
}

/// Delete what is selected: an exit at once, up to five rooms at once, more after a question.
pub fn request_delete(state: &mut MapViewState) -> Option<EditAction> {
    let editor = state.editor.as_deref_mut()?;
    if let Some((from, direction)) = editor.exit.clone() {
        return Some(EditAction::DeleteExit(from, direction));
    }
    if let Some(id) = editor.label.clone() {
        return Some(EditAction::DeleteLabel(id));
    }
    match editor.rooms.len() {
        0 => None,
        1..=5 => Some(EditAction::DeleteRooms(editor.rooms.clone())),
        _ => {
            editor.confirm_delete = Some(editor.rooms.clone());
            None
        }
    }
}

/// The direction a connection from `from` to the pointer (or to `target`) gets.
pub fn connect_direction(
    state: &MapViewState,
    tracker: &RoomMapTracker,
    from: &str,
    to: Pos2,
    target: Option<&str>,
) -> Option<&'static str> {
    let a = tracker.room(from)?;
    let b = match target.and_then(|id| tracker.room(id)) {
        Some(room) => (room.x, room.y, room.z),
        None => {
            let (x, y) = state.unproject(state.canvas, to);
            (x, y, a.z)
        }
    };
    infer_direction((a.x, a.y, a.z), b)
}

/// Snap an offset to whole cells, or to tenths when snap is off.
fn snapped(offset: (f64, f64), snap: bool) -> (f64, f64) {
    if snap {
        (offset.0.round(), offset.1.round())
    } else {
        ((offset.0 * 10.0).round() / 10.0, (offset.1 * 10.0).round() / 10.0)
    }
}

/// The offset the selected rooms are drawn at while being moved.
pub fn move_preview(state: &MapViewState) -> Option<(f64, f64)> {
    let editor = state.editor.as_deref()?;
    match editor.gesture {
        Gesture::Move { offset, .. } => Some(snapped(offset, editor.prefs.snap)),
        _ => None,
    }
}

/// The canvas's pointer handling while editing. Edits are returned, applied after drawing.
pub fn interact(
    ui: &Ui,
    response: &egui::Response,
    tracker: &RoomMapTracker,
    state: &mut MapViewState,
    hits: &Hits,
) -> Vec<EditAction> {
    let mut actions = Vec::new();
    let canvas = state.canvas;
    let (modifiers, space) = ui.input(|i| (i.modifiers, i.key_down(Key::Space)));
    let additive = modifiers.shift || modifiers.command;
    let pointer = response.hover_pos();
    let Some(editor) = state.editor.as_deref_mut() else {
        return actions;
    };
    editor.hover = if editor.gesture == Gesture::None || matches!(editor.gesture, Gesture::Connect { .. }) {
        pointer.and_then(|p| hit_at(hits, p))
    } else {
        None
    };
    if editor.confirm_delete.is_some() {
        return actions;
    }
    // ---- context menu ----
    if response.secondary_clicked()
        && let Some(at) = response.interact_pointer_pos()
    {
        let target = hit_at(hits, at).unwrap_or_else(|| {
            let (x, y) = state.unproject(canvas, at);
            Target::Empty(x, y)
        });
        let editor = state.editor.as_deref_mut().expect("checked");
        match &target {
            Target::Room(id) if !editor.is_selected(id) => editor.select_only(Some(id)),
            Target::Exit(f, d) => editor.select_exit(f, d),
            Target::Label(id) => editor.select_label(id),
            _ => {}
        }
        editor.menu = Some(ContextMenu { at, target, age: 0 });
        editor.gesture = Gesture::None;
        publish(state);
        return actions;
    }
    let editor = state.editor.as_deref_mut().expect("checked");
    if editor.menu.is_some() {
        return actions;
    }
    // ---- drags ----
    if response.drag_started() {
        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(canvas.center());
        let middle = response.dragged_by(PointerButton::Middle);
        let hit = hit_at(hits, origin);
        let anchor = state.unproject(canvas, origin);
        let editor = state.editor.as_deref_mut().expect("checked");
        let grip = on_grip(hits, editor.label.as_deref(), origin);
        editor.gesture = if middle || space {
            Gesture::Pan
        } else if let Some(id) = grip {
            Gesture::ResizeLabel {
                id,
                anchor,
                offset: (0.0, 0.0),
            }
        } else {
            match (editor.tool, hit) {
                (Tool::Select, Some(Target::Label(id))) => {
                    editor.select_label(&id);
                    Gesture::MoveLabel {
                        id,
                        anchor,
                        offset: (0.0, 0.0),
                    }
                }
                (Tool::Select, Some(Target::Room(id))) => {
                    if !editor.is_selected(&id) {
                        if additive {
                            editor.toggle(&id);
                        } else {
                            editor.select_only(Some(&id));
                        }
                    }
                    Gesture::Move {
                        anchor,
                        offset: (0.0, 0.0),
                    }
                }
                (Tool::Select, _) => Gesture::Marquee {
                    from: origin,
                    to: origin,
                    additive,
                },
                (Tool::Connect, Some(Target::Room(id))) => Gesture::Connect {
                    from: id,
                    to: origin,
                    target: None,
                },
                _ => Gesture::Pan,
            }
        };
        publish(state);
    }
    if response.dragged()
        && let Some(p) = response.interact_pointer_pos()
    {
        let at = state.unproject(canvas, p);
        let delta = response.drag_delta();
        let room_hit = match hit_at(hits, p) {
            Some(Target::Room(id)) => Some(id),
            _ => None,
        };
        let editor = state.editor.as_deref_mut().expect("checked");
        let mut pan = false;
        match &mut editor.gesture {
            Gesture::Pan => pan = true,
            Gesture::Move { anchor, offset }
            | Gesture::MoveLabel { anchor, offset, .. }
            | Gesture::ResizeLabel { anchor, offset, .. } => *offset = (at.0 - anchor.0, at.1 - anchor.1),
            Gesture::Marquee { to, .. } => *to = p,
            Gesture::Connect { from, to, target } => {
                *to = p;
                *target = room_hit.filter(|id| id != from);
            }
            Gesture::None => {}
        }
        if pan {
            state.fit_floor = false;
            state.pan += delta;
        }
    }
    if response.drag_stopped() {
        let editor = state.editor.as_deref_mut().expect("checked");
        let snap = editor.prefs.snap;
        match std::mem::take(&mut editor.gesture) {
            Gesture::Move { offset, .. } => {
                let (dx, dy) = snapped(offset, snap);
                if (dx, dy) != (0.0, 0.0) && !editor.rooms.is_empty() {
                    actions.push(EditAction::MoveRooms(editor.rooms.clone(), dx, dy, 0.0));
                }
            }
            Gesture::MoveLabel { id, offset, .. } => {
                let (dx, dy) = snapped(offset, snap);
                if (dx, dy) != (0.0, 0.0) {
                    actions.push(EditAction::MoveLabel(id, dx, dy));
                }
            }
            Gesture::ResizeLabel { id, offset, .. } => {
                if let Some(label) = tracker.label(&id) {
                    let (w, h) = resized(label, offset, snap);
                    if (w, h) != (label.width, label.height) {
                        actions.push(EditAction::ResizeLabel(id, w, h));
                    }
                }
            }
            Gesture::Marquee { from, to, additive } => {
                let picked = rooms_in(&hits.rooms, Rect::from_two_pos(from, to));
                if !additive {
                    editor.rooms.clear();
                }
                editor.exit = None;
                editor.label = None;
                for id in picked {
                    if !editor.is_selected(&id) {
                        editor.rooms.push(id);
                    }
                }
            }
            Gesture::Connect { from, to, target } => {
                if let Some(target) = target
                    && let Some(direction) = connect_direction(state, tracker, &from, to, Some(&target))
                {
                    actions.push(EditAction::Connect {
                        from,
                        to: target,
                        direction: direction.to_string(),
                        two_way: !modifiers.alt,
                    });
                }
            }
            Gesture::Pan | Gesture::None => {}
        }
        publish(state);
    }
    // ---- clicks ----
    if response.clicked()
        && let Some(at) = response.interact_pointer_pos()
    {
        let now = ui.input(|i| i.time);
        if let Some((id, _)) = hits.badges.iter().rev().find(|(_, r)| r.contains(at)) {
            // A floor badge shows the other floor.
            state.select_room(tracker, id);
            super::sync(state, tracker);
            return actions;
        }
        let hit = hit_at(hits, at);
        let (x, y) = state.unproject(canvas, at);
        let editor = state.editor.as_deref_mut().expect("checked");
        if let Some(source) = editor.merge_source.clone() {
            if let Some(Target::Room(target)) = &hit
                && *target != source
            {
                actions.push(EditAction::Merge(source, target.clone()));
            }
            editor.merge_source = None;
            return actions;
        }
        match (editor.tool, hit) {
            (Tool::AddRoom, None) => {
                let (cx, cy) = (x.round(), y.round());
                let taken = tracker
                    .rooms()
                    .find(|r| r.area_key() == state.area && r.z == state.floor && r.x == cx && r.y == cy)
                    .map(|r| r.id.clone());
                match taken {
                    Some(id) => editor.select_only(Some(&id)),
                    None => actions.push(EditAction::AddRoomAt(cx, cy)),
                }
            }
            (_, Some(Target::Room(id))) => {
                let double = !additive
                    && editor
                        .last_click
                        .as_ref()
                        .is_some_and(|(last, time)| *last == id && now - time < 0.45);
                if additive && editor.tool == Tool::Select {
                    editor.toggle(&id);
                } else {
                    editor.select_only(Some(&id));
                }
                if double || (!additive && response.double_clicked()) {
                    editor.select_only(Some(&id));
                    editor.focus_name = true;
                }
                editor.last_click = Some((id, now));
            }
            (Tool::AddLabel, None | Some(Target::Empty(..)) | Some(Target::Label(_))) => {
                actions.push(EditAction::AddLabelAt(
                    (x * 10.0).round() / 10.0,
                    (y * 10.0).round() / 10.0,
                ));
            }
            (_, Some(Target::Exit(from, direction))) => editor.select_exit(&from, &direction),
            (_, Some(Target::Label(id))) => {
                editor.select_label(&id);
                if response.double_clicked() {
                    editor.focus_name = true;
                }
            }
            (_, Some(Target::Empty(..))) | (_, None) => {
                if !additive {
                    editor.select_only(None);
                }
            }
        }
        publish(state);
    }
    // ---- the pointer's look ----
    if let Some(editor) = state.editor.as_deref()
        && (response.hovered() || editor.gesture != Gesture::None)
    {
        let icon = match (&editor.gesture, editor.tool, &editor.hover) {
            (Gesture::Pan | Gesture::Move { .. } | Gesture::MoveLabel { .. }, _, _) => CursorIcon::Grabbing,
            (Gesture::ResizeLabel { .. }, _, _) => CursorIcon::ResizeNwSe,
            (Gesture::Connect { .. }, _, _) => CursorIcon::Alias,
            (Gesture::Marquee { .. }, _, _) => CursorIcon::Crosshair,
            _ if editor.merge_source.is_some() => CursorIcon::PointingHand,
            _ if space => CursorIcon::Grab,
            (_, Tool::Select, Some(Target::Room(_))) => CursorIcon::Grab,
            (_, _, Some(Target::Exit(..))) => CursorIcon::PointingHand,
            (_, Tool::AddRoom, None) => CursorIcon::Cell,
            (_, Tool::AddLabel, _) => CursorIcon::Text,
            (_, Tool::Select, Some(Target::Label(_))) => CursorIcon::Grab,
            (_, Tool::Connect, Some(Target::Room(_))) => CursorIcon::Alias,
            _ => CursorIcon::Default,
        };
        ui.ctx().set_cursor_icon(icon);
    }
    actions
}

/// The editor's keys while nothing else holds the keyboard: V, R and E pick a tool, Delete
/// removes the selection, the arrows nudge it a cell, Undo and Redo, and Escape steps back
/// (closes a menu, cancels a gesture, returns to Select, clears the selection).
pub fn keys(ui: &Ui, state: &mut MapViewState) -> Vec<EditAction> {
    let mut actions = Vec::new();
    let free = ui.memory(|m| m.focused().is_none()) && !egui::Popup::is_any_open(ui.ctx());
    let Some(editor) = state.editor.as_deref_mut() else {
        return actions;
    };
    if !editor.open {
        return actions;
    }
    if editor.typing {
        // A field had the keyboard last frame: its Escape, Delete and letters are its own.
        return actions;
    }
    let escape = ui.input(|i| i.key_pressed(Key::Escape));
    if escape && editor.wants_escape() && !ui.memory(|m| m.focused().is_some()) {
        ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        if editor.confirm_delete.take().is_some() || editor.menu.take().is_some() || editor.picker.take().is_some() {
        } else if editor.gesture != Gesture::None {
            editor.gesture = Gesture::None;
        } else if editor.merge_source.take().is_some() {
        } else if editor.tool != Tool::Select {
            editor.tool = Tool::Select;
        } else {
            editor.select_only(None);
        }
        publish(state);
        return actions;
    }
    if !free || editor.confirm_delete.is_some() {
        return actions;
    }
    let pressed = |key: Key, modifiers: Modifiers| ui.input_mut(|i| i.consume_key(modifiers, key));
    if pressed(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT) || pressed(Key::Y, Modifiers::CTRL) {
        actions.push(EditAction::Redo);
    } else if pressed(Key::Z, Modifiers::COMMAND) {
        actions.push(EditAction::Undo);
    }
    for (key, tool) in [
        (Key::V, Tool::Select),
        (Key::R, Tool::AddRoom),
        (Key::E, Tool::Connect),
        (Key::L, Tool::AddLabel),
    ] {
        if pressed(key, Modifiers::NONE) {
            editor.tool = tool;
            editor.merge_source = None;
        }
    }
    if let Some(id) = editor.label.clone() {
        for (key, dx, dy) in [
            (Key::ArrowLeft, -1.0, 0.0),
            (Key::ArrowRight, 1.0, 0.0),
            (Key::ArrowUp, 0.0, 1.0),
            (Key::ArrowDown, 0.0, -1.0),
        ] {
            if pressed(key, Modifiers::NONE) {
                actions.push(EditAction::MoveLabel(id.clone(), dx, dy));
            }
        }
    }
    let rooms = editor.rooms.clone();
    if !rooms.is_empty() {
        for (key, dx, dy) in [
            (Key::ArrowLeft, -1.0, 0.0),
            (Key::ArrowRight, 1.0, 0.0),
            (Key::ArrowUp, 0.0, 1.0),
            (Key::ArrowDown, 0.0, -1.0),
        ] {
            if pressed(key, Modifiers::NONE) {
                actions.push(EditAction::MoveRooms(rooms.clone(), dx, dy, 0.0));
            }
        }
    }
    if (pressed(Key::Delete, Modifiers::NONE) || pressed(Key::Backspace, Modifiers::NONE))
        && let Some(action) = request_delete(state)
    {
        actions.push(action);
    }
    actions
}

/// Overlays over the drawn map: the selected and hovered exits, the marquee, the rubber band
/// with its direction.
pub fn paint_overlay(ui: &Ui, tracker: &RoomMapTracker, state: &MapViewState, hits: &Hits, theme: &Theme) {
    let Some(editor) = state.editor.as_deref() else {
        return;
    };
    let canvas = state.canvas;
    let painter = ui.painter_at(canvas);
    let accent = theme.accent;
    for (id, rect) in &hits.labels {
        if editor.label.as_deref() == Some(id.as_str()) {
            crate::map_view::labels::paint_selection(&painter, *rect, accent, theme.panel);
        } else if editor.hover == Some(Target::Label(id.clone())) {
            painter.rect_stroke(
                rect.expand(2.0),
                2.0,
                Stroke::new(1.5, accent.gamma_multiply(0.55)),
                StrokeKind::Outside,
            );
        }
    }
    for (from, direction, a, b) in &hits.links {
        let key = Some((from.clone(), direction.clone()));
        let selected = editor.exit == key;
        let hovered = editor.hover == Some(Target::Exit(from.clone(), direction.clone()));
        if selected || hovered {
            let color = if selected { accent } else { accent.gamma_multiply(0.55) };
            painter.line_segment([*a, *b], Stroke::new(if selected { 3.5 } else { 3.0 }, color));
            if selected {
                for p in [*a, *b] {
                    painter.circle(p, 3.5, theme.panel, Stroke::new(1.5, accent));
                }
            }
        }
    }
    match &editor.gesture {
        Gesture::Marquee { from, to, .. } => {
            let r = Rect::from_two_pos(*from, *to);
            painter.rect_filled(r, 2.0, accent.gamma_multiply(0.12));
            painter.rect_stroke(r, 2.0, Stroke::new(1.0, accent), StrokeKind::Inside);
        }
        Gesture::Connect { from, to, target } => {
            let Some(start) = hits.rooms.iter().find(|(id, _)| id == from).map(|(_, r)| r.center()) else {
                return;
            };
            let end = target
                .as_ref()
                .and_then(|t| hits.rooms.iter().find(|(id, _)| id == t))
                .map_or(*to, |(_, r)| r.center());
            painter.extend(Shape::dashed_line(&[start, end], Stroke::new(2.0, accent), 6.0, 4.0));
            painter.circle_filled(start, 4.0, accent);
            if let Some(t) = target
                && let Some((_, r)) = hits.rooms.iter().find(|(id, _)| id == t)
            {
                painter.rect_stroke(r.expand(2.0), 4.0, Stroke::new(2.0, accent), StrokeKind::Outside);
            }
            let direction = connect_direction(state, tracker, from, *to, target.as_deref());
            let one_way = ui.input(|i| i.modifiers.alt);
            let mut label = direction.map_or_else(|| "·".to_string(), crate::map_view::direction_label);
            if one_way {
                label = tf(S::MapConnectOneWay, &[&label]);
            }
            let galley = painter.layout_no_wrap(label, FontId::proportional(12.0), Color32::WHITE);
            let pill = Rect::from_min_size(*to + vec2(14.0, 10.0), galley.size() + vec2(16.0, 8.0));
            painter.rect_filled(pill, 10.0, accent);
            painter.galley(pill.min + vec2(8.0, 4.0), galley, Color32::WHITE);
        }
        _ => {}
    }
}

/// A row of the context menu.
fn item(ui: &mut Ui, text: &str, enabled: bool, danger: bool, theme: &Theme) -> bool {
    let width = MENU_WIDTH;
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(vec2(width, 28.0), sense);
    crate::widgets::name(&response, egui::WidgetType::Button, text);
    if enabled && response.hovered() {
        ui.painter()
            .rect_filled(rect, 5.0, mix(theme.panel, theme.accent, 0.14));
    }
    let color = if !enabled {
        theme.disabled_text()
    } else if danger {
        theme.error
    } else {
        theme.text
    };
    ui.painter().text(
        pos2(rect.left() + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(12.5),
        color,
    );
    response.clicked()
}

fn rule(ui: &mut Ui, theme: &Theme) {
    let (rect, _) = ui.allocate_exact_size(vec2(MENU_WIDTH, 7.0), Sense::hover());
    ui.painter().hline(
        rect.x_range().shrink(4.0),
        rect.center().y,
        Stroke::new(1.0, theme.border),
    );
}

/// The context menu, when open. Returns its edits.
pub fn context_menu(ui: &Ui, tracker: &RoomMapTracker, state: &mut MapViewState, theme: &Theme) -> Vec<EditAction> {
    let mut actions = Vec::new();
    let Some(menu) = state.editor.as_deref().and_then(|e| e.menu.clone()) else {
        return actions;
    };
    let rooms = state.editor.as_deref().map(|e| e.rooms.clone()).unwrap_or_default();
    let mut close = false;
    let area = egui::Area::new(Id::new("map-editor-context-menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(menu.at + vec2(2.0, 2.0))
        .constrain_to(ui.ctx().content_rect())
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(theme.panel)
                .stroke(Stroke::new(1.0, theme.border))
                .corner_radius(8.0)
                .inner_margin(egui::Margin::same(5))
                .shadow(egui::Shadow {
                    offset: [0, 4],
                    blur: 14,
                    spread: 0,
                    color: Color32::from_black_alpha(40),
                })
                .show(ui, |ui| {
                    ui.set_width(MENU_WIDTH);
                    ui.spacing_mut().item_spacing.y = 1.0;
                    match &menu.target {
                        Target::Room(id) => {
                            let room = tracker.room(id);
                            let locked = room.is_some_and(|r| r.is_locked);
                            if rooms.len() <= 1 {
                                if item(ui, t(S::MapMenuRename), true, false, theme) {
                                    if let Some(e) = state.editor.as_deref_mut() {
                                        e.focus_name = true;
                                    }
                                    close = true;
                                }
                                if item(ui, t(S::MapMenuConnectFrom), true, false, theme) {
                                    if let Some(e) = state.editor.as_deref_mut() {
                                        e.tool = Tool::Connect;
                                    }
                                    close = true;
                                }
                                if item(ui, t(S::MapSetCurrentRoom), true, false, theme) {
                                    actions.push(EditAction::SetCurrentRoom(id.clone()));
                                    close = true;
                                }
                            }
                            let lock = if locked { S::MapMenuUnlock } else { S::MapMenuLock };
                            if item(ui, t(lock), true, false, theme) {
                                actions.push(EditAction::EditRooms(rooms.clone(), super::RoomChange::Locked(!locked)));
                                close = true;
                            }
                            if rooms.len() <= 1 && item(ui, t(S::MapMenuMerge), tracker.room_count() > 1, false, theme)
                            {
                                if let Some(e) = state.editor.as_deref_mut() {
                                    e.merge_source = Some(id.clone());
                                    e.tool = Tool::Select;
                                }
                                close = true;
                            }
                            rule(ui, theme);
                            let text = if rooms.len() > 1 {
                                tf(S::MapMenuDeleteRooms, &[&rooms.len()])
                            } else {
                                t(S::MapDeleteRoom).to_string()
                            };
                            if item(ui, &text, true, true, theme) {
                                if let Some(a) = request_delete(state) {
                                    actions.push(a);
                                }
                                close = true;
                            }
                        }
                        Target::Exit(from, direction) => {
                            let two_way = !tracker.return_links(from, direction).is_empty();
                            let label = if two_way { S::MapMenuOneWay } else { S::MapMenuTwoWay };
                            if item(ui, t(label), true, false, theme) {
                                actions.push(EditAction::SetTwoWay(from.clone(), direction.clone(), !two_way));
                                close = true;
                            }
                            if let Some(link) = tracker.link(from, direction) {
                                let to = link.to_id.clone();
                                if item(ui, t(S::MapMenuGoToDestination), true, false, theme) {
                                    if let Some(e) = state.editor.as_deref_mut() {
                                        e.select_only(Some(&to));
                                    }
                                    close = true;
                                }
                            }
                            rule(ui, theme);
                            if item(ui, t(S::MapDeleteExit), true, true, theme) {
                                actions.push(EditAction::DeleteExit(from.clone(), direction.clone()));
                                close = true;
                            }
                        }
                        Target::Label(id) => {
                            let above = tracker.label(id).is_some_and(|l| l.above_rooms);
                            let text = if above {
                                S::MapLabelSendBelow
                            } else {
                                S::MapLabelBringAbove
                            };
                            if item(ui, t(text), true, false, theme) {
                                actions.push(EditAction::EditLabel(
                                    id.clone(),
                                    super::LabelChange::AboveRooms(!above),
                                ));
                                close = true;
                            }
                            if item(ui, t(S::MapChoosePicture), true, false, theme) {
                                let id = id.clone();
                                actions.extend(super::picture_action(|image| EditAction::SetLabelPicture(id, image)));
                                close = true;
                            }
                            rule(ui, theme);
                            if item(ui, t(S::MapDeleteLabel), true, true, theme) {
                                actions.push(EditAction::DeleteLabel(id.clone()));
                                close = true;
                            }
                        }
                        Target::Empty(x, y) => {
                            if item(ui, t(S::MapMenuAddHere), true, false, theme) {
                                actions.push(EditAction::AddRoomAt(x.round(), y.round()));
                                close = true;
                            }
                            if item(ui, t(S::MapMenuAddLabelHere), true, false, theme) {
                                actions.push(EditAction::AddLabelAt(
                                    (x * 10.0).round() / 10.0,
                                    (y * 10.0).round() / 10.0,
                                ));
                                close = true;
                            }
                            if item(ui, t(S::MapMenuAddPictureHere), true, false, theme) {
                                let (x, y) = ((x * 10.0).round() / 10.0, (y * 10.0).round() / 10.0);
                                actions.extend(super::picture_action(|image| {
                                    EditAction::AddPictureLabelAt(x, y, image)
                                }));
                                close = true;
                            }
                            if item(ui, t(S::MapFitFloor), true, false, theme) {
                                state.fit(tracker);
                                close = true;
                            }
                        }
                    }
                });
        });
    let clicked_outside = ui.input(|i| i.pointer.any_pressed())
        && menu.age > 0
        && ui
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| !area.response.rect.contains(p));
    if let Some(e) = state.editor.as_deref_mut() {
        if close || clicked_outside {
            e.menu = None;
        } else if let Some(m) = e.menu.as_mut() {
            m.age += 1;
        }
    }
    publish(state);
    actions
}

/// The question before deleting more than five rooms.
pub fn confirm_delete(ui: &Ui, state: &mut MapViewState, theme: &Theme) -> Vec<EditAction> {
    let mut actions = Vec::new();
    let Some(ids) = state.editor.as_deref().and_then(|e| e.confirm_delete.clone()) else {
        return actions;
    };
    let canvas = state.canvas;
    let painter = ui.painter_at(canvas);
    painter.rect_filled(canvas, 0.0, Color32::from_black_alpha(48));
    let mut cancel = false;
    egui::Area::new(Id::new("map-editor-confirm-delete"))
        .order(egui::Order::Foreground)
        .pivot(Align2::CENTER_CENTER)
        .fixed_pos(canvas.center())
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(theme.panel)
                .stroke(Stroke::new(1.0, theme.border))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::same(18))
                .show(ui, |ui| {
                    ui.set_width(300.0);
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.label(
                        egui::RichText::new(tf(S::MapConfirmDelete, &[&ids.len()]))
                            .size(14.0)
                            .color(theme.text),
                    );
                    ui.label(
                        egui::RichText::new(t(S::MapConfirmDeleteHint))
                            .size(12.0)
                            .color(theme.muted),
                    );
                    ui.add_space(4.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let delete = egui::Button::new(
                            egui::RichText::new(t(S::MapToolDelete))
                                .size(12.5)
                                .color(Color32::WHITE),
                        )
                        .fill(theme.error)
                        .min_size(vec2(84.0, 30.0));
                        if ui.add(delete).clicked() {
                            actions.push(EditAction::DeleteRooms(ids.clone()));
                        }
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new(t(S::Cancel)).size(12.5))
                                    .min_size(vec2(84.0, 30.0)),
                            )
                            .clicked()
                        {
                            cancel = true;
                        }
                    });
                });
        });
    if cancel && let Some(e) = state.editor.as_deref_mut() {
        e.confirm_delete = None;
    }
    actions
}

/// The hint for the tool in use (or merge mode).
pub fn hint(state: &MapViewState, tracker: &RoomMapTracker) -> String {
    let Some(editor) = state.editor.as_deref() else {
        return String::new();
    };
    if let Some(source) = &editor.merge_source {
        let name = tracker.room(source).map_or(source.as_str(), |r| r.name.as_str());
        return tf(S::MapHintMerge, &[&name]);
    }
    let alt = if cfg!(target_os = "macos") { "Option" } else { "Alt" };
    match editor.tool {
        Tool::Select => t(S::MapHintSelect).to_string(),
        Tool::AddRoom => t(S::MapHintAdd).to_string(),
        Tool::Connect => tf(S::MapHintConnect, &[&alt]),
        Tool::AddLabel => t(S::MapHintLabel).to_string(),
    }
}
