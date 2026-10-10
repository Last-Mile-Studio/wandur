//! The tabs across the top of the document area: one per open session (and Find a MUD while it
//! is open, as it is a document of its own), in the order of the dock's document leaf.
//!
//! Each tab shows the session's title ("World · Character", numbered when two match), a dot for
//! the connection (filled green connected, an amber ring connecting, an amber ring with a centre
//! reconnecting, a red ring disconnected), and an accent dot where its close button sits when
//! the session has output nobody has seen (the terminal's line and revision counters, as the
//! Workspace panel's marker). The close button shows on the active tab and under the pointer;
//! a middle click closes too. Tabs drag to reorder; a right click offers Reconnect or
//! Disconnect, Duplicate session, Rename, Close and Close other tabs. The "+" at the end opens
//! Find a MUD.
//!
//! Width: each tab takes what its title needs (at most [`TAB_MAX`]); when they do not fit they
//! shrink evenly, titles ellipsized, down to [`TAB_MIN`]; past that the strip scrolls (the wheel,
//! or the arrows) and a list of every tab opens from the chevron.
//!
//! The strip is drawn by the shell inside the shown document's body (so it costs a few rects and
//! galleys per frame, and nothing at all for hidden documents); what the person asks comes back
//! as [`StripAction`]s the app applies after the frame.

use std::borrow::Cow;

use egui::text::{LayoutJob, TextWrapping};
use egui::{Color32, CornerRadius, FontId, Id, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use wandur_core::l10n::{S, t};

use crate::panel_header::{HeaderAction, HeaderLook};
use crate::session_tab::{SessionId, Status};
use crate::theme::{Theme, mix};
use crate::widgets::Icon;
use crate::workspace::Tab;

/// The narrowest a tab gets before the strip scrolls.
pub const TAB_MIN: f32 = 120.0;
/// The widest a tab gets, however long its title.
pub const TAB_MAX: f32 = 220.0;
/// A short title's tab is never narrower than this.
const TAB_SHORTEST: f32 = 64.0;
/// Space at each end of a tab.
const PAD: f32 = 10.0;
/// The connection dot's box, and the gap after it.
const DOT: f32 = 8.0;
const DOT_GAP: f32 = 7.0;
/// The close button's box, and the gap before it.
const CLOSE: f32 = 18.0;
const CLOSE_GAP: f32 = 2.0;
/// The strip's own buttons (+, and the scroll arrows and the list when it overflows).
const BUTTON: f32 = 26.0;
/// Title text size.
const FONT: f32 = 12.5;

/// The strip's own buttons, by name.
const ACTION_LEFT: &str = "TabsScrollLeft";
const ACTION_RIGHT: &str = "TabsScrollRight";
const ACTION_ALL: &str = "TabsAll";
const ACTION_NEW: &str = "TabsNew";

/// A session's connection, as the tab's dot shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conn {
    Connected,
    Connecting,
    Reconnecting,
    Disconnected,
}

impl Conn {
    pub fn of(status: &Status) -> Self {
        match status {
            Status::Connected { .. } | Status::Demo => Conn::Connected,
            Status::Connecting => Conn::Connecting,
            Status::Waiting { .. } => Conn::Reconnecting,
            Status::Closed { .. } => Conn::Disconnected,
        }
    }

    fn paint(self, painter: &egui::Painter, c: Pos2, theme: &Theme) {
        match self {
            Conn::Connected => {
                painter.circle_filled(c, 3.5, theme.ok);
            }
            Conn::Connecting => {
                painter.circle_stroke(c, 3.0, Stroke::new(1.4, theme.warn));
            }
            Conn::Reconnecting => {
                painter.circle_stroke(c, 3.0, Stroke::new(1.4, theme.warn));
                painter.circle_filled(c, 1.3, theme.warn);
            }
            Conn::Disconnected => {
                painter.circle_stroke(c, 3.0, Stroke::new(1.4, theme.error));
            }
        }
    }
}

/// One tab to draw.
#[derive(Clone, Debug)]
pub struct Item<'a> {
    pub tab: Tab,
    pub title: Cow<'a, str>,
    /// `None` for Find a MUD (a search glyph instead of a dot).
    pub conn: Option<Conn>,
    /// New output nobody has seen (never on the shown tab).
    pub activity: bool,
}

/// What the person asked of the strip.
#[derive(Clone, Debug, PartialEq)]
pub enum StripAction {
    Focus(Tab),
    Close(Tab),
    CloseOthers(Tab),
    /// Put the tab at this index of the strip.
    Move(Tab, usize),
    Reconnect(SessionId),
    Disconnect(SessionId),
    Duplicate(SessionId),
    /// A name typed on the tab (empty: back to the world's name).
    Rename(SessionId, String),
    /// The "+": Find a MUD.
    New,
}

/// What the strip remembers between frames (in egui's memory, by the strip's id).
#[derive(Clone, Debug, Default)]
struct Memory {
    /// The tab being dragged and where the pointer took it (from its left edge).
    drag: Option<(Tab, f32)>,
    /// How far the tabs are scrolled (only when they overflow).
    scroll: f32,
    /// The active tab and the number of tabs last frame (a change scrolls the active one into
    /// view).
    last_active: Option<Tab>,
    last_len: usize,
    /// The tab being renamed and the text typed.
    renaming: Option<(Tab, String)>,
    /// Where each tab was drawn last frame (tests and scenes point at them).
    rects: Vec<(Tab, Rect)>,
    /// The strip's own buttons last frame: +, scroll left, scroll right, the list.
    buttons: [Option<Rect>; 4],
}

/// Where the strip drew its tabs last frame, by its id.
pub fn tab_rects(ctx: &egui::Context, id: Id) -> Vec<(Tab, Rect)> {
    ctx.data(|d| d.get_temp::<Memory>(id))
        .map(|m| m.rects)
        .unwrap_or_default()
}

/// Where the strip drew its buttons last frame: +, scroll left, scroll right, the list (the last
/// three only while the tabs overflow).
pub fn button_rects(ctx: &egui::Context, id: Id) -> [Option<Rect>; 4] {
    ctx.data(|d| d.get_temp::<Memory>(id))
        .map(|m| m.buttons)
        .unwrap_or_default()
}

/// Where a tab's close button is, for a tab drawn at `tab`.
pub fn close_rect(tab: Rect) -> Rect {
    Rect::from_center_size(
        pos2(tab.right() - PAD - CLOSE / 2.0 + 4.0, tab.center().y + 1.0),
        vec2(CLOSE, CLOSE),
    )
}

/// The strip's scroll offset last frame.
pub fn scroll_of(ctx: &egui::Context, id: Id) -> f32 {
    ctx.data(|d| d.get_temp::<Memory>(id)).map_or(0.0, |m| m.scroll)
}

/// The tabs' widths for `preferred` widths in `available` points, and whether they overflow
/// (then each is at most [`TAB_MIN`] and the strip scrolls). When the preferred widths do not
/// fit, the widest shrink first, evenly, to a common cap no narrower than [`TAB_MIN`].
pub fn fit(preferred: &[f32], available: f32) -> (Vec<f32>, bool) {
    let total: f32 = preferred.iter().sum();
    if total <= available {
        return (preferred.to_vec(), false);
    }
    let floor: f32 = preferred.iter().map(|p| p.min(TAB_MIN)).sum();
    if floor > available {
        return (preferred.iter().map(|p| p.min(TAB_MIN)).collect(), true);
    }
    let (mut low, mut high) = (TAB_MIN, TAB_MAX);
    for _ in 0..24 {
        let cap = (low + high) / 2.0;
        if preferred.iter().map(|p| p.min(cap)).sum::<f32>() > available {
            high = cap;
        } else {
            low = cap;
        }
    }
    (preferred.iter().map(|p| p.min(low)).collect(), false)
}

/// Where a dragged tab lands: the index among the strip's tabs for its left edge at `left`
/// (strip coordinates), given every tab's width in strip order and the dragged one's index. The
/// other tabs close up round a gap; the gap goes to the place nearest the dragged tab.
pub fn drop_index(widths: &[f32], dragged: usize, left: f32) -> usize {
    let mut best = (0, f32::INFINITY);
    let mut slot = 0.0;
    let mut k = 0;
    for (i, w) in widths.iter().enumerate() {
        if i == dragged {
            continue;
        }
        if (slot - left).abs() < best.1 {
            best = (k, (slot - left).abs());
        }
        slot += w;
        k += 1;
    }
    if (slot - left).abs() < best.1 {
        best = (k, (slot - left).abs());
    }
    best.0
}

/// A strip to draw.
pub struct Strip<'a> {
    pub id: Id,
    pub items: &'a [Item<'a>],
    pub active: Tab,
    pub look: &'a HeaderLook,
    pub theme: &'a Theme,
}

/// Draw the strip in `rect`. `tip` gives a tab's tooltip (asked only while it is hovered).
pub fn show(ui: &mut Ui, rect: Rect, strip: &Strip<'_>, tip: &mut dyn FnMut(Tab) -> String) -> Vec<StripAction> {
    let mut actions = Vec::new();
    let (theme, look, items) = (strip.theme, strip.look, strip.items);
    let mut mem: Memory = ui.data(|d| d.get_temp(strip.id)).unwrap_or_default();
    let painter = ui.painter_at(rect);
    crate::skin::gradient_on(&painter, rect, &look.fill);
    if let Some(line) = look.top_line {
        painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, line));
    }
    let line = look.line.unwrap_or(theme.border);
    let font = FontId::proportional(FONT);
    let n = items.len();

    // Widths: what each title needs, then fitted.
    let chrome = PAD + DOT + DOT_GAP + CLOSE_GAP + CLOSE + PAD - 4.0;
    let preferred: Vec<f32> = ui.fonts_mut(|f| {
        items
            .iter()
            .map(|it| {
                let text: f32 = it.title.chars().map(|c| f.glyph_width(&font, c)).sum();
                (chrome + text.ceil()).clamp(TAB_SHORTEST, TAB_MAX)
            })
            .collect()
    });
    let plus_w = BUTTON + 6.0;
    let (mut widths, overflow) = fit(&preferred, rect.width() - plus_w);
    let controls_w = if overflow { 3.0 * BUTTON + plus_w } else { plus_w };
    if overflow {
        widths = fit(&preferred, rect.width() - controls_w).0;
    }
    let total: f32 = widths.iter().sum();
    let area = Rect::from_min_max(
        rect.min,
        pos2((rect.right() - controls_w).max(rect.left()), rect.bottom()),
    );
    let max_scroll = (total - area.width()).max(0.0);
    let active_index = items.iter().position(|it| it.tab == strip.active);
    if overflow && (mem.last_active != Some(strip.active) || mem.last_len != n) {
        // A tab newly shown (or added) scrolls into view.
        if let Some(i) = active_index {
            let left: f32 = widths[..i].iter().sum();
            let right = left + widths[i];
            if left < mem.scroll {
                mem.scroll = left;
            } else if right > mem.scroll + area.width() {
                mem.scroll = right - area.width();
            }
        }
    }
    mem.last_active = Some(strip.active);
    mem.last_len = n;
    if overflow && ui.rect_contains_pointer(area) {
        let delta = ui.input(|i| i.smooth_scroll_delta);
        mem.scroll -= delta.x + delta.y;
    }
    mem.scroll = if overflow {
        mem.scroll.clamp(0.0, max_scroll)
    } else {
        0.0
    };

    // Strip order, with a dragged tab moved to where it would land.
    let mut order: Vec<usize> = (0..n).collect();
    let mut dragged: Option<(usize, f32)> = None;
    if let Some((tab, grab)) = mem.drag
        && let Some(d) = items.iter().position(|it| it.tab == tab)
        && let Some(p) = ui.input(|i| i.pointer.interact_pos())
    {
        let left = (p.x - area.left() + mem.scroll - grab).clamp(0.0, (total - widths[d]).max(0.0));
        let to = drop_index(&widths, d, left);
        order.remove(d);
        order.insert(to, d);
        dragged = Some((d, left));
    }
    let mut lefts = vec![0.0; n];
    let mut x = 0.0;
    for &i in &order {
        lefts[i] = x;
        x += widths[i];
    }
    if let Some((d, left)) = dragged {
        lefts[d] = left;
    }
    let screen = |i: usize| {
        Rect::from_min_size(
            pos2(area.left() + lefts[i] - mem.scroll, rect.top()),
            vec2(widths[i], rect.height()),
        )
    };

    // The line under the strip, open under the active tab.
    let bottom = rect.bottom() - 0.5;
    match active_index.map(screen).map(|r| r.intersect(area)) {
        Some(r) if r.width() > 0.0 => {
            painter.hline(rect.left()..=r.left(), bottom, Stroke::new(1.0, line));
            painter.hline(r.right()..=rect.right(), bottom, Stroke::new(1.0, line));
        }
        _ => {
            painter.hline(rect.x_range(), bottom, Stroke::new(1.0, line));
        }
    }

    let tab_painter = ui.painter_at(area);
    let pointer_down = ui.input(|i| i.pointer.any_down());
    mem.rects.clear();
    // The dragged tab is drawn last, over the others.
    let mut draw_order = order.clone();
    if let Some((d, _)) = dragged {
        draw_order.retain(|&i| i != d);
        draw_order.push(d);
    }
    let renaming = mem.renaming.clone();
    let mut new_renaming = renaming.clone();
    for &i in &draw_order {
        let it = &items[i];
        let r = screen(i);
        let hit = r.intersect(area);
        mem.rects.push((it.tab, r));
        if hit.width() < 1.0 {
            continue;
        }
        let active = it.tab == strip.active;
        let id = strip.id.with(("tab", it.tab));
        let response = ui.interact(hit, id, Sense::click_and_drag());
        crate::a11y::toggle(&response, egui::accesskit::Role::Tab, &it.title, active);
        let under = ui.rect_contains_pointer(hit) || response.has_focus();
        let lifted = dragged.is_some_and(|(d, _)| d == i);
        // Face: the shown tab joins the document under it; others lighten under the pointer.
        if active || lifted {
            let fill = if lifted && !active {
                mix(theme.panel, theme.text, 0.06)
            } else {
                theme.panel
            };
            let face = Rect::from_min_max(pos2(r.left(), r.top() + 3.0), r.max);
            tab_painter.rect_filled(
                face,
                CornerRadius {
                    nw: 5,
                    ne: 5,
                    sw: 0,
                    se: 0,
                },
                fill,
            );
            tab_painter.vline(face.left() + 0.5, face.y_range(), Stroke::new(1.0, line));
            tab_painter.vline(face.right() - 0.5, face.y_range(), Stroke::new(1.0, line));
            if active {
                tab_painter.hline(
                    (face.left() + 1.0)..=(face.right() - 1.0),
                    face.top() + 1.0,
                    Stroke::new(2.0, theme.accent),
                );
            }
        } else {
            if under && !pointer_down {
                let face = Rect::from_min_max(
                    pos2(r.left() + 1.0, r.top() + 4.0),
                    pos2(r.right() - 1.0, r.bottom() - 3.0),
                );
                tab_painter.rect_filled(face, 4.0, theme.hover_fill());
            }
            // A quiet separator at the right edge, unless the next tab is the shown one.
            let next_active = order
                .iter()
                .position(|&o| o == i)
                .and_then(|p| order.get(p + 1))
                .is_some_and(|&o| items[o].tab == strip.active);
            if !next_active {
                tab_painter.vline(
                    r.right() - 0.5,
                    (r.top() + 9.0)..=(r.bottom() - 9.0),
                    Stroke::new(1.0, line.gamma_multiply(0.7)),
                );
            }
        }
        if response.has_focus() {
            tab_painter.rect_stroke(
                r.shrink2(vec2(2.0, 4.0)),
                4.0,
                Stroke::new(1.0, theme.accent),
                egui::StrokeKind::Inside,
            );
        }
        // The dot (or Find a MUD's glyph).
        let cy = r.center().y + 1.0;
        let dot = pos2(r.left() + PAD + DOT / 2.0, cy);
        match it.conn {
            Some(conn) => conn.paint(&tab_painter, dot, theme),
            None => magnifier(&tab_painter, dot, mix(look.title_color, theme.muted, 0.5)),
        }
        // The title, ellipsized to its room.
        let text_left = r.left() + PAD + DOT + DOT_GAP;
        let text_right = r.right() - PAD - CLOSE - CLOSE_GAP + 4.0;
        let color = if active || it.activity {
            look.title_color
        } else {
            mix(look.title_color, theme.muted, 0.55)
        };
        let title_rect = Rect::from_min_max(pos2(text_left, r.top() + 4.0), pos2(text_right, r.bottom() - 2.0));
        let editing = renaming.as_ref().is_some_and(|(tab, _)| *tab == it.tab);
        if editing {
            if let Some((_, text)) = new_renaming.as_mut() {
                let edit = ui.put(
                    title_rect,
                    egui::TextEdit::singleline(text)
                        .font(font.clone())
                        .char_limit(100)
                        .id(strip.id.with(("rename", it.tab))),
                );
                crate::a11y::label(&edit, t(S::Rename));
                if !edit.has_focus() && !edit.lost_focus() {
                    edit.request_focus();
                }
                let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
                if escape {
                    new_renaming = None;
                } else if enter || edit.lost_focus() {
                    if let Tab::Session(sid) = it.tab {
                        actions.push(StripAction::Rename(sid, text.trim().to_string()));
                    }
                    new_renaming = None;
                }
            }
        } else {
            let mut job = LayoutJob::simple_singleline(it.title.to_string(), font.clone(), color);
            job.wrap = TextWrapping {
                max_width: (text_right - text_left).max(8.0),
                max_rows: 1,
                break_anywhere: true,
                overflow_character: Some('…'),
            };
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            let at = pos2(text_left, cy - galley.size().y / 2.0);
            tab_painter.galley(at, galley, color);
        }
        // The close button (on the shown tab and under the pointer), else the activity dot.
        let close_rect = close_rect(r);
        let close_name = if matches!(it.tab, Tab::Session(_)) {
            t(S::CloseSession2)
        } else {
            t(S::CloseWorkspaceItem)
        };
        let show_close = active || under || lifted;
        let close_hit = if show_close {
            close_rect.intersect(area)
        } else {
            Rect::from_center_size(close_rect.center(), vec2(0.0, 0.0))
        };
        let close = ui.interact(close_hit, id.with("close"), Sense::click());
        close.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, close_name));
        if show_close && !editing {
            if close.hovered() {
                tab_painter.rect_filled(close_rect, 4.0, theme.hover_fill());
            }
            let ink = if close.hovered() { theme.text } else { theme.muted };
            crate::widgets::paint_icon_in(&tab_painter, Icon::Close, close_rect.shrink(1.0), ink);
        } else if it.activity {
            tab_painter.circle_filled(close_rect.center(), 3.5, theme.accent);
        }
        let close = close.on_hover_text(close_name);
        // What the pointer did.
        if close.clicked() || response.middle_clicked() {
            actions.push(StripAction::Close(it.tab));
        } else if response.double_clicked() && matches!(it.tab, Tab::Session(_)) {
            new_renaming = Some((it.tab, it.title.to_string()));
        } else if response.clicked() {
            actions.push(StripAction::Focus(it.tab));
        }
        if response.drag_started()
            && let Some(p) = response.interact_pointer_pos()
        {
            mem.drag = Some((it.tab, p.x - r.left()));
        }
        if response.drag_stopped()
            && let Some((tab, _)) = mem.drag.take()
            && tab == it.tab
            && let Some(to) = order.iter().position(|&o| o == i)
            && to != i
        {
            actions.push(StripAction::Move(it.tab, to));
        }
        let response = if response.hovered() && !response.dragged() {
            let words = tip(it.tab);
            response.on_hover_text(words)
        } else {
            response
        };
        response.context_menu(|ui| {
            context_menu(ui, it, n, &mut actions, &mut new_renaming);
        });
    }
    if mem.drag.is_some() && !pointer_down {
        mem.drag = None;
    }
    mem.renaming = new_renaming;

    // The strip's own buttons: + (after the last tab, or at the right end when they overflow),
    // and while they overflow the scroll arrows and the list of every tab.
    let button_y = rect.center().y + 1.0;
    let button = |x: f32| Rect::from_center_size(pos2(x + BUTTON / 2.0, button_y), vec2(BUTTON - 2.0, BUTTON - 2.0));
    mem.buttons = [None; 4];
    if overflow {
        let mut x = area.right() + 2.0;
        let left = button(x);
        x += BUTTON;
        let right = button(x);
        x += BUTTON;
        let more = button(x);
        x += BUTTON;
        let plus = button(x + 2.0);
        let step = TAB_MIN * 2.0;
        let scroll_left =
            HeaderAction::button(ACTION_LEFT, Icon::ChevronLeft, S::SessionTabsScrollLeft).enabled(mem.scroll > 0.5);
        if crate::panel_header::action_button(ui, left, &scroll_left, theme).clicked() {
            mem.scroll = (mem.scroll - step).max(0.0);
        }
        let scroll_right = HeaderAction::button(ACTION_RIGHT, Icon::ChevronRight, S::SessionTabsScrollRight)
            .enabled(mem.scroll < max_scroll - 0.5);
        if crate::panel_header::action_button(ui, right, &scroll_right, theme).clicked() {
            mem.scroll = (mem.scroll + step).min(max_scroll);
        }
        let all = HeaderAction::button(ACTION_ALL, Icon::ChevronDown, S::SessionTabsAll);
        let list = crate::panel_header::action_button(ui, more, &all, theme);
        crate::a11y::name_popup(
            ui.ctx(),
            egui::Popup::default_response_id(&list),
            egui::accesskit::Role::Menu,
            t(S::SessionTabsAll),
        );
        egui::Popup::menu(&list).align(egui::RectAlign::BOTTOM_END).show(|ui| {
            ui.set_min_width(220.0);
            for it in items {
                if list_row(ui, it, it.tab == strip.active, theme).clicked() {
                    actions.push(StripAction::Focus(it.tab));
                    ui.close();
                }
            }
        });
        if plus_button(ui, plus, theme) {
            actions.push(StripAction::New);
        }
        mem.buttons = [Some(plus), Some(left), Some(right), Some(more)];
    } else {
        let plus = button(area.left() + total + 3.0);
        if plus_button(ui, plus, theme) {
            actions.push(StripAction::New);
        }
        mem.buttons[0] = Some(plus);
    }
    ui.data_mut(|d| d.insert_temp(strip.id, mem));
    actions
}

fn plus_button(ui: &mut Ui, rect: Rect, theme: &Theme) -> bool {
    let plus = HeaderAction::button(ACTION_NEW, Icon::Plus, S::FindAMUD);
    crate::panel_header::action_button(ui, rect, &plus, theme).clicked()
}

/// The right-click menu of a tab.
fn context_menu(
    ui: &mut Ui,
    it: &Item<'_>,
    count: usize,
    actions: &mut Vec<StripAction>,
    renaming: &mut Option<(Tab, String)>,
) {
    let mut item = |ui: &mut Ui, label: S, enabled: bool, action: Option<StripAction>| {
        if ui.add_enabled(enabled, egui::Button::new(t(label))).clicked() {
            if let Some(action) = action {
                actions.push(action);
            }
            ui.close();
            true
        } else {
            false
        }
    };
    if let Tab::Session(id) = it.tab {
        if matches!(it.conn, Some(Conn::Disconnected | Conn::Reconnecting)) {
            item(ui, S::Reconnect, true, Some(StripAction::Reconnect(id)));
        }
        if matches!(it.conn, Some(Conn::Connected | Conn::Connecting | Conn::Reconnecting)) {
            item(ui, S::Disconnect2, true, Some(StripAction::Disconnect(id)));
        }
        item(ui, S::SessionTabDuplicate, true, Some(StripAction::Duplicate(id)));
        if item(ui, S::Rename, true, None) {
            *renaming = Some((it.tab, it.title.to_string()));
        }
        ui.separator();
    }
    item(ui, S::CloseWorkspaceItem, true, Some(StripAction::Close(it.tab)));
    item(
        ui,
        S::SessionTabCloseOthers,
        count > 1,
        Some(StripAction::CloseOthers(it.tab)),
    );
}

/// A row of the list of every tab: the dot and the title, the shown one selected.
fn list_row(ui: &mut Ui, it: &Item<'_>, selected: bool, theme: &Theme) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width().max(200.0), 26.0), Sense::click());
    crate::a11y::toggle(&response, egui::accesskit::Role::MenuItem, &it.title, selected);
    if selected {
        ui.painter().rect_filled(rect, 4.0, theme.selection_fill());
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, theme.hover_fill());
    }
    let dot = pos2(rect.left() + 12.0, rect.center().y);
    match it.conn {
        Some(conn) => conn.paint(ui.painter(), dot, theme),
        None => magnifier(ui.painter(), dot, theme.muted),
    }
    let mut job = LayoutJob::simple_singleline(it.title.to_string(), FontId::proportional(13.0), theme.text);
    job.wrap = TextWrapping {
        max_width: (rect.width() - 44.0).max(8.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    ui.painter().galley(
        pos2(rect.left() + 24.0, rect.center().y - galley.size().y / 2.0),
        galley,
        theme.text,
    );
    if it.activity {
        ui.painter()
            .circle_filled(pos2(rect.right() - 12.0, rect.center().y), 3.5, theme.accent);
    }
    response
}

/// Find a MUD's mark in place of a session's dot: a small magnifying glass.
fn magnifier(painter: &egui::Painter, c: Pos2, color: Color32) {
    let stroke = Stroke::new(1.3, color);
    let lens = c + vec2(-0.8, -0.8);
    painter.circle_stroke(lens, 3.4, stroke);
    painter.line_segment([lens + vec2(2.5, 2.5), lens + vec2(5.0, 5.0)], stroke);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_keep_their_width_when_they_fit_shrink_evenly_then_overflow() {
        // Room for all: as preferred.
        let (w, over) = fit(&[180.0, 200.0, 80.0], 500.0);
        assert_eq!(w, [180.0, 200.0, 80.0]);
        assert!(!over);
        // Too wide: the widest shrink first, to a common cap.
        let (w, over) = fit(&[180.0, 200.0, 80.0], 360.0);
        assert!(!over);
        assert!((w.iter().sum::<f32>() - 360.0).abs() < 0.5, "{w:?}");
        assert_eq!(w[2], 80.0, "a short tab keeps its width");
        assert!((w[0] - w[1]).abs() < 0.5, "the two wide ones share the room: {w:?}");
        assert!(w[0] >= TAB_MIN);
        // Past the narrowest: every tab at most TAB_MIN, and the strip scrolls.
        let (w, over) = fit(&[200.0; 10], 600.0);
        assert!(over);
        assert!(w.iter().all(|&x| x == TAB_MIN));
    }

    #[test]
    fn a_dragged_tab_lands_where_its_centre_passes_the_others_centres() {
        let widths = [100.0, 100.0, 100.0, 100.0];
        // Tab 0 dragged a little: stays first.
        assert_eq!(drop_index(&widths, 0, 30.0), 0);
        // Its centre past tab 1's centre (150): second.
        assert_eq!(drop_index(&widths, 0, 110.0), 1);
        // All the way right: last.
        assert_eq!(drop_index(&widths, 0, 300.0), 3);
        // Tab 3 to the front.
        assert_eq!(drop_index(&widths, 3, 0.0), 0);
        assert_eq!(drop_index(&widths, 3, 160.0), 2);
    }

    #[test]
    fn the_dot_tells_the_four_connection_states_apart() {
        use std::time::Instant;
        assert_eq!(
            Conn::of(&Status::Connected {
                peer: String::new(),
                secure: false
            }),
            Conn::Connected
        );
        assert_eq!(Conn::of(&Status::Demo), Conn::Connected);
        assert_eq!(Conn::of(&Status::Connecting), Conn::Connecting);
        assert_eq!(
            Conn::of(&Status::Waiting {
                until: Instant::now(),
                attempt: 2
            }),
            Conn::Reconnecting
        );
        assert_eq!(Conn::of(&Status::Closed { reason: "x".into() }), Conn::Disconnected);
    }
}
