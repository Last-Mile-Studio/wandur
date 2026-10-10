//! The property inspector docked beside the canvas while editing: what is selected, as grouped
//! sections of label and value rows that open and close (remembered), in the spirit of the old
//! Visual Basic property grid but calmer.
//!
//! - One room: Room (name, area, terrain, color, symbol, cost, locked), Position (X, Y, floor
//!   with steppers), Exits (a compact table: direction, destination, door, one-way, excluded,
//!   cost, command; add and delete), Notes and description, and its actions.
//! - Several rooms: the fields they share, "Mixed" where they differ; Position is Move by.
//! - An exit: its own fields, and whether it is one-way.
//! - A label: its text (or picture), text size and colours, opacity, above or under the rooms,
//!   position and size; choose a picture, back to text, delete.
//! - Nothing: the map's counts, the area's grid, import and export, the shortcuts.
//!
//! Edits apply at once, each one undo step: a choice, a check or a stepper when used, a text
//! field on Enter or when it loses focus (Escape puts the value back). A value the map cannot
//! take is flagged under its field and not applied.

use egui::{Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};
use wandur_core::l10n::{S, t, tf};
use wandur_core::map::{DoorState, MapLabel, MapLink, MapRoom, RoomMapTracker, infer_direction};

use super::{EditAction, EditorState, ExitChange, LabelChange, RoomChange, Tool};
use crate::map_palette::{self, STYLES};
use crate::map_view::{MapViewState, area_label, areas, number};
use crate::theme::{Theme, mix};
use crate::widgets::{self, Icon};

/// The label column's width.
const LABEL: f32 = 86.0;
/// A field's height.
const FIELD: f32 = 26.0;
/// Swatches offered for a room's colour.
const SWATCHES: [&str; 12] = [
    "#C0504D", "#E08A3C", "#E3C452", "#7DB45A", "#3E9C8F", "#4A90C8", "#5B6CC9", "#9467BD", "#C76B98", "#8C6E5A",
    "#9AA3A8", "#3B4148",
];

/// A value shared by every item, or `None` when they differ.
fn shared<T: PartialEq, I>(items: &[I], value: impl Fn(&I) -> T) -> Option<T> {
    let mut it = items.iter();
    let first = value(it.next()?);
    it.all(|i| value(i) == first).then_some(first)
}

fn is_hex_color(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 7 && b[0] == b'#' && b[1..].iter().all(u8::is_ascii_hexdigit)
}

fn parse_color(text: &str) -> Option<Color32> {
    is_hex_color(text)
        .then(|| u32::from_str_radix(&text[1..], 16).ok())
        .flatten()
        .map(|v| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn parse_number(text: &str) -> Result<f64, String> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && v.abs() <= 1e6)
        .ok_or_else(|| t(S::MapInvalidNumber).to_string())
}

/// A room as lists show it: "Name · area, floor".
fn room_label(room: &MapRoom) -> String {
    format!(
        "{} · {}, {}",
        room.name,
        area_label(room.area_key()),
        tf(S::MapFloor, &[&number(room.z)])
    )
}

/// The section header and body; open state remembered in the editor's preferences.
fn section(
    ui: &mut Ui,
    editor: &mut EditorState,
    key: &str,
    title: &str,
    theme: &Theme,
    body: impl FnOnce(&mut Ui, &mut EditorState),
) {
    let open = editor.prefs.is_open(key);
    let (rect, header) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    crate::a11y::toggle(&header, egui::accesskit::Role::Button, title, open);
    if header.hovered() {
        ui.painter().rect_filled(rect, 4.0, theme.hover_fill());
    }
    let chevron = Rect::from_center_size(pos2(rect.left() + 10.0, rect.center().y), vec2(12.0, 12.0));
    widgets::paint_icon(
        ui,
        if open { Icon::ChevronDown } else { Icon::ChevronRight },
        chevron,
        theme.muted,
    );
    ui.painter().text(
        pos2(rect.left() + 22.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        FontId::proportional(12.5),
        theme.text,
    );
    if header.clicked() {
        editor.prefs.set_open(key, !open);
    }
    if open {
        ui.add_space(2.0);
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 8,
                right: 2,
                top: 0,
                bottom: 8,
            })
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                body(ui, editor);
            });
    }
    let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        line.x_range(),
        line.center().y,
        Stroke::new(1.0, mix(theme.panel, theme.border, 0.8)),
    );
}

/// A label and value row.
fn row<R>(ui: &mut Ui, label: &str, theme: &Theme, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _) = ui.allocate_exact_size(vec2(LABEL, FIELD), Sense::hover());
        let galley = crate::widgets::clipped(ui, label, FontId::proportional(12.0), theme.muted, LABEL - 4.0, 1);
        ui.painter().galley(
            pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
            galley,
            theme.muted,
        );
        crate::a11y::set_pending(ui, label);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
    })
    .inner
}

fn error_line(ui: &mut Ui, text: &str, theme: &Theme) {
    ui.label(RichText::new(text).size(11.0).color(theme.error));
}

/// A text field that commits on Enter or when it loses focus (multiline: when it loses focus,
/// or Cmd or Ctrl+Enter). `current` is the value shown (`None`: the selection differs, shown as
/// Mixed). Returns the accepted value once committed.
#[allow(clippy::too_many_arguments)]
fn text_field<T>(
    ui: &mut Ui,
    editor: &mut EditorState,
    key: &str,
    current: Option<&str>,
    multiline: bool,
    width: f32,
    theme: &Theme,
    validate: impl Fn(&str) -> Result<T, String>,
) -> (Option<T>, egui::Response) {
    let name = crate::a11y::pending(ui);
    let shown = current.unwrap_or("");
    let mut text = editor.drafts.get(key).cloned().unwrap_or_else(|| shown.to_string());
    let error = editor.errors.get(key).cloned();
    let id = ui.make_persistent_id(("map-inspector", key));
    let hint = match current {
        None => t(S::MapMixed),
        Some(_) if key == "color" => "#RRGGBB",
        Some(_) => "",
    };
    let edit = if multiline {
        egui::TextEdit::multiline(&mut text).desired_rows(3)
    } else {
        egui::TextEdit::singleline(&mut text)
    }
    .id(id)
    .hint_text(RichText::new(hint).italics())
    .font(FontId::proportional(12.5))
    .margin(egui::Margin::symmetric(8, 5))
    .min_size(vec2(0.0, FIELD))
    .desired_width(width);
    let response = crate::a11y::named(ui.add(edit), &name);
    if error.is_some() {
        ui.painter()
            .rect_stroke(response.rect, 4.0, Stroke::new(1.5, theme.error), StrokeKind::Inside);
    }
    if editor.focus_name && (key == "name" || key == "label-text") {
        editor.focus_name = false;
        response.request_focus();
        if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), id) {
            let end = egui::text::CCursor::new(text.chars().count());
            st.cursor
                .set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), end)));
            st.store(ui.ctx(), id);
        }
    }
    if response.changed() {
        editor.drafts.insert(key.to_string(), text.clone());
        editor.errors.remove(key);
    }
    let (escape, submit) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Escape),
            multiline && i.modifiers.command && i.key_pressed(egui::Key::Enter),
        )
    });
    let mut committed = None;
    if escape && (response.has_focus() || response.lost_focus()) {
        editor.drafts.remove(key);
        editor.errors.remove(key);
        response.surrender_focus();
    } else if response.lost_focus() || (submit && response.has_focus()) {
        if let Some(draft) = editor.drafts.get(key).cloned() {
            if current == Some(draft.as_str()) {
                editor.drafts.remove(key);
            } else {
                match validate(&draft) {
                    Ok(v) => {
                        editor.drafts.remove(key);
                        editor.errors.remove(key);
                        committed = Some(v);
                    }
                    Err(e) => {
                        editor.errors.insert(key.to_string(), e);
                    }
                }
            }
        }
        if submit {
            response.surrender_focus();
        }
    }
    if let Some(e) = editor.errors.get(key).cloned() {
        error_line(ui, &e, theme);
    }
    (committed, response)
}

/// A small square button with a glyph (the steppers, a row's delete).
fn glyph_button(ui: &mut Ui, glyph: &str, name: &str, enabled: bool, theme: &Theme) -> bool {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(vec2(22.0, FIELD), sense);
    crate::widgets::name(&response, egui::WidgetType::Button, name);
    if enabled && response.hovered() {
        ui.painter().rect_filled(rect, 4.0, theme.hover_fill());
    }
    let color = if enabled { theme.text } else { theme.disabled_text() };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(15.0),
        color,
    );
    response.on_hover_text(name).clicked()
}

/// A number field with − and + steppers. Returns the committed value.
#[allow(clippy::too_many_arguments)]
fn number_field(
    ui: &mut Ui,
    editor: &mut EditorState,
    key: &str,
    current: Option<f64>,
    step: f64,
    enabled: bool,
    theme: &Theme,
    validate: impl Fn(f64) -> Result<f64, String>,
) -> Option<f64> {
    let mut out = None;
    let shown = current.map(number);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let width = (ui.available_width() - 48.0).max(40.0);
        ui.add_enabled_ui(enabled, |ui| {
            let (value, _) = text_field(ui, editor, key, shown.as_deref(), false, width, theme, |text| {
                parse_number(text).and_then(&validate)
            });
            out = value;
        });
        let can_step = enabled && current.is_some();
        if glyph_button(ui, "−", t(S::MapStepDown), can_step, theme)
            && let Some(v) = current
            && let Ok(v) = validate(v - step)
        {
            out = Some(v);
        }
        if glyph_button(ui, "+", t(S::MapStepUp), can_step, theme)
            && let Some(v) = current
            && let Ok(v) = validate(v + step)
        {
            out = Some(v);
        }
    });
    out
}

/// A checkbox with its label.
fn check(ui: &mut Ui, on: Option<bool>, label: &str, theme: &Theme) -> Option<bool> {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), FontId::proportional(12.0), theme.text);
    let (rect, response) = ui.allocate_exact_size(vec2(22.0 + galley.size().x, 22.0), Sense::click());
    crate::a11y::toggle(&response, egui::accesskit::Role::CheckBox, label, on.unwrap_or(false));
    let square = Rect::from_center_size(pos2(rect.left() + 8.0, rect.center().y), vec2(15.0, 15.0));
    match on {
        Some(on) => widgets::paint_check(ui, square, on, true, theme),
        None => {
            // Mixed: a dash.
            widgets::paint_check(ui, square, false, true, theme);
            ui.painter().hline(
                square.x_range().shrink(4.0),
                square.center().y,
                Stroke::new(1.6, theme.text),
            );
        }
    }
    ui.painter().galley(
        pos2(rect.left() + 22.0, rect.center().y - galley.size().y / 2.0),
        galley,
        theme.text,
    );
    response.clicked().then(|| !on.unwrap_or(false))
}

/// A select over owned labels at the field height. Returns the chosen index when it changed.
fn choice(ui: &mut Ui, id: &str, name: &str, labels: &[String], selected: Option<usize>, width: f32) -> Option<usize> {
    let mut index = selected.unwrap_or(labels.len());
    let response = crate::select::Select::new(("map-inspector", id), name)
        .width(width)
        .height(FIELD)
        .font_size(12.5)
        .radius(4)
        .placeholder(t(S::MapMixed))
        .show_index(ui, &mut index, labels.len(), |i| labels[i].as_str());
    (response.changed() && index < labels.len()).then_some(index)
}

/// A flat text button.
fn link_button(ui: &mut Ui, text: &str, danger: bool, theme: &Theme) -> bool {
    let color = if danger {
        theme.error
    } else {
        mix(theme.text, theme.accent, 0.6)
    };
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(12.0), color);
    let (rect, response) = ui.allocate_exact_size(galley.size() + vec2(14.0, 10.0), Sense::click());
    crate::widgets::name(&response, egui::WidgetType::Button, text);
    let fill = if response.hovered() {
        mix(theme.panel, color, 0.14)
    } else {
        mix(theme.panel, color, 0.07)
    };
    ui.painter().rect_filled(rect, 5.0, fill);
    ui.painter().galley(rect.min + vec2(7.0, 5.0), galley, color);
    response.clicked()
}

/// A room picker: a field showing the chosen room; opening it shows a search and the first
/// matches inline. Returns the room picked.
#[allow(clippy::too_many_arguments)]
fn room_picker(
    ui: &mut Ui,
    editor: &mut EditorState,
    key: &str,
    tracker: &RoomMapTracker,
    current: Option<&str>,
    exclude: Option<&str>,
    width: f32,
    theme: &Theme,
) -> Option<String> {
    let name = crate::a11y::pending(ui);
    let shown = current
        .and_then(|id| tracker.room(id))
        .map_or_else(|| t(S::MapPickRoom).to_string(), |r| r.name.clone());
    let open = editor.picker.as_ref().is_some_and(|(k, _)| k == key);
    let (rect, response) = ui.allocate_exact_size(vec2(width, FIELD), Sense::click());
    crate::a11y::control(&response, egui::accesskit::Role::ComboBox, &name);
    crate::select::paint_field(ui, rect, CornerRadius::same(4), response.hovered(), open, false);
    let galley = crate::widgets::clipped(ui, &shown, FontId::proportional(12.5), theme.text, width - 28.0, 1);
    ui.painter().galley(
        pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0),
        galley,
        theme.text,
    );
    widgets::paint_icon(
        ui,
        Icon::Search,
        Rect::from_center_size(pos2(rect.right() - 13.0, rect.center().y), vec2(14.0, 14.0)),
        theme.muted,
    );
    if response.clicked() {
        editor.picker = if open {
            None
        } else {
            Some((key.to_string(), String::new()))
        };
    }
    let mut picked = None;
    if let Some((k, query)) = editor.picker.as_mut()
        && k == key
    {
        let search = ui.add(
            egui::TextEdit::singleline(query)
                .id_salt(("map-picker", key))
                .hint_text(t(S::MapPickSearch))
                .font(FontId::proportional(12.0))
                .margin(egui::Margin::symmetric(8, 4))
                .desired_width(width),
        );
        crate::a11y::label(&search, t(S::MapPickSearch));
        if !open {
            search.request_focus();
        }
        let q = query.trim().to_lowercase();
        let mut found: Vec<&MapRoom> = tracker
            .rooms()
            .filter(|r| Some(r.id.as_str()) != exclude)
            .filter(|r| q.is_empty() || r.name.to_lowercase().contains(&q) || r.id.to_lowercase().contains(&q))
            .take(400)
            .collect();
        found.sort_by_cached_key(|r| r.name.to_lowercase());
        found.truncate(8);
        for room in found {
            let label = crate::widgets::clipped(
                ui,
                &room_label(room),
                FontId::proportional(12.0),
                theme.text,
                width - 12.0,
                1,
            );
            let (r, resp) = ui.allocate_exact_size(vec2(width, 22.0), Sense::click());
            crate::widgets::name(&resp, egui::WidgetType::Button, &room.name);
            if resp.hovered() {
                ui.painter().rect_filled(r, 4.0, mix(theme.panel, theme.accent, 0.14));
            }
            ui.painter().galley(
                pos2(r.left() + 6.0, r.center().y - label.size().y / 2.0),
                label,
                theme.text,
            );
            if resp.clicked() {
                picked = Some(room.id.clone());
            }
        }
    }
    if picked.is_some() {
        editor.picker = None;
    }
    picked
}

fn door_label(door: DoorState) -> &'static str {
    t(match door {
        DoorState::None => S::MapDoorNone,
        DoorState::Open => S::MapDoorOpen,
        DoorState::Closed => S::MapDoorClosed,
        DoorState::Locked => S::MapDoorLocked,
    })
}

const DOORS: [DoorState; 4] = [DoorState::None, DoorState::Open, DoorState::Closed, DoorState::Locked];

/// Draw the inspector in `rect`. Edits are returned, applied once drawing is done.
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    tracker: &RoomMapTracker,
    state: &mut MapViewState,
    theme: &Theme,
) -> Vec<EditAction> {
    let mut actions = Vec::new();
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
    child.set_clip_rect(rect);
    child.painter().rect_filled(rect, 0.0, theme.panel);
    let shown_area = state.area.clone();
    let Some(editor) = state.editor.as_deref_mut() else {
        return actions;
    };
    editor.symbol_preview = None;
    let rooms: Vec<&MapRoom> = editor.rooms.iter().filter_map(|id| tracker.room(id)).collect();
    let exit = editor.exit.as_ref().and_then(|(f, d)| tracker.link(f, d)).cloned();
    let label = editor.label.as_deref().and_then(|id| tracker.label(id)).cloned();
    // ---- the header: what is selected ----
    let (title, subtitle) = match (&exit, rooms.as_slice()) {
        _ if label.is_some() => {
            let l = label.as_ref().expect("checked");
            (
                if l.image.is_some() {
                    t(S::MapLabelPicture).to_string()
                } else {
                    l.text.lines().next().unwrap_or_default().to_string()
                },
                format!("{} · {}", t(S::MapSectionLabel), area_label(l.area_key())),
            )
        }
        (Some(link), _) => (
            format!(
                "{} → {}",
                link.direction,
                tracker
                    .room(&link.to_id)
                    .map_or(link.to_id.as_str(), |r| r.name.as_str())
            ),
            tf(
                S::MapExitFromRoom,
                &[&tracker
                    .room(&link.from_id)
                    .map_or(link.from_id.as_str(), |r| r.name.as_str())],
            ),
        ),
        (None, [room]) => (
            room.name.clone(),
            format!(
                "{} · {}",
                t(S::MapSectionRoom),
                room.server_id.as_deref().unwrap_or(&room.id)
            ),
        ),
        (None, []) => (
            t(S::MapInspectorTitle).to_string(),
            t(S::MapInspectorNothing).to_string(),
        ),
        (None, many) => (
            tf(S::MapSelectionCount, &[&many.len()]),
            t(S::MapSelectionShared).to_string(),
        ),
    };
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 14,
            right: 12,
            top: 12,
            bottom: 8,
        })
        .show(&mut child, |ui| {
            ui.set_width(rect.width() - 26.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.horizontal(|ui| {
                if let [room] = rooms.as_slice() {
                    let (fill, _) = map_palette::resolve(room);
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    ui.painter()
                        .rect(r, 3.0, fill, Stroke::new(1.0, theme.border), StrokeKind::Inside);
                }
                ui.add(egui::Label::new(RichText::new(&title).size(15.0).color(theme.text).strong()).truncate());
            });
            ui.add(egui::Label::new(RichText::new(&subtitle).size(11.5).color(theme.muted)).truncate());
            if let Some(message) = &editor.message {
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(message).size(11.5).color(theme.warn)).wrap());
            }
        });
    let header_bottom = child.min_rect().bottom();
    child.painter().hline(
        rect.x_range(),
        header_bottom,
        Stroke::new(1.0, mix(theme.panel, theme.border, 0.8)),
    );
    let body = Rect::from_min_max(pos2(rect.left(), header_bottom + 1.0), rect.max);
    let mut scroll_ui = child.new_child(UiBuilder::new().max_rect(body));
    egui::ScrollArea::vertical()
        .id_salt("map-inspector-scroll")
        .auto_shrink([false, false])
        .show(&mut scroll_ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 6,
                    right: 12,
                    top: 4,
                    bottom: 12,
                })
                .show(ui, |ui| {
                    ui.set_width(rect.width() - 22.0);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    if let Some(l) = &label {
                        label_sections(ui, editor, tracker, l, theme, &mut actions);
                    } else if let Some(link) = &exit {
                        exit_sections(ui, editor, tracker, link, theme, &mut actions);
                    } else if !rooms.is_empty() {
                        let ids: Vec<String> = rooms.iter().map(|r| r.id.clone()).collect();
                        room_sections(ui, editor, tracker, &rooms, &ids, theme, &mut actions);
                    } else {
                        map_section(ui, editor, tracker, &shown_area, theme, &mut actions);
                    }
                });
        });
    editor.typing = ui
        .ctx()
        .memory(|m| m.focused())
        .is_some_and(|id| egui::TextEdit::load_state(ui.ctx(), id).is_some());
    actions
}

#[allow(clippy::too_many_arguments)]
fn room_sections(
    ui: &mut Ui,
    editor: &mut EditorState,
    tracker: &RoomMapTracker,
    rooms: &[&MapRoom],
    ids: &[String],
    theme: &Theme,
    actions: &mut Vec<EditAction>,
) {
    let single = rooms.len() == 1;
    let edit =
        |actions: &mut Vec<EditAction>, change: RoomChange| actions.push(EditAction::EditRooms(ids.to_vec(), change));
    if editor.focus_name {
        editor.prefs.set_open("room", true);
    }
    section(ui, editor, "room", t(S::MapSectionRoom), theme, |ui, editor| {
        let width = ui.available_width() - LABEL - 6.0;
        // Name.
        let name = shared(rooms, |r| r.name.clone());
        row(ui, t(S::MapRoomName), theme, |ui| {
            let (value, _) = text_field(ui, editor, "name", name.as_deref(), false, width, theme, |text| {
                let v = text.trim();
                if v.is_empty() || v.encode_utf16().count() > 512 {
                    Err(t(S::MapInvalidName).to_string())
                } else {
                    Ok(v.to_string())
                }
            });
            if let Some(v) = value {
                edit(actions, RoomChange::Name(v));
            }
        });
        // Area: the map's areas, and a new one.
        let area = shared(rooms, |r| r.area_key().to_string());
        let list = areas(tracker);
        let mut labels: Vec<String> = list.iter().map(|a| area_label(a)).collect();
        labels.push(t(S::MapNewArea).to_string());
        let typing_area = editor.drafts.contains_key("area-new");
        row(ui, t(S::MapArea), theme, |ui| {
            let selected = if typing_area {
                Some(list.len())
            } else {
                area.as_ref().and_then(|a| list.iter().position(|x| x == a))
            };
            if let Some(i) = choice(ui, "area", t(S::MapArea), &labels, selected, width) {
                if i == list.len() {
                    editor.drafts.insert("area-new".into(), String::new());
                } else {
                    editor.drafts.remove("area-new");
                    let a = &list[i];
                    edit(actions, RoomChange::Area((!a.is_empty()).then(|| a.clone())));
                }
            }
            if editor.drafts.contains_key("area-new") {
                let (value, _) = text_field(ui, editor, "area-new", Some(""), false, width, theme, |text| {
                    let v = text.trim();
                    if v.is_empty() || v.chars().count() > 512 || v.chars().any(char::is_control) {
                        Err(t(S::MapInvalidArea).to_string())
                    } else {
                        Ok(v.to_string())
                    }
                });
                if let Some(v) = value {
                    edit(actions, RoomChange::Area(Some(v)));
                }
            }
        });
        // Terrain: none, the palette's presets, the world's own word.
        let terrain = shared(rooms, |r| r.environment.clone().unwrap_or_default());
        let own = terrain
            .as_ref()
            .filter(|e| !e.is_empty() && !STYLES.iter().any(|s| s.key == e.as_str()));
        let mut keys: Vec<Option<String>> = vec![None];
        let mut terrain_labels = vec![t(S::MapTerrainNone).to_string()];
        for s in &STYLES {
            keys.push(Some(s.key.to_string()));
            terrain_labels.push(t(s.label).to_string());
        }
        if let Some(word) = own {
            keys.push(Some(word.clone()));
            terrain_labels.push(tf(S::MapTerrainOwn, &[word]));
        }
        let other = terrain_labels.len();
        terrain_labels.push(t(S::MapTerrainOther).to_string());
        let typing_word = editor.drafts.contains_key("terrain-word");
        row(ui, t(S::MapFieldTerrain), theme, |ui| {
            let selected = if typing_word {
                Some(other)
            } else {
                terrain.as_ref().and_then(|e| {
                    let key = (!e.is_empty()).then(|| e.clone());
                    keys.iter().position(|k| *k == key)
                })
            };
            if let Some(i) = choice(ui, "terrain", t(S::MapFieldTerrain), &terrain_labels, selected, width) {
                if i == other {
                    editor
                        .drafts
                        .insert("terrain-word".into(), own.cloned().unwrap_or_default());
                } else {
                    editor.drafts.remove("terrain-word");
                    edit(actions, RoomChange::Terrain(keys[i].clone()));
                }
            }
            if editor.drafts.contains_key("terrain-word") {
                let (value, _) = text_field(ui, editor, "terrain-word", Some(""), false, width, theme, |text| {
                    let v = text.trim();
                    if v.is_empty() || v.chars().count() > 128 || v.chars().any(char::is_control) {
                        Err(t(S::MapInvalidTerrain).to_string())
                    } else {
                        Ok(v.to_string())
                    }
                });
                if let Some(v) = value {
                    edit(actions, RoomChange::Terrain(Some(v)));
                }
            }
            if single
                && let Some(room) = rooms.first()
                && map_palette::uses_inference(room)
            {
                ui.label(RichText::new(map_palette::describe(room)).size(11.0).color(theme.muted));
            }
        });
        // Colour: swatches, none, and a hex field.
        let color = shared(rooms, |r| r.color.clone());
        row(ui, t(S::MapFieldColor), theme, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                ui.set_max_width(width);
                let none_on = color == Some(None);
                let (r, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                crate::a11y::toggle(&resp, egui::accesskit::Role::Button, t(S::MapColorNone), none_on);
                ui.painter().rect(
                    r.shrink(1.0),
                    4.0,
                    theme.panel,
                    Stroke::new(1.0, theme.border),
                    StrokeKind::Inside,
                );
                ui.painter().line_segment(
                    [r.left_bottom() + vec2(4.0, -4.0), r.right_top() + vec2(-4.0, 4.0)],
                    Stroke::new(1.4, theme.error),
                );
                if none_on {
                    ui.painter()
                        .rect_stroke(r.expand(1.5), 5.0, Stroke::new(2.0, theme.accent), StrokeKind::Outside);
                }
                if resp.on_hover_text(t(S::MapColorNone)).clicked() {
                    edit(actions, RoomChange::Color(None));
                }
                for hex in SWATCHES {
                    let on = color
                        .as_ref()
                        .and_then(|c| c.as_deref())
                        .is_some_and(|c| c.eq_ignore_ascii_case(hex));
                    let (r, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                    crate::a11y::toggle(&resp, egui::accesskit::Role::Button, hex, on);
                    let fill = parse_color(hex).unwrap_or(Color32::GRAY);
                    ui.painter().rect_filled(r.shrink(1.0), 4.0, fill);
                    if on || resp.hovered() {
                        let stroke = if on {
                            Stroke::new(2.0, theme.accent)
                        } else {
                            Stroke::new(1.0, theme.border)
                        };
                        ui.painter()
                            .rect_stroke(r.expand(1.5), 5.0, stroke, StrokeKind::Outside);
                    }
                    if resp.on_hover_text(hex).clicked() {
                        edit(actions, RoomChange::Color(Some(hex.to_string())));
                    }
                }
            });
            let shown = color.as_ref().map(|c| c.clone().unwrap_or_default());
            let (value, _) = text_field(ui, editor, "color", shown.as_deref(), false, 96.0, theme, |text| {
                let v = text.trim();
                if v.is_empty() {
                    Ok(None)
                } else if is_hex_color(v) {
                    Ok(Some(v.to_uppercase()))
                } else {
                    Err(t(S::MapInvalidColor).to_string())
                }
            });
            if let Some(v) = value {
                edit(actions, RoomChange::Color(v));
            }
        });
        // Symbol, shown on the map while it is typed.
        let symbol = shared(rooms, |r| r.symbol.clone().unwrap_or_default());
        row(ui, t(S::MapRoomSymbol), theme, |ui| {
            let (value, response) = text_field(ui, editor, "symbol", symbol.as_deref(), false, 96.0, theme, |text| {
                let v = text.trim();
                if v.chars().count() > 4 || v.chars().any(char::is_control) {
                    Err(t(S::MapInvalidSymbol).to_string())
                } else {
                    Ok((!v.is_empty()).then(|| v.to_string()))
                }
            });
            if response.has_focus()
                && let Some(draft) = editor.drafts.get("symbol")
            {
                editor.symbol_preview = Some(draft.trim().to_string());
            }
            if let Some(v) = value {
                edit(actions, RoomChange::Symbol(v));
            }
        });
        // Cost of passing through.
        let cost = shared(rooms, |r| r.weight);
        row(ui, t(S::MapFieldCost), theme, |ui| {
            if let Some(v) = number_field(ui, editor, "cost", cost, 1.0, true, theme, |v| {
                if v > 0.0 && v <= 1e9 {
                    Ok(v)
                } else {
                    Err(t(S::MapInvalidRoomCost).to_string())
                }
            }) {
                edit(actions, RoomChange::Cost(v));
            }
        });
        // Locked.
        let locked = shared(rooms, |r| r.is_locked);
        row(ui, "", theme, |ui| {
            if let Some(on) = check(ui, locked, t(S::MapFieldLocked), theme) {
                edit(actions, RoomChange::Locked(on));
            }
            ui.label(RichText::new(t(S::MapLockedHint)).size(11.0).color(theme.muted));
        });
    });
    // ---- Position, or Move by for several rooms ----
    let locked = rooms.iter().any(|r| r.is_locked);
    let title = if single {
        t(S::MapSectionPosition)
    } else {
        t(S::MapMoveBy)
    };
    section(ui, editor, "position", title, theme, |ui, editor| {
        let any = |v: f64| Ok(v);
        if single && let Some(room) = rooms.first() {
            for (key, label, value, change) in [
                ("x", "X", room.x, RoomChange::X as fn(f64) -> RoomChange),
                ("y", "Y", room.y, RoomChange::Y),
                ("z", t(S::MapFieldFloor), room.z, RoomChange::Z),
            ] {
                row(ui, label, theme, |ui| {
                    if let Some(v) = number_field(ui, editor, key, Some(value), 1.0, !locked, theme, any) {
                        edit(actions, change(v));
                    }
                });
            }
        } else {
            for (key, label, axis) in [("dx", "X", 0), ("dy", "Y", 1), ("dz", t(S::MapFieldFloor), 2)] {
                row(ui, label, theme, |ui| {
                    if let Some(v) = number_field(ui, editor, key, Some(0.0), 1.0, !locked, theme, any) {
                        let mut d = [0.0; 3];
                        d[axis] = v;
                        actions.push(EditAction::MoveRooms(ids.to_vec(), d[0], d[1], d[2]));
                    }
                });
            }
        }
        if locked {
            ui.label(RichText::new(t(S::MapRoomIsLocked)).size(11.0).color(theme.muted));
        }
    });
    // ---- Exits (one room) ----
    if single && let Some(room) = rooms.first() {
        let mut exits: Vec<&MapLink> = tracker.links().filter(|l| l.from_id == room.id).collect();
        exits.sort_by(|a, b| a.direction.cmp(&b.direction));
        let title = format!("{} · {}", t(S::MapSectionExits), exits.len());
        section(ui, editor, "exits", &title, theme, |ui, editor| {
            if exits.is_empty() {
                ui.label(RichText::new(t(S::MapExitNone)).size(11.5).color(theme.muted));
            }
            for link in &exits {
                exit_row(ui, editor, tracker, link, theme, actions);
            }
            ui.add_space(2.0);
            let width = ui.available_width();
            if link_button(ui, t(S::MapExitAdd), false, theme) {
                editor.picker = Some(("new-exit".into(), String::new()));
            }
            if editor.picker.as_ref().is_some_and(|(k, _)| k == "new-exit") {
                crate::a11y::set_pending(ui, t(S::MapExitAdd));
                if let Some(to) = room_picker(ui, editor, "new-exit", tracker, None, Some(&room.id), width, theme)
                    && let Some(target) = tracker.room(&to)
                {
                    let direction = infer_direction((room.x, room.y, room.z), (target.x, target.y, target.z))
                        .filter(|d| tracker.link(&room.id, d).is_none())
                        .unwrap_or("out");
                    actions.push(EditAction::Connect {
                        from: room.id.clone(),
                        to,
                        direction: direction.to_string(),
                        two_way: true,
                    });
                }
            }
        });
    }
    // ---- Notes and description ----
    section(ui, editor, "notes", t(S::MapSectionNotes), theme, |ui, editor| {
        let width = ui.available_width();
        for (key, label, value) in [
            (
                "description",
                t(S::MapRoomDescription),
                shared(rooms, |r| r.description.clone()),
            ),
            ("notes", t(S::MapRoomNotes), shared(rooms, |r| r.notes.clone())),
        ] {
            ui.label(RichText::new(label).size(12.0).color(theme.muted));
            crate::a11y::set_pending(ui, label);
            let (committed, _) = text_field(ui, editor, key, value.as_deref(), true, width, theme, |text| {
                if text.chars().count() > 16_000 {
                    Err(t(S::MapInvalidLong).to_string())
                } else {
                    Ok(text.to_string())
                }
            });
            if let Some(v) = committed {
                edit(
                    actions,
                    if key == "notes" {
                        RoomChange::Notes(v)
                    } else {
                        RoomChange::Description(v)
                    },
                );
            }
        }
    });
    // ---- Actions ----
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        if single && let Some(room) = rooms.first() {
            if link_button(ui, t(S::MapSetCurrentRoom), false, theme) {
                actions.push(EditAction::SetCurrentRoom(room.id.clone()));
            }
            if link_button(ui, t(S::MapMenuMerge), false, theme) {
                editor.merge_source = Some(room.id.clone());
                editor.tool = Tool::Select;
            }
        }
        let delete = if single {
            t(S::MapDeleteRoom).to_string()
        } else {
            tf(S::MapMenuDeleteRooms, &[&rooms.len()])
        };
        if link_button(ui, &delete, true, theme) {
            if ids.len() > 5 {
                editor.confirm_delete = Some(ids.to_vec());
            } else {
                actions.push(EditAction::DeleteRooms(ids.to_vec()));
            }
        }
    });
}

/// One exit of the room as a compact two-line row.
fn exit_row(
    ui: &mut Ui,
    editor: &mut EditorState,
    tracker: &RoomMapTracker,
    link: &MapLink,
    theme: &Theme,
    actions: &mut Vec<EditAction>,
) {
    let (from, dir) = (link.from_id.clone(), link.direction.clone());
    let key = |field: &str| format!("exit:{dir}:{field}");
    let change =
        |actions: &mut Vec<EditAction>, c: ExitChange| actions.push(EditAction::EditExit(from.clone(), dir.clone(), c));
    egui::Frame::new()
        .fill(mix(theme.panel, theme.text, 0.035))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            let width = ui.available_width();
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            ui.horizontal(|ui| {
                crate::a11y::set_pending(ui, t(S::MapExitDirectionShort));
                let (value, _) = text_field(ui, editor, &key("dir"), Some(&dir), false, 74.0, theme, |text| {
                    let v = text.trim().to_lowercase();
                    if v.is_empty() || v.chars().count() > 64 || v.chars().any(char::is_control) {
                        Err(t(S::MapInvalidDirection).to_string())
                    } else if tracker.link(&from, &v).is_some() {
                        Err(t(S::MapDirectionTaken).to_string())
                    } else {
                        Ok(v)
                    }
                });
                if let Some(v) = value {
                    change(actions, ExitChange::Direction(v));
                }
                ui.label(RichText::new("→").size(12.0).color(theme.muted));
                let rest = (width - 74.0 - 22.0 - 30.0 - 12.0).max(60.0);
                crate::a11y::set_pending(ui, t(S::MapExitTo));
                if let Some(to) = room_picker(
                    ui,
                    editor,
                    &key("to"),
                    tracker,
                    Some(&link.to_id),
                    Some(&from),
                    rest,
                    theme,
                ) {
                    change(actions, ExitChange::Destination(to));
                }
                if glyph_button(ui, "×", t(S::MapDeleteExit), true, theme) {
                    actions.push(EditAction::DeleteExit(from.clone(), dir.clone()));
                }
            });
            ui.horizontal(|ui| {
                let door_labels: Vec<String> = DOORS.iter().map(|d| door_label(*d).to_string()).collect();
                let selected = DOORS.iter().position(|d| *d == link.door_state);
                if let Some(i) = choice(ui, &key("door"), t(S::MapExitDoor), &door_labels, selected, 112.0) {
                    change(actions, ExitChange::Door(DOORS[i]));
                }
                ui.add_space(4.0);
                ui.label(RichText::new(t(S::MapFieldCost)).size(11.5).color(theme.muted));
                crate::a11y::set_pending(ui, t(S::MapFieldCost));
                let w = ui.available_width().min(64.0);
                let (cost, _) = text_field(
                    ui,
                    editor,
                    &key("cost"),
                    Some(&number(link.weight)),
                    false,
                    w,
                    theme,
                    |text| {
                        parse_number(text).and_then(|v| {
                            if v >= 0.0 {
                                Ok(v)
                            } else {
                                Err(t(S::MapInvalidExitCost).to_string())
                            }
                        })
                    },
                );
                if let Some(v) = cost {
                    change(actions, ExitChange::Cost(v));
                }
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let two_way = !tracker.return_links(&from, &dir).is_empty();
                if let Some(on) = check(ui, Some(!two_way), t(S::MapExitOneWay), theme) {
                    actions.push(EditAction::SetTwoWay(from.clone(), dir.clone(), !on));
                }
                if let Some(on) = check(ui, Some(link.is_locked), t(S::MapExitExcluded), theme) {
                    change(actions, ExitChange::Excluded(on));
                }
            });
            let command = link.command.clone().unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(RichText::new(t(S::MapExitCommandShort)).size(11.5).color(theme.muted));
                crate::a11y::set_pending(ui, t(S::MapExitCommandShort));
                let w = ui.available_width();
                let (value, _) = text_field(ui, editor, &key("command"), Some(&command), false, w, theme, |text| {
                    let v = text.trim();
                    if v.chars().count() > 512 || v.chars().any(char::is_control) {
                        Err(t(S::MapInvalidCommand).to_string())
                    } else {
                        Ok((!v.is_empty()).then(|| v.to_string()))
                    }
                });
                if let Some(v) = value {
                    change(actions, ExitChange::Command(v));
                }
            });
        });
}

fn exit_sections(
    ui: &mut Ui,
    editor: &mut EditorState,
    tracker: &RoomMapTracker,
    link: &MapLink,
    theme: &Theme,
    actions: &mut Vec<EditAction>,
) {
    let (from, dir) = (link.from_id.clone(), link.direction.clone());
    let change =
        |actions: &mut Vec<EditAction>, c: ExitChange| actions.push(EditAction::EditExit(from.clone(), dir.clone(), c));
    section(ui, editor, "exit", t(S::MapSectionExit), theme, |ui, editor| {
        let width = ui.available_width() - LABEL - 6.0;
        row(ui, t(S::MapExitFrom), theme, |ui| {
            let name = tracker.room(&from).map_or(from.as_str(), |r| r.name.as_str());
            if link_button(ui, name, false, theme) {
                editor.select_only(Some(&from));
            }
        });
        row(ui, t(S::MapExitTo), theme, |ui| {
            if let Some(to) = room_picker(
                ui,
                editor,
                "exit-to",
                tracker,
                Some(&link.to_id),
                Some(&from),
                width,
                theme,
            ) {
                change(actions, ExitChange::Destination(to));
            }
        });
        row(ui, t(S::MapExitDirectionShort), theme, |ui| {
            let (value, _) = text_field(ui, editor, "exit-dir", Some(&dir), false, width, theme, |text| {
                let v = text.trim().to_lowercase();
                if v.is_empty() || v.chars().count() > 64 || v.chars().any(char::is_control) {
                    Err(t(S::MapInvalidDirection).to_string())
                } else if tracker.link(&from, &v).is_some() {
                    Err(t(S::MapDirectionTaken).to_string())
                } else {
                    Ok(v)
                }
            });
            if let Some(v) = value {
                change(actions, ExitChange::Direction(v));
            }
        });
        row(ui, t(S::MapExitDoorShort), theme, |ui| {
            let labels: Vec<String> = DOORS.iter().map(|d| door_label(*d).to_string()).collect();
            let selected = DOORS.iter().position(|d| *d == link.door_state);
            if let Some(i) = choice(ui, "exit-door", t(S::MapExitDoor), &labels, selected, width) {
                change(actions, ExitChange::Door(DOORS[i]));
            }
        });
        row(ui, t(S::MapFieldCost), theme, |ui| {
            if let Some(v) = number_field(ui, editor, "exit-cost", Some(link.weight), 1.0, true, theme, |v| {
                if v >= 0.0 {
                    Ok(v)
                } else {
                    Err(t(S::MapInvalidExitCost).to_string())
                }
            }) {
                change(actions, ExitChange::Cost(v));
            }
        });
        row(ui, t(S::MapExitCommandShort), theme, |ui| {
            let command = link.command.clone().unwrap_or_default();
            let (value, _) = text_field(
                ui,
                editor,
                "exit-command",
                Some(&command),
                false,
                width,
                theme,
                |text| {
                    let v = text.trim();
                    if v.chars().count() > 512 || v.chars().any(char::is_control) {
                        Err(t(S::MapInvalidCommand).to_string())
                    } else {
                        Ok((!v.is_empty()).then(|| v.to_string()))
                    }
                },
            );
            if let Some(v) = value {
                change(actions, ExitChange::Command(v));
            }
            ui.label(RichText::new(t(S::MapExitCommandHint)).size(11.0).color(theme.muted));
        });
        row(ui, "", theme, |ui| {
            let two_way = !tracker.return_links(&from, &dir).is_empty();
            if let Some(on) = check(ui, Some(!two_way), t(S::MapExitOneWay), theme) {
                actions.push(EditAction::SetTwoWay(from.clone(), dir.clone(), !on));
            }
            if let Some(on) = check(ui, Some(link.is_locked), t(S::MapExitExcluded), theme) {
                change(actions, ExitChange::Excluded(on));
            }
        });
    });
    ui.add_space(8.0);
    if link_button(ui, t(S::MapDeleteExit), true, theme) {
        actions.push(EditAction::DeleteExit(from.clone(), dir.clone()));
    }
}

/// A colour row: none, the swatches, a hex field. Returns the colour picked.
fn color_choice(
    ui: &mut Ui,
    editor: &mut EditorState,
    key: &str,
    current: Option<&str>,
    width: f32,
    theme: &Theme,
) -> Option<Option<String>> {
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        ui.set_max_width(width);
        let (r, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
        crate::a11y::toggle(
            &resp,
            egui::accesskit::Role::Button,
            t(S::MapColorNone),
            current.is_none(),
        );
        ui.painter().rect(
            r.shrink(1.0),
            4.0,
            theme.panel,
            Stroke::new(1.0, theme.border),
            StrokeKind::Inside,
        );
        ui.painter().line_segment(
            [r.left_bottom() + vec2(4.0, -4.0), r.right_top() + vec2(-4.0, 4.0)],
            Stroke::new(1.4, theme.error),
        );
        if current.is_none() {
            ui.painter()
                .rect_stroke(r.expand(1.5), 5.0, Stroke::new(2.0, theme.accent), StrokeKind::Outside);
        }
        if resp.on_hover_text(t(S::MapColorNone)).clicked() {
            picked = Some(None);
        }
        for hex in SWATCHES.iter().chain(&["#FFFFFF", "#000000"]) {
            let on = current.is_some_and(|c| c.eq_ignore_ascii_case(hex));
            let (r, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
            crate::a11y::toggle(&resp, egui::accesskit::Role::Button, hex, on);
            ui.painter()
                .rect_filled(r.shrink(1.0), 4.0, parse_color(hex).unwrap_or(Color32::GRAY));
            if on || resp.hovered() {
                let stroke = if on {
                    Stroke::new(2.0, theme.accent)
                } else {
                    Stroke::new(1.0, theme.border)
                };
                ui.painter()
                    .rect_stroke(r.expand(1.5), 5.0, stroke, StrokeKind::Outside);
            }
            if resp.on_hover_text(*hex).clicked() {
                picked = Some(Some(hex.to_string()));
            }
        }
    });
    let (value, _) = text_field(
        ui,
        editor,
        key,
        Some(current.unwrap_or("")),
        false,
        96.0,
        theme,
        |text| {
            let v = text.trim();
            if v.is_empty() {
                Ok(None)
            } else if is_hex_color(v) {
                Ok(Some(v.to_uppercase()))
            } else {
                Err(t(S::MapInvalidColor).to_string())
            }
        },
    );
    value.or(picked)
}

/// One label: Label (text or picture, size, colours, opacity, above the rooms), Position and
/// Size, and its actions.
fn label_sections(
    ui: &mut Ui,
    editor: &mut EditorState,
    tracker: &RoomMapTracker,
    label: &MapLabel,
    theme: &Theme,
    actions: &mut Vec<EditAction>,
) {
    let id = label.id.clone();
    let change = |actions: &mut Vec<EditAction>, c: LabelChange| actions.push(EditAction::EditLabel(id.clone(), c));
    if editor.focus_name {
        editor.prefs.set_open("label", true);
    }
    section(ui, editor, "label", t(S::MapSectionLabel), theme, |ui, editor| {
        let width = ui.available_width() - LABEL - 6.0;
        if let Some(hash) = label.image.as_deref() {
            row(ui, t(S::MapLabelPicture), theme, |ui| {
                let info = tracker.image(hash).map_or_else(String::new, |image| {
                    let kind = wandur_core::map::images::ImageKind::of(&image.data).map_or("?", |k| k.name());
                    let (w, h) = wandur_core::map::images::dimensions(&image.data).unwrap_or((0, 0));
                    tf(
                        S::MapLabelPictureInfo,
                        &[&kind, &w, &h, &image.data.len().div_ceil(1024)],
                    )
                });
                ui.label(RichText::new(info).size(12.0).color(theme.text));
            });
        } else {
            row(ui, t(S::MapLabelText), theme, |ui| {
                let (value, _) = text_field(
                    ui,
                    editor,
                    "label-text",
                    Some(&label.text),
                    true,
                    width,
                    theme,
                    |text| {
                        if text.trim().is_empty() || text.encode_utf16().count() > 4_000 {
                            Err(t(S::MapInvalidLabelText).to_string())
                        } else {
                            Ok(text.trim_end().to_string())
                        }
                    },
                );
                if let Some(v) = value {
                    change(actions, LabelChange::Text(v));
                }
            });
            row(ui, t(S::MapLabelFontSize), theme, |ui| {
                if let Some(v) = number_field(ui, editor, "label-size", Some(label.font_size), 2.0, true, theme, |v| {
                    if (4.0..=400.0).contains(&v) {
                        Ok(v)
                    } else {
                        Err(t(S::MapInvalidFontSize).to_string())
                    }
                }) {
                    change(actions, LabelChange::FontSize(v));
                }
            });
            row(ui, t(S::MapLabelTextColor), theme, |ui| {
                if let Some(v) = color_choice(ui, editor, "label-color", label.color.as_deref(), width, theme) {
                    change(actions, LabelChange::Color(v));
                }
            });
            row(ui, t(S::MapLabelBackground), theme, |ui| {
                if let Some(v) = color_choice(
                    ui,
                    editor,
                    "label-background",
                    label.background.as_deref(),
                    width,
                    theme,
                ) {
                    change(actions, LabelChange::Background(v));
                }
            });
        }
        row(ui, t(S::MapLabelOpacity), theme, |ui| {
            if let Some(v) = number_field(
                ui,
                editor,
                "label-opacity",
                Some(label.opacity),
                0.1,
                true,
                theme,
                |v| {
                    let v = (v * 100.0).round() / 100.0;
                    if (0.05..=1.0).contains(&v) {
                        Ok(v)
                    } else {
                        Err(t(S::MapInvalidOpacity).to_string())
                    }
                },
            ) {
                change(actions, LabelChange::Opacity(v));
            }
        });
        row(ui, "", theme, |ui| {
            if let Some(on) = check(ui, Some(label.above_rooms), t(S::MapLabelAboveRooms), theme) {
                change(actions, LabelChange::AboveRooms(on));
            }
        });
    });
    section(
        ui,
        editor,
        "label-place",
        t(S::MapSectionPosition),
        theme,
        |ui, editor| {
            let coordinate = |v: f64| {
                if v.abs() <= 1e6 {
                    Ok(v)
                } else {
                    Err(t(S::MapInvalidNumber).to_string())
                }
            };
            for (key, name, value, step) in [
                ("label-x", "X", label.x, 1.0),
                ("label-y", "Y", label.y, 1.0),
                ("label-z", t(S::MapFloorPicker), label.z, 1.0),
            ] {
                row(ui, name, theme, |ui| {
                    if let Some(v) = number_field(ui, editor, key, Some(value), step, true, theme, coordinate) {
                        change(
                            actions,
                            match key {
                                "label-x" => LabelChange::X(v),
                                "label-y" => LabelChange::Y(v),
                                _ => LabelChange::Z(v),
                            },
                        );
                    }
                });
            }
            let size = |v: f64| {
                if v > 0.0 && v <= 10_000.0 {
                    Ok(v)
                } else {
                    Err(t(S::MapInvalidLabelSize).to_string())
                }
            };
            for (key, name, value) in [
                ("label-w", t(S::MapLabelWidth), label.width),
                ("label-h", t(S::MapLabelHeight), label.height),
            ] {
                row(ui, name, theme, |ui| {
                    if let Some(v) = number_field(ui, editor, key, Some(value), 0.5, true, theme, size) {
                        change(
                            actions,
                            if key == "label-w" {
                                LabelChange::Width(v)
                            } else {
                                LabelChange::Height(v)
                            },
                        );
                    }
                });
            }
        },
    );
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        if link_button(ui, t(S::MapChoosePicture), false, theme) {
            let id = id.clone();
            actions.extend(super::picture_action(|image| EditAction::SetLabelPicture(id, image)));
        }
        if label.image.is_some() && link_button(ui, t(S::MapLabelUseText), false, theme) {
            change(actions, LabelChange::UseText);
        }
        if link_button(ui, t(S::MapDeleteLabel), true, theme) {
            actions.push(EditAction::DeleteLabel(id.clone()));
        }
    });
}

/// Nothing selected: the map at a glance, the area's grid, import and export, shortcuts.
fn map_section(
    ui: &mut Ui,
    editor: &mut EditorState,
    tracker: &RoomMapTracker,
    area: &str,
    theme: &Theme,
    actions: &mut Vec<EditAction>,
) {
    section(ui, editor, "find", t(S::MapSearchAndEdit), theme, |ui, editor| {
        let width = ui.available_width();
        let search = ui.add(
            egui::TextEdit::singleline(&mut editor.find)
                .id_salt("map-inspector-find")
                .hint_text(t(S::MapSearchRooms))
                .font(FontId::proportional(12.5))
                .margin(egui::Margin::symmetric(8, 5))
                .min_size(vec2(0.0, FIELD))
                .desired_width(width),
        );
        crate::a11y::label(&search, t(S::MapSearchRooms));
        if !editor.find.trim().is_empty() {
            let found = super::search_results(tracker, &editor.find);
            if found.is_empty() {
                ui.label(RichText::new(t(S::MapNoSearchResults)).size(11.5).color(theme.muted));
            }
            for room in found.into_iter().take(12) {
                let label = crate::widgets::clipped(
                    ui,
                    &room_label(room),
                    FontId::proportional(12.0),
                    theme.text,
                    width - 12.0,
                    1,
                );
                let (r, resp) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click());
                crate::widgets::name(&resp, egui::WidgetType::Button, &room.name);
                if resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, mix(theme.panel, theme.accent, 0.14));
                }
                ui.painter().galley(
                    pos2(r.left() + 6.0, r.center().y - label.size().y / 2.0),
                    label,
                    theme.text,
                );
                if resp.clicked() {
                    actions.push(EditAction::Select(room.id.clone()));
                }
            }
        }
    });
    section(ui, editor, "map", t(S::MapSectionMap), theme, |ui, _editor| {
        ui.label(
            RichText::new(tf(S::MapCounts, &[&tracker.room_count(), &tracker.link_count()]))
                .size(12.0)
                .color(theme.text),
        );
        if tracker.label_count() > 0 {
            ui.label(
                RichText::new(tf(S::MapLabelCount, &[&tracker.label_count()]))
                    .size(12.0)
                    .color(theme.text),
            );
        }
        let grid = tracker.grid_mode(area);
        if let Some(on) = check(ui, Some(grid), t(S::MapGridMode), theme) {
            actions.push(EditAction::GridMode(area.to_string(), on));
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            if link_button(ui, t(S::MapImport), false, theme) {
                actions.push(EditAction::OpenImport);
            }
            if link_button(ui, t(S::MapExport), false, theme)
                && let Some(path) = super::pick_export()
            {
                actions.push(EditAction::Export(path));
            }
        });
    });
    section(ui, editor, "keys", t(S::MapSectionShortcuts), theme, |ui, _editor| {
        let alt = if cfg!(target_os = "macos") { "Option" } else { "Alt" };
        let undo = super::toolbar::primary("Z");
        for line in [
            format!("V  ·  {}", t(S::MapToolSelect)),
            format!("R  ·  {}", t(S::MapAddRoom)),
            format!("E  ·  {}", t(S::MapToolConnect)),
            format!("L  ·  {}", t(S::MapToolLabel)),
            format!(
                "{}  ·  {}",
                if cfg!(target_os = "macos") { "⌫" } else { "Del" },
                t(S::MapToolDelete)
            ),
            format!("{undo}  ·  {}", t(S::Undo)),
            format!("← ↑ → ↓  ·  {}", t(S::MapKeysNudge)),
            format!("{}  ·  {}", t(S::MapKeyShift), t(S::MapKeysAdd)),
            format!("{alt}  ·  {}", t(S::MapKeysOneWay)),
            format!("{}  ·  {}", t(S::MapKeySpace), t(S::MapKeysPan)),
            format!("{}  ·  {}", t(S::MapKeyEscape), t(S::MapKeysEscape)),
        ] {
            ui.label(RichText::new(line).size(11.5).color(theme.muted));
        }
    });
}
