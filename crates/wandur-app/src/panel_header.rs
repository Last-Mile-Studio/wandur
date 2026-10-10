//! Panel actions, as the C# `PanelHeaderAction`: a small icon button a panel offers (Add and Find
//! for Saved worlds), with a localized tooltip that is also its accessible name, and for a toggle
//! its checked state.
//!
//! A panel declares its actions as data each frame. Docked alone in its leaf, a tool panel gets a
//! header drawn here ([`header`]): the grip at the far left, the title, the actions on the right
//! (or in a row of their own under a header too narrow for them, [`action_row`]), and the dock's
//! options, pin and close buttons, shown only on hover or keyboard focus in System and Fleet.
//! [`action_bar`] draws the actions as a plain row (the C# `PanelActionBar`) where a panel has no
//! header of its own (the map editor document).

use egui::{Color32, Rect, Sense, Ui, Vec2, vec2};
use wandur_core::l10n::{S, t};

use crate::theme::Theme;
use crate::widgets::{self, Icon};

/// One action a panel puts in its title bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeaderAction {
    /// A stable name, so the panel knows which was clicked (and tests can find it).
    pub id: &'static str,
    pub icon: Icon,
    /// The tooltip, also the accessible name.
    pub tip: S,
    /// `Some` for a toggle, with its state.
    pub checked: Option<bool>,
    pub enabled: bool,
}

impl HeaderAction {
    pub const fn button(id: &'static str, icon: Icon, tip: S) -> Self {
        Self {
            id,
            icon,
            tip,
            checked: None,
            enabled: true,
        }
    }

    pub const fn toggle(id: &'static str, icon: Icon, tip: S, on: bool) -> Self {
        Self {
            id,
            icon,
            tip,
            checked: Some(on),
            enabled: true,
        }
    }

    pub const fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
}

/// Where a panel draws its actions this frame.
#[derive(Clone, Debug, Default)]
pub enum ActionsPlace {
    /// In its dock header (the app draws them and passes the click on).
    Header,
    /// In a row of their own under a header too narrow for them.
    Row(HeaderLook),
    /// As a plain row: the panel has no header (floating, slid out, or a document).
    #[default]
    Bar,
}

/// Draw `actions` where `place` says (nothing in the header case). Returns the id clicked.
pub fn place_actions(
    ui: &mut Ui,
    place: &ActionsPlace,
    actions: &[HeaderAction],
    theme: &Theme,
) -> Option<&'static str> {
    match place {
        ActionsPlace::Header => None,
        ActionsPlace::Row(look) => action_row(ui, actions, look, theme),
        ActionsPlace::Bar => action_bar(ui, actions, theme),
    }
}

/// The side of an action button (the C# `panel-action` class: 24 by 24 with a 14 point glyph).
pub const ACTION_SIZE: f32 = 24.0;
/// The gap between two actions.
pub const ACTION_GAP: f32 = 2.0;

/// The width a row of `n` actions takes.
pub fn actions_width(n: usize) -> f32 {
    if n == 0 {
        0.0
    } else {
        n as f32 * ACTION_SIZE + (n - 1) as f32 * ACTION_GAP
    }
}

/// One action button in `rect`: the glyph in the text colour (muted while disabled), a hover
/// and a checked fill. Returns its response, named for screen readers.
pub fn action_button(ui: &mut Ui, rect: Rect, action: &HeaderAction, theme: &Theme) -> egui::Response {
    let id = ui.id().with(("panel-action", action.id));
    let sense = if action.enabled { Sense::click() } else { Sense::hover() };
    let response = ui.interact(rect, id, sense);
    let on = action.checked == Some(true);
    // A short fade on hover (100 ms, once per change; C# UI review, item 19).
    let hot = action.enabled && (response.hovered() || response.has_focus());
    let hover = ui.ctx().animate_bool_with_time(id.with("hover"), hot, 0.1);
    if hover > 0.0 && !on {
        ui.painter()
            .rect_filled(rect, 4.0, theme.hover_fill().gamma_multiply(hover));
    } else if hot {
        ui.painter().rect_filled(rect, 4.0, theme.hover_fill());
    } else if on {
        // The C# checked toggle: a quiet step toward the text colour, not the accent.
        ui.painter()
            .rect_filled(rect, 3.0, crate::theme::mix(theme.panel, theme.text, 0.15));
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(0.5),
            4.0,
            egui::Stroke::new(1.0, theme.accent),
            egui::StrokeKind::Inside,
        );
    }
    let color = if action.enabled {
        theme.text
    } else {
        theme.disabled_text()
    };
    widgets::paint_icon(ui, action.icon, rect, color);
    let label = t(action.tip);
    let enabled = action.enabled;
    match action.checked {
        Some(on) => response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, on, label)),
        None => response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label)),
    }
    response.on_hover_text(label)
}

/// Draw `actions` right-aligned in `rect` (from its right edge). Returns the id of the one
/// clicked.
pub fn actions_in(ui: &mut Ui, rect: Rect, actions: &[HeaderAction], theme: &Theme) -> Option<&'static str> {
    let mut clicked = None;
    let mut x = rect.right() - actions_width(actions.len());
    for action in actions {
        let button = Rect::from_min_size(
            egui::pos2(x, rect.center().y - ACTION_SIZE / 2.0),
            vec2(ACTION_SIZE, ACTION_SIZE),
        );
        if action_button(ui, button, action, theme).clicked() {
            clicked = Some(action.id);
        }
        x += ACTION_SIZE + ACTION_GAP;
    }
    clicked
}

/// The actions as a plain row of their own (the C# `PanelActionBar`), right-aligned, for a panel
/// shown without a header. Returns the id of the one clicked.
pub fn action_bar(ui: &mut Ui, actions: &[HeaderAction], theme: &Theme) -> Option<&'static str> {
    if actions.is_empty() {
        return None;
    }
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ACTION_SIZE + 4.0), Sense::hover());
    actions_in(ui, rect.shrink2(vec2(6.0, 0.0)), actions, theme)
}

// ---- the panel header (the C# `ToolChromeControl` with `PanelHeaderHost`) ----

/// How a skin draws panel headers (the C# `ui/flat-headers` and `ui/panel-headers` rules).
#[derive(Clone, Debug)]
pub struct HeaderLook {
    pub height: f32,
    /// The header's surface: one colour in System and Fleet, the shaded metal in Armored.
    pub fill: crate::skin::Gradient,
    /// The line under the header (System: the theme's line colour; Fleet: the rim edge).
    pub line: Option<Color32>,
    /// A line over the header too (Fleet's rim).
    pub top_line: Option<Color32>,
    /// The grip's left edge and the title's, from the panel's edge.
    pub grip_x: f32,
    pub title_x: f32,
    pub title_size: f32,
    pub title_color: Color32,
    pub grip_color: Color32,
    /// The dock's own buttons (options, pin, close): their glyph colour, and on hover.
    pub glyph: Color32,
    pub glyph_hover: Color32,
    /// System and Fleet show the dock's buttons only while the header is hovered or one of them
    /// has the keyboard; Armored keeps them shown.
    pub buttons_on_hover: bool,
}

/// What the person asked of a header's own buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockCommand {
    /// Out of the dock into a floating window.
    Float,
    /// Unpin: onto its edge's strip.
    AutoHide,
    Close,
}

/// The dock's buttons: options, pin, close; 24 points on a 26 point pitch (the C# strip is 78
/// wide).
pub const DOCK_BUTTONS: usize = 3;
pub const DOCK_PITCH: f32 = 26.0;
/// The right margin of the header's last button, and the gap between the dock's buttons and the
/// actions.
pub const RIGHT_MARGIN: f32 = 8.0;
pub const STRIP_GAP: f32 = 6.0;
/// How much of a long title must stay readable beside the actions (C#
/// `PanelHeader.MinimumTitleCharacters`).
pub const MINIMUM_TITLE_CHARACTERS: usize = 8;
/// The row the actions drop to under a narrow header.
pub const ACTION_ROW_HEIGHT: f32 = 28.0;

/// The width of the dock's buttons strip.
pub fn dock_buttons_width() -> f32 {
    DOCK_BUTTONS as f32 * DOCK_PITCH - 2.0
}

/// Whether `actions` fit beside the title in a header `width` wide, keeping `title_min` points of
/// the title readable. Hidden dock buttons leave their room to the actions (C#); shown ones
/// (Armored) take theirs.
pub fn actions_fit(width: f32, title_x: f32, title_min: f32, actions: usize, buttons_shown: bool) -> bool {
    if actions == 0 {
        return true;
    }
    let buttons = if buttons_shown {
        dock_buttons_width() + STRIP_GAP
    } else {
        0.0
    };
    title_x + title_min + 8.0 + actions_width(actions) + buttons + RIGHT_MARGIN <= width
}

/// The width of the title's readable minimum: the first eight characters and an ellipsis (all of
/// it when shorter).
pub fn title_minimum(ui: &Ui, title: &str, size: f32) -> f32 {
    let short: String = if title.chars().count() > MINIMUM_TITLE_CHARACTERS {
        title.chars().take(MINIMUM_TITLE_CHARACTERS).chain(['…']).collect()
    } else {
        title.to_string()
    };
    ui.painter()
        .layout_no_wrap(short, egui::FontId::proportional(size), Color32::WHITE)
        .size()
        .x
}

/// The grip: two columns of three dots (C# `PART_Grid`, about 7 points from the edge).
pub fn paint_grip(ui: &Ui, left: f32, center_y: f32, color: Color32) {
    for col in 0..2 {
        for row in -1..=1 {
            let c = egui::pos2(left + 1.0 + col as f32 * 5.0, center_y + row as f32 * 5.5);
            ui.painter()
                .rect_filled(Rect::from_center_size(c, vec2(2.2, 2.2)), 0.6, color);
        }
    }
}

/// The header's output this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeaderOutput {
    pub action: Option<&'static str>,
    pub command: Option<DockCommand>,
    /// The pointer is on the grip (the move cursor shows there only).
    pub on_grip: bool,
}

/// One panel header over `rect` (the leaf's top `look.height` points): surface, line, grip, title,
/// the panel's actions on the right (unless they dropped to a row), and the dock's buttons. The
/// dock's own tab under it stays the drag handle; these buttons are added after it, so they take
/// the clicks.
#[allow(clippy::too_many_arguments)]
pub fn header(
    ui: &mut Ui,
    id: egui::Id,
    rect: Rect,
    title: &str,
    actions: &[HeaderAction],
    in_row: bool,
    can_hide: bool,
    look: &HeaderLook,
    theme: &Theme,
) -> HeaderOutput {
    let mut out = HeaderOutput::default();
    let painter = ui.painter().clone();
    crate::skin::gradient_on(&painter, rect, &look.fill);
    if let Some(line) = look.top_line {
        painter.hline(rect.x_range(), rect.top() + 0.5, egui::Stroke::new(1.0, line));
    }
    if let Some(line) = look.line {
        painter.hline(rect.x_range(), rect.bottom() - 0.5, egui::Stroke::new(1.0, line));
    }
    let cy = rect.center().y;
    paint_grip(ui, rect.left() + look.grip_x, cy, look.grip_color);
    let grip_rect = Rect::from_min_max(
        egui::pos2(rect.left() + look.grip_x - 3.0, rect.top()),
        egui::pos2(rect.left() + look.grip_x + 10.0, rect.bottom()),
    );
    out.on_grip = ui.rect_contains_pointer(grip_rect);

    // Remember whether the keyboard is in the dock's buttons (they show while it is).
    let focus_id = id.with("dock-buttons-focus");
    let focused_before = ui.ctx().data(|d| d.get_temp::<bool>(focus_id)).unwrap_or(false);
    let hovered = ui.rect_contains_pointer(rect);
    let shown = !look.buttons_on_hover || hovered || focused_before;

    // System and Fleet: the actions against the right edge, and the dock's buttons opening
    // between the title and them, so an action never moves under the pointer. Armored keeps the
    // dock's buttons at the right edge with the actions before them, as Dock lays them out.
    let shown_actions = if in_row { &[][..] } else { actions };
    let actions_w = actions_width(shown_actions.len());
    let gap = if shown_actions.is_empty() { 0.0 } else { STRIP_GAP };
    let (actions_left, strip_left) = if look.buttons_on_hover {
        let actions_left = rect.right() - RIGHT_MARGIN - actions_w;
        (actions_left, actions_left - gap - dock_buttons_width())
    } else {
        let strip_left = rect.right() - RIGHT_MARGIN - dock_buttons_width();
        (strip_left - gap - actions_w, strip_left)
    };
    if !shown_actions.is_empty() {
        let row = Rect::from_min_max(
            egui::pos2(actions_left, rect.top()),
            egui::pos2(actions_left + actions_w, rect.bottom()),
        );
        out.action = actions_in(ui, row, shown_actions, theme);
    }
    let mut focused_now = false;
    let buttons: [(Icon, S, Option<DockCommand>); DOCK_BUTTONS] = [
        (Icon::PanelMenu, S::DockPanelMenu, None),
        (Icon::Pin, S::AutoHide, Some(DockCommand::AutoHide)),
        (Icon::Close, S::DockClosePanel, Some(DockCommand::Close)),
    ];
    for (n, (icon, name, command)) in buttons.into_iter().enumerate() {
        let enabled = command != Some(DockCommand::AutoHide) || can_hide;
        let full = Rect::from_min_size(
            egui::pos2(strip_left + n as f32 * DOCK_PITCH, cy - ACTION_SIZE / 2.0),
            vec2(ACTION_SIZE, ACTION_SIZE),
        );
        // Hidden, a button keeps its place in the tab order and the accessibility tree with no
        // size, so it never takes a click meant for the title (the C# zero-width strip).
        let at = if shown {
            full
        } else {
            Rect::from_center_size(full.center(), Vec2::ZERO)
        };
        let sense = if enabled { Sense::click() } else { Sense::hover() };
        let response = ui.interact(at, id.with(("dock-button", n)), sense);
        let label = t(name);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
        if response.has_focus() {
            focused_now = true;
        }
        if shown {
            let hot = response.hovered() || response.has_focus();
            if enabled && hot {
                painter.rect_filled(full, 4.0, theme.hover_fill());
            }
            if response.has_focus() {
                painter.rect_stroke(
                    full.shrink(0.5),
                    4.0,
                    egui::Stroke::new(1.0, theme.accent),
                    egui::StrokeKind::Inside,
                );
            }
            let color = if !enabled {
                theme.disabled_text()
            } else if hovered || response.has_focus() {
                look.glyph_hover
            } else {
                look.glyph
            };
            widgets::paint_icon(ui, icon, full, color);
        }
        let response = response.on_hover_text(match command {
            Some(DockCommand::Close) => t(S::DockCloseTip),
            Some(DockCommand::AutoHide) => t(S::AutoHideHint),
            _ => label,
        });
        match command {
            Some(c) => {
                if response.clicked() {
                    out.command = Some(c);
                }
            }
            None => {
                crate::a11y::name_popup(
                    &response.ctx,
                    egui::Popup::default_response_id(&response),
                    egui::accesskit::Role::Menu,
                    label,
                );
                egui::Popup::menu(&response).show(|ui| {
                    ui.set_min_width(140.0);
                    if ui.button(t(S::DockFloat)).clicked() {
                        out.command = Some(DockCommand::Float);
                        ui.close();
                    }
                    if can_hide && ui.button(t(S::AutoHide)).clicked() {
                        out.command = Some(DockCommand::AutoHide);
                        ui.close();
                    }
                    if ui.button(t(S::DockClosePanel)).clicked() {
                        out.command = Some(DockCommand::Close);
                        ui.close();
                    }
                });
            }
        }
    }
    ui.ctx().data_mut(|d| d.insert_temp(focus_id, focused_now));
    if focused_now && !focused_before {
        ui.ctx().request_repaint();
    }

    // The title takes what is left between the grip and the buttons (the dock's buttons take the
    // title's room while they show).
    let title_right = if shown {
        strip_left.min(actions_left) - 6.0
    } else {
        actions_left - 6.0
    };
    let title_rect = Rect::from_min_max(
        egui::pos2(rect.left() + look.title_x, rect.top()),
        egui::pos2(title_right.max(rect.left() + look.title_x), rect.bottom()),
    );
    let galley = elided(ui, title, look.title_size, look.title_color, title_rect.width());
    painter.with_clip_rect(title_rect).galley(
        egui::pos2(title_rect.left(), cy - galley.size().y / 2.0),
        galley,
        look.title_color,
    );
    out
}

/// `text` cut to `width` with an ellipsis.
fn elided(ui: &Ui, text: &str, size: f32, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), egui::FontId::proportional(size), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
    ui.fonts_mut(|f| f.layout_job(job))
}

/// The actions' own row under a header too narrow for them: the header's colour and line, the
/// buttons right-aligned as in the header. Returns the id of the one clicked.
pub fn action_row(ui: &mut Ui, actions: &[HeaderAction], look: &HeaderLook, theme: &Theme) -> Option<&'static str> {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ACTION_ROW_HEIGHT), Sense::hover());
    crate::skin::gradient_on(ui.painter(), rect, &look.fill);
    if let Some(line) = look.line {
        ui.painter()
            .hline(rect.x_range(), rect.bottom() - 0.5, egui::Stroke::new(1.0, line));
    }
    let row = Rect::from_min_max(rect.min, egui::pos2(rect.right() - RIGHT_MARGIN, rect.bottom()));
    actions_in(ui, row, actions, theme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_drop_to_a_row_when_the_title_would_lose_its_eight_characters() {
        // System or Fleet (dock buttons hidden): the C# Map at 298 wide keeps its four actions in
        // the header; at 159 they drop.
        let title_min = 26.0;
        assert!(actions_fit(298.0, 23.0, title_min, 4, false));
        assert!(!actions_fit(159.0, 23.0, title_min, 4, false));
        // The boundary is exact: grip and title start, the title's minimum, a gap, the actions,
        // the margin.
        let need = 23.0 + title_min + 8.0 + actions_width(4) + RIGHT_MARGIN;
        assert!(actions_fit(need, 23.0, title_min, 4, false));
        assert!(!actions_fit(need - 0.5, 23.0, title_min, 4, false));
        // Armored keeps its buttons shown, so Saved worlds' two actions drop where System keeps
        // them (the C# captures at 232 wide).
        let saved = 60.0;
        assert!(actions_fit(232.0, 23.0, saved, 2, false));
        assert!(!actions_fit(222.0, 38.0, saved, 2, true));
        // A panel without actions always fits.
        assert!(actions_fit(10.0, 23.0, 100.0, 0, true));
    }

    fn look(on_hover: bool) -> HeaderLook {
        let theme = Theme::preset("Linen");
        HeaderLook {
            height: 30.0,
            fill: crate::skin::Gradient::flat(theme.panel),
            line: Some(theme.border),
            top_line: None,
            grip_x: 7.0,
            title_x: 23.0,
            title_size: 13.0,
            title_color: theme.text,
            grip_color: theme.muted,
            glyph: theme.muted,
            glyph_hover: theme.text,
            buttons_on_hover: on_hover,
        }
    }

    /// One frame with a header at the top of a 400 by 300 screen; returns its output and the
    /// rectangles of the dock's buttons as interacted.
    fn frame(ctx: &egui::Context, events: Vec<egui::Event>, on_hover: bool) -> (HeaderOutput, Vec<Rect>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 300.0))),
            events,
            ..Default::default()
        };
        let mut out = HeaderOutput::default();
        let id = egui::Id::new("test-header");
        let actions = [
            HeaderAction::button("Add", Icon::Plus, S::AddAWorld),
            HeaderAction::toggle("Find", Icon::Search, S::SavedWorldsFind, false),
        ];
        let mut full = ctx.run_ui(input, |ui| {
            let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(300.0, 30.0));
            out = header(
                ui,
                id,
                rect,
                "Saved worlds",
                &actions,
                false,
                true,
                &look(on_hover),
                &Theme::preset("Linen"),
            );
        });
        full.textures_delta.clear();
        let rects = (0..DOCK_BUTTONS)
            .map(|n| {
                ctx.read_response(id.with(("dock-button", n)))
                    .map_or(Rect::NOTHING, |r| r.rect)
            })
            .collect();
        (out, rects)
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn hover_only_buttons_stay_reachable_by_keyboard_and_named() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        // Away from the header the dock's buttons have no size (they never catch a click on the
        // title), but they exist, named, after the actions in the tab order.
        let (_, rects) = frame(&ctx, vec![egui::Event::PointerMoved(egui::pos2(200.0, 200.0))], true);
        assert!(rects.iter().all(|r| r.width() == 0.0), "{rects:?}");
        let names: Vec<String> = {
            let input = egui::RawInput::default();
            let mut out = ctx.run_ui(input, |ui| {
                let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(300.0, 30.0));
                header(
                    ui,
                    egui::Id::new("test-header"),
                    rect,
                    "Saved worlds",
                    &[],
                    false,
                    true,
                    &look(true),
                    &Theme::preset("Linen"),
                );
            });
            out.textures_delta.clear();
            out.platform_output
                .accesskit_update
                .map(|u| {
                    u.nodes
                        .iter()
                        .filter_map(|(_, n)| n.label().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        for name in [t(S::DockPanelMenu), t(S::AutoHide), t(S::DockClosePanel)] {
            assert!(names.iter().any(|n| n == name), "{name} in {names:?}");
        }
        // Tab walks the actions (Add, Find), then the dock's buttons; the focused one shows them.
        let mut shown = false;
        for _ in 0..6 {
            let (_, rects) = frame(&ctx, vec![key(egui::Key::Tab, egui::Modifiers::NONE)], true);
            let (_, rects_after) = frame(&ctx, vec![], true);
            let focused = ctx.memory(|m| m.focused());
            let close = egui::Id::new("test-header").with(("dock-button", 2));
            if focused == Some(close) {
                assert!(rects_after[2].width() >= ACTION_SIZE, "shown while focused: {rects:?}");
                shown = true;
                // Space or Enter on it closes the panel, as a click does.
                let (out, _) = frame(&ctx, vec![key(egui::Key::Enter, egui::Modifiers::NONE)], true);
                assert_eq!(out.command, Some(DockCommand::Close));
                break;
            }
        }
        assert!(shown, "Tab reached the close button");
        // Armored keeps them shown without a hover.
        let ctx = egui::Context::default();
        let (_, rects) = frame(&ctx, vec![egui::Event::PointerMoved(egui::pos2(200.0, 200.0))], false);
        assert!(rects.iter().all(|r| r.width() >= ACTION_SIZE));
        // And the pointer over the header shows them in System and Fleet.
        let ctx = egui::Context::default();
        let _ = frame(&ctx, vec![egui::Event::PointerMoved(egui::pos2(60.0, 15.0))], true);
        let (_, rects) = frame(&ctx, vec![], true);
        assert!(rects.iter().all(|r| r.width() >= ACTION_SIZE), "{rects:?}");
        assert!(
            rects[2].right() <= 300.0 - RIGHT_MARGIN - actions_width(2) - STRIP_GAP + 0.5,
            "between title and actions"
        );
    }
}
