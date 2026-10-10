//! The dock's tabs, the C# client's layout (Workspace over Saved worlds on the left, sessions
//! and Find a MUD in the middle, Map over Channels on the right), the View > Layout presets (a
//! new install starts with Panels on the left), and where new tabs go.

use egui_dock::{DockState, Node, NodeIndex, Split};
use wandur_core::l10n::{S, t};

use crate::session_tab::SessionId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    /// Find a MUD and the open sessions.
    Workspace,
    /// The saved worlds (their own panel since the C# `ui/panel-headers` work).
    SavedWorlds,
    /// The world directory (Find a MUD).
    Directory,
    Map,
    Channels,
    Session(SessionId),
}

impl Tab {
    /// The panels a person can open from the View menu, in menu order.
    pub const PANELS: [Tab; 5] = [
        Tab::Workspace,
        Tab::SavedWorlds,
        Tab::Directory,
        Tab::Map,
        Tab::Channels,
    ];

    /// The name used in the saved layout; sessions are not saved.
    pub fn persist_name(self) -> Option<&'static str> {
        Some(match self {
            Tab::Workspace => "workspace",
            Tab::SavedWorlds => "saved_worlds",
            Tab::Directory => "directory",
            Tab::Map => "map",
            Tab::Channels => "channels",
            Tab::Session(_) => return None,
        })
    }

    pub fn from_name(name: &str) -> Option<Tab> {
        Tab::PANELS.into_iter().find(|t| t.persist_name() == Some(name))
    }

    /// The tab's name in the current language.
    pub fn label(self) -> &'static str {
        t(match self {
            Tab::Workspace => S::Workspace,
            Tab::SavedWorlds => S::SavedWorlds,
            Tab::Directory => S::FindAMUD,
            Tab::Map => S::Map,
            Tab::Channels => S::Channels,
            Tab::Session(_) => S::Session,
        })
    }

    /// How many kinds of tab there are ([`Self::kind_index`]).
    pub const KINDS: usize = 6;

    /// A small number per kind of tab (sessions share one), for per-panel statistics.
    pub fn kind_index(self) -> usize {
        match self {
            Tab::Workspace => 0,
            Tab::Directory => 1,
            Tab::Map => 2,
            Tab::Channels => 3,
            Tab::Session(_) => 4,
            Tab::SavedWorlds => 5,
        }
    }

    /// Tabs that belong in the central document area.
    pub fn is_document(self) -> bool {
        matches!(self, Tab::Directory | Tab::Session(_))
    }
}

/// The Workspace's share of the left column, over Saved worlds (C# `WorkspaceFactory`).
pub const WORKSPACE_SHARE: f32 = 0.35;

/// The C# client's proportions: left 0.18 (Workspace 0.35 over Saved worlds), documents 0.59,
/// right 0.23 (Map 0.58 over Channels).
pub fn default_layout() -> DockState<Tab> {
    layout_with_shares(0.59, 0.23)
}

/// The default layout with the documents and the right column (Map over Channels) taking these
/// shares of the width (the Workspace keeps 0.18). Scenes widen the map this way.
pub fn layout_with_shares(documents: f32, right: f32) -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Directory]);
    let tree = dock.main_surface_mut();
    let [documents_node, workspace] = tree.split(NodeIndex::root(), Split::Left, 0.18, Node::leaf(Tab::Workspace));
    tree.split(workspace, Split::Below, WORKSPACE_SHARE, Node::leaf(Tab::SavedWorlds));
    let [_docs, right] = tree.split(
        documents_node,
        Split::Right,
        documents / (documents + right),
        Node::leaf(Tab::Map),
    );
    tree.split(right, Split::Below, 0.58, Node::leaf(Tab::Channels));
    focus_tab(&mut dock, Tab::Directory);
    dock
}

/// The arrangements View > Layout offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// One column of panels at the left (Map, Saved worlds, Channels, top to bottom), the
    /// transcript in the middle, the session's script rail at its right; the Workspace panel is
    /// closed (View > Workspace opens it). The layout of a new install.
    Left,
    /// The same column at the right.
    Right,
    /// The C# client's layout: Workspace over Saved worlds at the left, Map over Channels at the
    /// right.
    Both,
    /// Every tool panel unpinned to the window's edges: the transcript (and its script rail) alone.
    Focus,
}

impl Preset {
    /// In menu order.
    pub const ALL: [Preset; 4] = [Preset::Left, Preset::Right, Preset::Both, Preset::Focus];

    pub fn label(self) -> S {
        match self {
            Preset::Left => S::LayoutPanelsLeft,
            Preset::Right => S::LayoutPanelsRight,
            Preset::Both => S::LayoutBothSides,
            Preset::Focus => S::LayoutFocus,
        }
    }
}

/// The share of the width the panel column takes in the one-column presets (about 260 points
/// in a 1300 point window).
pub const COLUMN_SHARE: f32 = 0.2;
/// The panel column's rows in the one-column presets: Map, then Saved worlds, then Channels.
const COLUMN_ROWS: [f32; 2] = [0.32, 0.26];

/// The layout a new install starts with (no `layout.json` yet).
pub const NEW_INSTALL: Preset = Preset::Left;

/// A preset's dock and its unpinned panels.
pub fn preset_layout(preset: Preset) -> (DockState<Tab>, crate::autohide::AutoHide) {
    use crate::autohide::{AutoHide, Edge, Hidden, Place};
    let mut auto = AutoHide::default();
    let dock = match preset {
        Preset::Both => default_layout(),
        Preset::Left | Preset::Right => {
            let mut dock = DockState::new(vec![Tab::Directory]);
            let tree = dock.main_surface_mut();
            let (side, fraction) = if preset == Preset::Left {
                (Split::Left, COLUMN_SHARE)
            } else {
                (Split::Right, 1.0 - COLUMN_SHARE)
            };
            let [_, column] = tree.split(NodeIndex::root(), side, fraction, Node::leaf(Tab::Map));
            // Map on top, then Saved worlds, then Channels.
            let [_, rest] = tree.split(column, Split::Below, COLUMN_ROWS[0], Node::leaf(Tab::SavedWorlds));
            let saved_share = COLUMN_ROWS[1] / (1.0 - COLUMN_ROWS[0]);
            tree.split(rest, Split::Below, saved_share, Node::leaf(Tab::Channels));
            focus_tab(&mut dock, Tab::Directory);
            dock
        }
        Preset::Focus => {
            let mut dock = DockState::new(vec![Tab::Directory]);
            focus_tab(&mut dock, Tab::Directory);
            // Pinned again, each goes back beside the documents on its own side.
            let hidden = |tab: Tab, edge: Edge| Hidden {
                tab,
                edge,
                size: 300.0,
                place: Place::Split {
                    others: vec![Tab::Directory],
                    first: edge == Edge::Left,
                    horizontal: true,
                    fraction: if edge == Edge::Left { 0.2 } else { 0.77 },
                },
            };
            auto.hidden = vec![
                hidden(Tab::Workspace, Edge::Left),
                hidden(Tab::SavedWorlds, Edge::Left),
                hidden(Tab::Map, Edge::Right),
                hidden(Tab::Channels, Edge::Right),
            ];
            dock
        }
    };
    (dock, auto)
}

/// Make a tab active in its leaf and focus that leaf. Returns false if the tab is not open.
pub fn focus_tab(dock: &mut DockState<Tab>, tab: Tab) -> bool {
    match dock.find_tab(&tab) {
        Some(path) => {
            let _ = dock.set_active_tab(path);
            dock.set_focused_node_and_surface(path.node_path());
            true
        }
        None => false,
    }
}

/// Add a tab to the document area: next to the sessions or Find a MUD, else in the focused
/// leaf. Focuses it.
pub fn add_document(dock: &mut DockState<Tab>, tab: Tab) {
    let target = dock
        .find_tab_from(|t| matches!(t, Tab::Session(_)))
        .or_else(|| dock.find_tab(&Tab::Directory));
    match target.and_then(|path| dock.leaf_mut(path.node_path()).ok().map(|leaf| (path, leaf))) {
        Some((path, leaf)) => {
            leaf.append_tab(tab);
            let last = leaf.len() - 1;
            let _ = leaf.set_active_tab(last);
            dock.set_focused_node_and_surface(path.node_path());
        }
        None => dock.push_to_focused_leaf(tab),
    }
}

/// Open a panel where it belongs, or focus it if it is open.
pub fn open_panel(dock: &mut DockState<Tab>, tab: Tab) {
    if focus_tab(dock, tab) {
        return;
    }
    match tab {
        Tab::Directory | Tab::Session(_) => add_document(dock, tab),
        Tab::Map | Tab::Channels => {
            // Next to the other right-hand panel, else to the right of the documents.
            let other = if tab == Tab::Map { Tab::Channels } else { Tab::Map };
            if let Some(path) = dock.find_tab(&other)
                && let Ok(leaf) = dock.leaf_mut(path.node_path())
            {
                leaf.append_tab(tab);
                let last = leaf.len() - 1;
                let _ = leaf.set_active_tab(last);
                return;
            }
            split_from_documents(dock, tab, Split::Right, 0.75);
        }
        Tab::Workspace | Tab::SavedWorlds => {
            // Over or under the other left-hand panel, else to the left of the documents.
            let (other, split, share) = if tab == Tab::Workspace {
                (Tab::SavedWorlds, Split::Above, WORKSPACE_SHARE)
            } else {
                (Tab::Workspace, Split::Below, WORKSPACE_SHARE)
            };
            if let Some(path) = dock.find_tab(&other)
                && path.surface == egui_dock::SurfaceIndex::main()
            {
                dock.main_surface_mut().split(path.node, split, share, Node::leaf(tab));
                focus_tab(dock, tab);
                return;
            }
            split_from_documents(dock, tab, Split::Left, 0.2);
        }
    }
}

/// Put a tab in a new leaf beside the main surface's root (the whole main area).
fn split_from_documents(dock: &mut DockState<Tab>, tab: Tab, split: Split, fraction: f32) {
    let tree = dock.main_surface_mut();
    if tree.root_node().is_none_or(|n| n.is_empty()) {
        dock.push_to_first_leaf(tab);
        return;
    }
    tree.split(NodeIndex::root(), split, fraction, Node::leaf(tab));
}

/// The document tabs in the order the session tabs show them: each main-surface leaf that holds
/// only documents, in tree order (in practice one leaf: Find a MUD and the sessions).
pub fn documents(dock: &DockState<Tab>) -> Vec<Tab> {
    let mut out = Vec::new();
    for (path, leaf) in dock.iter_leaves() {
        if path.surface.is_main() && !leaf.tabs.is_empty() && leaf.tabs.iter().all(|t| t.is_document()) {
            out.extend(leaf.tabs.iter().copied());
        }
    }
    out
}

/// The tabs of each main-surface leaf that holds only documents (each gets a tab strip).
pub fn document_leaves(dock: &DockState<Tab>) -> Vec<Vec<Tab>> {
    dock.iter_leaves()
        .filter(|(path, leaf)| {
            path.surface.is_main() && !leaf.tabs.is_empty() && leaf.tabs.iter().all(|t| t.is_document())
        })
        .map(|(_, leaf)| leaf.tabs.clone())
        .collect()
}

/// Move a document tab to `to` within its leaf (the session tabs' drag). The shown tab stays
/// shown. Returns false when the tab is not open.
pub fn move_document(dock: &mut DockState<Tab>, tab: Tab, to: usize) -> bool {
    let Some(path) = dock.find_tab(&tab) else {
        return false;
    };
    let Ok(leaf) = dock.leaf_mut(path.node_path()) else {
        return false;
    };
    let shown = leaf.tabs.get(leaf.active.0).copied();
    let moved = leaf.tabs.remove(path.tab.0);
    let to = to.min(leaf.tabs.len());
    leaf.tabs.insert(to, moved);
    if let Some(shown) = shown
        && let Some(i) = leaf.tabs.iter().position(|t| *t == shown)
    {
        leaf.active = egui_dock::TabIndex(i);
    }
    true
}

/// Close a document tab. A shown tab hands over to its right-hand neighbour (the left one when
/// it was last), as browsers do; the last session gives way to Find a MUD, so the document area
/// never disappears; Find a MUD alone stays. Returns the tab shown in its place, if the closed
/// one was shown.
pub fn remove_document(dock: &mut DockState<Tab>, tab: Tab) -> Option<Tab> {
    let path = dock.find_tab(&tab)?;
    let directory_open = dock.find_tab(&Tab::Directory).is_some();
    let node = path.node_path();
    let (next, empty) = {
        let leaf = dock.leaf_mut(node).ok()?;
        if leaf.tabs.len() == 1 {
            if tab == Tab::Directory {
                return None;
            }
            if !directory_open {
                leaf.tabs.push(Tab::Directory);
            }
        }
        let index = path.tab.0;
        let was_shown = leaf.active.0 == index;
        let shown = leaf.tabs.get(leaf.active.0).copied();
        leaf.tabs.remove(index);
        if leaf.tabs.is_empty() {
            (None, true)
        } else if was_shown {
            let i = index.min(leaf.tabs.len() - 1);
            leaf.active = egui_dock::TabIndex(i);
            (Some(leaf.tabs[i]), false)
        } else {
            if let Some(i) = shown.and_then(|s| leaf.tabs.iter().position(|t| *t == s)) {
                leaf.active = egui_dock::TabIndex(i);
            }
            (None, false)
        }
    };
    if empty {
        // Find a MUD was open in another leaf: this one goes.
        dock.remove_leaf(node);
    } else if next.is_some() {
        dock.set_focused_node_and_surface(node);
    }
    next
}

/// The session whose tab is active in the focused leaf, if any.
pub fn focused_session(dock: &mut DockState<Tab>) -> Option<SessionId> {
    match dock.find_active_focused() {
        Some((_, Tab::Session(id))) => Some(*id),
        _ => None,
    }
}

/// Sessions whose tabs are the active tab of their leaf (visible now).
pub fn visible_sessions(dock: &DockState<Tab>) -> Vec<SessionId> {
    dock.iter_leaves()
        .filter_map(|(_, leaf)| match leaf.tabs.get(leaf.active.0) {
            Some(Tab::Session(id)) => Some(*id),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(dock: &DockState<Tab>) -> Vec<Tab> {
        dock.iter_all_tabs().map(|(_, t)| *t).collect()
    }

    #[test]
    fn the_default_layout_mirrors_the_csharp_client() {
        let dock = default_layout();
        let all = tabs(&dock);
        for t in [
            Tab::Workspace,
            Tab::SavedWorlds,
            Tab::Directory,
            Tab::Map,
            Tab::Channels,
        ] {
            assert!(all.contains(&t), "{t:?}");
        }
        assert_eq!(all.len(), 5);
        // Saved worlds is its own leaf under the Workspace in the left column.
        let workspace = dock.find_tab(&Tab::Workspace).unwrap();
        let saved = dock.find_tab(&Tab::SavedWorlds).unwrap();
        assert_ne!(workspace.node_path(), saved.node_path());
        assert_eq!(saved.node.parent(), workspace.node.parent(), "siblings in one split");
    }

    /// Panels on the left: one column at the left, Map over Saved worlds over Channels, the
    /// documents at the right, the Workspace closed. Panels on the right mirrors it. Focus
    /// unpins every tool panel; Both sides is the C# layout.
    #[test]
    fn presets_place_the_panels() {
        use crate::autohide::Edge;
        for preset in [Preset::Left, Preset::Right] {
            let (dock, auto) = preset_layout(preset);
            assert!(auto.hidden.is_empty());
            assert!(dock.find_tab(&Tab::Workspace).is_none(), "{preset:?}");
            let tree = dock.main_surface();
            let map = dock.find_tab(&Tab::Map).unwrap().node;
            let saved = dock.find_tab(&Tab::SavedWorlds).unwrap().node;
            let channels = dock.find_tab(&Tab::Channels).unwrap().node;
            let docs = dock.find_tab(&Tab::Directory).unwrap().node;
            // Left or right of the documents at the root.
            let Node::Horizontal(root) = &tree[NodeIndex::root()] else {
                panic!("{preset:?}: a side-by-side root")
            };
            let column_first = preset == Preset::Left;
            let column = if column_first {
                NodeIndex::root().left()
            } else {
                NodeIndex::root().right()
            };
            assert_eq!(
                docs,
                if column_first {
                    NodeIndex::root().right()
                } else {
                    NodeIndex::root().left()
                }
            );
            assert!((root.fraction - if column_first { COLUMN_SHARE } else { 1.0 - COLUMN_SHARE }).abs() < 1e-6);
            // Map, then Saved worlds, then Channels, top to bottom inside the column.
            assert_eq!(map, column.left());
            assert_eq!(saved, column.right().left());
            assert_eq!(channels, column.right().right());
        }
        let (dock, auto) = preset_layout(Preset::Focus);
        assert_eq!(tabs(&dock), [Tab::Directory]);
        let hidden: Vec<(Tab, Edge)> = auto.hidden.iter().map(|h| (h.tab, h.edge)).collect();
        assert_eq!(
            hidden,
            [
                (Tab::Workspace, Edge::Left),
                (Tab::SavedWorlds, Edge::Left),
                (Tab::Map, Edge::Right),
                (Tab::Channels, Edge::Right)
            ]
        );
        let (dock, auto) = preset_layout(Preset::Both);
        assert_eq!(tabs(&dock), tabs(&default_layout()));
        assert!(auto.hidden.is_empty());
    }

    #[test]
    fn sessions_join_the_document_area_and_panels_reopen() {
        let mut dock = default_layout();
        add_document(&mut dock, Tab::Session(1));
        add_document(&mut dock, Tab::Session(2));
        let dir = dock.find_tab(&Tab::Directory).unwrap();
        let s2 = dock.find_tab(&Tab::Session(2)).unwrap();
        assert_eq!(dir.node_path(), s2.node_path(), "same leaf as Find a MUD");
        // Close Map and Channels, then reopen Map: it comes back on the right.
        for t in [Tab::Map, Tab::Channels] {
            let path = dock.find_tab(&t).unwrap();
            dock.remove_tab(path);
        }
        open_panel(&mut dock, Tab::Map);
        open_panel(&mut dock, Tab::Channels);
        let map = dock.find_tab(&Tab::Map).unwrap();
        let channels = dock.find_tab(&Tab::Channels).unwrap();
        assert_eq!(map.node_path(), channels.node_path(), "Channels joins Map");
        open_panel(&mut dock, Tab::Map);
        assert_eq!(
            tabs(&dock).iter().filter(|t| **t == Tab::Map).count(),
            1,
            "focusing, not duplicating"
        );
        assert_eq!(visible_sessions(&dock), vec![2]);
    }
}
