//! Pinned and auto-hidden tool panels, as Visual Studio (and the C# client, through
//! Dock.Avalonia) do: unpinning a tool panel (Workspace, Map, Channels) takes it out of the dock
//! and puts a labelled tab on the strip of its edge; hovering or clicking that tab slides the panel
//! out over the workspace; Escape, a click elsewhere, or moving away (when it was opened by
//! hovering) hides it again; pinning puts it back where it was.
//!
//! egui_dock has no auto-hide, so this is ours: this module holds the state and the dock surgery
//! (where the panel was, how to put it back), the app draws the strips and the overlay.
//!
//! Putting it back: a panel that shared a leaf rejoins a leaf holding one of its old neighbours;
//! a panel that was one side of a split is split back in beside the smallest part of the dock that
//! still holds everything that was on the other side, on the same side and with the same fraction.
//! If none of that is open any more, it opens where the View menu would put it.

use std::time::{Duration, Instant};

use egui::Rect;
use egui_dock::{DockState, Node, NodeIndex, Split};
use serde::{Deserialize, Serialize};

use crate::workspace::{self, Tab};

/// Which strip an auto-hidden panel sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Left,
    Right,
    Bottom,
}

/// Where a panel was in the dock when it was unpinned.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    /// It shared a leaf with these tabs.
    Leaf(Vec<Tab>),
    /// It was one side of a split; the other side held `others`.
    Split {
        others: Vec<Tab>,
        /// It was the left (or top) side.
        first: bool,
        horizontal: bool,
        /// The share of the left or top side.
        fraction: f32,
    },
    /// Floating, or nothing else was open.
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hidden {
    pub tab: Tab,
    pub edge: Edge,
    /// Width (left, right) or height (bottom) of the slid-out panel, in points.
    pub size: f32,
    pub place: Place,
}

/// The overlay that is slid out now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    pub tab: Tab,
    /// Opened by a click: it stays until Escape or a click elsewhere. Opened by hovering: it also
    /// hides when the pointer has been away for [`HOVER_GRACE`].
    pub by_click: bool,
    /// When the pointer left the strip tab and the overlay (hover mode).
    pub away_since: Option<Instant>,
}

/// How long the pointer may be away before a hovered-open panel hides.
pub const HOVER_GRACE: Duration = Duration::from_millis(400);
/// Strip thickness in points.
pub const STRIP: f32 = 24.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutoHide {
    pub hidden: Vec<Hidden>,
    pub shown: Option<Shown>,
}

/// Tool panels can be unpinned; documents (sessions, Find a MUD, Settings) cannot.
pub fn can_hide(tab: Tab) -> bool {
    matches!(tab, Tab::Workspace | Tab::SavedWorlds | Tab::Map | Tab::Channels)
}

/// Every tab in the subtree under `at`.
fn tabs_under(tree: &egui_dock::Tree<Tab>, at: NodeIndex, out: &mut Vec<Tab>) {
    if at.0 >= tree.len() {
        return;
    }
    match &tree[at] {
        Node::Empty => {}
        Node::Leaf(leaf) => out.extend(leaf.tabs.iter().copied()),
        Node::Horizontal(_) | Node::Vertical(_) => {
            tabs_under(tree, at.left(), out);
            tabs_under(tree, at.right(), out);
        }
    }
}

impl AutoHide {
    pub fn is_hidden(&self, tab: Tab) -> bool {
        self.hidden.iter().any(|h| h.tab == tab)
    }

    pub fn on_edge(&self, edge: Edge) -> impl Iterator<Item = &Hidden> {
        self.hidden.iter().filter(move |h| h.edge == edge)
    }

    /// Take `tab` out of the dock onto the strip of its nearest edge of `area` (the dock's
    /// rectangle). Returns false for documents or panels that are not open.
    pub fn unpin(&mut self, dock: &mut DockState<Tab>, tab: Tab, area: Rect) -> bool {
        if !can_hide(tab) || self.is_hidden(tab) {
            return false;
        }
        let Some(path) = dock.find_tab(&tab) else {
            return false;
        };
        let mut place = Place::Unknown;
        let mut rect = Rect::NOTHING;
        if path.surface.is_main() {
            let tree = dock.main_surface();
            let node = path.node;
            if let Node::Leaf(leaf) = &tree[node] {
                rect = leaf.rect;
                if leaf.tabs.len() > 1 {
                    place = Place::Leaf(leaf.tabs.iter().copied().filter(|t| *t != tab).collect());
                } else if let Some(parent) = node.parent() {
                    let first = node == parent.left();
                    let other = if first { parent.right() } else { parent.left() };
                    let mut others = Vec::new();
                    tabs_under(tree, other, &mut others);
                    if let Node::Horizontal(s) | Node::Vertical(s) = &tree[parent] {
                        place = Place::Split {
                            others,
                            first,
                            horizontal: matches!(tree[parent], Node::Horizontal(_)),
                            fraction: s.fraction,
                        };
                    }
                }
            }
        }
        let edge = edge_for(tab, rect, area);
        let size = if rect.is_positive() {
            match edge {
                Edge::Left | Edge::Right => rect.width(),
                Edge::Bottom => rect.height(),
            }
        } else {
            default_size(edge)
        };
        dock.remove_tab(path);
        self.hidden.push(Hidden {
            tab,
            edge,
            size: size.clamp(160.0, 900.0),
            place,
        });
        self.shown = None;
        true
    }

    /// Put an auto-hidden panel back into the dock where it was. Returns false if it was not hidden.
    pub fn pin(&mut self, dock: &mut DockState<Tab>, tab: Tab) -> bool {
        let Some(i) = self.hidden.iter().position(|h| h.tab == tab) else {
            return false;
        };
        let hidden = self.hidden.remove(i);
        if self.shown.is_some_and(|s| s.tab == tab) {
            self.shown = None;
        }
        restore(dock, &hidden);
        true
    }

    /// Show a panel's overlay (from its strip tab).
    pub fn show(&mut self, tab: Tab, by_click: bool) {
        match &mut self.shown {
            Some(s) if s.tab == tab => {
                s.by_click |= by_click;
                s.away_since = None;
            }
            _ => {
                self.shown = Some(Shown {
                    tab,
                    by_click,
                    away_since: None,
                })
            }
        }
    }

    pub fn hide(&mut self) {
        self.shown = None;
    }

    /// The pointer is over the strip tab or the overlay (`inside`), or not. Hides a hovered-open
    /// overlay once the pointer has been away for the grace period. Returns when to look again.
    pub fn track_pointer(&mut self, inside: bool, now: Instant) -> Option<Duration> {
        let shown = self.shown.as_mut()?;
        if inside || shown.by_click {
            shown.away_since = None;
            return None;
        }
        let since = *shown.away_since.get_or_insert(now);
        let away = now.duration_since(since);
        if away >= HOVER_GRACE {
            self.shown = None;
            None
        } else {
            Some(HOVER_GRACE - away)
        }
    }
}

fn default_size(edge: Edge) -> f32 {
    match edge {
        Edge::Left | Edge::Right => 300.0,
        Edge::Bottom => 240.0,
    }
}

/// The strip a panel goes to: by where its leaf was in the dock, else by kind.
fn edge_for(tab: Tab, rect: Rect, area: Rect) -> Edge {
    if rect.is_positive() && area.is_positive() {
        let x = (rect.center().x - area.left()) / area.width();
        if x < 0.4 {
            return Edge::Left;
        }
        if x > 0.6 {
            return Edge::Right;
        }
        return Edge::Bottom;
    }
    match tab {
        Tab::Workspace | Tab::SavedWorlds => Edge::Left,
        _ => Edge::Right,
    }
}

fn restore(dock: &mut DockState<Tab>, hidden: &Hidden) {
    let tab = hidden.tab;
    if dock.find_tab(&tab).is_some() {
        return;
    }
    match &hidden.place {
        Place::Leaf(neighbours) => {
            if let Some(path) = neighbours.iter().find_map(|n| dock.find_tab(n))
                && let Ok(leaf) = dock.leaf_mut(path.node_path())
            {
                leaf.append_tab(tab);
                let last = leaf.len() - 1;
                let _ = leaf.set_active_tab(last);
                return;
            }
        }
        Place::Split {
            others,
            first,
            horizontal,
            fraction,
        } => {
            let tree = dock.main_surface_mut();
            let open: Vec<Tab> = others.iter().copied().filter(|t| tree.find_tab(t).is_some()).collect();
            if !open.is_empty() {
                // The smallest subtree holding every one of them.
                let mut best: Option<(NodeIndex, usize)> = None;
                for i in 0..tree.len() {
                    let at = NodeIndex(i);
                    if matches!(tree[at], Node::Empty) {
                        continue;
                    }
                    let mut under = Vec::new();
                    tabs_under(tree, at, &mut under);
                    if open.iter().all(|t| under.contains(t)) && best.is_none_or(|(_, n)| under.len() < n) {
                        best = Some((at, under.len()));
                    }
                }
                if let Some((at, _)) = best {
                    let split = match (horizontal, first) {
                        (true, true) => Split::Left,
                        (true, false) => Split::Right,
                        (false, true) => Split::Above,
                        (false, false) => Split::Below,
                    };
                    tree.split(at, split, fraction.clamp(0.05, 0.95), Node::leaf(tab));
                    return;
                }
            }
        }
        Place::Unknown => {}
    }
    workspace::open_panel(dock, tab);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    fn area() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 700.0))
    }

    /// The default layout with leaf rectangles as egui_dock would set them when drawn.
    fn laid_out() -> DockState<Tab> {
        let mut dock = workspace::default_layout();
        for (_, leaf) in dock.iter_leaves_mut() {
            leaf.rect = match leaf.tabs[0] {
                Tab::Workspace => Rect::from_min_max(pos2(0.0, 0.0), pos2(180.0, 700.0)),
                Tab::Map => Rect::from_min_max(pos2(770.0, 0.0), pos2(1000.0, 400.0)),
                Tab::Channels => Rect::from_min_max(pos2(770.0, 400.0), pos2(1000.0, 700.0)),
                _ => Rect::from_min_max(pos2(180.0, 0.0), pos2(770.0, 700.0)),
            };
        }
        dock
    }

    fn json(dock: &DockState<Tab>) -> String {
        crate::layout::to_json(dock, &AutoHide::default()).unwrap()
    }

    #[test]
    fn unpinning_moves_a_tool_panel_to_its_edge_and_pinning_restores_it_exactly() {
        for (tab, edge) in [
            (Tab::Workspace, Edge::Left),
            (Tab::Map, Edge::Right),
            (Tab::Channels, Edge::Right),
        ] {
            let mut dock = laid_out();
            let before = json(&dock);
            let mut auto = AutoHide::default();
            assert!(auto.unpin(&mut dock, tab, area()), "{tab:?}");
            assert!(dock.find_tab(&tab).is_none(), "out of the dock");
            let hidden = &auto.hidden[0];
            assert_eq!(hidden.edge, edge, "{tab:?}");
            assert!(hidden.size >= 160.0);
            assert!(!auto.unpin(&mut dock, tab, area()), "already hidden");
            assert!(auto.pin(&mut dock, tab));
            assert!(auto.hidden.is_empty());
            assert_eq!(json(&dock), before, "{tab:?} is back in its place with its fraction");
        }
    }

    #[test]
    fn documents_cannot_be_unpinned() {
        let mut dock = laid_out();
        let mut auto = AutoHide::default();
        assert!(!auto.unpin(&mut dock, Tab::Directory, area()));
        assert!(
            !auto.unpin(&mut dock, Tab::Session(9), area()),
            "not open, and a document"
        );
        assert!(auto.hidden.is_empty());
    }

    #[test]
    fn a_panel_that_shared_a_leaf_rejoins_its_neighbour() {
        let mut dock = laid_out();
        // Channels joins Map's leaf.
        let path = dock.find_tab(&Tab::Channels).unwrap();
        dock.remove_tab(path);
        workspace::open_panel(&mut dock, Tab::Channels);
        let mut auto = AutoHide::default();
        auto.unpin(&mut dock, Tab::Channels, area());
        assert_eq!(auto.hidden[0].place, Place::Leaf(vec![Tab::Map]));
        auto.pin(&mut dock, Tab::Channels);
        let map = dock.find_tab(&Tab::Map).unwrap();
        let channels = dock.find_tab(&Tab::Channels).unwrap();
        assert_eq!(map.node_path(), channels.node_path());
    }

    #[test]
    fn with_its_neighbours_gone_it_opens_where_view_would_put_it() {
        let mut dock = laid_out();
        let mut auto = AutoHide::default();
        auto.unpin(&mut dock, Tab::Channels, area());
        let path = dock.find_tab(&Tab::Map).unwrap();
        dock.remove_tab(path);
        auto.pin(&mut dock, Tab::Channels);
        assert!(dock.find_tab(&Tab::Channels).is_some());
    }

    #[test]
    fn overlays_open_by_hover_or_click_and_hide_by_leaving_or_escape() {
        let mut auto = AutoHide::default();
        let mut dock = laid_out();
        auto.unpin(&mut dock, Tab::Map, area());
        let t0 = Instant::now();
        auto.show(Tab::Map, false);
        assert!(auto.track_pointer(true, t0).is_none());
        // Away for less than the grace period: still out, look again later.
        assert!(auto.track_pointer(false, t0).is_some());
        assert!(auto.track_pointer(false, t0 + HOVER_GRACE / 2).is_some());
        assert!(auto.shown.is_some());
        // Back in time resets the clock.
        auto.track_pointer(true, t0 + HOVER_GRACE / 2);
        auto.track_pointer(false, t0 + HOVER_GRACE);
        assert!(auto.shown.is_some());
        auto.track_pointer(false, t0 + HOVER_GRACE * 3);
        assert!(auto.shown.is_none(), "hidden after moving away");
        // A click keeps it out while the pointer is elsewhere.
        auto.show(Tab::Map, true);
        auto.track_pointer(false, t0 + HOVER_GRACE * 10);
        assert!(auto.shown.is_some());
        auto.hide();
        assert!(auto.shown.is_none());
        // Pinning a shown panel closes its overlay.
        auto.show(Tab::Map, true);
        auto.pin(&mut dock, Tab::Map);
        assert!(auto.shown.is_none());
    }
}
