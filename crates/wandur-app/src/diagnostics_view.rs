//! The session's Diagnostics page (the C# `ProtocolDiagnosticsView` and `ConsoleView`): the
//! message count with Follow latest and Clear above four tabs. Messages: a filter box, a chip per
//! kind with its count, the list and the selected message's data. Observed fields: the
//! values-free inventory. Console: the raw stream with control characters made visible, private
//! stretches as `[private]`, Pause, Wrap lines, Copy and Clear. Server details: what the world
//! said about itself through MSSP. The model lives in `wandur_core::diagnostics`; this draws it.

use std::collections::VecDeque;

use egui::text::LayoutJob;
use egui::{Color32, FontId, Rect, RichText, TextFormat, Ui};
use wandur_core::diagnostics::SessionProtocol;
use wandur_core::diagnostics::console::{self, ConsoleLog};
use wandur_core::diagnostics::messages::Messages;
use wandur_core::l10n::{S, t, tf};

use crate::theme::Theme;
use crate::widgets::{Icon, paint_icon};

/// The Diagnostics tabs, in the C# order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiagTab {
    #[default]
    Messages,
    Observed,
    Console,
    Server,
}

impl DiagTab {
    pub const ALL: [DiagTab; 4] = [DiagTab::Messages, DiagTab::Observed, DiagTab::Console, DiagTab::Server];

    fn label(self) -> &'static str {
        t(match self {
            DiagTab::Messages => S::DiagnosticsMessages,
            DiagTab::Observed => S::DiagnosticsObservedFields,
            DiagTab::Console => S::ConsoleTab,
            DiagTab::Server => S::DiagnosticsServerDetails,
        })
    }

    /// Scene and command line names.
    pub fn parse(name: &str) -> Option<DiagTab> {
        match name {
            "messages" => Some(DiagTab::Messages),
            "observed" => Some(DiagTab::Observed),
            "console" => Some(DiagTab::Console),
            "server" => Some(DiagTab::Server),
            _ => None,
        }
    }
}

/// The console's display text, brought up to date incrementally: entries the ring evicted leave
/// the top, new ones are appended, so a long session never renders the whole buffer again.
#[derive(Debug, Default)]
struct ConsoleText {
    revision: Option<u64>,
    text: String,
    /// Sequence and byte length (with its newline) of each rendered entry.
    shown: VecDeque<(u64, usize)>,
    /// `text` cut into display rows for a column count (wrapped) or not (0).
    rows: Vec<(usize, usize)>,
    rows_for: Option<(usize, usize)>,
}

impl ConsoleText {
    fn refresh(&mut self, log: &ConsoleLog, offset: i64) {
        if self.revision == Some(log.revision()) {
            return;
        }
        self.revision = Some(log.revision());
        let first = log.entries().next().map(|e| e.sequence);
        let mut evicted = 0;
        while let Some(&(seq, len)) = self.shown.front() {
            if first.is_some_and(|f| seq < f) {
                evicted += len;
                self.shown.pop_front();
            } else {
                break;
            }
        }
        if evicted > 0 {
            self.text.drain(..evicted.min(self.text.len()));
        }
        if first.is_none() || self.shown.front().is_some_and(|&(s, _)| Some(s) != first) {
            self.text.clear();
            self.shown.clear();
        }
        let last = self.shown.back().map(|&(s, _)| s);
        for entry in log.entries() {
            if last.is_some_and(|l| entry.sequence <= l) {
                continue;
            }
            let line = entry.render(offset);
            self.text.push_str(&line);
            self.text.push('\n');
            self.shown.push_back((entry.sequence, line.len() + 1));
        }
        self.rows_for = None;
    }

    /// Byte ranges of the display rows: lines, cut every `columns` characters when wrapping.
    fn rows(&mut self, columns: usize, wrap: bool) -> &[(usize, usize)] {
        let key = (if wrap { columns.max(1) } else { 0 }, self.text.len());
        if self.rows_for != Some(key) {
            self.rows.clear();
            let mut start = 0;
            for line in self.text.split_inclusive('\n') {
                let body = line.trim_end_matches('\n');
                if wrap && body.chars().count() > key.0 {
                    let mut from = start;
                    for (count, (i, _)) in body.char_indices().enumerate() {
                        if count > 0 && count % key.0 == 0 {
                            self.rows.push((from, start + i));
                            from = start + i;
                        }
                    }
                    self.rows.push((from, start + body.len()));
                } else {
                    self.rows.push((start, start + body.len()));
                }
                start += line.len();
            }
            self.rows_for = Some(key);
        }
        &self.rows
    }
}

/// What the Diagnostics page remembers between frames.
#[derive(Debug)]
pub struct DiagnosticsViewState {
    pub tab: DiagTab,
    filter: String,
    /// The list's share of the panes.
    split: f32,
    pub console_paused: bool,
    pub console_wrap: bool,
    console: ConsoleText,
    /// The console shows older text than the log has (paused or off screen).
    pub console_stale: bool,
    schema: Option<(u64, LayoutJob)>,
    detail: Option<(u64, Option<u64>, LayoutJob)>,
    followed: Option<u64>,
    /// Show the console from its first line on the next frame (scenes).
    pub console_to_top: bool,
    /// Where each kind chip was drawn in the last frame (tests).
    pub chip_rects: Vec<(String, Rect)>,
    /// Message rows drawn in the last frame.
    pub rows_drawn: usize,
}

impl Default for DiagnosticsViewState {
    fn default() -> Self {
        Self {
            tab: DiagTab::Messages,
            filter: String::new(),
            split: 0.4,
            console_paused: false,
            console_wrap: true,
            console: ConsoleText::default(),
            console_stale: true,
            schema: None,
            detail: None,
            followed: None,
            console_to_top: false,
            chip_rects: Vec::new(),
            rows_drawn: 0,
        }
    }
}

impl DiagnosticsViewState {
    /// The console's current display text (tests).
    pub fn console_text(&self) -> &str {
        &self.console.text
    }

    /// The console was not on screen: note that it is behind.
    pub fn console_hidden(&mut self, log: &ConsoleLog) {
        if self.console.revision != Some(log.revision()) {
            self.console_stale = true;
        }
    }
}

/// The editor background the C# diagnostics bodies use (a paper white in light themes).
pub fn editor_fill(theme: &Theme) -> Color32 {
    if theme.light {
        crate::theme::mix(theme.panel, Color32::WHITE, 0.72)
    } else {
        crate::theme::mix(theme.panel, Color32::BLACK, 0.35)
    }
}

/// JSON colours of the C# `DiagnosticsBodyEditor` (paper and dark).
struct Palette {
    field: Color32,
    string: Color32,
    number: Color32,
    literal: Color32,
    punctuation: Color32,
}

fn palette(theme: &Theme) -> Palette {
    let c = |hex: u32| Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
    if theme.light {
        Palette {
            field: c(0x8250DF),
            string: c(0x0A3069),
            number: c(0x0550AE),
            literal: c(0x953800),
            punctuation: c(0x1F2328),
        }
    } else {
        Palette {
            field: c(0xD2A8FF),
            string: c(0xA5D6FF),
            number: c(0x79C0FF),
            literal: c(0xFFA657),
            punctuation: c(0xC9D1D9),
        }
    }
}

const BODY_SIZE: f32 = 13.0;

/// JSON (or anything) coloured as the C# editor colours JSON.
pub fn json_job(text: &str, theme: &Theme, wrap: Option<f32>) -> LayoutJob {
    let p = palette(theme);
    let font = FontId::monospace(BODY_SIZE);
    let mut job = LayoutJob::default();
    if let Some(width) = wrap {
        job.wrap.max_width = width;
    }
    let push = |job: &mut LayoutJob, s: &str, color: Color32| {
        job.append(s, 0.0, TextFormat::simple(font.clone(), color));
    };
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut plain_from = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let token_end;
        let color;
        if b == b'"' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'"' && bytes[j] != b'\n' {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            token_end = (j + 1).min(bytes.len());
            let mut k = token_end;
            while k < bytes.len() && bytes[k] == b' ' {
                k += 1;
            }
            color = if bytes.get(k) == Some(&b':') { p.field } else { p.string };
        } else if b == b'-' || b.is_ascii_digit() {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j].is_ascii_digit() || matches!(bytes[j], b'.' | b'e' | b'E' | b'+' | b'-'))
            {
                j += 1;
            }
            token_end = j;
            color = p.number;
        } else if text[i..].starts_with("true") || text[i..].starts_with("false") || text[i..].starts_with("null") {
            token_end = i + if text[i..].starts_with("false") { 5 } else { 4 };
            color = p.literal;
        } else {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        if plain_from < i {
            push(&mut job, &text[plain_from..i], p.punctuation);
        }
        let end = token_end.min(text.len());
        // Never cut inside a character (a stray byte count at a multibyte boundary).
        let end = (end..=text.len())
            .find(|&e| text.is_char_boundary(e))
            .unwrap_or(text.len());
        push(&mut job, &text[i..end], color);
        i = end;
        plain_from = end;
    }
    if plain_from < text.len() {
        push(&mut job, &text[plain_from..], p.punctuation);
    }
    job
}

/// Draw the Diagnostics page into the space given.
pub fn show(ui: &mut Ui, protocol: &mut SessionProtocol, state: &mut DiagnosticsViewState, theme: &Theme) {
    let rect = ui.available_rect_before_wrap();
    ui.painter().rect_filled(rect, 0.0, theme.panel);
    ui.spacing_mut().item_spacing.y = 4.0;
    if state.tab != DiagTab::Console {
        top_bar(ui, &mut protocol.messages, theme);
    } else {
        ui.add_space(6.0);
    }
    tab_strip(ui, state, theme);
    if state.tab != DiagTab::Console {
        state.console_hidden(&protocol.console);
    }
    match state.tab {
        DiagTab::Messages => messages_tab(ui, &mut protocol.messages, state, theme),
        DiagTab::Observed => observed_tab(ui, &mut protocol.messages, state, theme),
        DiagTab::Console => console_tab(ui, &mut protocol.console, state, theme),
        DiagTab::Server => server_tab(ui, &protocol.messages, theme),
    }
}

/// "14 messages · latest 200 retained", Follow latest and Clear.
fn top_bar(ui: &mut Ui, messages: &mut Messages, theme: &Theme) {
    let width = ui.available_width();
    ui.allocate_ui_with_layout(
        egui::vec2(width, 40.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_size(egui::vec2(width, 40.0));
            ui.add_space(12.0);
            ui.label(RichText::new(messages.count_label()).size(13.0).color(theme.muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(12.0);
                if icon_button(ui, Icon::Trash, t(S::DiagnosticsClear), theme).clicked() {
                    messages.clear();
                }
                ui.add_space(8.0);
                let mut follow = messages.follow();
                if ui
                    .checkbox(&mut follow, RichText::new(t(S::DiagnosticsFollow)).size(13.0))
                    .changed()
                {
                    messages.set_follow(follow);
                }
            });
        },
    );
}

/// A 32 point square button with an icon, its name for hover and accessibility.
fn icon_button(ui: &mut Ui, icon: Icon, label: &str, theme: &Theme) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, theme.hover_fill());
    }
    paint_icon(ui, icon, rect, theme.text);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response.on_hover_text(label)
}

/// A text toggle (Pause, Wrap lines), pressed while on.
fn text_toggle(ui: &mut Ui, label: &str, on: bool, theme: &Theme) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), FontId::proportional(13.0), theme.text);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(galley.size().x + 20.0, 32.0), egui::Sense::click());
    if on {
        ui.painter().rect_filled(rect, 3.0, theme.selection_fill());
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 3.0, theme.hover_fill());
    }
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, theme.text);
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, label));
    response.on_hover_text(label)
}

fn tab_strip(ui: &mut Ui, state: &mut DiagnosticsViewState, theme: &Theme) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for tab in DiagTab::ALL {
            let selected = state.tab == tab;
            let color = if selected { theme.text } else { theme.muted };
            let galley = ui
                .painter()
                .layout_no_wrap(tab.label().to_string(), FontId::proportional(13.0), color);
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(galley.size().x + 24.0, 36.0), egui::Sense::click());
            if selected {
                ui.painter().hline(
                    rect.shrink2(egui::vec2(0.0, 0.0)).x_range(),
                    rect.bottom() - 1.0,
                    egui::Stroke::new(2.0, theme.accent),
                );
            } else if response.hovered() {
                ui.painter().rect_filled(rect, 3.0, theme.hover_fill());
            }
            ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, tab.label())
            });
            if response.clicked() {
                state.tab = tab;
            }
        }
    });
}

fn messages_tab(ui: &mut Ui, messages: &mut Messages, state: &mut DiagnosticsViewState, theme: &Theme) {
    state.chip_rects.clear();
    state.rows_drawn = 0;
    if messages.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.set_max_width(480.0);
            ui.label(RichText::new(t(S::DiagnosticsEmpty)).size(13.0).color(theme.muted));
        });
        return;
    }
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            if state.filter != messages.filter() {
                state.filter = messages.filter().to_string();
            }
            let edit = egui::TextEdit::singleline(&mut state.filter)
                .hint_text(t(S::DiagnosticsFilterPlaceholder))
                .font(FontId::proportional(13.0))
                .margin(egui::Margin::symmetric(10, 7))
                .desired_width(f32::INFINITY);
            let response = ui.add(edit);
            if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                state.filter.clear();
            }
            if state.filter != messages.filter() {
                let text = state.filter.clone();
                messages.set_filter(&text);
            }
            ui.add_space(4.0);
            chips(ui, messages, state, theme);
        });
    panes(ui, messages, state, theme);
}

fn chip(ui: &mut Ui, label: &str, on: bool, theme: &Theme) -> egui::Response {
    let font = FontId::proportional(13.0);
    let mut job = LayoutJob::simple_singleline(label.to_string(), font, theme.text);
    job.wrap = egui::text::TextWrapping::truncate_at_width(240.0);
    let galley = ui.painter().layout_job(job);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(galley.size().x + 22.0, 32.0), egui::Sense::click());
    let fill = if on {
        theme.selection_fill()
    } else if response.hovered() {
        theme.hover_fill()
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect(
        rect.shrink(1.0),
        8.0,
        fill,
        egui::Stroke::new(1.0, theme.border),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, theme.text);
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, label));
    response.on_hover_text(label)
}

fn chips(ui: &mut Ui, messages: &mut Messages, state: &mut DiagnosticsViewState, theme: &Theme) {
    let mut toggle = None;
    let mut all = false;
    let mut more = None;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        let all_button = chip(ui, t(S::DiagnosticsAllKinds), false, theme);
        all = all_button.clicked();
        let more_width = if messages.has_more_kinds() { 90.0 } else { 0.0 };
        let width = (ui.available_width() - more_width).max(60.0);
        ui.allocate_ui(egui::vec2(width, 72.0), |ui| {
            egui::ScrollArea::vertical()
                .max_height(72.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                        for kind in messages.chip_kinds() {
                            let response = chip(ui, &kind.label(), kind.active, theme);
                            state.chip_rects.push((kind.name.clone(), response.rect));
                            if response.clicked() {
                                toggle = Some(kind.name.clone());
                            }
                        }
                    });
                });
        });
        if messages.has_more_kinds()
            && chip(ui, &messages.more_kinds_label(), messages.kinds_expanded(), theme).clicked()
        {
            more = Some(!messages.kinds_expanded());
        }
    });
    if all {
        messages.clear_kinds();
    }
    if let Some(name) = toggle {
        messages.toggle_kind(&name);
    }
    if let Some(on) = more {
        messages.set_kinds_expanded(on);
    }
}

const ROW_HEIGHT: f32 = 48.0;

/// The list and the detail, side by side from 840 points, else one above the other, with a
/// divider that can be dragged.
fn panes(ui: &mut Ui, messages: &mut Messages, state: &mut DiagnosticsViewState, theme: &Theme) {
    let area = ui.available_rect_before_wrap().shrink2(egui::vec2(0.0, 0.0));
    let area = Rect::from_min_max(
        egui::pos2(area.left(), area.top()),
        egui::pos2(area.right() - 0.0, area.bottom()),
    );
    ui.allocate_rect(area, egui::Sense::hover());
    let wide = area.width() >= 840.0;
    let divider = 8.0;
    let (list, bar, detail) = if wide {
        let w = (area.width() - divider) * state.split;
        let list = Rect::from_min_size(area.min, egui::vec2(w, area.height()));
        let bar = Rect::from_min_size(egui::pos2(list.right(), area.top()), egui::vec2(divider, area.height()));
        (
            list,
            bar,
            Rect::from_min_max(egui::pos2(bar.right(), area.top()), area.max),
        )
    } else {
        let h = (area.height() - divider) * state.split;
        let list = Rect::from_min_size(area.min, egui::vec2(area.width(), h));
        let bar = Rect::from_min_size(
            egui::pos2(area.left(), list.bottom()),
            egui::vec2(area.width(), divider),
        );
        (
            list,
            bar,
            Rect::from_min_max(egui::pos2(area.left(), bar.bottom()), area.max),
        )
    };
    let drag = ui.interact(bar, ui.id().with("diag-divider"), egui::Sense::drag());
    crate::a11y::control(&drag, egui::accesskit::Role::Splitter, t(S::A11yResizePanels));
    ui.painter().rect_filled(bar, 0.0, theme.border);
    if drag.hovered() || drag.dragged() {
        ui.ctx().set_cursor_icon(if wide {
            egui::CursorIcon::ResizeHorizontal
        } else {
            egui::CursorIcon::ResizeVertical
        });
    }
    if let Some(pos) = drag.interact_pointer_pos().filter(|_| drag.dragged()) {
        state.split = if wide {
            (pos.x - area.left()) / area.width()
        } else {
            (pos.y - area.top()) / area.height()
        }
        .clamp(0.15, 0.85);
    }
    let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list.shrink2(egui::vec2(12.0, 0.0))));
    message_list(&mut list_ui, messages, state, theme);
    let mut detail_ui = ui.new_child(egui::UiBuilder::new().max_rect(detail));
    detail_pane(&mut detail_ui, messages, state, theme);
}

fn message_list(ui: &mut Ui, messages: &mut Messages, state: &mut DiagnosticsViewState, theme: &Theme) {
    let offset = crate::app::local_offset_seconds();
    // Rows sit edge to edge, so row i starts at i * ROW_HEIGHT.
    ui.spacing_mut().item_spacing.y = 0.0;
    let count = messages.visible_len();
    let selected = messages.selected_id();
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt("diag-list")
        .auto_shrink([false, false]);
    // A new selection (following, or set from outside) is scrolled into view.
    if selected != state.followed
        && let Some(index) = selected.and_then(|s| messages.visible_ids().iter().position(|&id| id == s))
    {
        let target = (index as f32 + 1.0) * ROW_HEIGHT - ui.available_height();
        scroll = scroll.vertical_scroll_offset(target.max(0.0));
    }
    // Settled once the selected row was drawn (the first frames may not know the full height).
    if selected.is_none() {
        state.followed = None;
    }
    let mut clicked = None;
    scroll.show_rows(ui, ROW_HEIGHT, count, |ui, range| {
        for index in range {
            let Some(&id) = messages.visible_ids().get(index) else {
                continue;
            };
            let Some(entry) = messages.entry(id) else { continue };
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), egui::Sense::click());
            if Some(id) == selected {
                state.followed = selected;
                ui.painter().rect_filled(rect, 3.0, theme.selection_fill());
            } else if response.hovered() {
                ui.painter().rect_filled(rect, 3.0, theme.hover_fill());
            }
            let name = entry.title().to_string();
            let meta = format!("{}  {}", console::clock(entry.received_at, offset), entry.protocol);
            let mut job = LayoutJob::simple_singleline(name.clone(), FontId::proportional(13.0), theme.text);
            job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 8.0);
            let galley = ui.painter().layout_job(job);
            ui.painter().galley(rect.min + egui::vec2(4.0, 6.0), galley, theme.text);
            ui.painter().text(
                rect.min + egui::vec2(4.0, 26.0),
                egui::Align2::LEFT_TOP,
                &meta,
                FontId::proportional(12.0),
                theme.muted,
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, Some(id) == selected, &name)
            });
            if response.clicked() {
                clicked = Some(id);
            }
            state.rows_drawn += 1;
        }
    });
    if let Some(id) = clicked {
        messages.select_by_person(id);
        state.followed = Some(id);
    }
}

fn detail_pane(ui: &mut Ui, messages: &Messages, state: &mut DiagnosticsViewState, theme: &Theme) {
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, 0.0, editor_fill(theme));
    let key = (messages.revision(), messages.selected_id());
    let width = rect.width() - 28.0;
    if state.detail.as_ref().map(|(r, s, j)| (*r, *s, j.wrap.max_width)) != Some((key.0, key.1, width)) {
        state.detail = Some((key.0, key.1, json_job(&messages.detail(), theme, Some(width))));
    }
    let job = state.detail.as_ref().map(|(_, _, j)| j.clone()).unwrap_or_default();
    egui::ScrollArea::vertical()
        .id_salt("diag-detail")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    ui.add(egui::Label::new(job).selectable(true));
                });
        });
}

fn help(ui: &mut Ui, text: &str, theme: &Theme) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(14, 6))
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(text).size(13.0).color(theme.muted)).wrap());
        });
}

fn observed_tab(ui: &mut Ui, messages: &mut Messages, state: &mut DiagnosticsViewState, theme: &Theme) {
    help(ui, t(S::DiagnosticsSchemaHelp), theme);
    let revision = messages.schema().revision();
    if state.schema.as_ref().map(|(r, _)| *r) != Some(revision) {
        let text = messages.schema_detail();
        state.schema = Some((revision, json_job(&text, theme, None)));
    }
    let job = state.schema.as_ref().map(|(_, j)| j.clone()).unwrap_or_default();
    editor(ui, "diag-schema", theme, false, |ui| {
        ui.add(egui::Label::new(job).selectable(true).extend());
    });
}

/// The paper-white body area with its scrollbars.
fn editor(ui: &mut Ui, id: &str, theme: &Theme, wrap: bool, add: impl FnOnce(&mut Ui)) {
    let rect = ui.available_rect_before_wrap();
    ui.painter().rect_filled(rect, 0.0, editor_fill(theme));
    let scroll = if wrap {
        egui::ScrollArea::vertical()
    } else {
        egui::ScrollArea::both()
    };
    scroll.id_salt(id).auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, add);
    });
}

fn server_tab(ui: &mut Ui, messages: &Messages, theme: &Theme) {
    help(ui, t(S::DiagnosticsServerDetailsHelp), theme);
    let job = details_job(messages.server_details(), theme, ui.available_width() - 28.0);
    editor(ui, "diag-server", theme, true, |ui| {
        ui.add(egui::Label::new(job).selectable(true));
    });
}

/// The Server details text, with web and mail addresses underlined as the C# editor shows them.
fn details_job(text: &str, theme: &Theme, width: f32) -> LayoutJob {
    let p = palette(theme);
    let font = FontId::monospace(BODY_SIZE);
    let mut job = LayoutJob::default();
    job.wrap.max_width = width;
    let plain = TextFormat::simple(font.clone(), p.punctuation);
    let mut link = TextFormat::simple(font, p.number);
    link.underline = egui::Stroke::new(1.0, p.number);
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            job.append("\n", 0.0, plain.clone());
        }
        let mut first = true;
        for word in line.split(' ') {
            if !first {
                job.append(" ", 0.0, plain.clone());
            }
            first = false;
            let address = word.starts_with("http://")
                || word.starts_with("https://")
                || (word.contains('@') && word.contains('.') && !word.ends_with(':'));
            job.append(word, 0.0, if address { link.clone() } else { plain.clone() });
        }
    }
    job
}

fn console_tab(ui: &mut Ui, log: &mut ConsoleLog, state: &mut DiagnosticsViewState, theme: &Theme) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 4))
        .show(ui, |ui| {
            let width = ui.available_width();
            ui.allocate_ui_with_layout(
                egui::vec2(width, 36.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_size(egui::vec2(width, 36.0));
                    ui.label(
                        RichText::new(tf(S::ConsoleCount, &[&log.len(), &console::MAX_ENTRIES]))
                            .size(13.0)
                            .color(theme.muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if icon_button(ui, Icon::Trash, t(S::ConsoleClear), theme).clicked() {
                            log.clear();
                        }
                        if icon_button(ui, Icon::Copy, t(S::ConsoleCopy), theme).clicked() {
                            ui.ctx().copy_text(state.console.text.clone());
                        }
                        if text_toggle(ui, t(S::ConsoleWrap), state.console_wrap, theme).clicked() {
                            state.console_wrap = !state.console_wrap;
                        }
                        if text_toggle(ui, t(S::ConsolePause), state.console_paused, theme).clicked() {
                            state.console_paused = !state.console_paused;
                        }
                    });
                },
            );
            ui.label(RichText::new(t(S::ConsolePrivate)).size(12.0).color(theme.muted));
        });
    if state.console_paused {
        state.console_hidden(log);
    } else {
        state.console.refresh(log, crate::app::local_offset_seconds());
        state.console_stale = false;
    }
    let rect = ui.available_rect_before_wrap();
    ui.painter().rect_filled(rect, 0.0, editor_fill(theme));
    let font = FontId::monospace(BODY_SIZE);
    let (char_w, row_h) = ui.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
    let columns = ((rect.width() - 28.0 - 12.0) / char_w.max(1.0)).floor().max(1.0) as usize;
    let wrap = state.console_wrap;
    let color = palette(theme).punctuation;
    let rows = state.console.rows(columns, wrap).to_vec();
    let text = &state.console.text;
    let mut scroll = if wrap {
        egui::ScrollArea::vertical()
    } else {
        egui::ScrollArea::both()
    };
    if std::mem::take(&mut state.console_to_top) {
        scroll = scroll.vertical_scroll_offset(0.0);
    } else {
        scroll = scroll.stick_to_bottom(true);
    }
    scroll
        .id_salt("diag-console")
        .auto_shrink([false, false])
        .show_rows(ui, row_h, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for &(start, end) in &rows[range] {
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.add(
                        egui::Label::new(RichText::new(&text[start..end]).font(font.clone()).color(color))
                            .extend()
                            .selectable(true),
                    );
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use wandur_core::diagnostics::console::ConsoleKind;

    #[test]
    fn the_console_text_drops_evicted_entries_from_the_top_instead_of_rendering_everything() {
        let mut log = ConsoleLog::default();
        let mut text = ConsoleText::default();
        for i in 1..=console::MAX_ENTRIES + 50 {
            log.append(ConsoleKind::Received, &format!("entry {i}\r\n"), false, &[]);
            if i % 300 == 0 {
                text.refresh(&log, 0);
            }
        }
        text.refresh(&log, 0);
        let lines: Vec<&str> = text.text.lines().collect();
        assert_eq!(lines.len(), console::MAX_ENTRIES);
        assert!(lines[0].ends_with("<< entry 51␍␊"));
        assert!(
            lines
                .last()
                .unwrap()
                .ends_with(&format!("<< entry {}␍␊", console::MAX_ENTRIES + 50))
        );
        assert_eq!(text.text, log.render(0) + "\n");
        log.clear();
        text.refresh(&log, 0);
        assert_eq!(text.text, "");
        log.append(ConsoleKind::Received, "After clear.\r\n", false, &[]);
        text.refresh(&log, 0);
        assert!(text.text.contains("<< After clear."));
    }

    #[test]
    fn console_rows_wrap_at_the_column_count_or_not_at_all() {
        let mut text = ConsoleText {
            text: "abcdefghij\nxy\n".into(),
            ..Default::default()
        };
        let ranges = text.rows(4, true).to_vec();
        let rows: Vec<String> = ranges.iter().map(|&(s, e)| text.text[s..e].to_string()).collect();
        assert_eq!(rows, ["abcd", "efgh", "ij", "xy"]);
        let rows = text.rows(4, false).len();
        assert_eq!(rows, 2);
    }

    #[test]
    fn json_is_coloured_by_token() {
        let theme = Theme::preset("Hull");
        let job = json_job(
            "{\n  \"hp\": 42,\n  \"ok\": false,\n  \"name\": \"Odo\"\n}",
            &theme,
            None,
        );
        let p = palette(&theme);
        let color_of = |piece: &str| {
            job.sections
                .iter()
                .find(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0] == piece)
                .map(|s| s.format.color)
        };
        assert_eq!(color_of("\"hp\""), Some(p.field));
        assert_eq!(color_of("42"), Some(p.number));
        assert_eq!(color_of("false"), Some(p.literal));
        assert_eq!(color_of("\"Odo\""), Some(p.string));
        assert_eq!(job.text, "{\n  \"hp\": 42,\n  \"ok\": false,\n  \"name\": \"Odo\"\n}");
        // Any text survives whole, multibyte characters too.
        let job = json_job("[redacted] é \"unterminated", &theme, None);
        assert_eq!(job.text, "[redacted] é \"unterminated");
    }

    /// The view side of `ChipsAndTheFilterBoxNarrowTheMessagesTab`: chips show the kinds with
    /// counts, a click on one narrows the list and the count, a second click restores it, and
    /// the console is noted stale while another tab shows.
    #[test]
    fn a_click_on_a_chip_narrows_the_messages_tab() {
        use egui::{Event, PointerButton, RawInput, pos2, vec2};
        use std::time::SystemTime;
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::preset("Hull");
        let mut protocol = SessionProtocol::default();
        let gate = wandur_core::diagnostics::Gate::default();
        let gmcp = wandur_core::protocol::parse_gmcp(br#"Char.Vitals {"hp":42,"maxhp":100}"#).unwrap();
        protocol.receive_gmcp(&gmcp, gate, SystemTime::now());
        protocol.receive_msdp(b"\x01OPPONENTHEALTH\x0280\x01HEALTH\x0242", gate, SystemTime::now());
        protocol.receive_mssp(
            &wandur_core::protocol::parse_mssp(b"\x01NAME\x02Fixture"),
            gate,
            SystemTime::now(),
        );
        let mut state = DiagnosticsViewState::default();
        let frame = |protocol: &mut SessionProtocol, state: &mut DiagnosticsViewState, events: Vec<Event>| {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 680.0))),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| show(ui, protocol, state, &theme));
            });
            out.textures_delta.clear();
        };
        frame(&mut protocol, &mut state, vec![]);
        let names: Vec<&str> = state.chip_rects.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Char.Vitals", "HEALTH", "OPPONENTHEALTH", "MSSP"]);
        assert_eq!(state.rows_drawn, 3);
        let chip = state
            .chip_rects
            .iter()
            .find(|(n, _)| n == "OPPONENTHEALTH")
            .unwrap()
            .1
            .center();
        let click = |pos| {
            vec![
                vec![
                    Event::PointerMoved(pos),
                    Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                vec![Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                vec![],
            ]
        };
        for events in click(chip) {
            frame(&mut protocol, &mut state, events);
        }
        assert!(
            protocol
                .messages
                .kinds()
                .iter()
                .any(|k| k.name == "OPPONENTHEALTH" && k.active)
        );
        assert_eq!(protocol.messages.visible_len(), 1);
        assert_eq!(protocol.messages.count_label(), "1 of 3");
        assert_eq!(state.rows_drawn, 1);
        for events in click(chip) {
            frame(&mut protocol, &mut state, events);
        }
        assert_eq!(protocol.messages.visible_len(), 3);
        // The console is behind while Messages shows, and catches up when it is shown.
        protocol.receive_text("hidden page\r\n", gate);
        frame(&mut protocol, &mut state, vec![]);
        assert!(state.console_stale);
        state.tab = DiagTab::Console;
        frame(&mut protocol, &mut state, vec![]);
        assert!(!state.console_stale);
        assert!(state.console_text().contains("hidden page"));
    }
}
