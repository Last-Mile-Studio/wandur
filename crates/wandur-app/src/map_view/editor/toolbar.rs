//! The editor's toolbar across the top of the canvas: the tools (Select, Add room, Connect,
//! Add label),
//! Delete, Undo and Redo, grid snap, the area and floor shown, and Fit. Flat icon buttons with
//! tooltips naming their shortcut; the active tool is filled with the accent.

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};
use wandur_core::l10n::{S, t, tf};
use wandur_core::map::RoomMapTracker;

use super::{EditAction, Tool};
use crate::map_view::{MapViewState, area_label, floors, number};
use crate::theme::{Theme, mix};
use crate::widgets::{self, Icon};

/// The toolbar's height.
pub const HEIGHT: f32 = 40.0;
const BUTTON: f32 = 30.0;

/// What the toolbar asked for.
#[derive(Debug, Default)]
pub struct Output {
    pub actions: Vec<EditAction>,
    /// Delete what is selected (rooms ask first when there are many).
    pub delete: bool,
    pub fit: bool,
}

/// The shortcut text of the primary modifier with a key (Cmd on macOS, Ctrl elsewhere).
pub fn primary(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// A tooltip: the name, then its shortcut.
fn tip(name: &str, shortcut: &str) -> String {
    if shortcut.is_empty() {
        name.to_string()
    } else {
        format!("{name}   {shortcut}")
    }
}

/// A flat icon button; `on` fills it with the accent (the active tool, a toggle that is on).
fn button(
    ui: &mut Ui,
    icon: Icon,
    on: bool,
    enabled: bool,
    name: &str,
    shortcut: &str,
    theme: &Theme,
) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(vec2(BUTTON, BUTTON), sense);
    crate::a11y::toggle(&response, egui::accesskit::Role::Button, name, on);
    let painter = ui.painter();
    let hover = ui
        .ctx()
        .animate_bool_with_time(response.id.with("hover"), enabled && response.hovered(), 0.1);
    if on {
        painter.rect_filled(rect, 6.0, mix(theme.panel, theme.accent, 0.18));
        painter.rect_stroke(
            rect,
            6.0,
            Stroke::new(1.0, mix(theme.panel, theme.accent, 0.55)),
            StrokeKind::Inside,
        );
    } else if hover > 0.0 {
        painter.rect_filled(rect, 6.0, theme.hover_fill().gamma_multiply(hover));
    }
    let color = if !enabled {
        theme.disabled_text()
    } else if on {
        mix(theme.text, theme.accent, 0.65)
    } else {
        theme.text
    };
    widgets::paint_icon(ui, icon, rect, color);
    response.on_hover_text(tip(name, shortcut))
}

fn separator(ui: &mut Ui, theme: &Theme) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, BUTTON), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range().shrink(6.0),
        Stroke::new(1.0, mix(theme.panel, theme.border, 0.9)),
    );
}

/// Draw the toolbar in `rect`.
pub fn show(ui: &mut Ui, rect: Rect, tracker: &RoomMapTracker, state: &mut MapViewState, theme: &Theme) -> Output {
    let mut out = Output::default();
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, theme.panel);
    painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, theme.border));
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(vec2(8.0, (HEIGHT - BUTTON) / 2.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing.x = 2.0;
    let Some(editor) = state.editor.as_deref_mut() else {
        return out;
    };
    for (tool, icon, name, key) in [
        (Tool::Select, Icon::Pointer, t(S::MapToolSelect), "V"),
        (Tool::AddRoom, Icon::AddRoom, t(S::MapAddRoom), "R"),
        (Tool::Connect, Icon::Connect, t(S::MapToolConnect), "E"),
        (Tool::AddLabel, Icon::Label, t(S::MapToolLabel), "L"),
    ] {
        if button(ui, icon, editor.tool == tool, true, name, key, theme).clicked() {
            editor.tool = tool;
            editor.merge_source = None;
        }
    }
    separator(ui, theme);
    let can_delete = !editor.rooms.is_empty() || editor.exit.is_some() || editor.label.is_some();
    let delete_key = if cfg!(target_os = "macos") { "⌫" } else { "Del" };
    if button(
        ui,
        Icon::Trash,
        false,
        can_delete,
        t(S::MapToolDelete),
        delete_key,
        theme,
    )
    .clicked()
    {
        out.delete = true;
    }
    separator(ui, theme);
    if button(
        ui,
        Icon::Undo,
        false,
        tracker.can_undo(),
        t(S::Undo),
        &primary("Z"),
        theme,
    )
    .clicked()
    {
        out.actions.push(EditAction::Undo);
    }
    let redo_key = if cfg!(target_os = "macos") {
        "⇧⌘Z".to_string()
    } else {
        "Ctrl+Y".to_string()
    };
    if button(ui, Icon::Redo, false, tracker.can_redo(), t(S::Redo), &redo_key, theme).clicked() {
        out.actions.push(EditAction::Redo);
    }
    separator(ui, theme);
    if button(ui, Icon::Snap, editor.prefs.snap, true, t(S::MapToolSnap), "", theme).clicked() {
        editor.prefs.snap = !editor.prefs.snap;
    }
    separator(ui, theme);
    // The area and the floor, with the shared select; Fit at the far right.
    let fit_width = BUTTON + 6.0;
    let room_for = (ui.available_width() - fit_width - 8.0).max(0.0);
    let areas = crate::map_view::areas(tracker);
    let labels: Vec<String> = areas.iter().map(|a| area_label(a)).collect();
    let floor_list = floors(tracker, &state.area);
    let floor_labels: Vec<String> = floor_list.iter().map(|f| tf(S::MapFloor, &[&number(*f)])).collect();
    let floor_width = 96.0f32.min(room_for * 0.4);
    let area_width = (room_for - floor_width - 6.0).clamp(0.0, 220.0);
    if area_width >= 80.0 {
        let mut index = areas.iter().position(|a| *a == state.area).unwrap_or(0);
        let picked = crate::select::Select::new("map-edit-area", t(S::MapArea))
            .width(area_width)
            .height(BUTTON)
            .font_size(12.0)
            .radius(6)
            .show_index(ui, &mut index, labels.len(), |i| labels[i].as_str());
        if picked.changed()
            && let Some(area) = areas.get(index)
        {
            let area = area.clone();
            state.set_area(tracker, &area);
        }
        ui.add_space(4.0);
    }
    if floor_width >= 60.0 && !floor_list.is_empty() {
        let mut index = floor_list.iter().position(|f| *f == state.floor).unwrap_or(0);
        let picked = crate::select::Select::new("map-edit-floor", t(S::MapFloorPicker))
            .width(floor_width)
            .height(BUTTON)
            .font_size(12.0)
            .radius(6)
            .show_index(ui, &mut index, floor_labels.len(), |i| floor_labels[i].as_str());
        if picked.changed()
            && let Some(f) = floor_list.get(index)
        {
            state.set_floor(*f);
            state.fit(tracker);
        }
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if button(ui, Icon::Fit, false, true, t(S::MapFitFloor), "", theme).clicked() {
            out.fit = true;
        }
    });
    out
}

/// The canvas's hint line for the tool in use, at its bottom left.
pub fn hint(ui: &Ui, canvas: Rect, text: &str, theme: &Theme) {
    if canvas.width() < 260.0 {
        return;
    }
    let galley = crate::widgets::clipped(
        ui,
        text,
        egui::FontId::proportional(11.5),
        theme.text,
        (canvas.width() - 40.0).min(560.0),
        2,
    );
    let size = galley.size() + vec2(20.0, 12.0);
    let plate = Rect::from_min_size(pos2(canvas.left() + 10.0, canvas.bottom() - 10.0 - size.y), size);
    let painter = ui.painter_at(canvas);
    painter.rect_filled(plate, 8.0, theme.panel.gamma_multiply(0.94));
    painter.rect_stroke(plate, 8.0, Stroke::new(1.0, theme.border), StrokeKind::Inside);
    painter.galley(plate.min + vec2(10.0, 6.0), galley, Color32::PLACEHOLDER);
}
