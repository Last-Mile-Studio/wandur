//! The session rail and the script panels in it (the C# `ScriptPanelRailView` and
//! `ScriptPanelView`): a column beside the transcript that holds the session's visible panels
//! that are not bars, as an accordion. Headers stay visible, the open panel's body sits under its
//! header at content height (capped so the headers below stay on screen), and a top bar folds the
//! rail to a 28 point strip. The rail shows only while a panel is visible.
//!
//! Panel content is game data, so it reads like the transcript: the terminal's monospace face
//! and its palette for colour codes (`&Y`, `^b`, SGR). Gauges look like the vitals strip's cards.
//! Colour codes are parsed when a widget's revision moves, not every frame: a script that re-sends
//! unchanged values costs no new layout (see [`RailState::layouts_built`]).

use std::collections::HashMap;

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Rect, RichText, Sense, Ui, pos2, vec2};
use wandur_core::l10n::{S, t};
use wandur_core::mudcolor::{self, Paint};
use wandur_core::scripting::panels::{Panel, PanelEvent, PanelHost, Widget, WidgetKind, measure};

use crate::theme::Theme;

/// Width of the rail while it shows panels (the C# column's 260), and its least width.
pub const RAIL_WIDTH: f32 = 260.0;
/// The rail widens to fit its widest table up to this; past it, table cells wrap.
pub const RAIL_MAX_WIDTH: f32 = 420.0;
/// Space between two table columns.
const TABLE_GAP: f32 = 10.0;
/// The open panel's margins left and right ([`show`]'s inner frame).
const BODY_MARGIN: f32 = 16.0;
/// Width while folded to the top restore control.
pub const COLLAPSED_WIDTH: f32 = 28.0;
/// Height of the top bar and the least height of a section header.
const BAR_HEIGHT: f32 = 28.0;
/// Height of a gauge (one vitals card).
const GAUGE_HEIGHT: f32 = crate::vitals_view::CARD_HEIGHT;

/// A widget, by script, panel and widget id.
type Key = (String, String, String);

fn key(panel: &Panel, widget: &str) -> Key {
    (panel.script.clone(), panel.id.clone(), widget.to_string())
}

/// What the rail remembers between frames.
#[derive(Debug, Default)]
pub struct RailState {
    /// Folded to the restore control.
    pub collapsed: bool,
    /// The open panel (script, panel id).
    pub selected: Option<(String, String)>,
    /// The panels the last frame showed, in order (tests and the probe).
    pub shown: Vec<String>,
    /// Where the last frame drew each widget of the open panel: (panel, widget, rect).
    pub drawn: Vec<(String, String, Rect)>,
    /// Where the last frame drew the section headers, in order.
    pub headers: Vec<Rect>,
    /// Text layouts built from widget text (colour codes parsed) since the start: unchanged
    /// widgets build none.
    pub layouts_built: u64,
    jobs: HashMap<Key, (u64, Vec<LayoutJob>)>,
    /// The palette the cached layouts were coloured with.
    palette: Option<[Color32; 17]>,
    inputs: HashMap<Key, (u64, String)>,
    toggles: HashMap<Key, (u64, bool)>,
    lists: HashMap<Key, String>,
    /// Each table's natural width (every column as wide as its widest cell), by revision.
    table_widths: HashMap<Key, (u64, f32)>,
    /// The width the rail asks for: its widest table plus margins (at least [`RAIL_WIDTH`]).
    pub wanted: f32,
}

/// A widget callback the person caused this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelInput {
    pub script: String,
    pub panel: String,
    pub widget: String,
    pub event: PanelEvent,
    pub text: Option<String>,
}

/// The colour a code names, in this theme.
pub fn paint(paint: Paint, theme: &Theme) -> Option<Color32> {
    match paint {
        Paint::Default => None,
        Paint::Index(i) => Some(theme.indexed(i)),
        Paint::Rgb(r, g, b) => Some(Color32::from_rgb(r, g, b)),
    }
}

/// A layout of text that may carry colour codes, uncoloured parts in `color`.
pub fn mud_job(text: &str, font: FontId, color: Color32, theme: &Theme) -> LayoutJob {
    let mut job = LayoutJob::default();
    for run in mudcolor::parse(text) {
        let mut format = TextFormat::simple(font.clone(), paint(run.style.fg, theme).unwrap_or(color));
        if let Some(bg) = paint(run.style.bg, theme) {
            format.background = bg;
        }
        format.italics = run.style.italic;
        if run.style.underline {
            format.underline = egui::Stroke::new(1.0, format.color);
        }
        if run.style.bold {
            format.font_id = crate::fonts::bold_of(&font);
        }
        job.append(&run.text, 0.0, format);
    }
    job
}

/// Open the panel a script asked to focus, or keep the open one, or open the first; returns
/// whether the rail shows (the session has a visible panel that is not bars).
pub fn sync(host: &mut PanelHost, state: &mut RailState) -> bool {
    let mut focus = None;
    for panel in host.panels.iter_mut().filter(|p| p.visible && !p.is_bars()) {
        if panel.take_focus_request() {
            focus = Some((panel.script.clone(), panel.id.clone()));
        }
    }
    let live: Vec<(String, String)> = host.rail().map(|p| (p.script.clone(), p.id.clone())).collect();
    if live.is_empty() {
        state.selected = None;
        return false;
    }
    if let Some(focus) = focus {
        state.collapsed = false;
        state.selected = Some(focus);
    } else if !state.selected.as_ref().is_some_and(|s| live.contains(s)) {
        state.selected = live.first().cloned();
    }
    true
}

/// The rail's width this frame (0 when it does not show): wide enough for its widest table,
/// between [`RAIL_WIDTH`] and [`RAIL_MAX_WIDTH`].
pub fn width(shows: bool, state: &RailState) -> f32 {
    match (shows, state.collapsed) {
        (false, _) => 0.0,
        (true, true) => COLLAPSED_WIDTH,
        (true, false) => state.wanted.clamp(RAIL_WIDTH, RAIL_MAX_WIDTH),
    }
}

/// Column widths for a table with these natural widths in `room` points: all natural when they
/// fit; otherwise the narrow columns keep theirs and the wide ones share what is left equally
/// (their cells wrap), never under 24 points.
pub fn column_widths(natural: &[f32], room: f32) -> Vec<f32> {
    let gaps = TABLE_GAP * natural.len().saturating_sub(1) as f32;
    let room = (room - gaps).max(0.0);
    if natural.iter().sum::<f32>() <= room {
        return natural.to_vec();
    }
    let mut order: Vec<usize> = (0..natural.len()).collect();
    order.sort_by(|a, b| natural[*a].total_cmp(&natural[*b]));
    let mut widths = natural.to_vec();
    let mut left = room;
    for (k, &i) in order.iter().enumerate() {
        let share = left / (order.len() - k) as f32;
        if natural[i] <= share {
            left -= natural[i];
        } else {
            for &j in &order[k..] {
                widths[j] = share.max(24.0);
            }
            break;
        }
    }
    widths
}

/// The natural widths of a table's columns: the widest header or cell of each.
fn natural_columns(ui: &Ui, widget: &Widget, jobs: &[LayoutJob]) -> Vec<f32> {
    let p = &widget.props;
    let columns = p.columns.len().max(p.rows.iter().map(Vec::len).max().unwrap_or(0));
    let mut widths = vec![0.0f32; columns];
    let measure = |job: &LayoutJob| {
        let mut job = job.clone();
        job.wrap.max_width = f32::INFINITY;
        ui.fonts_mut(|f| f.layout_job(job)).size().x
    };
    for (c, job) in jobs[1..1 + p.columns.len()].iter().enumerate() {
        widths[c] = widths[c].max(measure(job));
    }
    let mut cells = jobs[1 + p.columns.len()..].iter();
    for row in &p.rows {
        for (c, job) in cells.by_ref().take(row.len()).enumerate() {
            if let Some(w) = widths.get_mut(c) {
                *w = w.max(measure(job));
            }
        }
    }
    widths.iter().map(|w| w.ceil()).collect()
}

/// A table's natural width (cached by the widget's revision).
fn table_width(ui: &Ui, state: &mut RailState, panel: &Panel, widget: &Widget, theme: &Theme) -> f32 {
    let k = key(panel, &widget.id);
    if let Some((revision, width)) = state.table_widths.get(&k)
        && *revision == widget.revision
    {
        return *width;
    }
    let jobs = jobs(state, panel, widget, theme);
    let natural = natural_columns(ui, widget, &jobs);
    let width = natural.iter().sum::<f32>() + TABLE_GAP * natural.len().saturating_sub(1) as f32;
    state.table_widths.insert(k, (widget.revision, width));
    width
}

/// Draw the rail into `rect`; returns the callbacks the person caused.
pub fn show(ui: &mut Ui, rect: Rect, host: &PanelHost, state: &mut RailState, theme: &Theme) -> Vec<PanelInput> {
    let mut inputs = Vec::new();
    let palette = palette_of(theme);
    if state.palette != Some(palette) {
        // A theme switch recolours coded text: layouts are built again from their text.
        state.jobs.clear();
        state.palette = Some(palette);
    }
    state.drawn.clear();
    state.shown.clear();
    state.headers.clear();
    // What the rail would like to be: its widest table (any panel's, so opening another panel
    // does not change the width) with the margins around it. Tables in a group sit 16 points in.
    let mut wanted = RAIL_WIDTH;
    for panel in host.rail() {
        for widget in panel.widgets.iter().filter(|w| w.kind == WidgetKind::Table) {
            let nested = if PanelHost::claimed(panel).contains_key(widget.id.as_str()) {
                18.0
            } else {
                0.0
            };
            wanted = wanted.max(table_width(ui, state, panel, widget, theme) + BODY_MARGIN + nested + 2.0);
        }
    }
    if (wanted - state.wanted).abs() > 0.5 {
        state.wanted = wanted;
        ui.ctx().request_repaint();
    }
    ui.painter().rect_filled(rect, 0.0, theme.shell);
    ui.painter()
        .vline(rect.left(), rect.y_range(), egui::Stroke::new(1.0, theme.border));
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), BAR_HEIGHT));
    ui.painter()
        .hline(bar.x_range(), bar.bottom() - 0.5, egui::Stroke::new(1.0, theme.border));
    let button = Rect::from_min_size(pos2(bar.right() - BAR_HEIGHT, bar.top()), vec2(BAR_HEIGHT, BAR_HEIGHT));
    let response = ui.interact(button, ui.id().with("rail-collapse"), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(button, 0.0, theme.hover_fill());
    }
    chevron(ui, button, state.collapsed, theme.text);
    let tip = t(if state.collapsed {
        S::ScriptPanelRailExpand
    } else {
        S::ScriptPanelRailCollapse
    });
    let response = response.on_hover_text(tip);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip));
    if response.clicked() {
        state.collapsed = !state.collapsed;
    }
    if state.collapsed {
        return inputs;
    }
    let body = Rect::from_min_max(pos2(rect.left() + 1.0, bar.bottom()), rect.max);
    let panels: Vec<&Panel> = host.rail().collect();
    let header_height = header_height(ui);
    let mut ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(body)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    ui.set_clip_rect(body.intersect(ui.clip_rect()));
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    for (i, panel) in panels.iter().enumerate() {
        state.shown.push(panel.title.clone());
        let open = state
            .selected
            .as_ref()
            .is_some_and(|(s, p)| s == &panel.script && p == &panel.id);
        let (header, response) = ui.allocate_exact_size(vec2(body.width(), header_height), Sense::click());
        state.headers.push(header);
        let painter = ui.painter();
        painter.rect_filled(header, 0.0, if open { theme.panel } else { theme.shell });
        painter.hline(
            header.x_range(),
            header.bottom() - 0.5,
            egui::Stroke::new(1.0, theme.border),
        );
        let title = crate::widgets::clipped(
            &ui,
            &panel.title,
            FontId::proportional(14.0),
            theme.text,
            header.width() - 20.0,
            1,
        );
        painter.galley(
            pos2(header.left() + 10.0, header.center().y - title.size().y / 2.0),
            title,
            theme.text,
        );
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        response
            .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, open, &panel.title));
        if response.clicked() {
            state.selected = Some((panel.script.clone(), panel.id.clone()));
        }
        if !open {
            continue;
        }
        // The open body: content height, capped so the headers below stay visible.
        let below = (panels.len() - i - 1) as f32 * header_height;
        let cap = (ui.available_height() - below).max(48.0);
        egui::Frame::new().fill(theme.panel).show(&mut ui, |ui| {
            ui.set_width(body.width());
            egui::ScrollArea::vertical()
                .id_salt(("script-panel", &panel.script, &panel.id))
                .max_height(cap)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                            panel_ui(ui, panel, state, theme, &mut inputs);
                        });
                });
        });
    }
    inputs
}

fn palette_of(theme: &Theme) -> [Color32; 17] {
    let mut p = [theme.text; 17];
    p[..16].copy_from_slice(&theme.ansi);
    p[16] = theme.muted;
    p
}

fn header_height(ui: &Ui) -> f32 {
    let row = ui.fonts_mut(|f| f.row_height(&FontId::proportional(14.0)));
    (row + 12.0).max(BAR_HEIGHT)
}

/// A horizontal chevron: `>` while open (tuck the rail away), `<` while folded.
fn chevron(ui: &Ui, rect: Rect, collapsed: bool, color: Color32) {
    let c = rect.center();
    let (dx, dy) = (3.0, 5.0);
    let points = if collapsed {
        vec![pos2(c.x + dx, c.y - dy), pos2(c.x - dx, c.y), pos2(c.x + dx, c.y + dy)]
    } else {
        vec![pos2(c.x - dx, c.y - dy), pos2(c.x + dx, c.y), pos2(c.x - dx, c.y + dy)]
    };
    ui.painter()
        .add(egui::Shape::line(points, egui::Stroke::new(1.6, color)));
}

/// One panel's widgets, in declaration order; widgets a group names are drawn inside it.
pub fn panel_ui(ui: &mut Ui, panel: &Panel, state: &mut RailState, theme: &Theme, inputs: &mut Vec<PanelInput>) {
    if panel.widgets.is_empty() {
        ui.add_space(6.0);
        ui.add(egui::Label::new(RichText::new(t(S::ScriptPanelEmpty)).size(12.0).color(theme.muted)).wrap());
        return;
    }
    let claimed = PanelHost::claimed(panel);
    for widget in panel.widgets.iter().filter(|w| !claimed.contains_key(w.id.as_str())) {
        widget_ui(ui, panel, widget, &claimed, state, theme, inputs);
    }
}

/// The text layouts of a widget, built when its revision moved.
fn jobs(state: &mut RailState, panel: &Panel, widget: &Widget, theme: &Theme) -> Vec<LayoutJob> {
    let k = key(panel, &widget.id);
    if let Some((revision, jobs)) = state.jobs.get(&k)
        && *revision == widget.revision
    {
        return jobs.clone();
    }
    state.layouts_built += 1;
    let p = &widget.props;
    let mono = |size: f32| FontId::monospace(size);
    let id_or = |s: &Option<String>| s.clone().unwrap_or_else(|| widget.id.clone());
    let jobs = match widget.kind {
        WidgetKind::Gauge => vec![mud_job(&id_or(&p.label), mono(12.0), theme.text, theme)],
        WidgetKind::Label => vec![mud_job(p.text.as_deref().unwrap_or(""), mono(13.0), theme.text, theme)],
        WidgetKind::Text => vec![mud_job(p.text.as_deref().unwrap_or(""), mono(12.0), theme.muted, theme)],
        WidgetKind::Button => vec![mud_job(&id_or(&p.label), mono(14.0), theme.text, theme)],
        WidgetKind::Toggle => vec![mud_job(&id_or(&p.label), mono(12.0), theme.text, theme)],
        WidgetKind::Group => vec![mud_job(p.title.as_deref().unwrap_or(""), mono(12.0), theme.text, theme)],
        WidgetKind::List => std::iter::once(mud_job(
            p.title.as_deref().unwrap_or(""),
            mono(11.0),
            theme.muted,
            theme,
        ))
        .chain(p.items.iter().map(|item| mud_job(item, mono(12.0), theme.text, theme)))
        .collect(),
        WidgetKind::Table => std::iter::once(mud_job(
            p.title.as_deref().unwrap_or(""),
            mono(11.0),
            theme.muted,
            theme,
        ))
        .chain(p.columns.iter().map(|c| mud_job(c, mono(11.0), theme.muted, theme)))
        .chain(
            p.rows
                .iter()
                .flatten()
                .map(|cell| mud_job(cell, mono(12.0), theme.text, theme)),
        )
        .collect(),
        WidgetKind::Input | WidgetKind::Separator => Vec::new(),
    };
    state.jobs.insert(k, (widget.revision, jobs.clone()));
    jobs
}

/// A block of wrapped coloured text.
fn text_block(ui: &mut Ui, mut job: LayoutJob, fallback: Color32) -> egui::Response {
    job.wrap.max_width = ui.available_width().max(10.0);
    let galley = ui.painter().layout_job(job);
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(rect.min, galley, fallback);
    response
}

#[allow(clippy::too_many_arguments)]
fn widget_ui(
    ui: &mut Ui,
    panel: &Panel,
    widget: &Widget,
    claimed: &HashMap<&str, &str>,
    state: &mut RailState,
    theme: &Theme,
    inputs: &mut Vec<PanelInput>,
) {
    let jobs = jobs(state, panel, widget, theme);
    let p = &widget.props;
    let k = key(panel, &widget.id);
    let input = |event: PanelEvent, text: Option<String>| PanelInput {
        script: panel.script.clone(),
        panel: panel.id.clone(),
        widget: widget.id.clone(),
        event,
        text,
    };
    let width = ui.available_width();
    let rect = match widget.kind {
        WidgetKind::Gauge => {
            let (rect, _) = ui.allocate_exact_size(vec2(width, GAUGE_HEIGHT), Sense::hover());
            let label = mudcolor::strip(p.label.as_deref().unwrap_or(&widget.id));
            let values = measure(p.number, p.maximum);
            let color = p.warned().then(|| crate::vitals_view::color_for("health")).flatten();
            crate::vitals_view::gauge_ui(
                ui,
                rect,
                &format!("{}:{}", panel.id, widget.id),
                jobs[0].clone(),
                (&values, FontId::monospace(12.0)),
                p.percentage(),
                color,
                theme,
                &format!("{label}: {values}"),
            );
            rect
        }
        WidgetKind::Label | WidgetKind::Text => {
            let fallback = if widget.kind == WidgetKind::Label {
                theme.text
            } else {
                theme.muted
            };
            text_block(ui, jobs[0].clone(), fallback).rect
        }
        WidgetKind::Button => {
            // Full width, the label centred (C# `HorizontalContentAlignment.Center`).
            let mut job = jobs[0].clone();
            job.wrap = egui::text::TextWrapping::truncate_at_width(width - 16.0);
            let galley = ui.painter().layout_job(job);
            let height = (galley.size().y + 12.0).max(30.0);
            let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
            // The C# Fluent button: a grey plate without a border, darker under the pointer.
            let share = if response.is_pointer_button_down_on() {
                0.32
            } else if response.hovered() {
                0.25
            } else {
                0.18
            };
            ui.painter()
                .rect_filled(rect, 3.0, crate::theme::mix(theme.panel, theme.text, share));
            ui.painter()
                .galley(rect.center() - galley.size() / 2.0, galley, theme.text);
            let plain = mudcolor::strip(p.label.as_deref().unwrap_or(&widget.id));
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &plain));
            if response.clicked() {
                inputs.push(input(PanelEvent::Click, None));
            }
            response.rect
        }
        WidgetKind::Toggle => {
            let entry = state.toggles.entry(k.clone()).or_insert((widget.revision, p.on));
            if entry.0 != widget.revision {
                *entry = (widget.revision, p.on);
            }
            let mut on = entry.1;
            let response = ui.add(egui::Checkbox::new(&mut on, jobs[0].clone()));
            if response.changed() {
                entry.1 = on;
                inputs.push(input(PanelEvent::Change(on), None));
            }
            response.rect
        }
        WidgetKind::Input => {
            let entry = state.inputs.entry(k.clone()).or_insert((0, String::new()));
            if entry.0 != widget.revision {
                if let Some(value) = &p.value {
                    entry.1 = value.clone();
                }
                entry.0 = widget.revision;
            }
            let response = ui.add(
                egui::TextEdit::singleline(&mut entry.1)
                    .id(ui.id().with(("panel-input", &k)))
                    .hint_text(p.placeholder.clone().unwrap_or_default())
                    .font(FontId::monospace(12.0))
                    .margin(egui::Margin::symmetric(6, 5))
                    .desired_width(width),
            );
            crate::a11y::label(&response, &mudcolor::strip(p.label.as_deref().unwrap_or(&widget.id)));
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let text = std::mem::take(&mut entry.1);
                inputs.push(input(PanelEvent::Submit, Some(text)));
                response.request_focus();
            }
            response.rect
        }
        WidgetKind::Separator => {
            let (rect, _) = ui.allocate_exact_size(vec2(width, 5.0), Sense::hover());
            ui.painter()
                .hline(rect.x_range(), rect.center().y, egui::Stroke::new(1.0, theme.border));
            rect
        }
        WidgetKind::List => {
            let top = ui.cursor().min;
            if p.title.as_deref().is_some_and(|t| !t.is_empty()) {
                text_block(ui, jobs[0].clone(), theme.muted);
            }
            let selected = state.lists.get(&k).filter(|s| p.items.contains(s)).cloned();
            let mut chosen = None;
            egui::ScrollArea::vertical()
                .id_salt(("panel-list", &k))
                .max_height(220.0)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (item, job) in p.items.iter().zip(&jobs[1..]) {
                        let is = selected.as_deref() == Some(item.as_str());
                        let mut job = job.clone();
                        job.wrap.max_width = ui.available_width() - 8.0;
                        let galley = ui.painter().layout_job(job);
                        let (rect, response) =
                            ui.allocate_exact_size(vec2(ui.available_width(), galley.size().y + 6.0), Sense::click());
                        if is {
                            ui.painter().rect_filled(rect, 0.0, theme.selection_fill());
                        } else if response.hovered() {
                            ui.painter().rect_filled(rect, 0.0, theme.hover_fill());
                        }
                        ui.painter()
                            .galley(pos2(rect.left() + 4.0, rect.top() + 3.0), galley, theme.text);
                        let plain = mudcolor::strip(item);
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, is, &plain)
                        });
                        if response.clicked() && !is {
                            chosen = Some(item.clone());
                        }
                    }
                });
            if let Some(item) = chosen {
                state.lists.insert(k.clone(), item.clone());
                inputs.push(input(PanelEvent::Select, Some(item)));
            }
            Rect::from_min_max(top, pos2(top.x + width, ui.cursor().min.y))
        }
        WidgetKind::Table => {
            let top = ui.cursor().min;
            if p.title.as_deref().is_some_and(|t| !t.is_empty()) {
                text_block(ui, jobs[0].clone(), theme.muted);
            }
            let natural = natural_columns(ui, widget, &jobs);
            if !natural.is_empty() {
                let widths = column_widths(&natural, width);
                let header_count = p.columns.len();
                let mut cells = jobs[1 + header_count..].iter();
                let mut lines: Vec<Vec<&LayoutJob>> = Vec::new();
                if header_count > 0 {
                    lines.push(jobs[1..1 + header_count].iter().collect());
                }
                for row in &p.rows {
                    let mut line = Vec::new();
                    for _ in 0..row.len() {
                        if let Some(job) = cells.next() {
                            line.push(job);
                        }
                    }
                    lines.push(line);
                }
                let spacing = ui.spacing().item_spacing.y;
                ui.spacing_mut().item_spacing.y = 2.0;
                for line in lines {
                    // Each cell wraps at its column's width; the row is as tall as its tallest.
                    let galleys: Vec<_> = line
                        .iter()
                        .take(widths.len())
                        .enumerate()
                        .map(|(c, job)| {
                            let mut job = (*job).clone();
                            job.wrap.max_width = widths[c].max(10.0);
                            ui.painter().layout_job(job)
                        })
                        .collect();
                    let height = galleys.iter().map(|g| g.size().y).fold(0.0, f32::max);
                    let (rect, _) = ui.allocate_exact_size(vec2(width, height + 2.0), Sense::hover());
                    let mut x = rect.left();
                    for (c, galley) in galleys.into_iter().enumerate() {
                        ui.painter().galley(pos2(x, rect.top()), galley, theme.text);
                        x += widths[c] + TABLE_GAP;
                    }
                }
                ui.spacing_mut().item_spacing.y = spacing;
            }
            Rect::from_min_max(top, pos2(top.x + width, ui.cursor().min.y))
        }
        WidgetKind::Group => {
            let response = egui::Frame::new()
                .stroke(egui::Stroke::new(1.0, theme.border))
                .corner_radius(4)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if p.title.as_deref().is_some_and(|t| !t.is_empty()) {
                        text_block(ui, jobs[0].clone(), theme.text);
                    }
                    for child in panel
                        .widgets
                        .iter()
                        .filter(|w| claimed.get(w.id.as_str()) == Some(&widget.id.as_str()))
                    {
                        widget_ui(ui, panel, child, claimed, state, theme, inputs);
                    }
                })
                .response;
            response.rect
        }
    };
    state.drawn.push((panel.id.clone(), widget.id.clone(), rect));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use wandur_core::scripting::panels::PanelAction;

    fn host(actions: &[&str]) -> PanelHost {
        let mut host = PanelHost::default();
        for a in actions {
            host.apply("s", PanelAction::parse(a).unwrap(), Instant::now());
        }
        host
    }

    fn frame(
        ctx: &egui::Context,
        host: &mut PanelHost,
        state: &mut RailState,
        theme: &Theme,
        events: Vec<egui::Event>,
    ) -> Vec<PanelInput> {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut out = Vec::new();
        let mut output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let shows = sync(host, state);
                if shows {
                    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(width(shows, state), 600.0));
                    out = show(ui, rect, host, state, theme);
                }
            });
        });
        output.textures_delta.clear();
        out
    }

    fn click(at: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    const SHIP: &[&str] = &[
        r#"{"panel":"ship","action":"create","title":"Ship","dock":"right"}"#,
        r#"{"panel":"ship","action":"widget","widget":"hull","kind":"gauge","props":{"label":"&CHull&D","value":10,"max":100}}"#,
        r#"{"panel":"ship","action":"widget","widget":"system","kind":"label","props":{"text":"&RRed&D plain"}}"#,
        r#"{"panel":"ship","action":"widget","widget":"flee","kind":"button","props":{"label":"Flee"}}"#,
        r#"{"panel":"ship","action":"widget","widget":"auto","kind":"toggle","props":{"label":"Auto","value":false}}"#,
        r#"{"panel":"ship","action":"widget","widget":"say","kind":"input","props":{"placeholder":"Say..."}}"#,
        r#"{"panel":"ship","action":"widget","widget":"crew","kind":"list","props":{"title":"Crew","items":["Ann","Bo"]}}"#,
        r#"{"panel":"ship","action":"widget","widget":"skills","kind":"table","props":{"columns":["Skill","Level"],"rows":[["Piloting","12"]]}}"#,
        r#"{"panel":"ship","action":"widget","widget":"s1","kind":"separator"}"#,
        r#"{"panel":"cargo","action":"create","title":"Cargo","dock":"left"}"#,
        r#"{"panel":"cargo","action":"widget","widget":"hold","kind":"label","props":{"text":"Empty hold"}}"#,
        r#"{"panel":"vitals","action":"create","title":"Vitals","dock":"bars"}"#,
        r#"{"panel":"vitals","action":"widget","widget":"force","kind":"gauge","props":{"label":"Force","value":40,"max":80}}"#,
    ];

    /// C# `PanelsGetADockOfTheirOwn...` and `ADeclaredPanelDocks...` on the rail: left and right
    /// panels share it in declaration order, bars stay out, the first is open; every widget kind
    /// is drawn; button, toggle, list and input callbacks come out; hide, focus and close follow.
    #[test]
    fn the_rail_shows_panels_as_an_accordion_and_widgets_call_back() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::preset("Hull");
        let mut host = host(SHIP);
        let mut state = RailState::default();
        frame(&ctx, &mut host, &mut state, &theme, vec![]);
        frame(&ctx, &mut host, &mut state, &theme, vec![]);
        assert_eq!(state.shown, ["Ship", "Cargo"], "bars panels are not in the rail");
        assert_eq!(state.selected, Some(("s".into(), "ship".into())));
        let drawn: Vec<&str> = state.drawn.iter().map(|(_, w, _)| w.as_str()).collect();
        assert_eq!(drawn, ["hull", "system", "flee", "auto", "say", "crew", "skills", "s1"]);
        let at = |state: &RailState, id: &str| state.drawn.iter().find(|(_, w, _)| w == id).unwrap().2;
        // Gauges are vitals cards.
        assert_eq!(at(&state, "hull").height(), GAUGE_HEIGHT);

        let flee = at(&state, "flee").center();
        let out = frame(&ctx, &mut host, &mut state, &theme, click(flee));
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].widget.as_str(), out[0].event), ("flee", PanelEvent::Click));
        let toggle = at(&state, "auto");
        let out = frame(
            &ctx,
            &mut host,
            &mut state,
            &theme,
            click(pos2(toggle.left() + 8.0, toggle.center().y)),
        );
        assert_eq!(out[0].event, PanelEvent::Change(true));
        let crew = at(&state, "crew");
        let out = frame(
            &ctx,
            &mut host,
            &mut state,
            &theme,
            click(pos2(crew.center().x, crew.bottom() - 8.0)),
        );
        assert_eq!((out[0].event, out[0].text.as_deref()), (PanelEvent::Select, Some("Bo")));

        // Type into the input and press Enter: submitted and cleared.
        let say = at(&state, "say");
        frame(&ctx, &mut host, &mut state, &theme, click(say.center()));
        let mut events = vec![egui::Event::Text("hello".into())];
        frame(&ctx, &mut host, &mut state, &theme, std::mem::take(&mut events));
        let enter = vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }];
        let out = frame(&ctx, &mut host, &mut state, &theme, enter);
        assert_eq!(
            (out[0].event, out[0].text.as_deref()),
            (PanelEvent::Submit, Some("hello"))
        );

        // A click on the second header opens it, and the first folds to its header.
        let header = state.headers[1];
        frame(&ctx, &mut host, &mut state, &theme, click(header.center()));
        assert_eq!(state.selected, Some(("s".into(), "cargo".into())));
        frame(&ctx, &mut host, &mut state, &theme, vec![]);
        let drawn: Vec<&str> = state.drawn.iter().map(|(_, w, _)| w.as_str()).collect();
        assert_eq!(drawn, ["hold"]);

        // focus() opens a panel; hiding the open one opens the first; the collapse control folds.
        let now = Instant::now();
        host.apply(
            "s",
            PanelAction::parse(r#"{"panel":"ship","action":"focus"}"#).unwrap(),
            now,
        );
        frame(&ctx, &mut host, &mut state, &theme, vec![]);
        assert_eq!(state.selected, Some(("s".into(), "ship".into())));
        host.apply(
            "s",
            PanelAction::parse(r#"{"panel":"ship","action":"hide"}"#).unwrap(),
            now,
        );
        frame(&ctx, &mut host, &mut state, &theme, vec![]);
        assert_eq!(state.shown, ["Cargo"]);
        assert_eq!(state.selected, Some(("s".into(), "cargo".into())));
        frame(
            &ctx,
            &mut host,
            &mut state,
            &theme,
            click(pos2(RAIL_WIDTH - 14.0, 14.0)),
        );
        assert!(state.collapsed);
        assert_eq!(width(true, &state), COLLAPSED_WIDTH);
        host.apply(
            "s",
            PanelAction::parse(r#"{"panel":"cargo","action":"close"}"#).unwrap(),
            now,
        );
        assert!(!sync(&mut host, &mut state), "the rail hides with its last panel");
    }

    /// Unchanged values cost no new layout: the coded text of a widget is parsed once per
    /// revision, however many frames are drawn or identical updates arrive.
    #[test]
    fn table_columns_keep_their_width_or_share_what_is_left() {
        // They fit: every column as wide as its widest cell.
        assert_eq!(column_widths(&[120.0, 30.0, 30.0], 300.0), [120.0, 30.0, 30.0]);
        // They do not: the narrow columns keep theirs, the wide one takes the rest and wraps.
        let w = column_widths(&[400.0, 30.0, 30.0], 300.0);
        assert_eq!(&w[1..], [30.0, 30.0]);
        assert_eq!(w[0], 300.0 - 2.0 * TABLE_GAP - 60.0);
        // Two wide columns share equally.
        let w = column_widths(&[400.0, 300.0, 20.0], 300.0);
        assert_eq!(w[2], 20.0);
        assert_eq!(w[0], w[1]);
        assert!((w.iter().sum::<f32>() + 2.0 * TABLE_GAP - 300.0).abs() < 0.01);
    }

    /// The rail widens to its widest table (labels in one column, values right after it, nothing
    /// cut short), up to its cap; a table wider than that wraps its cells instead.
    #[test]
    fn the_rail_fits_its_widest_table_and_wraps_past_the_cap() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::preset("Hull");
        let run = |host: &mut PanelHost, state: &mut RailState| {
            for _ in 0..3 {
                let input = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                    ..Default::default()
                };
                let mut output = ctx.run_ui(input, |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let shows = sync(host, state);
                        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(width(shows, state), 600.0));
                        show(ui, rect, host, state, &theme);
                    });
                });
                output.textures_delta.clear();
            }
        };
        let skills = r#"{"panel":"skills","action":"widget","widget":"t","kind":"table","props":{"columns":["Skill","Level","Adept"],"rows":[["lightsaber combat","87%","95%"],["droid construction and repair","33%","85%"],["hide","70%","70%"]]}}"#;
        let mut first = host(&[
            r#"{"panel":"skills","action":"create","title":"Skills","dock":"right"}"#,
            skills,
        ]);
        let mut state = RailState::default();
        run(&mut first, &mut state);
        let rail = width(true, &state);
        assert!(rail > RAIL_WIDTH && rail < RAIL_MAX_WIDTH, "{rail}");
        let table = state.drawn.iter().find(|(_, w, _)| w == "t").unwrap().2;
        // Four rows (headers and three), one line each: nothing wrapped, nothing cut.
        let line = ctx.fonts_mut(|f| f.row_height(&FontId::monospace(12.0)));
        assert!(table.height() < 4.0 * (line + 4.0), "{}", table.height());
        assert!(table.width() + BODY_MARGIN <= rail);

        let wide = r#"{"panel":"skills","action":"widget","widget":"t","kind":"table","props":{"columns":["Skill","Notes"],"rows":[["droid construction and repair","learned from the old mechanic at the Coruscant spaceport, practise daily"]]}}"#;
        let mut second = host(&[
            r#"{"panel":"skills","action":"create","title":"Skills","dock":"right"}"#,
            wide,
        ]);
        let mut state = RailState::default();
        run(&mut second, &mut state);
        assert_eq!(width(true, &state), RAIL_MAX_WIDTH);
        let table = state.drawn.iter().find(|(_, w, _)| w == "t").unwrap().2;
        assert!(table.height() > 3.0 * line, "the long cell wraps: {}", table.height());
    }

    #[test]
    fn unchanged_panel_values_build_no_new_layouts() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::preset("Hull");
        let mut host = host(SHIP);
        let mut state = RailState::default();
        frame(&ctx, &mut host, &mut state, &theme, vec![]);
        let built = state.layouts_built;
        assert!(built >= 8);
        let same = r#"{"panel":"ship","action":"widget","widget":"hull","kind":"gauge","props":{"label":"&CHull&D","value":10,"max":100}}"#;
        for _ in 0..30 {
            host.apply("s", PanelAction::parse(same).unwrap(), Instant::now());
            frame(&ctx, &mut host, &mut state, &theme, vec![]);
        }
        assert_eq!(state.layouts_built, built);
        let changed = r#"{"panel":"ship","action":"widget","widget":"hull","kind":"gauge","props":{"label":"&CHull&D","value":11,"max":100}}"#;
        host.apply("s", PanelAction::parse(changed).unwrap(), Instant::now());
        frame(&ctx, &mut host, &mut state, &theme, vec![]);
        assert_eq!(state.layouts_built, built + 1, "only the changed widget");
        // A theme switch recolours: everything is laid out again once.
        frame(&ctx, &mut host, &mut state, &Theme::preset("Slate"), vec![]);
        assert!(state.layouts_built > built + 1);
    }

    #[test]
    fn coded_text_takes_the_theme_palette() {
        let theme = Theme::preset("Hull");
        let job = mud_job("&RRed&D plain", FontId::monospace(13.0), theme.text, &theme);
        assert_eq!(job.text, "Red plain");
        assert_eq!(job.sections.len(), 2);
        assert_eq!(job.sections[0].format.color, theme.ansi[9]);
        assert_eq!(job.sections[1].format.color, theme.text);
        let job = mud_job("&228x^bstowed", FontId::monospace(13.0), theme.text, &theme);
        assert_eq!(job.sections[0].format.color, Color32::from_rgb(0xFF, 0xFF, 0x87));
        assert_eq!(job.sections[1].format.background, theme.ansi[4]);
    }
}
