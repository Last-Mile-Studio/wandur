//! Help for the client's own commands in the command box, in the spirit of Minecraft's chat
//! command hints: typing the command character at a command start opens a compact list of
//! matching commands above the box; once a command is chosen its remaining parameters show as
//! grey placeholders after the caret, with the forms that still fit listed above; a part that is
//! certainly wrong turns red in the box, with one line saying why above it.
//!
//! Everything here reads the line through core's one scanner ([`command_line::context_at`],
//! [`command_line::check`]) and core's table ([`client_commands`]), so what it offers cannot
//! drift from what Enter sends. The keys are routed by `terminal_view::input_line`; this module
//! keeps the state and draws.

use std::ops::Range;

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, vec2};
use wandur_core::client_commands::{self, CommandId, CommandSpec, Head};
use wandur_core::command_line::{self, Context, Diagnostic, Problem, Syntax};
use wandur_core::l10n::{S, t, tf};

use crate::theme::Theme;

/// Height of one row of the list and of the signature help.
pub const ROW_HEIGHT: f32 = 26.0;
/// Size of the interface text in the rows (summaries, messages) and in the tip strip.
pub const TEXT_SIZE: f32 = 13.5;
/// Size of the key hint at the foot of the list.
const HINT_SIZE: f32 = 11.5;
/// Gap between the panel and the top of the command box.
const GAP: f32 = 4.0;
/// Space left of the rows' text, so that text lines up under the command in the box.
pub const PAD_X: f32 = 10.0;

/// The command help's state for one command box.
#[derive(Debug, Default)]
pub struct CommandHelp {
    /// The draft as the last frame left it, to tell typing from other changes.
    last_draft: String,
    /// The last change to the draft was typed in the box (not history, paste or the app).
    typed: bool,
    /// The next outside change counts as typed (a scene's typed text).
    force_typed: bool,
    /// The draft Escape put the list or hints away for; they come back when it changes.
    dismissed: Option<String>,
    /// The list's selected row, and whether the arrows moved it (Enter then takes it).
    selected: usize,
    armed: bool,
    /// Tab is cycling through the matches: what was typed, and what the box shows now.
    cycle: Option<Cycle>,
    /// What the last frame showed.
    pub shown: Shown,
}

#[derive(Clone, Debug)]
struct Cycle {
    start: usize,
    typed: String,
    applied: String,
}

/// What the command box showed in the last frame (tests, scenes, the probe).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    /// The list's rows, as shown (`/wait`, `/<count>`), and the selected one.
    pub list: Vec<String>,
    pub selected: usize,
    /// The forms signature help listed (`/wait <seconds> {text}`).
    pub forms: Vec<String>,
    /// The placeholders after the caret.
    pub ghost: Option<String>,
    /// The line above the box, and whether it is a problem (red) rather than a hint (muted).
    pub message: Option<(String, bool)>,
    /// Byte ranges of the draft drawn as wrong.
    pub wrong: Vec<Range<usize>>,
}

impl Shown {
    pub fn panel(&self) -> bool {
        !self.list.is_empty() || !self.forms.is_empty() || self.message.is_some()
    }
}

/// What the draft and caret call for, before the gates (typed, dismissed, focus) apply.
#[derive(Debug, Default)]
pub struct Plan {
    /// The head the caret is in, and the commands it could become.
    pub head: Option<Range<usize>>,
    pub entries: Vec<&'static CommandSpec>,
    /// The head as the filter read it (what was typed, while Tab cycles).
    pub filter: String,
    /// After a head: the command, its head, the forms that fit, the parameter at the caret.
    pub args: Option<(CommandId, Range<usize>, Vec<usize>, usize)>,
    pub ghost: Option<String>,
    pub message: Option<(String, bool)>,
    pub wrong: Vec<Range<usize>>,
}

/// Read `draft` with the caret at byte `caret`. `cycle` is the head as typed while Tab cycles,
/// so the list keeps the matches it started from.
pub fn plan(draft: &str, caret: usize, syntax: Syntax, cycle: Option<(usize, &str)>) -> Plan {
    let mut plan = Plan::default();
    let diagnostics = command_line::check(draft, syntax);
    plan.wrong = wrong_spans(&diagnostics);
    let context = command_line::context_at(draft, caret, syntax);
    plan.ghost = context.remaining(syntax);
    match &context {
        Context::Head { range } => {
            let filter = match cycle {
                Some((start, typed)) if start == range.start => typed.to_string(),
                _ => draft[range.start..caret].to_string(),
            };
            plan.entries = client_commands::matching(&filter, syntax);
            plan.filter = filter;
            plan.head = Some(range.clone());
        }
        Context::Args {
            id,
            head,
            fitting,
            active,
            ..
        } => plan.args = Some((*id, head.clone(), fitting.clone(), *active)),
        Context::None => {}
    }
    // The line above the box: a problem behind the caret first, else the lone count's hint for
    // the command at the caret.
    let own_head = match &context {
        Context::Head { range } => Some(range.clone()),
        Context::Args { head, .. } => Some(head.clone()),
        Context::None => None,
    };
    plan.message = diagnostics
        .iter()
        .find(|d| d.is_wrong() && d.span.start < caret)
        .map(|d| (d.message(syntax), true))
        .or_else(|| {
            diagnostics
                .iter()
                .find(|d| matches!(d.problem, Problem::LoneCount(_)) && Some(&d.span) == own_head.as_ref())
                .map(|d| (d.message(syntax), false))
        });
    plan
}

/// The certainly wrong parts of a line, sorted and merged.
fn wrong_spans(diagnostics: &[Diagnostic]) -> Vec<Range<usize>> {
    let mut spans: Vec<Range<usize>> = diagnostics
        .iter()
        .filter(|d| d.is_wrong() && !d.span.is_empty())
        .map(|d| d.span.clone())
        .collect();
    spans.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for span in spans {
        match merged.last_mut() {
            Some(last) if span.start <= last.end => last.end = last.end.max(span.end),
            _ => merged.push(span),
        }
    }
    merged
}

/// What a list entry puts in place of the head: its word, or for the repeat the count typed so
/// far (or only the command character).
fn completion(spec: &CommandSpec, typed: &str, syntax: Syntax) -> String {
    match spec.head {
        Head::Word(word) => format!("{}{word}", syntax.command),
        Head::Count if typed.len() > syntax.command.len_utf8() => typed.to_string(),
        Head::Count => syntax.command.to_string(),
    }
}

/// An entry as the list shows it, and how many of its bytes the typed head matched.
fn row_name(spec: &CommandSpec, filter: &str, syntax: Syntax) -> (String, usize) {
    match spec.head {
        Head::Count if filter.len() > syntax.command.len_utf8() => (filter.to_string(), filter.len()),
        _ => (spec.display(syntax), filter.len().min(spec.display(syntax).len())),
    }
}

/// What a key did to the draft.
pub enum Edit {
    /// Replace the draft and put the caret at this byte.
    Set(String, usize),
}

impl CommandHelp {
    /// The next change to the draft from outside the box counts as typed (a scene's text).
    pub fn treat_next_as_typed(&mut self) {
        self.force_typed = true;
    }

    /// Start of a frame: a draft changed from outside the box (history, the app) was not typed.
    pub fn begin(&mut self, draft: &str) {
        if draft != self.last_draft {
            self.typed = std::mem::take(&mut self.force_typed);
            self.reset_list();
            self.last_draft = draft.to_string();
        }
        if self.dismissed.as_deref().is_some_and(|d| d != draft) {
            self.dismissed = None;
        }
        if let Some(cycle) = &self.cycle
            && draft.get(cycle.start..cycle.start + cycle.applied.len()) != Some(cycle.applied.as_str())
        {
            self.cycle = None;
        }
    }

    /// After the box handled its events: a change there was typed unless it was a paste.
    pub fn after_edit(&mut self, draft: &str, pasted: bool) {
        if draft != self.last_draft {
            self.typed = !pasted;
            self.reset_list();
            self.last_draft = draft.to_string();
        }
    }

    /// The draft was replaced by history (Up, Down) or sent.
    pub fn not_typed(&mut self, draft: &str) {
        self.typed = false;
        self.reset_list();
        self.last_draft = draft.to_string();
    }

    fn reset_list(&mut self) {
        self.selected = 0;
        self.armed = false;
        self.cycle = None;
    }

    /// The head as typed while Tab cycles.
    pub fn cycle_filter(&self) -> Option<(usize, &str)> {
        self.cycle.as_ref().map(|c| (c.start, c.typed.as_str()))
    }

    /// Lists and hints may show for this draft: it was typed and Escape did not put them away.
    pub fn allowed(&self, draft: &str) -> bool {
        self.typed && self.dismissed.as_deref() != Some(draft)
    }

    /// Escape put the list or hints away for this draft.
    pub fn is_dismissed(&self, draft: &str) -> bool {
        self.dismissed.as_deref() == Some(draft)
    }

    /// Escape: put the list or hints away until the draft changes.
    pub fn dismiss(&mut self, draft: &str) {
        self.dismissed = Some(draft.to_string());
    }

    pub fn selected(&self, count: usize) -> usize {
        self.selected.min(count.saturating_sub(1))
    }

    /// Up or Down in the open list (wrapping); Enter then takes the row.
    pub fn step(&mut self, count: usize, down: bool) {
        if count == 0 {
            return;
        }
        let at = self.selected(count);
        self.selected = if down {
            (at + 1) % count
        } else {
            (at + count - 1) % count
        };
        self.armed = true;
    }

    pub fn armed(&self) -> bool {
        self.armed
    }

    /// Tab (or Shift+Tab, `back`): the first press puts the selected match in the box, the next
    /// ones move through the matches, the box showing each in turn.
    pub fn cycle(&mut self, draft: &str, plan: &Plan, syntax: Syntax, back: bool) -> Option<Edit> {
        let head = plan.head.clone()?;
        let count = plan.entries.len();
        if count == 0 {
            return None;
        }
        if self.cycle.is_some() {
            let at = self.selected(count);
            self.selected = if back {
                (at + count - 1) % count
            } else {
                (at + 1) % count
            };
        } else if back {
            self.selected = (self.selected(count) + count - 1) % count;
        }
        let spec = plan.entries[self.selected(count)];
        let text = completion(spec, &plan.filter, syntax);
        let mut out = String::with_capacity(draft.len() + text.len());
        out.push_str(&draft[..head.start]);
        out.push_str(&text);
        out.push_str(&draft[head.end..]);
        self.cycle = Some(Cycle {
            start: head.start,
            typed: plan.filter.clone(),
            applied: text.clone(),
        });
        self.armed = false;
        self.last_draft = out.clone();
        Some(Edit::Set(out, head.start + text.len()))
    }

    /// Enter after the arrows, or a click: the row's command and a space, the list closed and
    /// signature help taking over. The repeat with no count yet has nothing to put in.
    pub fn accept(&mut self, draft: &str, plan: &Plan, row: usize, syntax: Syntax) -> Option<Edit> {
        let head = plan.head.clone()?;
        let spec = *plan.entries.get(row)?;
        let text = completion(spec, &plan.filter, syntax);
        if spec.head == Head::Count && text.len() == syntax.command.len_utf8() {
            self.armed = false;
            return None;
        }
        let rest = draft[head.end..].trim_start();
        let out = format!("{}{text} {rest}", &draft[..head.start]);
        self.reset_list();
        self.last_draft = out.clone();
        Some(Edit::Set(out, head.start + text.len() + 1))
    }
}

/// The colours of the panel above the box: the transcript's own, a little translucent, so the
/// panel reads as part of the command box in every theme, dark or light.
struct Look {
    fill: Color32,
    border: Color32,
    text: Color32,
    muted: Color32,
    accent: Color32,
    selected: Color32,
    error: Color32,
}

impl Look {
    fn of(theme: &Theme) -> Self {
        let base = crate::theme::mix(theme.terminal, theme.terminal_text, 0.05);
        let [r, g, b, _] = base.to_array();
        let accent = readable(theme.accent, theme.terminal, theme.terminal_text);
        Self {
            fill: Color32::from_rgba_unmultiplied(r, g, b, 240),
            border: crate::theme::mix(theme.terminal, theme.terminal_text, 0.18),
            text: theme.terminal_text,
            muted: crate::theme::mix(theme.terminal_text, theme.terminal, 0.38),
            accent,
            selected: crate::theme::mix(theme.terminal, accent, 0.3),
            error: error_color(theme),
        }
    }
}

/// `color`, moved toward `ink` until it reads on `background` (4.5:1): a light theme's accent
/// over a dark transcript (Hull) would otherwise be too dark to read.
fn readable(color: Color32, background: Color32, ink: Color32) -> Color32 {
    (0..=10)
        .map(|step| crate::theme::mix(color, ink, step as f32 / 10.0))
        .find(|c| crate::theme::contrast(*c, background) >= 4.5)
        .unwrap_or(ink)
}

/// Red for a wrong part, readable on the transcript's background (the theme's own error colour
/// is chosen for its panels, which may be light while the transcript is dark).
pub fn error_color(theme: &Theme) -> Color32 {
    let red = if crate::theme::is_light(theme.terminal) {
        Color32::from_rgb(0xB4, 0x23, 0x18)
    } else {
        Color32::from_rgb(0xF4, 0x7B, 0x7B)
    };
    readable(red, theme.terminal, theme.terminal_text)
}

/// The draft's layout in the box: the wrong parts red and underlined.
pub fn layout_job(text: &str, wrong: &[Range<usize>], font: &FontId, color: Color32, error: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let normal = TextFormat::simple(font.clone(), color);
    let mut bad = TextFormat::simple(font.clone(), error);
    bad.underline = Stroke::new(1.0, error);
    let mut at = 0;
    for span in wrong {
        let (start, end) = (span.start.min(text.len()), span.end.min(text.len()));
        if start < at || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        job.append(&text[at..start], 0.0, normal.clone());
        job.append(&text[start..end], 0.0, bad.clone());
        at = end;
    }
    job.append(&text[at..], 0.0, normal);
    job
}

/// One painted row: its text runs (text, font, colour) left to right, then a muted summary.
struct Row {
    runs: Vec<(String, FontId, Color32)>,
    summary: String,
    label: String,
}

/// Everything the panel shows this frame.
pub struct Panel<'a> {
    pub plan: &'a Plan,
    pub list: bool,
    pub forms: bool,
    pub message: bool,
    pub selected: usize,
}

/// What a click in the panel asked for.
pub struct PanelOutput {
    pub clicked: Option<usize>,
    pub shown: Shown,
}

/// Draw the panel above the box: the list, or signature help, and the message line. `anchor_x`
/// is the panel's left edge (the command's x in the box less [`PAD_X`]); `boxed` is the command box.
#[allow(clippy::too_many_arguments)]
pub fn show_panel(
    ui: &Ui,
    tab_id: u64,
    input_id: egui::Id,
    panel: Panel<'_>,
    anchor_x: f32,
    boxed: Rect,
    bounds: Rect,
    theme: &Theme,
    font: &FontId,
    syntax: Syntax,
) -> PanelOutput {
    let look = Look::of(theme);
    let plan = panel.plan;
    let ui_font = FontId::proportional(TEXT_SIZE);
    let mut shown = Shown::default();
    let mut rows: Vec<Row> = Vec::new();
    let mut note = None;
    if panel.list {
        for spec in &plan.entries {
            let (name, matched) = row_name(spec, &plan.filter, syntax);
            let matched = matched.min(name.len());
            shown.list.push(name.clone());
            let mut runs = vec![(name[..matched].to_string(), font.clone(), look.accent)];
            if matched < name.len() {
                runs.push((name[matched..].to_string(), font.clone(), look.text));
            }
            rows.push(Row {
                runs,
                summary: t(spec.summary).to_string(),
                label: format!("{name}, {}", t(spec.summary)),
            });
        }
        shown.selected = panel.selected;
    } else if panel.forms
        && let Some((id, _, fitting, active)) = &plan.args
    {
        let spec = CommandSpec::get(*id);
        for &i in fitting {
            let form = &spec.forms[i];
            let mut runs = Vec::new();
            let mut head = String::new();
            head.push(syntax.command);
            if let Head::Word(word) = spec.head {
                head.push_str(word);
            }
            runs.push((head, font.clone(), look.text));
            for (p, param) in form.params.iter().enumerate() {
                let space = if p == 0 && param.kind == client_commands::ParamKind::Count {
                    ""
                } else {
                    " "
                };
                let color = if p == *active { look.accent } else { look.muted };
                runs.push((format!("{space}{}", param.placeholder(syntax)), font.clone(), color));
            }
            let line = spec.syntax_line(form, syntax);
            shown.forms.push(line.clone());
            rows.push(Row {
                runs,
                summary: String::new(),
                label: line,
            });
        }
        // What the first form that fits does, under them (a problem's line takes its place).
        note = fitting.first().map(|&i| CommandSpec::form_summary(&spec.forms[i]));
    }
    if panel.message {
        shown.message = plan.message.clone();
    }
    let line = shown.message.clone().or(note.map(|n| (n, false)));
    if rows.is_empty() && shown.message.is_none() {
        return PanelOutput { clicked: None, shown };
    }

    // Measure: the widest row decides the width, within the box.
    let gap = 18.0;
    let measure = |text: &str, font: &FontId| {
        ui.fonts_mut(|f| {
            f.layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
    };
    let name_width = rows
        .iter()
        .map(|r| r.runs.iter().map(|(text, font, _)| measure(text, font)).sum::<f32>())
        .fold(0.0, f32::max);
    let summary_width = rows.iter().map(|r| measure(&r.summary, &ui_font)).fold(0.0, f32::max);
    let message_width = line.as_ref().map_or(0.0, |(m, _)| measure(m, &ui_font) + 22.0);
    let hint = panel.list.then(|| t(S::CmdListKeys));
    let hint_width = hint.map_or(0.0, |h| measure(h, &FontId::proportional(HINT_SIZE)));
    let content = (name_width + gap + summary_width).max(message_width).max(hint_width);
    let width = (content + 2.0 * PAD_X).clamp(240.0, (bounds.width() - 4.0).max(240.0));
    let hint_height = if hint.is_some() { 22.0 } else { 0.0 };
    let message_height = if line.is_some() { ROW_HEIGHT } else { 0.0 };
    let height = rows.len() as f32 * ROW_HEIGHT + message_height + hint_height + 8.0;
    let left = anchor_x.clamp(bounds.left() + 2.0, (bounds.right() - width - 2.0).max(bounds.left()));
    let rect = Rect::from_min_size(Pos2::new(left, boxed.top() - GAP - height), vec2(width, height));

    let area_id = egui::Id::new(("wandur-command-help", tab_id));
    let mut clicked = None;
    egui::Area::new(area_id)
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .constrain(false)
        .show(ui.ctx(), |ui| {
            ui.set_min_size(rect.size());
            ui.painter().rect(
                rect,
                6.0,
                look.fill,
                Stroke::new(1.0, look.border),
                egui::StrokeKind::Inside,
            );
            let mut y = rect.top() + 4.0;
            let a11y = crate::a11y::active(ui.ctx());
            let mut row_ids = Vec::new();
            for (i, row) in rows.iter().enumerate() {
                let row_rect = Rect::from_min_size(Pos2::new(rect.left() + 4.0, y), vec2(width - 8.0, ROW_HEIGHT));
                let sense = if panel.list { Sense::click() } else { Sense::hover() };
                let response = ui.interact(row_rect, area_id.with(("row", i)), sense);
                let is_selected = panel.list && i == panel.selected;
                if is_selected {
                    ui.painter().rect_filled(row_rect, 4.0, look.selected);
                } else if panel.list && response.hovered() {
                    ui.painter()
                        .rect_filled(row_rect, 4.0, theme.hover_fill().gamma_multiply(0.6));
                }
                let mut x = row_rect.left() + PAD_X - 4.0;
                let cy = row_rect.center().y;
                for (text, font, color) in &row.runs {
                    let galley = ui.fonts_mut(|f| f.layout_no_wrap(text.clone(), font.clone(), *color));
                    let w = galley.size().x;
                    ui.painter()
                        .galley(Pos2::new(x, cy - galley.size().y / 2.0), galley, *color);
                    x += w;
                }
                let summary_x = rect.left() + PAD_X + name_width + gap;
                if row.summary.is_empty() {
                    row_ids.push(response.id);
                    if a11y {
                        let label = row.label.clone();
                        ui.ctx().accesskit_node_builder(response.id, |node| {
                            node.set_role(egui::accesskit::Role::Label);
                            node.set_label(label);
                        });
                    }
                    y += ROW_HEIGHT;
                    continue;
                }
                let room = (rect.right() - PAD_X - summary_x).max(0.0);
                let job = {
                    let mut job = LayoutJob::simple_singleline(row.summary.clone(), ui_font.clone(), look.muted);
                    job.wrap.max_width = room;
                    job.wrap.max_rows = 1;
                    job.wrap.break_anywhere = true;
                    job
                };
                let galley = ui.fonts_mut(|f| f.layout_job(job));
                ui.painter()
                    .galley(Pos2::new(summary_x, cy - galley.size().y / 2.0), galley, look.muted);
                if panel.list && response.clicked() {
                    clicked = Some(i);
                }
                if a11y {
                    let role = if panel.list {
                        egui::accesskit::Role::ListBoxOption
                    } else {
                        egui::accesskit::Role::Label
                    };
                    let label = row.label.clone();
                    ui.ctx().accesskit_node_builder(response.id, |node| {
                        node.set_role(role);
                        node.set_label(label);
                        if panel.list {
                            node.set_selected(is_selected);
                        }
                    });
                }
                row_ids.push(response.id);
                y += ROW_HEIGHT;
            }
            if let Some((message, wrong)) = &line {
                let color = if *wrong { look.error } else { look.muted };
                let mut x = rect.left() + PAD_X;
                if *wrong {
                    // A small warning mark before the text.
                    let c = Pos2::new(x + 5.0, y + ROW_HEIGHT / 2.0);
                    ui.painter().circle_filled(c, 6.0, color);
                    ui.painter().text(
                        c,
                        Align2::CENTER_CENTER,
                        "!",
                        FontId::proportional(10.5),
                        look.fill.to_opaque(),
                    );
                    x += 16.0;
                }
                let mut job = LayoutJob::simple_singleline(message.clone(), ui_font.clone(), color);
                job.wrap.max_width = (rect.right() - PAD_X - x).max(0.0);
                job.wrap.max_rows = 1;
                let galley = ui.fonts_mut(|f| f.layout_job(job));
                ui.painter()
                    .galley(Pos2::new(x, y + (ROW_HEIGHT - galley.size().y) / 2.0), galley, color);
                if a11y && *wrong {
                    let text = message.clone();
                    ui.ctx().accesskit_node_builder(area_id.with("message"), |node| {
                        node.set_role(egui::accesskit::Role::Status);
                        node.set_label(text);
                        node.set_live(egui::accesskit::Live::Polite);
                    });
                }
                y += ROW_HEIGHT;
            }
            if let Some(hint) = hint {
                ui.painter().hline(
                    rect.left() + 6.0..=rect.right() - 6.0,
                    y + 1.0,
                    Stroke::new(1.0, look.border),
                );
                ui.painter().text(
                    Pos2::new(rect.left() + PAD_X, y + 12.0),
                    Align2::LEFT_CENTER,
                    hint,
                    FontId::proportional(HINT_SIZE),
                    look.muted,
                );
            }
            if a11y && panel.list {
                let list_node = area_id.with("move");
                let active_row = row_ids.get(panel.selected).copied();
                ui.ctx().accesskit_node_builder(list_node, |node| {
                    node.set_role(egui::accesskit::Role::ListBox);
                    node.set_label(t(S::CmdListName));
                });
                ui.ctx().accesskit_node_builder(input_id, |node| {
                    node.set_has_popup(egui::accesskit::HasPopup::Listbox);
                    node.set_expanded(true);
                    node.set_controls(vec![list_node.accesskit_id()]);
                    if let Some(row) = active_row {
                        node.set_active_descendant(row.accesskit_id());
                    }
                });
            }
        });
    if crate::a11y::active(ui.ctx()) && !panel.list {
        // Signature help is read on demand, as the box's description.
        let description = shown.forms.first().map(|form| match &plan.args {
            Some((id, _, fitting, active)) => {
                let spec = CommandSpec::get(*id);
                let now = fitting
                    .first()
                    .and_then(|&i| spec.forms[i].params.get(*active))
                    .map_or(String::new(), |p| t(p.label).to_string());
                tf(S::CmdA11ySignature, &[form, &now])
            }
            None => form.clone(),
        });
        if let Some(description) = description {
            ui.ctx()
                .accesskit_node_builder(input_id, |node| node.set_description(description));
        }
    }
    PanelOutput { clicked, shown }
}

/// The byte offset of char index `chars` in `text`.
pub fn byte_of(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i)
}

/// The char index of byte offset `byte` in `text`.
pub fn char_of(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].chars().count()
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_lists_matches_and_hints_and_flags_only_what_is_wrong() {
        wandur_core::l10n::override_thread(Some(wandur_core::l10n::Language::En));
        let w = Syntax::WANDUR;
        let names = |p: &Plan| p.entries.iter().map(|c| c.name()).collect::<Vec<_>>();
        let p = plan("/", 1, w, None);
        assert_eq!(names(&p), ["count", "help", "wait"]);
        assert_eq!(p.head, Some(0..1));
        assert_eq!(names(&plan("look;/W", 7, w, None)), ["wait"]);
        assert!(plan("say /", 5, w, None).entries.is_empty());
        // While Tab cycles, the list keeps the matches of what was typed.
        assert_eq!(names(&plan("/help", 5, w, Some((0, "/")))), ["count", "help", "wait"]);

        let p = plan("/wait ", 6, w, None);
        assert_eq!(p.ghost.as_deref(), Some("<seconds> {text}"));
        assert_eq!(p.args.as_ref().map(|a| a.2.clone()), Some(vec![0, 1, 2]));
        assert_eq!(p.message, None, "incomplete is not an error");
        assert!(p.wrong.is_empty());

        let p = plan("/wait 601", 9, w, None);
        assert_eq!(p.wrong, [6..9]);
        assert!(p.message.as_ref().is_some_and(|(m, wrong)| *wrong && m.contains("600")));
        let p = plan("/0 look", 7, w, None);
        assert_eq!(p.wrong, [1..2]);
        let p = plan("/3", 2, w, None);
        assert_eq!(names(&p), ["count"]);
        assert_eq!(
            p.message,
            Some(("/3 needs a command after it; Enter sends /3 as it is".into(), false))
        );
        assert!(plan("/3 look", 7, w, None).message.is_none());
        wandur_core::l10n::override_thread(None);
    }

    #[test]
    fn wrong_parts_are_red_and_the_rest_keeps_its_colour() {
        let font = FontId::monospace(13.0);
        let job = layout_job("/wait 601", &[6..9], &font, Color32::WHITE, Color32::RED);
        let colours: Vec<(String, Color32)> = job
            .sections
            .iter()
            .map(|s| {
                (
                    job.text[s.byte_range.start.0..s.byte_range.end.0].to_string(),
                    s.format.color,
                )
            })
            .filter(|(text, _)| !text.is_empty())
            .collect();
        assert_eq!(
            colours,
            [
                ("/wait ".to_string(), Color32::WHITE),
                ("601".to_string(), Color32::RED)
            ]
        );
    }
}
