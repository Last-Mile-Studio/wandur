//! Dragging a panel the way the C# client does (Visual Studio's way, C# `ui/dock-drop-preview`):
//! themed guide tiles, a translucent accent rectangle of exactly where the panel will land (30%
//! fill, a stronger edge), window-edge guides that dock a panel along a whole edge at a quarter
//! of the window, and a drop that lands at the size its preview showed.
//!
//! egui_dock decides every drop on a panel and draws its guide glyphs; it has no hook for a
//! preview, so this module works out the same guide geometry it uses
//! (`DragDropState::resolve_icon_based`: a cross of five tiles centred on the hovered panel)
//! from the pointer, paints the tiles' faces and the preview under egui_dock's glyphs, and after
//! the drop corrects the split so the new panel has the previewed extent in points (taking the
//! dragged panel out of its old place can widen the target first). The window-edge guides are
//! this module's own: egui_dock does nothing with a panel released there (panels never float,
//! `TabViewer::allowed_in_windows`), and the app then docks it along that edge of the main dock.
//! A panel released away from every guide stays where it was.

use egui::{Color32, CornerRadius, Id, LayerId, Order, Pos2, Rect, Stroke, StrokeKind, Vec2, vec2};
use egui_dock::{DockState, Node, NodeIndex, NodePath, Split, SurfaceIndex};

use crate::theme::{Theme, mix};
use crate::workspace::Tab;

/// The guide tiles' side and the gap between them (the C# tiles: 34 points, 4 apart).
pub const TILE: f32 = 34.0;
pub const TILE_GAP: f32 = 4.0;
/// How far around a tile's glyph the pointer still counts as on it.
pub const TILE_SLOP: f32 = 2.0;
/// The share of the window a panel docked at a window edge takes (C#
/// `DockSettings.GlobalDockingProportion`).
pub const EDGE_SHARE: f32 = 0.25;
/// The preview's fill and edge opacity (the C# `DockDropPalette`).
pub const FILL_ALPHA: f32 = 0.30;
pub const EDGE_ALPHA: f32 = 0.50;

/// The window edges a panel can be docked along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowEdge {
    Top,
    Bottom,
    Left,
    Right,
}

impl WindowEdge {
    pub const ALL: [WindowEdge; 4] = [WindowEdge::Top, WindowEdge::Bottom, WindowEdge::Left, WindowEdge::Right];

    fn split(self) -> Split {
        match self {
            WindowEdge::Top => Split::Above,
            WindowEdge::Bottom => Split::Below,
            WindowEdge::Left => Split::Left,
            WindowEdge::Right => Split::Right,
        }
    }
}

/// Where the dragged panel would land if released now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// On a panel's guide: beside it (`Some` split) or in it as a tab (`None`).
    Leaf {
        path: NodePath,
        rect: Rect,
        split: Option<Split>,
    },
    /// On a window-edge guide.
    Edge(WindowEdge),
}

/// Egui_dock's own guide geometry for a hovered panel `rect`: the centre tile, then below,
/// right, above, left (its order, which decides overlaps).
pub fn compass(rect: Rect) -> Vec<(Option<Split>, Rect)> {
    let inner = rect.shrink(TILE_GAP);
    let side = ((inner.width() - TILE_GAP * 2.0) / 3.0)
        .min((inner.height() - TILE_GAP * 2.0) / 3.0)
        .min(TILE);
    let center = inner.center();
    let mut tiles = vec![(None, Rect::from_center_size(center, egui::Vec2::splat(side)))];
    let step = side + TILE_GAP;
    for (split, offset) in [
        (Split::Below, vec2(0.0, step)),
        (Split::Right, vec2(step, 0.0)),
        (Split::Above, vec2(0.0, -step)),
        (Split::Left, vec2(-step, 0.0)),
    ] {
        tiles.push((
            Some(split),
            Rect::from_center_size(center + offset, egui::Vec2::splat(side)),
        ));
    }
    tiles
}

/// Whether the pointer is on a guide tile (egui_dock's test: the glyph, 10% in, plus the slop).
fn on_tile(tile: Rect, pointer: Pos2) -> bool {
    tile.shrink(tile.width() * 0.1).expand(TILE_SLOP).contains(pointer)
}

/// The window-edge guides inside the dock `area`: top and bottom centred, left and right in the
/// middle (the C# `GlobalDockTarget`).
pub fn edge_tiles(area: Rect) -> [(WindowEdge, Rect); 4] {
    let size = egui::Vec2::splat(TILE);
    let inset = TILE / 2.0 + 6.0;
    [
        (
            WindowEdge::Top,
            Rect::from_center_size(egui::pos2(area.center().x, area.top() + inset), size),
        ),
        (
            WindowEdge::Bottom,
            Rect::from_center_size(egui::pos2(area.center().x, area.bottom() - inset), size),
        ),
        (
            WindowEdge::Left,
            Rect::from_center_size(egui::pos2(area.left() + inset, area.center().y), size),
        ),
        (
            WindowEdge::Right,
            Rect::from_center_size(egui::pos2(area.right() - inset, area.center().y), size),
        ),
    ]
}

/// The leaf of the main dock under the pointer that egui_dock would offer guides for, with its
/// tab bar's height: the pointer on a tab bar inserts a tab there instead (no guides).
fn hovered_leaf(dock: &DockState<Tab>, pointer: Pos2, bar: f32) -> Option<(NodePath, Rect)> {
    dock.iter_leaves()
        .filter(|(path, leaf)| path.surface == SurfaceIndex::main() && leaf.rect.contains(pointer))
        .find(|(_, leaf)| {
            let bar = if leaf.tab_bar_hidden { 0.0 } else { bar };
            pointer.y >= leaf.rect.top() + bar
        })
        .map(|(path, leaf)| (path, leaf.rect))
}

/// Whether the pointer is on a leaf's tab bar, where egui_dock adds the dragged panel as a tab.
pub fn over_tab_bar(dock: &DockState<Tab>, pointer: Pos2, bar: f32) -> bool {
    dock.iter_leaves().any(|(path, leaf)| {
        path.surface == SurfaceIndex::main()
            && !leaf.tab_bar_hidden
            && leaf.rect.contains(pointer)
            && pointer.y < leaf.rect.top() + bar
    })
}

/// Where a panel dragged from `source` lands if released at `pointer`, or `None` (it stays
/// where it was, or joins a tab bar). `bar` is the tab bar height.
pub fn target(dock: &DockState<Tab>, source: NodePath, area: Rect, pointer: Pos2, bar: f32) -> Option<Target> {
    if let Some((path, rect)) = hovered_leaf(dock, pointer, bar) {
        // egui_dock offers only the centre guide on the panel being dragged when it is alone.
        let deserted = path == source && dock.leaf(path).is_ok_and(|l| l.tabs.len() == 1);
        for (split, tile) in compass(rect) {
            if split.is_some() && deserted {
                continue;
            }
            if on_tile(tile, pointer) {
                return Some(Target::Leaf { path, rect, split });
            }
        }
    }
    edge_tiles(area)
        .into_iter()
        .find(|(_, tile)| tile.contains(pointer))
        .map(|(edge, _)| Target::Edge(edge))
}

/// The rectangle the preview shows for a target: half of the panel for a split, all of it for a
/// tab, a quarter of the dock along an edge.
pub fn preview(target: Target, area: Rect) -> Rect {
    match target {
        Target::Leaf { rect, split, .. } => split_part(rect, split, 0.5),
        Target::Edge(edge) => split_part(area, Some(edge.split()), EDGE_SHARE),
    }
}

fn split_part(rect: Rect, split: Option<Split>, share: f32) -> Rect {
    let (w, h) = (rect.width() * share, rect.height() * share);
    match split {
        None => rect,
        Some(Split::Left) => Rect::from_min_size(rect.min, vec2(w, rect.height())),
        Some(Split::Right) => Rect::from_min_max(egui::pos2(rect.right() - w, rect.top()), rect.max),
        Some(Split::Above) => Rect::from_min_size(rect.min, vec2(rect.width(), h)),
        Some(Split::Below) => Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - h), rect.max),
    }
}

/// The extent along the split's axis the dropped panel should get, in points.
fn extent(rect: Rect, split: Split) -> f32 {
    if split.is_left_right() {
        rect.width()
    } else {
        rect.height()
    }
}

/// egui_dock's overlay in the theme: its glyphs in the accent over our tiles, the lit tile tinted,
/// no highlight of the whole panel, tiles at the C# size.
pub fn style(style: &mut egui_dock::Style, theme: &Theme) {
    let o = &mut style.overlay;
    o.max_button_size = TILE;
    o.button_spacing = TILE_GAP;
    o.button_color = theme.accent;
    o.button_border_stroke = Stroke::NONE;
    o.selection_color = theme.accent.gamma_multiply(FILL_ALPHA);
    // egui_dock's own outline of a floating drop is replaced by ours (`paint`), which hides on
    // a guide as the C# drag preview does.
    o.selection_stroke_width = 0.0;
    o.hovered_leaf_highlight.color = Color32::TRANSPARENT;
    o.hovered_leaf_highlight.stroke = Stroke::NONE;
    o.feel.interact_expansion = TILE_SLOP;
}

/// A panel drop to finish after egui_dock moved it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pending {
    /// Give the panel this extent (points) along the split's axis.
    Size { tab: Tab, split: Split, extent: f32 },
    /// Dock the dragged panel along this edge.
    Edge { tab: Tab, edge: WindowEdge },
}

/// The drag in progress, followed frame by frame.
#[derive(Clone, Debug, Default)]
pub struct DropState {
    /// The panel being dragged and where it would land now.
    pub dragging: Option<(Tab, Option<Target>)>,
    pub pending: Option<Pending>,
}

/// The id of egui_dock's tab number `index` in the leaf at `path` (its `DockArea` id, then
/// surface, node and tab index). Dragging that id drags the tab.
pub fn tab_id(path: NodePath, index: usize) -> Id {
    Id::new("egui_dock::DockArea")
        .with((path.surface, "surface"))
        .with((path.node, "node"))
        .with((index, "tab"))
}

/// The tab egui_dock is dragging now.
pub fn dragged(ctx: &egui::Context, dock: &DockState<Tab>) -> Option<(Tab, NodePath)> {
    let id = ctx.dragged_id()?;
    dock.iter_leaves().find_map(|(path, leaf)| {
        leaf.tabs
            .iter()
            .enumerate()
            .find_map(|(index, tab)| (tab_id(path, index) == id).then_some((*tab, path)))
    })
}

impl DropState {
    /// Before the dock is shown: follow the drag (returns what to paint), and note the drop
    /// when the button is released over a guide.
    pub fn track(&mut self, ctx: &egui::Context, dock: &DockState<Tab>, area: Rect, bar: f32) -> Option<Target> {
        let released = ctx.input(|i| i.pointer.any_released());
        let pointer = ctx.input(|i| i.pointer.interact_pos().or(i.pointer.latest_pos()));
        let now = dragged(ctx, dock).filter(|_| ctx.input(|i| i.pointer.is_decidedly_dragging()));
        let current = now.and_then(|(tab, source)| {
            let pointer = pointer?;
            Some((tab, target(dock, source, area, pointer, bar)))
        });
        if released && let Some((tab, Some(target))) = self.dragging.or(current) {
            self.pending = Some(match target {
                Target::Leaf { split: Some(split), .. } => Pending::Size {
                    tab,
                    split,
                    extent: extent(preview(target, area), split),
                },
                Target::Leaf { split: None, .. } => {
                    self.dragging = None;
                    return None;
                }
                Target::Edge(edge) => Pending::Edge { tab, edge },
            });
            self.dragging = None;
            return None;
        }
        self.dragging = current;
        current.and_then(|(_, t)| t)
    }

    /// Finish a drop egui_dock has applied: dock an edge drop along its edge, give a split drop
    /// its previewed size once the new split has been laid out. Returns true when the layout
    /// changed.
    pub fn finish(&mut self, dock: &mut DockState<Tab>) -> bool {
        let Some(pending) = self.pending else { return false };
        match pending {
            Pending::Edge { tab, edge } => {
                self.pending = None;
                let Some(path) = dock.find_tab(&tab) else { return false };
                let Some(tab) = dock.remove_tab(path) else { return false };
                if path.surface != SurfaceIndex::main() && dock.get_surface(path.surface).is_some_and(|s| s.is_empty())
                {
                    dock.remove_surface(path.surface);
                }
                let tree = dock.main_surface_mut();
                if tree.root_node().is_none_or(|n| n.is_empty()) {
                    dock.push_to_first_leaf(tab);
                    return true;
                }
                let share = match edge {
                    WindowEdge::Top | WindowEdge::Left => EDGE_SHARE,
                    WindowEdge::Bottom | WindowEdge::Right => 1.0 - EDGE_SHARE,
                };
                tree.split(NodeIndex::root(), edge.split(), share, Node::leaf(tab));
                true
            }
            Pending::Size { tab, split, extent } => {
                let Some(path) = dock.find_tab(&tab) else {
                    self.pending = None;
                    return false;
                };
                let Some(parent) = path.node.parent() else {
                    self.pending = None;
                    return false;
                };
                let tree = &mut dock[path.surface];
                let first = path.node == parent.left();
                let (Node::Horizontal(node) | Node::Vertical(node)) = &mut tree[parent] else {
                    self.pending = None;
                    return false;
                };
                // Wait for the new split's first layout.
                if !node.rect.is_positive() {
                    return false;
                }
                let along = if split.is_left_right() {
                    node.rect.width()
                } else {
                    node.rect.height()
                };
                let share = (extent / along).clamp(0.05, 0.95);
                node.fraction = if first { share } else { 1.0 - share };
                self.pending = None;
                true
            }
        }
    }
}

/// Paint the guide tiles' faces and the preview, under egui_dock's guide glyphs (a middle layer
/// over the panels, below its overlay).
pub fn paint(ctx: &egui::Context, theme: &Theme, dock: &DockState<Tab>, area: Rect, bar: f32, target: Option<Target>) {
    let painter = ctx.layer_painter(LayerId::new(Order::Middle, Id::new("dock-drop-preview")));
    let accent = theme.accent;
    if let Some(target) = target {
        let rect = preview(target, area).shrink(1.0);
        painter.rect_filled(rect, CornerRadius::same(2), accent.gamma_multiply(FILL_ALPHA));
        painter.rect_stroke(
            rect,
            CornerRadius::same(2),
            Stroke::new(2.0, accent.gamma_multiply(EDGE_ALPHA)),
            StrokeKind::Inside,
        );
    }
    let face = mix(theme.panel, theme.text, 0.07);
    let edge = mix(theme.panel, theme.text, 0.30);
    let tile = |rect: Rect, lit: bool| {
        painter.rect_filled(
            rect.translate(vec2(0.0, 1.5)),
            CornerRadius::same(6),
            Color32::from_black_alpha(28),
        );
        let fill = if lit { mix(face, accent, 0.25) } else { face };
        painter.rect_filled(rect, CornerRadius::same(6), fill);
        painter.rect_stroke(
            rect,
            CornerRadius::same(6),
            Stroke::new(1.0, if lit { accent } else { edge }),
            StrokeKind::Inside,
        );
    };
    let pointer = ctx.input(|i| i.pointer.latest_pos());
    if let Some(pointer) = pointer
        && let Some((_, rect)) = hovered_leaf(dock, pointer, bar)
    {
        for (split, t) in compass(rect) {
            let lit = matches!(target, Some(Target::Leaf { split: s, rect: r, .. }) if s == split && r == rect);
            tile(t.expand(3.0), lit);
        }
    }
    // The window-edge guides, with their own glyph: the window outline and the docking zone.
    for (edge_kind, t) in edge_tiles(area) {
        let lit = target == Some(Target::Edge(edge_kind));
        tile(t, lit);
        let glyph = t.shrink(8.0);
        painter.rect_stroke(
            glyph,
            CornerRadius::same(1),
            Stroke::new(1.2, theme.muted),
            StrokeKind::Inside,
        );
        let zone = split_part(glyph, Some(edge_kind.split()), 0.33);
        painter.rect_filled(zone, CornerRadius::ZERO, accent);
    }
}

// ---- the drag card ----

/// The card's size: the title's room plus the grip and the margins, at least this wide.
pub const CARD_MIN_WIDTH: f32 = 140.0;
pub const CARD_MAX_WIDTH: f32 = 240.0;
pub const CARD_HEIGHT: f32 = 34.0;
/// How far below and right of the pointer (and of the guide under it) the card sits.
pub const CARD_GAP: Vec2 = vec2(14.0, 18.0);
/// Its opacity while it leads, and while a guide's landing preview leads instead.
pub const CARD_OPACITY: f32 = 0.92;
pub const CARD_OPACITY_ON_GUIDE: f32 = 0.72;

/// What the drag card says about the drop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardState {
    /// Over a guide: the landing preview shows where the panel goes; the card steps back.
    OnGuide,
    /// Over no guide: a release puts the panel back where it was (muted, with a return glyph).
    Returns,
}

/// The guide tile the pointer is on for `target`.
pub fn target_tile(target: Target, area: Rect) -> Rect {
    match target {
        Target::Leaf { rect, split, .. } => compass(rect)
            .into_iter()
            .find(|(s, _)| *s == split)
            .map_or(Rect::NOTHING, |(_, t)| t),
        Target::Edge(edge) => edge_tiles(area)
            .into_iter()
            .find(|(e, _)| *e == edge)
            .map_or(Rect::NOTHING, |(_, t)| t),
    }
}

/// Every guide tile shown for the pointer at `pointer` (the hovered panel's cross, as painted,
/// and the window-edge guides): the card keeps clear of them.
pub fn guide_tiles(dock: &DockState<Tab>, area: Rect, pointer: Pos2, bar: f32) -> Vec<Rect> {
    let mut tiles: Vec<Rect> = edge_tiles(area).into_iter().map(|(_, t)| t).collect();
    if let Some((_, rect)) = hovered_leaf(dock, pointer, bar) {
        tiles.extend(compass(rect).into_iter().map(|(_, t)| t.expand(3.0)));
    }
    tiles
}

/// Where the card of `size` goes for the pointer at `pointer`, `tile` being the guide under it:
/// below and right of both, else below left, above right or above left, the first that stays
/// on `screen` and covers none of the guides (`avoid`); failing that the first on screen, and
/// failing that below right, pushed inside `screen`.
pub fn card_rect(pointer: Pos2, tile: Option<Rect>, avoid: &[Rect], size: Vec2, screen: Rect) -> Rect {
    let at = Rect::from_min_max(pointer, pointer);
    let (anchor, gap) = match tile.filter(|t| t.is_positive()) {
        Some(t) => (t.expand(3.0).union(at), vec2(6.0, 6.0)),
        None => (at, CARD_GAP),
    };
    let margin = 4.0;
    let inside = screen.shrink(margin);
    let right = anchor.right() + gap.x;
    let left = anchor.left() - gap.x - size.x;
    let below = anchor.bottom() + gap.y;
    let above = anchor.top() - gap.y - size.y;
    let candidates = [
        egui::pos2(right, below),
        egui::pos2(left, below),
        egui::pos2(right, above),
        egui::pos2(left, above),
    ]
    .map(|min| Rect::from_min_size(min, size));
    let on_screen = |r: &Rect| inside.contains_rect(*r);
    let clear = |r: &Rect| !avoid.iter().any(|t| t.intersects(*r));
    candidates
        .iter()
        .find(|r| on_screen(r) && clear(r))
        .or_else(|| candidates.iter().find(|r| on_screen(r)))
        .copied()
        .unwrap_or_else(|| {
            let x = right.clamp(inside.left(), (inside.right() - size.x).max(inside.left()));
            let y = below.clamp(inside.top(), (inside.bottom() - size.y).max(inside.top()));
            Rect::from_min_size(egui::pos2(x, y), size)
        })
}

/// The drag card: a small rounded card at the pointer in the panel header's look (its surface,
/// grip and title), with a soft shadow, standing for the panel being dragged. Over a guide it
/// steps back for the landing preview; over no guide it is muted and shows a return glyph, as
/// a release there leaves the panel where it was. Returns where it was painted.
#[allow(clippy::too_many_arguments)]
pub fn paint_card(
    ctx: &egui::Context,
    theme: &Theme,
    look: &crate::panel_header::HeaderLook,
    title: &str,
    pointer: Pos2,
    tile: Option<Rect>,
    avoid: &[Rect],
    state: CardState,
) -> Rect {
    let font = egui::FontId::proportional(look.title_size.min(13.5));
    let galley = ctx.fonts_mut(|f| f.layout_no_wrap(title.to_string(), font.clone(), Color32::WHITE));
    let returns = state == CardState::Returns;
    let glyph_room = if returns { 26.0 } else { 0.0 };
    let width = (12.0 + 10.0 + 10.0 + galley.size().x + 14.0 + glyph_room).clamp(CARD_MIN_WIDTH, CARD_MAX_WIDTH);
    let size = vec2(width, CARD_HEIGHT);
    let rect = card_rect(pointer, tile, avoid, size, ctx.content_rect());
    let mut painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("dock-drag-card")));
    painter.set_opacity(if returns || tile.is_none() {
        CARD_OPACITY
    } else {
        CARD_OPACITY_ON_GUIDE
    });
    let radius = CornerRadius::same(6);
    painter.add(
        egui::epaint::Shadow {
            offset: [0, 3],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(if crate::theme::is_light(theme.panel) { 46 } else { 110 }),
        }
        .as_shape(rect, radius),
    );
    let surface = look.fill.middle();
    let (fill, text, grip) = if returns {
        // Muted: the surface toward the panel, the title in the muted text colour.
        (
            mix(surface, theme.panel, 0.5),
            readable(theme.muted, mix(surface, theme.panel, 0.5)),
            theme.muted,
        )
    } else {
        (surface, look.title_color, look.grip_color)
    };
    painter.rect_filled(rect, radius, fill);
    let edge = look.line.unwrap_or(theme.border);
    painter.rect_stroke(rect, radius, Stroke::new(1.0, edge), StrokeKind::Inside);
    if !returns {
        // A thin accent along the top: this is the panel in hand.
        let top = Rect::from_min_size(rect.min + vec2(6.0, 0.0), vec2(rect.width() - 12.0, 2.0));
        painter.rect_filled(top, CornerRadius::same(1), theme.accent);
    }
    // The grip: two columns of three dots, as on the header.
    let cy = rect.center().y;
    for col in 0..2 {
        for row in -1..=1 {
            let c = egui::pos2(rect.left() + 13.0 + col as f32 * 5.0, cy + row as f32 * 5.5);
            painter.rect_filled(Rect::from_center_size(c, vec2(2.2, 2.2)), 0.6, grip);
        }
    }
    let title_left = rect.left() + 30.0;
    let title_right = rect.right() - 12.0 - glyph_room;
    let mut job = egui::text::LayoutJob::simple_singleline(title.to_string(), font, text);
    job.wrap = egui::text::TextWrapping::truncate_at_width((title_right - title_left).max(0.0));
    let galley = ctx.fonts_mut(|f| f.layout_job(job));
    painter.galley(egui::pos2(title_left, cy - galley.size().y / 2.0), galley, text);
    if returns {
        // Back where it was: the return key's arrow (down, then back to the left).
        let at = Rect::from_center_size(egui::pos2(rect.right() - 20.0, cy), vec2(16.0, 16.0));
        let stroke = Stroke::new(1.5, text);
        let p = |x: f32, y: f32| at.min + vec2(x, y);
        painter.add(egui::Shape::line(
            vec![p(13.0, 3.0), p(13.0, 10.0), p(3.5, 10.0)],
            stroke,
        ));
        painter.add(egui::Shape::line(vec![p(6.5, 7.0), p(3.5, 10.0), p(6.5, 13.0)], stroke));
    }
    rect
}

/// `color`, or the panel's text colour when `color` is too faint on `on`.
fn readable(color: Color32, on: Color32) -> Color32 {
    if crate::theme::contrast(color, on) >= 3.0 {
        color
    } else {
        mix(
            color,
            if crate::theme::contrast(Color32::WHITE, on) > crate::theme::contrast(Color32::BLACK, on) {
                Color32::WHITE
            } else {
                Color32::BLACK
            },
            0.4,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rects(dock: &mut DockState<Tab>, area: Rect) {
        // Lay the dock out headless: one frame of a DockArea in a context.
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(area),
                ..Default::default()
            },
            |ui| {
                struct V;
                impl egui_dock::TabViewer for V {
                    type Tab = Tab;
                    fn id(&mut self, t: &mut Tab) -> egui::Id {
                        egui::Id::new(*t)
                    }
                    fn title(&mut self, t: &mut Tab) -> egui::WidgetText {
                        format!("{t:?}").into()
                    }
                    fn ui(&mut self, _: &mut egui::Ui, _: &mut Tab) {}
                }
                egui_dock::DockArea::new(dock).show_inside(ui, &mut V);
            },
        );
        out.textures_delta.clear();
    }

    #[test]
    fn the_compass_is_egui_docks_and_the_preview_is_the_part_the_drop_takes() {
        let leaf = Rect::from_min_size(egui::pos2(236.0, 48.0), vec2(762.0, 750.0));
        let tiles = compass(leaf);
        assert_eq!(tiles.len(), 5);
        assert_eq!(tiles[0].1.size(), vec2(TILE, TILE));
        assert_eq!(tiles[0].1.center(), leaf.center());
        let left = tiles.iter().find(|(s, _)| *s == Some(Split::Left)).unwrap().1;
        assert_eq!(left.center().x, leaf.center().x - TILE - TILE_GAP);
        let target = Target::Leaf {
            path: NodePath {
                surface: SurfaceIndex::main(),
                node: NodeIndex::root(),
            },
            rect: leaf,
            split: Some(Split::Left),
        };
        let area = Rect::from_min_size(egui::pos2(0.0, 48.0), vec2(1300.0, 750.0));
        assert_eq!(preview(target, area), Rect::from_min_size(leaf.min, vec2(381.0, 750.0)));
        // The bottom window edge: a quarter of the dock's height.
        let bottom = preview(Target::Edge(WindowEdge::Bottom), area);
        assert_eq!(bottom.height(), 750.0 * EDGE_SHARE);
        assert_eq!(bottom.max, area.max);
    }

    #[test]
    fn the_target_follows_the_guides_and_the_edges() {
        let area = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(1200.0, 800.0));
        let mut dock = crate::workspace::default_layout();
        crate::workspace::add_document(&mut dock, Tab::Session(1));
        rects(&mut dock, area);
        let session = dock.find_tab(&Tab::Session(1)).unwrap().node_path();
        let leaf = dock.leaf(session).unwrap().rect;
        let source = dock.find_tab(&Tab::Channels).unwrap().node_path();
        let left = compass(leaf)[4].1;
        match target(&dock, source, area, left.center(), 24.0) {
            Some(Target::Leaf { path, split, .. }) => {
                assert_eq!(path, session);
                assert_eq!(split, Some(Split::Left));
            }
            other => panic!("{other:?}"),
        }
        // Between the guides: nowhere (the panel would float).
        assert_eq!(
            target(&dock, source, area, leaf.left_top() + vec2(40.0, 200.0), 24.0),
            None
        );
        let bottom = edge_tiles(area)[1].1;
        assert_eq!(
            target(&dock, source, area, bottom.center(), 24.0),
            Some(Target::Edge(WindowEdge::Bottom))
        );
        // The panel being dragged, alone in its leaf, offers only its centre.
        let own = dock.leaf(source).unwrap().rect;
        let own_left = compass(own)[4].1;
        assert!(!matches!(
            target(&dock, source, area, own_left.center(), 24.0),
            Some(Target::Leaf { split: Some(_), .. })
        ));
    }

    #[test]
    fn a_drop_lands_at_the_size_its_preview_showed() {
        let area = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(1200.0, 800.0));
        let mut dock = crate::workspace::default_layout();
        crate::workspace::add_document(&mut dock, Tab::Session(1));
        rects(&mut dock, area);
        let session = dock.find_tab(&Tab::Session(1)).unwrap().node_path();
        let leaf = dock.leaf(session).unwrap().rect;
        let target = Target::Leaf {
            path: session,
            rect: leaf,
            split: Some(Split::Left),
        };
        let previewed = preview(target, area).width();
        // What egui_dock does on the drop: take Map out of the right column (Channels widens
        // to the whole column) and split the session's leaf in two halves.
        let map = dock.find_tab(&Tab::Map).unwrap();
        let tab = dock.remove_tab(map).unwrap();
        let session = dock.find_tab(&Tab::Session(1)).unwrap().node_path();
        dock[session.surface].split(session.node, Split::Left, 0.5, Node::leaf(tab));
        let mut state = DropState {
            dragging: None,
            pending: Some(Pending::Size {
                tab: Tab::Map,
                split: Split::Left,
                extent: previewed,
            }),
        };
        // Before the new split is laid out nothing changes; after it, the fraction is fixed.
        assert!(!state.finish(&mut dock));
        rects(&mut dock, area);
        assert!(state.finish(&mut dock));
        rects(&mut dock, area);
        let width = dock
            .leaf(dock.find_tab(&Tab::Map).unwrap().node_path())
            .unwrap()
            .rect
            .width();
        assert!(
            (width - previewed).abs() <= 3.0,
            "docked {width}, previewed {previewed}"
        );
        assert!(state.pending.is_none());
    }

    #[test]
    fn an_edge_drop_docks_the_panel_along_that_edge_at_a_quarter() {
        // egui_dock leaves a panel released on a window-edge guide where it was (panels never
        // float); the app moves it.
        let mut dock = crate::workspace::default_layout();
        let mut state = DropState {
            dragging: None,
            pending: Some(Pending::Edge {
                tab: Tab::Channels,
                edge: WindowEdge::Bottom,
            }),
        };
        assert!(state.finish(&mut dock));
        let path = dock.find_tab(&Tab::Channels).unwrap();
        assert_eq!(path.surface, SurfaceIndex::main());
        assert_eq!(dock.iter_surfaces().count(), 1);
        assert_eq!(path.node, NodeIndex::root().right());
        assert_eq!(dock.iter_all_tabs().filter(|(_, t)| **t == Tab::Channels).count(), 1);
        match &dock.main_surface()[NodeIndex::root()] {
            Node::Vertical(split) => assert!((split.fraction - (1.0 - EDGE_SHARE)).abs() < 1e-6),
            other => panic!("{other:?}"),
        }
    }

    /// The card sits below and right of the pointer, or past the guide it is on, never over a
    /// guide, and flips left or up near the window's edges, staying inside it.
    #[test]
    fn the_drag_card_keeps_clear_of_the_guides_and_inside_the_window() {
        let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(1300.0, 820.0));
        let size = vec2(160.0, CARD_HEIGHT);
        // No guide: just below and right of the pointer.
        let p = egui::pos2(400.0, 300.0);
        let r = card_rect(p, None, &[], size, screen);
        assert_eq!(r.min, p + CARD_GAP);
        // On a panel's left guide: past that tile, covering none of the cross.
        let leaf = Rect::from_min_size(egui::pos2(240.0, 60.0), vec2(760.0, 720.0));
        let tiles = compass(leaf);
        let avoid: Vec<Rect> = tiles.iter().map(|(_, t)| t.expand(3.0)).collect();
        for (_, tile) in &tiles {
            let r = card_rect(tile.center(), Some(*tile), &avoid, size, screen);
            assert!(avoid.iter().all(|t| !t.intersects(r)), "{tile:?} -> {r:?}");
            assert!(screen.contains_rect(r));
        }
        // At the bottom right corner: flipped above and to the left, inside the window.
        let corner = egui::pos2(1290.0, 810.0);
        let r = card_rect(corner, None, &[], size, screen);
        assert!(screen.shrink(4.0).contains_rect(r), "{r:?}");
        assert!(r.right() < corner.x && r.bottom() < corner.y);
        // The bottom edge guide: the card goes above it.
        let area = screen.shrink(8.0);
        let bottom = target_tile(Target::Edge(WindowEdge::Bottom), area);
        let r = card_rect(bottom.center(), Some(bottom), &[bottom], size, screen);
        assert!(!r.intersects(bottom) && r.bottom() <= bottom.top());
    }
}
