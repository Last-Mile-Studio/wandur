//! The Workspace panel, as the C# client's: a Find a MUD button and the open sessions (status,
//! new activity, rename, close). The saved worlds have their own panel (`saved_worlds_panel`).

use egui::{Align, CornerRadius, Layout, RichText, Sense, Ui, vec2};
use wandur_core::l10n::{S, t};

use crate::session_tab::{SessionId, Status};
use crate::sessions::{AppAction, Sessions};
use crate::theme::Theme;
use crate::widgets;

/// The Workspace and Saved worlds panels' state.
#[derive(Debug, Default)]
pub struct PanelState {
    /// The saved world Connect opens (toolbar, Session menu, Enter in Saved worlds).
    pub selected_world: Option<usize>,
    /// The saved world waiting for its delete confirmation.
    pub pending_delete: Option<usize>,
    renaming: Option<(SessionId, String)>,
    /// Saved worlds: the Find box is open, its text, and a focus request for it.
    pub filter_open: bool,
    pub filter: String,
    pub focus_filter: bool,
    /// Saved worlds: the row to give the keyboard focus to next frame.
    pub focus_row: Option<usize>,
    /// Saved worlds: where each shown row was drawn last frame (scenes point at them).
    pub row_rects: Vec<(usize, egui::Rect)>,
}

pub struct PanelContext<'a> {
    pub sessions: &'a Sessions,
    pub theme: &'a Theme,
    pub actions: &'a mut Vec<AppAction>,
    /// The directory is the shown document.
    pub directory_active: bool,
    /// The session shown in the document area.
    pub active_session: Option<SessionId>,
    /// Sessions visible now (no activity marker for them).
    pub visible: &'a [SessionId],
}

pub fn show(ui: &mut Ui, state: &mut PanelState, cx: &mut PanelContext<'_>) {
    let theme = cx.theme;
    ui.add_space(4.0);
    // Find a MUD is a row like the sessions' (C#), filled while it is the shown document.
    let (rect, find) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    crate::a11y::toggle(&find, egui::accesskit::Role::Tab, t(S::FindAMUD), cx.directory_active);
    if cx.directory_active || find.hovered() {
        let fill = if cx.directory_active {
            theme.selection_fill()
        } else {
            theme.hover_fill()
        };
        ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
    }
    ui.painter().text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        t(S::FindAMUD),
        egui::FontId::proportional(13.0),
        theme.text,
    );
    if find.clicked() {
        cx.actions.push(AppAction::OpenDirectory);
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(RichText::new(t(S::OpenSessions)).size(11.0).color(theme.muted));
    });
    ui.add_space(2.0);
    egui::ScrollArea::vertical()
        .id_salt("workspace-sessions")
        .auto_shrink([false, false])
        .show(ui, |ui| sessions_list(ui, state, cx));
}

fn sessions_list(ui: &mut Ui, state: &mut PanelState, cx: &mut PanelContext<'_>) {
    let theme = cx.theme;
    // No sessions: the heading alone, as C# (no placeholder line).
    if cx.sessions.is_empty() {
        return;
    }
    // Sessions on one world as the same name are numbered, as in C#.
    let titles: Vec<String> = cx.sessions.iter().map(|e| e.tab.title().to_string()).collect();
    for (n, entry) in cx.sessions.iter().enumerate() {
        let s = &entry.tab;
        let same: Vec<usize> = titles
            .iter()
            .enumerate()
            .filter(|(_, t)| **t == titles[n])
            .map(|(i, _)| i)
            .collect();
        let title = if same.len() > 1 && s.custom_name.is_none() {
            format!("{} · {}", titles[n], same.iter().position(|&i| i == n).unwrap_or(0) + 1)
        } else {
            titles[n].clone()
        };
        if let Some((id, text)) = &mut state.renaming
            && *id == s.id
        {
            let mut done = None;
            ui.horizontal(|ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(text)
                        .desired_width(ui.available_width() - 70.0)
                        .char_limit(100),
                );
                crate::a11y::label(&edit, t(S::Rename));
                edit.request_focus();
                if ui.small_button(t(S::Rename)).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = Some(true);
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    done = Some(false);
                }
            });
            match done {
                Some(true) => {
                    cx.actions.push(AppAction::Rename(s.id, text.trim().to_string()));
                    state.renaming = None;
                }
                Some(false) => state.renaming = None,
                None => {}
            }
            continue;
        }
        let active = cx.active_session == Some(s.id) && !cx.directory_active;
        let activity = !cx.visible.contains(&s.id) && s.has_unseen();
        let row_h = 48.0;
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
        crate::a11y::toggle(&response, egui::accesskit::Role::Tab, &title, active);
        if active || response.hovered() {
            let fill = if active {
                theme.selection_fill()
            } else {
                theme.hover_fill()
            };
            ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
        }
        let color = match s.status {
            Status::Connected { .. } | Status::Demo => theme.ok,
            Status::Connecting | Status::Waiting { .. } => theme.warn,
            Status::Closed { .. } => theme.muted,
        };
        // The name, then what matters about the session: its state in words with a coloured dot
        // (C# UI review, item 9), not its address and encoding.
        let inner = rect.shrink2(vec2(10.0, 6.0));
        let text_w = (inner.width() - 30.0).max(40.0);
        let mut text = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(inner.min, vec2(text_w, inner.height())))
                .layout(Layout::top_down(Align::Min).with_main_align(Align::Center)),
        );
        text.spacing_mut().item_spacing.y = 3.0;
        let mut name = RichText::new(&title).size(13.0).color(theme.text);
        if active {
            name = name.strong();
        }
        text.add(egui::Label::new(name).truncate().selectable(false));
        text.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            if s.is_connected() {
                Theme::dot(ui, color);
            } else {
                Theme::ring(ui, color);
            }
            let words = if s.is_demo() {
                t(S::OFFLINEDEMOASmallWorldOnYourOwnMachine).to_string()
            } else {
                s.status.label()
            };
            ui.add(
                egui::Label::new(RichText::new(words).size(11.0).color(theme.muted))
                    .truncate()
                    .selectable(false),
            );
        });
        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::right_to_left(Align::Center)),
        );
        let close = right.small_button("×").on_hover_text(t(S::CloseSession2));
        crate::a11y::label(&close, t(S::CloseSession2));
        if close.clicked() {
            cx.actions.push(AppAction::Close(s.id));
        }
        if activity {
            // New output where the session is not shown: an accent dot (its words in the
            // tooltip).
            widgets::live_dot(&mut right, theme.accent).on_hover_text(t(S::NewActivity));
        }
        let response = response.on_hover_text(if s.is_demo() {
            s.status.label()
        } else {
            format!("{} · {}", s.endpoint, s.status.label())
        });
        if response.clicked() {
            cx.actions.push(AppAction::Focus(s.id));
        }
        response.context_menu(|ui| {
            if ui.button(t(S::OpenWorkspaceItem)).clicked() {
                cx.actions.push(AppAction::Focus(s.id));
                ui.close();
            }
            if ui.button(t(S::Rename)).clicked() {
                state.renaming = Some((s.id, s.title().to_string()));
                ui.close();
            }
            if s.is_closed() && ui.button(t(S::Reconnect)).clicked() {
                cx.actions.push(AppAction::Reconnect(s.id));
                ui.close();
            }
            if ui.button(t(S::CloseWorkspaceItem)).clicked() {
                cx.actions.push(AppAction::Close(s.id));
                ui.close();
            }
        });
    }
}
