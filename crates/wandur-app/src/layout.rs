//! Saving and restoring the workspace layout (`layout.json` in the data directory).
//!
//! The file is our own small description of the main dock tree (splits with their fractions,
//! leaves with their panels), not egui_dock's internal state: a restored layout is rebuilt through
//! egui_dock's own split calls, so any file that parses gives a valid dock, an unknown panel name
//! is dropped, and a file that does not parse is moved aside (`layout.json.bad`) and the default
//! layout is used. Session tabs are not saved (sessions are not restored on start). Panels that
//! were in floating windows come back docked in the first leaf.
//!
//! Version 2 (Saved worlds became its own panel): a version 1 file gets Saved worlds under the
//! Workspace (or at the left of the dock when the Workspace is not docked), as the default
//! layout has it; nothing else in the file changes.

use std::path::Path;
use wandur_core::l10n::{S, t, tf};

use egui_dock::{DockState, Node, NodeIndex, Split};
use serde::{Deserialize, Serialize};

use crate::autohide::{AutoHide, Edge, Hidden, Place};
use crate::workspace::{self, Tab};

pub const LAYOUT_FILE: &str = "layout.json";
pub const LAYOUT_VERSION: u32 = 2;
/// The oldest layout file this build reads (and migrates).
pub const OLDEST_LAYOUT_VERSION: u32 = 1;
/// Deeper trees than this are not real layouts.
const MAX_DEPTH: usize = 16;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavedNode {
    Leaf {
        tabs: Vec<String>,
        #[serde(default)]
        active: usize,
    },
    Split {
        /// Side by side (true) or one above the other.
        horizontal: bool,
        /// The share of the left or top child.
        fraction: f32,
        first: Box<SavedNode>,
        second: Box<SavedNode>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutFile {
    pub version: u32,
    pub root: SavedNode,
    /// Panels that were in floating windows.
    #[serde(default)]
    pub floating: Vec<String>,
    /// Unpinned (auto-hidden) panels, with where to put them back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_hide: Vec<SavedHidden>,
    /// The map's share of a session's width with Play and Map side by side, last settled on
    /// (sessions are not saved; new ones start at this share).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_share: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedHidden {
    pub panel: String,
    pub edge: Edge,
    pub size: f32,
    /// Panels that shared its leaf (`leaf`) or were on the other side of its split.
    #[serde(default)]
    pub beside: Vec<String>,
    /// `leaf`, `split` or `unknown`.
    #[serde(default)]
    pub place: String,
    #[serde(default)]
    pub first: bool,
    #[serde(default)]
    pub horizontal: bool,
    #[serde(default)]
    pub fraction: f32,
}

fn names(tabs: &[Tab]) -> Vec<String> {
    tabs.iter()
        .filter_map(|t| t.persist_name())
        .map(str::to_string)
        .collect()
}

fn tabs_of(names: &[String]) -> Vec<Tab> {
    names.iter().filter_map(|n| Tab::from_name(n)).collect()
}

fn save_hidden(h: &Hidden) -> Option<SavedHidden> {
    let mut saved = SavedHidden {
        panel: h.tab.persist_name()?.to_string(),
        edge: h.edge,
        size: h.size,
        beside: Vec::new(),
        place: "unknown".into(),
        first: false,
        horizontal: false,
        fraction: 0.5,
    };
    match &h.place {
        Place::Leaf(tabs) => {
            saved.place = "leaf".into();
            saved.beside = names(tabs);
        }
        Place::Split {
            others,
            first,
            horizontal,
            fraction,
        } => {
            saved.place = "split".into();
            saved.beside = names(others);
            saved.first = *first;
            saved.horizontal = *horizontal;
            saved.fraction = *fraction;
        }
        Place::Unknown => {}
    }
    Some(saved)
}

fn load_hidden(s: &SavedHidden) -> Option<Hidden> {
    let tab = Tab::from_name(&s.panel).filter(|t| crate::autohide::can_hide(*t))?;
    let place = match s.place.as_str() {
        "leaf" => Place::Leaf(tabs_of(&s.beside)),
        "split" => Place::Split {
            others: tabs_of(&s.beside),
            first: s.first,
            horizontal: s.horizontal,
            fraction: if s.fraction.is_finite() {
                s.fraction.clamp(0.05, 0.95)
            } else {
                0.5
            },
        },
        _ => Place::Unknown,
    };
    Some(Hidden {
        tab,
        edge: s.edge,
        size: if s.size.is_finite() {
            s.size.clamp(160.0, 900.0)
        } else {
            300.0
        },
        place,
    })
}

/// Describe the dock and the unpinned panels (sessions left out). `None` if no panel is open.
pub fn capture_all(dock: &DockState<Tab>, auto: &AutoHide) -> Option<LayoutFile> {
    let mut file = capture(dock)?;
    file.auto_hide = auto.hidden.iter().filter_map(save_hidden).collect();
    Some(file)
}

/// Describe the dock (sessions left out). `None` if no panel is open.
pub fn capture(dock: &DockState<Tab>) -> Option<LayoutFile> {
    let tree = dock.main_surface();
    let root = capture_node(tree, NodeIndex::root(), 0);
    let mut floating = Vec::new();
    for (path, tab) in dock.iter_all_tabs() {
        if path.surface != egui_dock::SurfaceIndex::main()
            && let Some(name) = tab.persist_name()
        {
            floating.push(name.to_string());
        }
    }
    let root = match root {
        Some(root) => root,
        None if !floating.is_empty() => SavedNode::Leaf {
            tabs: std::mem::take(&mut floating),
            active: 0,
        },
        None => return None,
    };
    Some(LayoutFile {
        version: LAYOUT_VERSION,
        root,
        floating,
        auto_hide: Vec::new(),
        split_share: None,
    })
}

fn capture_node(tree: &egui_dock::Tree<Tab>, index: NodeIndex, depth: usize) -> Option<SavedNode> {
    if index.0 >= tree.len() || depth > MAX_DEPTH {
        return None;
    }
    match &tree[index] {
        Node::Empty => None,
        Node::Leaf(leaf) => {
            let active_tab = leaf.tabs.get(leaf.active.0).copied();
            let tabs: Vec<Tab> = leaf
                .tabs
                .iter()
                .copied()
                .filter(|t| t.persist_name().is_some())
                .collect();
            if tabs.is_empty() {
                return None;
            }
            let active = active_tab.and_then(|a| tabs.iter().position(|t| *t == a)).unwrap_or(0);
            Some(SavedNode::Leaf {
                tabs: tabs
                    .iter()
                    .filter_map(|t| t.persist_name())
                    .map(str::to_string)
                    .collect(),
                active,
            })
        }
        Node::Horizontal(split) | Node::Vertical(split) => {
            let horizontal = matches!(tree[index], Node::Horizontal(_));
            let first = capture_node(tree, index.left(), depth + 1);
            let second = capture_node(tree, index.right(), depth + 1);
            match (first, second) {
                (Some(a), Some(b)) => Some(SavedNode::Split {
                    horizontal,
                    fraction: split.fraction,
                    first: Box::new(a),
                    second: Box::new(b),
                }),
                (a, b) => a.or(b),
            }
        }
    }
}

/// Keep known panels once each, drop empty leaves and collapse their splits.
fn clean(node: SavedNode, seen: &mut Vec<Tab>, depth: usize) -> Option<SavedNode> {
    if depth > MAX_DEPTH {
        return None;
    }
    match node {
        SavedNode::Leaf { tabs, active } => {
            let active_name = tabs.get(active).cloned();
            let mut kept = Vec::new();
            for name in tabs {
                if let Some(tab) = Tab::from_name(&name)
                    && !seen.contains(&tab)
                {
                    seen.push(tab);
                    kept.push(name);
                }
            }
            if kept.is_empty() {
                return None;
            }
            let active = active_name.and_then(|a| kept.iter().position(|n| *n == a)).unwrap_or(0);
            Some(SavedNode::Leaf { tabs: kept, active })
        }
        SavedNode::Split {
            horizontal,
            fraction,
            first,
            second,
        } => {
            let a = clean(*first, seen, depth + 1);
            let b = clean(*second, seen, depth + 1);
            match (a, b) {
                (Some(a), Some(b)) => Some(SavedNode::Split {
                    horizontal,
                    fraction: if fraction.is_finite() {
                        fraction.clamp(0.05, 0.95)
                    } else {
                        0.5
                    },
                    first: Box::new(a),
                    second: Box::new(b),
                }),
                (a, b) => a.or(b),
            }
        }
    }
}

/// Rebuild a dock and the unpinned panels from a saved layout. `None` when nothing usable is
/// left. A panel both docked and unpinned stays docked.
pub fn restore_all(file: LayoutFile) -> Option<(DockState<Tab>, AutoHide)> {
    let saved = file.auto_hide.clone();
    let version = file.version;
    let mut dock = restore(file)?;
    let mut auto = AutoHide::default();
    for s in &saved {
        if let Some(h) = load_hidden(s)
            && dock.find_tab(&h.tab).is_none()
            && !auto.is_hidden(h.tab)
        {
            auto.hidden.push(h);
        }
    }
    if version < 2 {
        migrate_saved_worlds(&mut dock, &auto);
    }
    Some((dock, auto))
}

/// Version 1 to 2: the saved worlds were part of the Workspace panel; give them their own panel
/// where the default layout has it (under the Workspace).
fn migrate_saved_worlds(dock: &mut DockState<Tab>, auto: &AutoHide) {
    if dock.find_tab(&Tab::SavedWorlds).is_some() || auto.is_hidden(Tab::SavedWorlds) {
        return;
    }
    let focused = dock.focused_leaf();
    workspace::open_panel(dock, Tab::SavedWorlds);
    // The migration does not move the focus.
    if let Some(path) = focused {
        dock.set_focused_node_and_surface(path);
    }
}

/// Rebuild a dock from a saved layout. `None` when nothing usable is left.
pub fn restore(file: LayoutFile) -> Option<DockState<Tab>> {
    if !(OLDEST_LAYOUT_VERSION..=LAYOUT_VERSION).contains(&file.version) {
        return None;
    }
    let mut seen = Vec::new();
    let root = clean(file.root, &mut seen, 0)?;
    let mut dock = DockState::new(vec![Tab::Workspace]);
    build(dock.main_surface_mut(), NodeIndex::root(), root);
    // Panels that floated come back at the end of the first leaf, without taking its focus.
    if let Some((_, leaf)) = dock.iter_leaves_mut().next() {
        let active = leaf.active;
        for name in file.floating {
            if let Some(tab) = Tab::from_name(&name)
                && !seen.contains(&tab)
            {
                seen.push(tab);
                leaf.tabs.push(tab);
            }
        }
        leaf.active = active;
    }
    Some(dock)
}

/// Fill the leaf at `at` with `node`, splitting it as the node says.
fn build(tree: &mut egui_dock::Tree<Tab>, at: NodeIndex, node: SavedNode) {
    match node {
        SavedNode::Leaf { tabs, active } => {
            let tabs: Vec<Tab> = tabs.iter().filter_map(|n| Tab::from_name(n)).collect();
            let mut leaf = Node::leaf_with(tabs);
            if let Some(l) = leaf.get_leaf_mut() {
                let _ = l.set_active_tab(active.min(l.len().saturating_sub(1)));
            }
            tree[at] = leaf;
        }
        SavedNode::Split {
            horizontal,
            fraction,
            first,
            second,
        } => {
            let split = if horizontal { Split::Right } else { Split::Below };
            let [a, b] = tree.split(at, split, fraction, Node::leaf(Tab::Workspace));
            build(tree, a, *first);
            build(tree, b, *second);
        }
    }
}

pub fn to_json(dock: &DockState<Tab>, auto: &AutoHide) -> Option<String> {
    to_json_with_split(dock, auto, None)
}

/// [`to_json`] with the side-by-side share remembered for new sessions.
pub fn to_json_with_split(dock: &DockState<Tab>, auto: &AutoHide, split_share: Option<f32>) -> Option<String> {
    let mut file = capture_all(dock, auto)?;
    file.split_share = split_share.filter(|s| s.is_finite());
    serde_json::to_string_pretty(&file).ok()
}

/// The side-by-side share saved in a layout file's text, if it has a usable one.
pub fn split_share_of(text: &str) -> Option<f32> {
    #[derive(Deserialize)]
    struct Split {
        #[serde(default)]
        split_share: Option<f32>,
    }
    let share = serde_json::from_str::<Split>(text).ok()?.split_share?;
    crate::terminal_view::SPLIT_SHARES.contains(&share).then_some(share)
}

/// The side-by-side share saved in `dir`'s layout file.
pub fn load_split_share(dir: &Path) -> Option<f32> {
    split_share_of(&std::fs::read_to_string(dir.join(LAYOUT_FILE)).ok()?)
}

/// A dock, its unpinned panels, and a note when the saved file could not be used.
pub type Loaded = (DockState<Tab>, AutoHide, Option<String>);

/// Read the layout from `dir`; with no file (a new install), or one that cannot be used, the
/// `fresh` preset.
pub fn load(dir: &Path, fresh: workspace::Preset) -> Loaded {
    let path = dir.join(LAYOUT_FILE);
    let default = || workspace::preset_layout(fresh);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let (d, a) = default();
            return (d, a, None);
        }
        Err(e) => {
            let (d, a) = default();
            return (d, a, Some(tf(S::LayoutUnreadable, &[&e])));
        }
    };
    match parse(&text) {
        Ok((dock, auto)) => (dock, auto, None),
        Err(reason) => {
            let _ = std::fs::rename(&path, dir.join(format!("{LAYOUT_FILE}.bad")));
            let (d, a) = default();
            (d, a, Some(tf(S::LayoutNotUsable, &[&reason])))
        }
    }
}

/// Parse a layout file's text into a dock and its unpinned panels.
pub fn parse(text: &str) -> Result<(DockState<Tab>, AutoHide), String> {
    let file: LayoutFile = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if !(OLDEST_LAYOUT_VERSION..=LAYOUT_VERSION).contains(&file.version) {
        return Err(tf(S::LayoutVersionUnsupported, &[&file.version]));
    }
    restore_all(file).ok_or_else(|| t(S::LayoutNoKnownPanels).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(dock: &DockState<Tab>) -> Vec<Tab> {
        let mut t: Vec<Tab> = dock.iter_all_tabs().map(|(_, t)| *t).collect();
        t.sort_by_key(|t| format!("{t:?}"));
        t
    }

    #[test]
    fn round_trip_keeps_panels_splits_and_fractions_but_not_sessions() {
        let mut dock = workspace::default_layout();
        workspace::add_document(&mut dock, Tab::Session(7));
        workspace::open_panel(&mut dock, Tab::Channels);
        let json = to_json(&dock, &AutoHide::default()).unwrap();
        assert!(!json.contains("session"));
        let (restored, _) = parse(&json).unwrap();
        let mut expected = tabs(&dock);
        expected.retain(|t| !matches!(t, Tab::Session(_)));
        assert_eq!(tabs(&restored), expected);
        // Saving the restored layout gives the same file: splits and fractions survived.
        assert_eq!(to_json(&restored, &AutoHide::default()).unwrap(), json);
        let file: LayoutFile = serde_json::from_str(&json).unwrap();
        match file.root {
            SavedNode::Split {
                horizontal, fraction, ..
            } => {
                assert!(horizontal);
                assert!((fraction - 0.18).abs() < 0.01, "{fraction}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unknown_panels_are_dropped_and_duplicates_ignored() {
        let text = r#"{"version":2,"root":{"kind":"split","horizontal":true,"fraction":7.0,
            "first":{"kind":"leaf","tabs":["scripts","agent"]},
            "second":{"kind":"split","horizontal":false,"fraction":0.5,
                "first":{"kind":"leaf","tabs":["map","directory","map"],"active":1},
                "second":{"kind":"leaf","tabs":["future-panel"]}}},
            "floating":["channels","nonsense"]}"#;
        let (dock, _) = parse(text).unwrap();
        assert_eq!(tabs(&dock), [Tab::Channels, Tab::Directory, Tab::Map]);
        let file = capture(&dock).unwrap();
        match file.root {
            SavedNode::Leaf { tabs, active } => {
                assert_eq!(tabs, ["map", "directory", "channels"]);
                assert_eq!(active, 1);
            }
            other => panic!("splits with nothing left collapse: {other:?}"),
        }
    }

    #[test]
    fn corrupt_old_or_empty_files_fall_back_to_the_default() {
        let dir = std::env::temp_dir().join(format!("wandur-layout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Missing: the default, no note.
        let (dock, _, note) = load(&dir, workspace::Preset::Both);
        assert!(note.is_none());
        assert_eq!(tabs(&dock), tabs(&workspace::default_layout()));
        for bad in [
            "{ not json",
            r#"{"version":99,"root":{"kind":"leaf","tabs":["map"]}}"#,
            r#"{"version":1,"root":{"kind":"leaf","tabs":["unknown"]}}"#,
            r#"{"version":1,"root":{"kind":"tree"}}"#,
        ] {
            std::fs::write(dir.join(LAYOUT_FILE), bad).unwrap();
            let (dock, _, note) = load(&dir, workspace::Preset::Both);
            assert!(note.is_some(), "{bad}");
            assert_eq!(tabs(&dock), tabs(&workspace::default_layout()));
            assert!(dir.join("layout.json.bad").exists());
            assert!(!dir.join(LAYOUT_FILE).exists());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A new install (no layout.json) starts with Panels on the left; a person's saved layout is
    /// kept whatever the default.
    #[test]
    fn a_new_install_gets_panels_on_the_left_and_a_saved_layout_is_kept() {
        let dir = std::env::temp_dir().join(format!("wandur-layout-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (dock, auto, note) = load(&dir, workspace::NEW_INSTALL);
        assert!(note.is_none());
        let (left, _) = workspace::preset_layout(workspace::Preset::Left);
        assert_eq!(to_json(&dock, &auto), to_json(&left, &AutoHide::default()));
        assert!(dock.find_tab(&Tab::Workspace).is_none(), "the Workspace starts closed");
        // Someone's own layout (here the C# one) is read as saved.
        let both = workspace::default_layout();
        std::fs::write(dir.join(LAYOUT_FILE), to_json(&both, &AutoHide::default()).unwrap()).unwrap();
        let (dock, _, _) = load(&dir, workspace::NEW_INSTALL);
        assert_eq!(tabs(&dock), tabs(&both));
        assert!(dock.find_tab(&Tab::Workspace).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every preset survives being saved and read back: the same panels, places and unpinned
    /// panels.
    #[test]
    fn presets_round_trip_through_the_saved_layout() {
        for preset in workspace::Preset::ALL {
            let (dock, auto) = workspace::preset_layout(preset);
            let json = to_json(&dock, &auto).unwrap();
            let (back, back_auto) = parse(&json).unwrap();
            assert_eq!(to_json(&back, &back_auto).unwrap(), json, "{preset:?}");
        }
    }

    #[test]
    fn a_version_1_layout_gets_saved_worlds_under_the_workspace() {
        // The default layout as version 1 saved it: Workspace alone on the left.
        let v1 = r#"{"version":1,"root":{"kind":"split","horizontal":true,"fraction":0.2,
            "first":{"kind":"leaf","tabs":["workspace"]},
            "second":{"kind":"split","horizontal":true,"fraction":0.7,
                "first":{"kind":"leaf","tabs":["directory"]},
                "second":{"kind":"split","horizontal":false,"fraction":0.6,
                    "first":{"kind":"leaf","tabs":["map"]},
                    "second":{"kind":"leaf","tabs":["channels"]}}}}}"#;
        let (dock, auto) = parse(v1).unwrap();
        let workspace = dock.find_tab(&Tab::Workspace).unwrap();
        let saved = dock.find_tab(&Tab::SavedWorlds).unwrap();
        assert_ne!(workspace.node_path(), saved.node_path(), "its own panel");
        assert_eq!(saved.node.parent(), workspace.node.parent(), "beside the Workspace");
        let file = capture_all(&dock, &auto).unwrap();
        assert_eq!(file.version, LAYOUT_VERSION);
        match &file.root {
            SavedNode::Split {
                horizontal: true,
                fraction,
                first,
                ..
            } => {
                assert!((fraction - 0.2).abs() < 0.001, "the column keeps its width: {fraction}");
                match first.as_ref() {
                    SavedNode::Split {
                        horizontal: false,
                        fraction,
                        first,
                        second,
                    } => {
                        assert!((fraction - workspace::WORKSPACE_SHARE).abs() < 0.001);
                        assert_eq!(
                            **first,
                            SavedNode::Leaf {
                                tabs: vec!["workspace".into()],
                                active: 0
                            }
                        );
                        assert_eq!(
                            **second,
                            SavedNode::Leaf {
                                tabs: vec!["saved_worlds".into()],
                                active: 0
                            }
                        );
                    }
                    other => panic!("{other:?}"),
                }
            }
            other => panic!("{other:?}"),
        }
        // Saved again, it is a version 2 file that reads back unchanged (no second migration).
        let json = to_json(&dock, &auto).unwrap();
        let (again, _) = parse(&json).unwrap();
        assert_eq!(to_json(&again, &AutoHide::default()).unwrap(), json);

        // A version 1 file with the Workspace unpinned: Saved worlds docks at the left.
        let v1 = r#"{"version":1,"root":{"kind":"leaf","tabs":["directory"]},
            "auto_hide":[{"panel":"workspace","edge":"left","size":240}]}"#;
        let (dock, auto) = parse(v1).unwrap();
        assert!(auto.is_hidden(Tab::Workspace));
        assert!(dock.find_tab(&Tab::SavedWorlds).is_some());
        // A version 2 file without Saved worlds was closed on purpose: it stays closed.
        let v2 = r#"{"version":2,"root":{"kind":"leaf","tabs":["workspace","directory"]}}"#;
        let (dock, _) = parse(v2).unwrap();
        assert!(dock.find_tab(&Tab::SavedWorlds).is_none());
    }

    #[test]
    fn floating_panels_come_back_docked() {
        let mut dock = workspace::default_layout();
        let path = dock.find_tab(&Tab::Map).unwrap();
        let map = dock.remove_tab(path).unwrap();
        dock.add_window(vec![map]);
        let file = capture(&dock).unwrap();
        assert_eq!(file.floating, ["map"]);
        let restored = restore(file).unwrap();
        assert!(restored.find_tab(&Tab::Map).is_some());
        assert_eq!(restored.iter_surfaces().count(), 1);
    }

    /// The side-by-side share rides along in `layout.json` (an older file has none; a share out
    /// of range is ignored), and the dock reads the same with it.
    #[test]
    fn the_side_by_side_share_is_saved_with_the_layout() {
        let dock = workspace::default_layout();
        let plain = to_json(&dock, &AutoHide::default()).unwrap();
        assert!(!plain.contains("split_share"));
        assert_eq!(split_share_of(&plain), None);
        let json = to_json_with_split(&dock, &AutoHide::default(), Some(0.62)).unwrap();
        assert_eq!(split_share_of(&json), Some(0.62));
        let (restored, auto) = parse(&json).unwrap();
        assert_eq!(to_json(&restored, &auto).unwrap(), plain);
        let wild = json.replace("0.62", "7.5");
        assert_eq!(split_share_of(&wild), None);
    }

    #[test]
    fn a_layout_with_every_panel_closed_has_nothing_to_save() {
        let mut dock = workspace::default_layout();
        dock.retain_tabs(|_| false);
        assert!(capture(&dock).is_none());
    }

    #[test]
    fn unpinned_panels_persist_with_their_place() {
        let mut dock = workspace::default_layout();
        for (_, leaf) in dock.iter_leaves_mut() {
            leaf.rect = match leaf.tabs[0] {
                Tab::Workspace => egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(180.0, 700.0)),
                Tab::Map => egui::Rect::from_min_max(egui::pos2(770.0, 0.0), egui::pos2(1000.0, 400.0)),
                Tab::Channels => egui::Rect::from_min_max(egui::pos2(770.0, 400.0), egui::pos2(1000.0, 700.0)),
                _ => egui::Rect::from_min_max(egui::pos2(180.0, 0.0), egui::pos2(770.0, 700.0)),
            };
        }
        let area = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1000.0, 700.0));
        let pinned = to_json(&dock, &AutoHide::default()).unwrap();
        let mut auto = AutoHide::default();
        assert!(auto.unpin(&mut dock, Tab::Channels, area));
        assert!(auto.unpin(&mut dock, Tab::Workspace, area));
        let json = to_json(&dock, &auto).unwrap();
        assert!(json.contains("auto_hide"), "{json}");
        let (mut restored, mut restored_auto) = parse(&json).unwrap();
        assert_eq!(restored_auto.hidden, auto.hidden, "edges, sizes and places survive");
        assert!(
            restored.find_tab(&Tab::Channels).is_none(),
            "still unpinned after a restart"
        );
        // Pinning after the restart puts both back as they were.
        restored_auto.pin(&mut restored, Tab::Channels);
        restored_auto.pin(&mut restored, Tab::Workspace);
        assert_eq!(to_json(&restored, &restored_auto).unwrap(), pinned);
        // A file listing a panel as both docked and unpinned keeps it docked; documents cannot be
        // unpinned.
        let text = r#"{"version":1,"root":{"kind":"leaf","tabs":["directory","map"]},
            "auto_hide":[{"panel":"map","edge":"right","size":300},
                         {"panel":"directory","edge":"left","size":300},
                         {"panel":"channels","edge":"bottom","size":5000,"place":"split","beside":["map"],"fraction":9}]}"#;
        let (dock, auto) = parse(text).unwrap();
        assert!(dock.find_tab(&Tab::Map).is_some() && dock.find_tab(&Tab::Directory).is_some());
        assert_eq!(auto.hidden.len(), 1);
        assert_eq!(auto.hidden[0].tab, Tab::Channels);
        assert_eq!(auto.hidden[0].size, 900.0, "clamped");
    }
}
