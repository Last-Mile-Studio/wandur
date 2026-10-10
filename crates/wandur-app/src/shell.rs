//! Routing the dock's tabs to their views, and the status bar.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::{RichText, Ui, WidgetText};
use egui_dock::TabViewer;
use egui_dock::tab_viewer::OnCloseResponse;
use wandur_core::directory::DirectoryStatus;
use wandur_core::l10n::{S, t, tf};
use wandur_core::settings::Settings;

use crate::artwork::ArtLoader;
use crate::channels_view::{self, ChannelsViewState};
use crate::directory_view::{DirContext, DirectoryView};
use crate::fonts::FallbackFonts;
use crate::map_view::{self, MapViewState};
use crate::saved_worlds_panel::{self, SavedContext};
use crate::session_tab::{SessionId, Status};
use crate::sessions::{AppAction, Sessions};
use crate::terminal_view::{self, TermFonts};
use crate::theme::Theme;
use crate::workspace::Tab;
use crate::workspace_panel::{self, PanelContext, PanelState};

pub struct Viewer<'a> {
    pub sessions: &'a mut Sessions,
    pub actions: &'a mut Vec<AppAction>,
    pub theme: &'a Theme,
    pub fonts: &'a TermFonts,
    pub fallback: &'a mut FallbackFonts,
    pub settings: &'a mut Settings,
    pub frame: u64,
    pub art: &'a mut ArtLoader,
    pub directory: &'a mut DirectoryView,
    pub directory_status: &'a DirectoryStatus,
    pub directory_base: &'a str,
    pub panel: &'a mut PanelState,
    pub channels: &'a mut HashMap<SessionId, ChannelsViewState>,
    /// The docked Map panel's mini map, by the session it follows.
    pub maps: &'a mut HashMap<SessionId, MapViewState>,
    /// Each session's full map (its Map page).
    pub full_maps: &'a mut HashMap<SessionId, MapViewState>,
    /// Where a session's Play and Map side by side starts (the share last settled on).
    pub split_share: f32,
    /// The room classifier, when this build and run have one.
    pub classification: Option<&'a wandur_core::classify::RoomClassificationService>,
    /// The session the Map and Channels panels follow.
    pub active_session: Option<SessionId>,
    pub directory_active: bool,
    pub visible: &'a [SessionId],
    pub now: i64,
    /// Time and allocation per panel (the shell bench); `None` normally.
    pub stats: Option<&'a mut PanelStats>,
    /// Tool panels docked alone in their leaf: the app draws their header over the dock's tab.
    pub heads: &'a HashMap<Tab, Head>,
    /// The tabs of each leaf of documents (Find a MUD and the sessions): each shows the session
    /// tabs over its shown document.
    pub strips: &'a [Vec<Tab>],
    /// The panel headers' look, which the session tabs follow.
    pub look: &'a crate::panel_header::HeaderLook,
}

/// A tool panel whose header the app draws: the header's width last frame (the dock's tab is
/// stretched to it, so the whole header drags), and where the panel's actions go.
#[derive(Clone, Debug)]
pub struct Head {
    pub width: f32,
    pub place: crate::panel_header::ActionsPlace,
}

impl Viewer<'_> {
    fn actions_place(&self, tab: Tab) -> crate::panel_header::ActionsPlace {
        self.heads.get(&tab).map(|h| h.place.clone()).unwrap_or_default()
    }
}

/// Time and bytes allocated while building each kind of panel, summed over frames. Only the
/// shell bench turns it on; tessellation happens later and is not attributed here.
#[derive(Clone, Debug, Default)]
pub struct PanelStats {
    /// Indexed by [`Tab::kind_index`]: time, bytes allocated, calls.
    pub panels: [(Duration, u64, u64); Tab::KINDS],
}

impl PanelStats {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(*tab)
    }

    fn title(&mut self, tab: &mut Tab) -> WidgetText {
        if self.heads.contains_key(tab) {
            // Its header shows the title; the dock's own tab under it stays plain (invisible).
            return tab.label().into();
        }
        match tab {
            Tab::Session(id) => match self.sessions.get(*id) {
                Some(entry) => {
                    let s = &entry.tab;
                    let unseen = !self.visible.contains(id) && s.has_unseen();
                    let mark = match s.status {
                        Status::Connected { .. } | Status::Demo => "",
                        Status::Connecting | Status::Waiting { .. } => "… ",
                        Status::Closed { .. } => "× ",
                    };
                    let text = format!("{mark}{}{}", s.title(), if unseen { "  •" } else { "" });
                    if unseen {
                        RichText::new(text).color(self.theme.accent).into()
                    } else {
                        text.into()
                    }
                }
                None => t(S::SessionClosedTab).into(),
            },
            Tab::Channels => {
                let unread = self
                    .active_session
                    .and_then(|id| self.sessions.get(id))
                    .map_or(0, |e| e.tab.channels.log.unread_total());
                if unread > 0 {
                    RichText::new(format!("{}  {unread}", t(S::Channels)))
                        .color(self.theme.accent)
                        .into()
                } else {
                    t(S::Channels).into()
                }
            }
            other => other.label().into(),
        }
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        let started = self
            .stats
            .is_some()
            .then(|| (Instant::now(), crate::sysstat::allocated_bytes()));
        match self.strips.iter().position(|leaf| leaf.contains(tab)) {
            Some(leaf) if tab.is_document() => {
                // The session tabs across the top, the document under them.
                let body = ui.clip_rect();
                let full = ui.max_rect();
                let strip = egui::Rect::from_min_max(body.min, egui::pos2(body.right(), body.top() + self.look.height));
                self.tab_strip(ui, leaf, *tab, strip);
                let rest = egui::Rect::from_min_max(egui::pos2(full.left(), strip.bottom()), full.max);
                let clip = rest.intersect(ui.clip_rect());
                ui.scope_builder(egui::UiBuilder::new().max_rect(rest), |ui| {
                    ui.set_clip_rect(clip);
                    self.panel_ui(ui, tab);
                });
            }
            _ => self.panel_ui(ui, tab),
        }
        if let (Some(stats), Some((t0, a0))) = (self.stats.as_deref_mut(), started) {
            let entry = &mut stats.panels[tab.kind_index()];
            entry.0 += t0.elapsed();
            entry.1 += crate::sysstat::allocated_bytes().saturating_sub(a0);
            entry.2 += 1;
        }
    }

    fn is_closeable(&self, tab: &Tab) -> bool {
        // A headed panel's close button is in the header the app draws.
        !self.heads.contains_key(tab)
    }

    fn tab_style_override(&self, tab: &Tab, global: &egui_dock::TabStyle) -> Option<egui_dock::TabStyle> {
        let head = self.heads.get(tab)?;
        // The dock's tab spans the whole header, invisible: it stays the drag handle (and the
        // click that focuses the panel) under the header the app paints over it.
        let mut style = global.clone();
        style.minimum_width = Some((head.width - 1.0).max(0.0));
        style.hline_below_active_tab_name = false;
        for state in [
            &mut style.active,
            &mut style.inactive,
            &mut style.focused,
            &mut style.hovered,
            &mut style.inactive_with_kb_focus,
            &mut style.active_with_kb_focus,
            &mut style.focused_with_kb_focus,
        ] {
            state.bg_fill = egui::Color32::TRANSPARENT;
            state.outline_color = egui::Color32::TRANSPARENT;
            state.text_color = egui::Color32::TRANSPARENT;
        }
        Some(style)
    }

    fn context_menu(&mut self, ui: &mut Ui, tab: &mut Tab, _path: egui_dock::NodePath) {
        if crate::autohide::can_hide(*tab) && ui.button(t(S::AutoHide)).on_hover_text(t(S::AutoHideHint)).clicked() {
            self.actions.push(AppAction::Unpin(*tab));
            ui.close();
        }
    }

    fn on_close(&mut self, tab: &mut Tab) -> OnCloseResponse {
        if let Tab::Session(id) = tab {
            self.actions.push(AppAction::Close(*id));
        }
        OnCloseResponse::Close
    }

    fn scroll_bars(&self, _tab: &Tab) -> [bool; 2] {
        // Each tab manages its own scrolling (the terminal and the directory virtualize rows).
        [false, false]
    }
}

/// The session tabs' items for a document leaf's tabs: titles (sessions of one world under the
/// same name numbered, as the Workspace panel numbers them), connection dots, and new output on
/// sessions not shown.
pub fn strip_items<'a>(
    tabs: &[Tab],
    sessions: &'a Sessions,
    visible: &[SessionId],
) -> Vec<crate::session_tabs::Item<'a>> {
    use crate::session_tabs::{Conn, Item};
    use std::borrow::Cow;
    let titles: Vec<&str> = tabs
        .iter()
        .map(|tab| match tab {
            Tab::Session(id) => sessions.get(*id).map_or("", |e| e.tab.title()),
            _ => "",
        })
        .collect();
    tabs.iter()
        .enumerate()
        .map(|(n, tab)| match tab {
            Tab::Session(id) => match sessions.get(*id) {
                Some(entry) => {
                    let s = &entry.tab;
                    let title = titles[n];
                    let same = titles.iter().filter(|t| **t == title).count();
                    let title = if same > 1 && s.custom_name.is_none() {
                        let k = titles[..n].iter().filter(|t| **t == title).count() + 1;
                        Cow::Owned(format!("{title} · {k}"))
                    } else {
                        Cow::Borrowed(title)
                    };
                    Item {
                        tab: *tab,
                        title,
                        conn: Some(Conn::of(&s.status)),
                        activity: !visible.contains(id) && s.has_unseen(),
                    }
                }
                None => Item {
                    tab: *tab,
                    title: Cow::Borrowed(t(S::SessionClosedTab)),
                    conn: Some(Conn::Disconnected),
                    activity: false,
                },
            },
            other => Item {
                tab: *other,
                title: Cow::Borrowed(other.label()),
                conn: None,
                activity: false,
            },
        })
        .collect()
}

/// The id of the session tab strip over document leaf `leaf` (tests and scenes read where it
/// drew its tabs with [`crate::session_tabs::tab_rects`]).
pub fn strip_id(leaf: usize) -> egui::Id {
    egui::Id::new(("session-tabs", leaf))
}

impl Viewer<'_> {
    /// The session tabs over a document leaf's shown tab `shown`.
    fn tab_strip(&mut self, ui: &mut Ui, leaf: usize, shown: Tab, rect: egui::Rect) {
        use crate::session_tabs::{self, StripAction};
        let tabs = &self.strips[leaf];
        let sessions = &*self.sessions;
        let items = strip_items(tabs, sessions, self.visible);
        let strip = session_tabs::Strip {
            id: strip_id(leaf),
            items: &items,
            active: shown,
            look: self.look,
            theme: self.theme,
        };
        let mut tip = |tab: Tab| match tab {
            Tab::Session(id) => sessions.get(id).map_or_else(String::new, |e| {
                let s = &e.tab;
                if s.is_demo() {
                    s.status.label()
                } else {
                    format!("{} · {} · {}", s.title(), s.endpoint, s.status.label())
                }
            }),
            other => other.label().to_string(),
        };
        let actions = session_tabs::show(ui, rect, &strip, &mut tip);
        for action in actions {
            self.actions.push(match action {
                StripAction::Focus(Tab::Session(id)) => AppAction::Focus(id),
                StripAction::Focus(_) | StripAction::New => AppAction::OpenDirectory,
                StripAction::Close(Tab::Session(id)) => AppAction::Close(id),
                StripAction::Close(_) => AppAction::CloseDirectory,
                StripAction::CloseOthers(tab) => AppAction::CloseOthers(tab),
                StripAction::Move(tab, to) => AppAction::MoveTab(tab, to),
                StripAction::Reconnect(id) => AppAction::Reconnect(id),
                StripAction::Disconnect(id) => AppAction::Disconnect(id),
                StripAction::Duplicate(id) => AppAction::Duplicate(id),
                StripAction::Rename(id, name) => AppAction::Rename(id, name),
            });
        }
    }

    /// The Map panel's mini map (following `id`), or `id`'s full map (its Map page).
    fn map_ui(&mut self, ui: &mut Ui, id: Option<SessionId>, full: bool) {
        let actions_place = if full {
            crate::panel_header::ActionsPlace::Bar
        } else {
            self.actions_place(Tab::Map)
        };
        let tab = id.and_then(|id| self.sessions.get_mut(id)).map(|e| &mut e.tab);
        let state = if full {
            self.full_maps.entry(id.unwrap_or(0)).or_insert_with(MapViewState::full)
        } else {
            self.maps.entry(id.unwrap_or(0)).or_insert_with(MapViewState::mini)
        };
        // The mini map follows the auto-center setting; the full map has its own toggle.
        let mut auto_center = if full {
            state.follow
        } else {
            self.settings.map_auto_center
        };
        let mut enabled = self.settings.classify_rooms_locally;
        let mut threshold = self.settings.room_classification_threshold;
        let mut editor_prefs = self.settings.map_editor.clone();
        let mut cx = map_view::MapContext::new(self.theme, &mut auto_center);
        cx.actions_place = actions_place;
        if full {
            cx.editor_prefs = Some(&mut editor_prefs);
        }
        cx.inference = self.classification.map(|service| map_view::InferenceControls {
            service,
            enabled: &mut enabled,
            threshold: &mut threshold,
            install: None,
        });
        map_view::show(ui, tab, state, &mut cx);
        let (open_editor, open_full, deleted) = (cx.open_editor, cx.open_full, cx.deleted);
        if let (Some(deleted), Some(id)) = (deleted, id) {
            self.actions.push(AppAction::MapDeleted(id, deleted));
        }
        if cx.open_import {
            self.actions.push(AppAction::OpenMapImport(id));
        }
        let install = cx.inference.and_then(|c| c.install);
        let follow = if full {
            state.follow = auto_center;
            self.settings.map_auto_center
        } else {
            auto_center
        };
        if open_editor && let Some(id) = id {
            self.actions.push(AppAction::OpenMapEditor(id));
        } else if open_full && let Some(id) = id {
            self.actions.push(AppAction::OpenFullMap(id));
        }
        if let Some(path) = install {
            self.actions.push(AppAction::InstallClassifier(path));
        }
        if follow != self.settings.map_auto_center
            || editor_prefs != self.settings.map_editor
            || enabled != self.settings.classify_rooms_locally
            || threshold != self.settings.room_classification_threshold
        {
            self.settings.map_auto_center = follow;
            self.settings.map_editor = editor_prefs;
            self.settings.classify_rooms_locally = enabled;
            self.settings.room_classification_threshold = threshold;
            self.actions.push(AppAction::SettingsChanged);
        }
    }

    fn panel_ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        match tab {
            Tab::Workspace => {
                let mut cx = PanelContext {
                    sessions: self.sessions,
                    theme: self.theme,
                    actions: self.actions,
                    directory_active: self.directory_active,
                    active_session: self.active_session,
                    visible: self.visible,
                };
                workspace_panel::show(ui, self.panel, &mut cx);
            }
            Tab::SavedWorlds => {
                let catalog = self.directory_status.catalog.clone();
                let actions_place = self.actions_place(Tab::SavedWorlds);
                let open: Vec<usize> = self.sessions.iter().filter_map(|e| e.tab.world).collect();
                let mut cx = SavedContext {
                    worlds: &self.settings.worlds,
                    open: &open,
                    catalog: catalog.as_deref(),
                    base: self.directory_base,
                    art: self.art,
                    theme: self.theme,
                    actions: self.actions,
                    actions_place,
                };
                saved_worlds_panel::show(ui, self.panel, &mut cx);
            }
            Tab::Directory => {
                let mut cx = DirContext {
                    status: self.directory_status,
                    base: self.directory_base,
                    art: self.art,
                    theme: self.theme,
                    settings: self.settings,
                    actions: self.actions,
                    now: self.now,
                };
                self.directory.show(ui, &mut cx);
            }
            Tab::Map => {
                let id = self.active_session;
                self.map_ui(ui, id, false);
            }
            Tab::Channels => {
                let id = self.active_session;
                let state = self.channels.entry(id.unwrap_or(0)).or_default();
                let tab = id.and_then(|id| self.sessions.get_mut(id)).map(|e| &mut e.tab);
                channels_view::show(ui, tab, state, self.theme, self.actions);
            }
            Tab::Session(id) => {
                let Some(entry) = self.sessions.get_mut(*id) else {
                    ui.label(t(S::ThisSessionIsClosed));
                    return;
                };
                entry.tab.last_shown_frame = self.frame;
                if entry.view.split_share == 0.0 {
                    entry.view.split_share = self.split_share;
                }
                let actions = terminal_view::show(
                    ui,
                    &mut entry.tab,
                    &mut entry.view,
                    self.theme,
                    self.fonts,
                    self.fallback,
                    &terminal_view::PaintOptions::from_settings(self.settings),
                );
                entry.tab.mark_seen();
                if let Some(share) = actions.share {
                    self.settings.scroll_tail_share = share;
                    self.actions.push(AppAction::SettingsChanged);
                }
                if let Some(url) = actions.open_link {
                    self.actions.push(AppAction::OpenLink(url));
                }
                if let Some((example, second)) = actions.mark_channel {
                    self.actions.push(AppAction::MarkChannel(*id, example, second));
                }
                if let Some(notice) = actions.notice {
                    self.actions.push(AppAction::Notice(t(notice).to_string()));
                }
                if let Some(size) = actions.resized {
                    entry.tab.resized(size);
                }
                if actions.edit_scripts {
                    self.actions.push(AppAction::EditScripts(*id));
                }
                if actions.reload_scripts {
                    self.actions.push(AppAction::ReloadScripts(*id));
                }
                if actions.edit_agent {
                    self.actions.push(AppAction::EditAgent(*id));
                }
                if actions.save_world {
                    self.actions.push(AppAction::SaveWorld(*id));
                }
                if actions.disconnect {
                    self.actions.push(AppAction::Disconnect(*id));
                }
                if actions.reconnect {
                    self.actions.push(AppAction::Reconnect(*id));
                }
                if let Some(share) = actions.split_share {
                    self.actions.push(AppAction::RememberSplit(share));
                }
                // The full map, where the session's view left room for it (the Map page, or
                // side by side). Hidden, it is not drawn at all.
                if let Some(rect) = actions.map_rect {
                    let id = *id;
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .id_salt(("full-map", id))
                            .max_rect(rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    child.set_clip_rect(rect.intersect(ui.clip_rect()));
                    child.painter().rect_filled(rect, 0.0, self.theme.panel);
                    self.map_ui(&mut child, Some(id), true);
                }
            }
        }
    }
}

/// The status bar, as the C# footer: sessions and connections and the active session's state
/// (no standing hint since the C# UI review, item 6).
pub fn status_bar(ui: &mut Ui, sessions: &Sessions, active: Option<SessionId>, theme: &Theme) {
    ui.horizontal(|ui| {
        let count = sessions.len();
        let connected = sessions.connected();
        let label = tf(
            if count == 1 { S::SessionsOne } else { S::SessionsMany },
            &[&count, &connected],
        );
        ui.label(RichText::new(label).size(12.0).color(theme.muted));
        ui.label(RichText::new("·").size(10.0).color(theme.muted));
        match active.and_then(|id| sessions.get(id)) {
            Some(entry) => {
                let s = &entry.tab;
                let color = match s.status {
                    Status::Connected { .. } | Status::Demo => theme.ok,
                    Status::Connecting | Status::Waiting { .. } => theme.warn,
                    Status::Closed { .. } => theme.muted,
                };
                if s.is_connected() {
                    Theme::dot(ui, color);
                } else {
                    Theme::ring(ui, color);
                }
                ui.label(
                    RichText::new(format!("{}  ·  {}", s.title(), s.status.label()))
                        .size(12.0)
                        .color(theme.muted),
                );
            }
            None => {
                ui.label(RichText::new(t(S::ReadyToWander)).size(12.0).color(theme.muted));
            }
        }
    });
}
