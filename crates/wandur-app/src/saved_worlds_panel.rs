//! The Saved worlds panel, as the C# `WorldLibraryView` since its `ui/panel-headers` work: its own
//! dock panel under the Workspace, with Add and Find as panel actions, a filter box that Find
//! opens, the rows (the directory's picture or the initials, the name, the address), a
//! double-click or Enter that connects or goes back to the world's open session, Delete that asks
//! first, and the row menu: Connect, Connect in new tab, Edit, Duplicate, Explore in directory
//! (only for a world the directory lists) and Delete.

use egui::{Align, CornerRadius, Key, Layout, RichText, Sense, Stroke, Ui, vec2};
use wandur_core::l10n::{S, t, tf};
use wandur_core::settings::{SavedWorld, recent_order};

use crate::artwork::{ArtLoader, ArtState, Target};
use crate::directory_view::art_request;
use crate::panel_header::{self, HeaderAction};
use crate::sessions::AppAction;
use crate::theme::Theme;
use crate::widgets::{self, Icon};
use crate::workspace_panel::PanelState;

/// What a row menu item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowCommand {
    /// Connect, or go back to the world's open session.
    Connect,
    /// Always another session.
    ConnectInNewTab,
    Edit,
    /// A copy after the world, without its saved password.
    Duplicate,
    /// The world's page in Find a MUD (only when the directory lists it).
    Explore,
    /// Asks first.
    Delete,
}

impl RowCommand {
    pub fn label(self) -> S {
        match self {
            RowCommand::Connect => S::ConnectSavedWorld,
            RowCommand::ConnectInNewTab => S::ConnectInNewTab,
            RowCommand::Edit => S::Edit2,
            RowCommand::Duplicate => S::ScriptDuplicate,
            RowCommand::Explore => S::ExploreInDirectory,
            RowCommand::Delete => S::DeleteSavedWorld,
        }
    }
}

/// The row menu in the C# order; `None` is a separator. Explore in directory shows only for a
/// world the directory lists.
pub fn row_menu(listed: bool) -> Vec<Option<RowCommand>> {
    let mut menu = vec![
        Some(RowCommand::Connect),
        Some(RowCommand::ConnectInNewTab),
        None,
        Some(RowCommand::Edit),
        Some(RowCommand::Duplicate),
    ];
    if listed {
        menu.push(Some(RowCommand::Explore));
    }
    menu.extend([None, Some(RowCommand::Delete)]);
    menu
}

/// The action a row command asks of the app (Delete is decided in the panel: at once, or a
/// question when a session is open from the world).
pub fn row_action(command: RowCommand, world: usize) -> Option<AppAction> {
    Some(match command {
        RowCommand::Connect => AppAction::GoToWorld(world),
        RowCommand::ConnectInNewTab => AppAction::ConnectWorld(world),
        RowCommand::Edit => AppAction::EditWorld(world),
        RowCommand::Duplicate => AppAction::DuplicateWorld(world),
        RowCommand::Explore => AppAction::ExploreWorld(world),
        RowCommand::Delete => return None,
    })
}

/// Whether a world matches the filter: every word in its name (ignoring case) or in its
/// `host:port`, as the C# `Matching`.
pub fn matches(world: &SavedWorld, filter: &str) -> bool {
    let name = world.name.to_lowercase();
    let address = format!("{}:{}", world.host, world.port).to_lowercase();
    filter
        .split_whitespace()
        .map(str::to_lowercase)
        .all(|w| name.contains(&w) || address.contains(&w))
}

/// The worlds shown, in order: most recently used first, then the filter.
pub fn shown(worlds: &[SavedWorld], filter: &str) -> Vec<usize> {
    recent_order(worlds)
        .into_iter()
        .filter(|&i| matches(&worlds[i], filter))
        .collect()
}

pub const ACTION_ADD: &str = "AddSavedWorld";
pub const ACTION_FIND: &str = "FindSavedWorld";

/// The panel's actions (in its title bar, or a row of their own outside a dock header): Add,
/// and Find as a toggle that shows the filter box.
pub fn header_actions(state: &PanelState) -> Vec<HeaderAction> {
    vec![
        HeaderAction::button(ACTION_ADD, Icon::Plus, S::AddAWorld),
        HeaderAction::toggle(ACTION_FIND, Icon::Search, S::SavedWorldsFind, state.filter_open),
    ]
}

/// Run a clicked action.
pub fn on_action(id: &str, state: &mut PanelState, actions: &mut Vec<AppAction>) {
    match id {
        ACTION_ADD => actions.push(AppAction::NewWorld),
        ACTION_FIND => {
            state.filter_open = !state.filter_open;
            // Closing the box clears what it filtered by; opening it puts the cursor in it.
            if state.filter_open {
                state.focus_filter = true;
            } else {
                state.filter.clear();
            }
        }
        _ => {}
    }
}

pub struct SavedContext<'a> {
    pub worlds: &'a [SavedWorld],
    /// The saved worlds open sessions came from (deleting one of them asks first).
    pub open: &'a [usize],
    pub catalog: Option<&'a wandur_core::directory::Catalog>,
    pub base: &'a str,
    pub art: &'a mut ArtLoader,
    pub theme: &'a Theme,
    pub actions: &'a mut Vec<AppAction>,
    /// Where the actions go: the dock header, a row under it, or a plain bar.
    pub actions_place: panel_header::ActionsPlace,
}

/// Height of one row (the C# row: a 40 by 30 picture and two lines of text).
pub const ROW_HEIGHT: f32 = 44.0;

pub fn show(ui: &mut Ui, state: &mut PanelState, cx: &mut SavedContext<'_>) {
    let theme = cx.theme;
    if let Some(id) = panel_header::place_actions(ui, &cx.actions_place, &header_actions(state), theme) {
        on_action(id, state, cx.actions);
    }
    if state.selected_world.is_some_and(|i| i >= cx.worlds.len()) {
        state.selected_world = None;
    }
    let order = shown(cx.worlds, if state.filter_open { &state.filter } else { "" });
    // The selection stays on a shown world (the first one when it was filtered away).
    if !order.is_empty() && !state.selected_world.is_some_and(|i| order.contains(&i)) {
        state.selected_world = order.first().copied();
    }
    if state.filter_open {
        filter_box(ui, state, cx, &order);
    }
    if let Some(i) = state.pending_delete {
        confirm_delete(ui, state, cx, i);
    }
    if cx.worlds.is_empty() {
        empty(ui, t(S::KeepYourFavoriteWorldsHereAddOneToStart), theme);
        return;
    }
    if order.is_empty() {
        empty(ui, t(S::SavedWorldsNoMatch), theme);
        return;
    }
    state.row_rects.clear();
    let mut list_focus = false;
    let focus_row = state.focus_row.take();
    let ppp = ui.ctx().pixels_per_point();
    let target = Target {
        width: (40.0 * ppp).round() as u32,
        height: (30.0 * ppp).round() as u32,
        cover: true,
    };
    egui::ScrollArea::vertical()
        .id_salt("saved-worlds")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(6.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            for &i in &order {
                let world = &cx.worlds[i];
                let selected = state.selected_world == Some(i);
                let width = ui.available_width() - 8.0;
                let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
                let rect = egui::Rect::from_min_size(outer.min + vec2(4.0, 0.0), vec2(width, ROW_HEIGHT));
                let response = ui.interact(rect, ui.id().with(("saved-world", i)), Sense::click());
                state.row_rects.push((i, rect));
                crate::a11y::toggle(
                    &response,
                    egui::accesskit::Role::ListBoxOption,
                    &format!("{}, {}:{}", world.name, world.host, world.port),
                    selected,
                );
                if focus_row == Some(i) {
                    response.request_focus();
                    response.scroll_to_me(None);
                }
                if response.has_focus() {
                    list_focus = true;
                    if !selected && response.gained_focus() {
                        state.selected_world = Some(i);
                    }
                }
                if selected || response.hovered() {
                    let fill = if selected {
                        theme.selection_fill()
                    } else {
                        theme.hover_fill()
                    };
                    ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
                }
                if response.has_focus() {
                    ui.painter().rect_stroke(
                        rect.shrink(0.5),
                        CornerRadius::same(4),
                        Stroke::new(1.0, theme.accent),
                        egui::StrokeKind::Inside,
                    );
                }
                let thumb = egui::Rect::from_min_size(rect.min + vec2(8.0, 7.0), vec2(40.0, 30.0));
                let listing = listing_of(cx.catalog, world);
                let state_art = listing
                    .and_then(|l| art_request(l, cx.base, "thumb", target, Some("400"), None))
                    .map_or(ArtState::Failed, |r| cx.art.request(&r));
                if matches!(state_art, ArtState::Ready { .. }) {
                    widgets::plate(ui, thumb, CornerRadius::same(3), &state_art, &world.name, theme, 13.0);
                } else {
                    // The C# initials tile: the panel's colour, a fine line, the initials in the
                    // accent.
                    ui.painter().rect_filled(thumb, CornerRadius::same(3), theme.panel);
                    ui.painter().rect_stroke(
                        thumb,
                        CornerRadius::same(3),
                        Stroke::new(1.0, theme.border),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        thumb.center(),
                        egui::Align2::CENTER_CENTER,
                        widgets::initials(&world.name),
                        egui::FontId::new(12.0, crate::fonts::bold_family()),
                        theme.accent,
                    );
                }
                let text_rect = egui::Rect::from_min_max(
                    egui::pos2(thumb.right() + 8.0, rect.top() + 5.0),
                    rect.max - vec2(6.0, 5.0),
                );
                let mut text = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(text_rect)
                        .layout(Layout::top_down(Align::Min).with_main_align(Align::Center)),
                );
                text.spacing_mut().item_spacing.y = 3.0;
                text.add(
                    egui::Label::new(RichText::new(&world.name).size(13.0).color(theme.text))
                        .truncate()
                        .selectable(false),
                );
                let endpoint = format!(
                    "{}{}",
                    wandur_core::directory::listing::format_host_port(&world.host, world.port),
                    if world.tls { " · TLS" } else { "" }
                );
                text.add(
                    egui::Label::new(RichText::new(&endpoint).size(10.0).color(theme.muted))
                        .truncate()
                        .selectable(false),
                );
                let response = response.on_hover_text(tf(S::DoubleClickToConnect, &[&world.name, &endpoint]));
                if response.double_clicked() {
                    state.selected_world = Some(i);
                    cx.actions.push(AppAction::GoToWorld(i));
                } else if response.clicked() || response.secondary_clicked() {
                    state.selected_world = Some(i);
                }
                let listed = listing.is_some();
                crate::a11y::name_popup(
                    &response.ctx,
                    egui::Popup::default_response_id(&response),
                    egui::accesskit::Role::Menu,
                    &world.name,
                );
                response.context_menu(|ui| {
                    state.selected_world = Some(i);
                    for entry in row_menu(listed) {
                        match entry {
                            None => {
                                ui.separator();
                            }
                            Some(command) => {
                                // The C# row menu: 14 point items on a 28 point pitch.
                                let item = egui::Button::new(RichText::new(t(command.label())).size(14.0))
                                    .frame_when_inactive(false)
                                    .min_size(vec2(170.0, 26.0));
                                if ui.add(item).clicked() {
                                    match row_action(command, i) {
                                        Some(action) => cx.actions.push(action),
                                        None => request_delete(state, cx, i),
                                    }
                                    ui.close();
                                }
                            }
                        }
                    }
                });
            }
        });
    // Keys on the focused list: arrows move the selection, Enter connects, Delete (Backspace on
    // a Mac keyboard) asks first.
    if list_focus && let Some(selected) = state.selected_world {
        let (up, down, enter, delete) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, Key::Enter),
                i.consume_key(egui::Modifiers::NONE, Key::Delete)
                    || i.consume_key(egui::Modifiers::NONE, Key::Backspace),
            )
        });
        let at = order.iter().position(|&i| i == selected).unwrap_or(0);
        if up && at > 0 {
            state.selected_world = Some(order[at - 1]);
            state.focus_row = state.selected_world;
        }
        if down && at + 1 < order.len() {
            state.selected_world = Some(order[at + 1]);
            state.focus_row = state.selected_world;
        }
        if enter {
            cx.actions.push(AppAction::GoToWorld(selected));
        }
        if delete {
            request_delete(state, cx, selected);
        }
    }
}

fn listing_of<'a>(
    catalog: Option<&'a wandur_core::directory::Catalog>,
    world: &SavedWorld,
) -> Option<&'a wandur_core::directory::listing::WorldListing> {
    catalog.and_then(|c| {
        c.by_id(&world.listing_id)
            .or_else(|| c.find_endpoint(&world.host, world.port, world.tls))
    })
}

/// Whether the directory lists this world (so it has a page to explore).
pub fn is_listed(catalog: Option<&wandur_core::directory::Catalog>, world: &SavedWorld) -> bool {
    listing_of(catalog, world).is_some()
}

fn empty(ui: &mut Ui, text: &str, theme: &Theme) {
    egui::Frame::new().inner_margin(14).show(ui, |ui| {
        ui.add(egui::Label::new(RichText::new(text).size(12.0).color(theme.muted)).wrap());
    });
}

/// The Find box: words matched against name or address. Escape closes it (and clears the
/// filter), Down moves to the list, Enter connects to the first match.
fn filter_box(ui: &mut Ui, state: &mut PanelState, cx: &mut SavedContext<'_>, order: &[usize]) {
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 8,
            right: 8,
            top: 6,
            bottom: 0,
        })
        .show(ui, |ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut state.filter)
                    .hint_text(t(S::SavedWorldsFilterPlaceholder))
                    .font(egui::FontId::proportional(12.0))
                    .desired_width(ui.available_width())
                    .char_limit(100),
            );
            crate::a11y::label(&edit, t(S::SavedWorldsFind));
            if std::mem::take(&mut state.focus_filter) {
                edit.request_focus();
            }
            if edit.has_focus() || edit.lost_focus() {
                let (escape, down, enter) = ui.input_mut(|i| {
                    (
                        i.consume_key(egui::Modifiers::NONE, Key::Escape),
                        i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                        i.key_pressed(Key::Enter),
                    )
                });
                if escape {
                    state.filter_open = false;
                    state.filter.clear();
                    state.focus_row = state.selected_world;
                } else if down && !order.is_empty() {
                    state.focus_row = state.selected_world.or(order.first().copied());
                } else if enter
                    && edit.lost_focus()
                    && let Some(&first) = order.first()
                {
                    let target = state.selected_world.filter(|i| order.contains(i)).unwrap_or(first);
                    cx.actions.push(AppAction::GoToWorld(target));
                }
            }
        });
}

/// Delete a saved world: at once (the undo toast offers it back), or after a question when a
/// session is open from it.
fn request_delete(state: &mut PanelState, cx: &mut SavedContext<'_>, i: usize) {
    if cx.open.contains(&i) {
        state.pending_delete = Some(i);
    } else {
        cx.actions.push(AppAction::DeleteWorld(i));
        state.pending_delete = None;
    }
}

/// The two-step delete of a world with an open session names the world, since the Delete key
/// acts on the selection.
fn confirm_delete(ui: &mut Ui, state: &mut PanelState, cx: &mut SavedContext<'_>, i: usize) {
    let theme = cx.theme;
    let Some(world) = cx.worlds.get(i) else {
        state.pending_delete = None;
        return;
    };
    egui::Frame::new()
        .stroke(Stroke::new(1.0, theme.border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .outer_margin(egui::Margin::symmetric(6, 6))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.label(RichText::new(&world.name).size(12.0).strong());
            ui.add(egui::Label::new(RichText::new(t(S::DeleteSavedWorldPrompt)).size(12.0)).wrap());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if ui.button(t(S::DeleteSavedWorld)).clicked() {
                    cx.actions.push(AppAction::DeleteWorld(i));
                    state.pending_delete = None;
                    state.selected_world = None;
                }
                if ui.button(t(S::Cancel)).clicked() {
                    state.pending_delete = None;
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(name: &str, host: &str, port: u16) -> SavedWorld {
        SavedWorld {
            name: name.into(),
            host: host.into(),
            port,
            ..SavedWorld::default()
        }
    }

    #[test]
    fn the_row_menu_has_the_csharp_items_and_explore_only_when_listed() {
        use RowCommand as R;
        assert_eq!(
            row_menu(true),
            [
                Some(R::Connect),
                Some(R::ConnectInNewTab),
                None,
                Some(R::Edit),
                Some(R::Duplicate),
                Some(R::Explore),
                None,
                Some(R::Delete)
            ]
        );
        assert!(!row_menu(false).contains(&Some(R::Explore)));
        assert_eq!(row_menu(false).len(), 7);
        // Labels come from the C# tables.
        assert_eq!(R::Connect.label(), S::ConnectSavedWorld);
        assert_eq!(R::ConnectInNewTab.label(), S::ConnectInNewTab);
        assert_eq!(R::Explore.label(), S::ExploreInDirectory);
    }

    #[test]
    fn row_commands_ask_the_app_for_the_right_thing() {
        use RowCommand as R;
        assert_eq!(row_action(R::Connect, 2), Some(AppAction::GoToWorld(2)));
        assert_eq!(row_action(R::ConnectInNewTab, 2), Some(AppAction::ConnectWorld(2)));
        assert_eq!(row_action(R::Edit, 2), Some(AppAction::EditWorld(2)));
        assert_eq!(row_action(R::Duplicate, 2), Some(AppAction::DuplicateWorld(2)));
        assert_eq!(row_action(R::Explore, 2), Some(AppAction::ExploreWorld(2)));
        assert_eq!(row_action(R::Delete, 2), None, "Delete asks in the panel first");
    }

    #[test]
    fn the_filter_matches_every_word_in_the_name_or_the_address() {
        let w = world("The Lantern Road", "lantern.example", 4000);
        assert!(matches(&w, ""));
        assert!(matches(&w, "lantern"));
        assert!(matches(&w, "ROAD the"));
        assert!(matches(&w, "example:4000"));
        assert!(matches(&w, "road 4000"));
        assert!(!matches(&w, "road harbor"));
        let worlds = vec![w, world("Starfall Reach", "starfall.example", 23)];
        assert_eq!(shown(&worlds, "star"), [1]);
        assert_eq!(shown(&worlds, "").len(), 2);
    }

    #[test]
    fn find_toggles_the_box_and_closing_clears_the_filter() {
        let mut state = PanelState::default();
        let mut actions = Vec::new();
        assert_eq!(header_actions(&state)[1].checked, Some(false));
        on_action(ACTION_FIND, &mut state, &mut actions);
        assert!(state.filter_open && state.focus_filter);
        assert_eq!(header_actions(&state)[1].checked, Some(true));
        state.filter = "star".into();
        on_action(ACTION_FIND, &mut state, &mut actions);
        assert!(!state.filter_open);
        assert!(state.filter.is_empty());
        on_action(ACTION_ADD, &mut state, &mut actions);
        assert_eq!(actions, [AppAction::NewWorld]);
    }
}
