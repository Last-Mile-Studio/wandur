//! Exposing the painted transcript to screen readers through AccessKit.
//!
//! The terminal grid is painted, so without this a screen reader finds an empty rectangle. egui
//! builds an AccessKit tree only while assistive technology is active (eframe turns it on when the
//! platform asks), and the closures below run only then, so this costs nothing otherwise.
//!
//! Two nodes: the terminal widget itself becomes a read-only multi-line text field labelled
//! "Transcript" whose value is the rows on screen; and a polite live region ("Latest output")
//! holds the last lines at the bottom of the transcript, so new output is announced as it
//! arrives. What is still missing (documented in `docs/milestone-3.md`): text ranges and caret
//! navigation inside the transcript (AccessKit `TextRun` children with character positions),
//! announcing only what is new rather than the last lines, and testing with VoiceOver, NVDA and
//! Orca by a person.

use egui::accesskit::{Live, Role};
use wandur_core::l10n::{S, t};
use wandur_term::Terminal;
use wandur_term::alacritty_terminal::term::cell::Flags;

/// Lines in the live region.
pub const LIVE_LINES: usize = 5;

/// The text of a screen row, without trailing blanks.
pub fn row_text(term: &Terminal, row: usize) -> String {
    let mut s = String::new();
    for cell in term.view_row(row) {
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) || cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        s.push(cell.c);
        if let Some(extra) = cell.zerowidth() {
            s.extend(extra.iter());
        }
    }
    s.truncate(s.trim_end().len());
    s
}

/// The rows on screen as text.
pub fn screen_text(term: &Terminal) -> String {
    let rows = term.size().rows;
    let mut lines: Vec<String> = (0..rows).map(|r| row_text(term, r)).collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

/// The last non-empty lines on screen (when the view is at the bottom).
pub fn latest_lines(term: &Terminal, count: usize) -> String {
    let rows = term.size().rows;
    let mut lines: Vec<String> = (0..rows)
        .rev()
        .map(|r| row_text(term, r))
        .filter(|l| !l.is_empty())
        .take(count)
        .collect();
    lines.reverse();
    lines.join("\n")
}

/// Describe the terminal widget `id` to AccessKit (only when it is active).
pub fn expose_terminal(ctx: &egui::Context, id: egui::Id, term: &Terminal) {
    if ctx
        .accesskit_node_builder(id, |node| {
            node.set_role(Role::MultilineTextInput);
            node.set_read_only();
            node.set_label(t(S::A11yTranscript));
            node.set_value(screen_text(term));
        })
        .is_none()
    {
        return;
    }
    if term.display_offset() == 0 {
        let latest = latest_lines(term, LIVE_LINES);
        ctx.accesskit_node_builder(id.with("latest-output"), |node| {
            node.set_role(Role::Log);
            node.set_live(Live::Polite);
            node.set_label(t(S::A11yLatestOutput));
            node.set_value(latest);
        });
    }
}

/// Give a control its accessible name, keeping the role egui gave it (a text field, a combo box).
/// Costs nothing unless a screen reader is active.
pub fn label(response: &egui::Response, text: &str) {
    response
        .ctx
        .accesskit_node_builder(response.id, |node| node.set_label(text));
}

fn pending_id() -> egui::Id {
    egui::Id::new("wandur-a11y-pending-label")
}

/// Remember `text` as the name of the next field a helper draws ([`pending`]), for helpers
/// that draw a caption and then a closure's field.
pub fn set_pending(ui: &egui::Ui, text: &str) {
    ui.ctx().data_mut(|d| d.insert_temp(pending_id(), text.to_string()));
}

/// The caption [`set_pending`] remembered (empty if none).
pub fn pending(ui: &egui::Ui) -> String {
    ui.ctx()
        .data(|d| d.get_temp::<String>(pending_id()))
        .unwrap_or_default()
}

/// [`label`] inside an expression: `named(ui.add(field), "Name").changed()`.
pub fn named(response: egui::Response, text: &str) -> egui::Response {
    label(&response, text);
    response
}

/// [`label`] for a combo box's button, and the same name for its list while it is open.
pub fn named_combo<R>(inner: egui::InnerResponse<R>, text: &str) -> egui::InnerResponse<R> {
    label_combo(&inner.response, text);
    inner
}

/// Name a combo box's button and its open list.
pub fn label_combo(response: &egui::Response, text: &str) {
    label(response, text);
    name_popup(&response.ctx, response.id.with("popup"), Role::ListBox, text);
}

/// Name an open popup (a menu, a combo box's list) by its popup id: egui gives the popup's
/// area a clickable node without a name.
pub fn name_popup(ctx: &egui::Context, popup_id: egui::Id, role: Role, text: &str) {
    if egui::Popup::is_id_open(ctx, popup_id) {
        name_shown_popup(ctx, popup_id, role, text);
    }
}

/// [`name_popup`] for a popup the caller has just shown (one opened with `open_bool`, whose
/// state egui does not keep).
pub fn name_shown_popup(ctx: &egui::Context, popup_id: egui::Id, role: Role, text: &str) {
    ctx.accesskit_node_builder(popup_id.with("move"), |node| {
        node.set_role(role);
        node.set_label(text);
    });
}

/// Make a painted control (an allocated rectangle with a click sense) a named control of
/// `role` for screen readers.
pub fn control(response: &egui::Response, role: Role, text: &str) {
    response.ctx.accesskit_node_builder(response.id, |node| {
        node.set_role(role);
        node.set_label(text);
    });
}

/// The same for a control with an on or off state (a check mark, a toggle, a selected tab).
pub fn toggle(response: &egui::Response, role: Role, text: &str, on: bool) {
    use egui::accesskit::Toggled;
    response.ctx.accesskit_node_builder(response.id, |node| {
        node.set_role(role);
        node.set_label(text);
        node.set_toggled(if on { Toggled::True } else { Toggled::False });
    });
}

/// Whether this frame builds an AccessKit tree (a screen reader asked, or a test turned it on).
/// Asking costs one empty, role-less node when it does; work done only for screen readers
/// checks this first, so it costs nothing otherwise.
pub fn active(ctx: &egui::Context) -> bool {
    ctx.accesskit_node_builder(egui::Id::new("wandur-a11y-active"), |_| ())
        .is_some()
}

/// Names for controls this app does not build itself (the dock's separators), matched by
/// where they are on screen when the frame's AccessKit tree is sent ([`RectNamer`]).
#[derive(Clone, Default)]
struct RectNames(Vec<(egui::Rect, Role, String, Match)>);

/// How a named rectangle finds its node.
#[derive(Clone, Copy, PartialEq)]
enum Match {
    /// The node is drawn exactly there.
    Exact,
    /// A thin node (8 points or less high) lies inside it: a scroll bar of a tab strip.
    ThinInside,
}

fn rect_names_id() -> egui::Id {
    egui::Id::new("wandur-a11y-rect-names")
}

/// Name the unnamed control drawn exactly at `rect` this frame.
pub fn name_rect(ctx: &egui::Context, rect: egui::Rect, role: Role, label: &str) {
    push_rect(ctx, rect, role, label, Match::Exact);
}

/// Name every unnamed thin control (8 points high or less) inside `rect` this frame.
pub fn name_thin_inside(ctx: &egui::Context, rect: egui::Rect, role: Role, label: &str) {
    push_rect(ctx, rect, role, label, Match::ThinInside);
}

fn push_rect(ctx: &egui::Context, rect: egui::Rect, role: Role, label: &str, how: Match) {
    if !active(ctx) {
        return;
    }
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<RectNames>(rect_names_id())
            .0
            .push((rect, role, label.to_string(), how))
    });
}

/// The plugin that applies [`name_rect`] names to the tree update at the end of each frame.
pub struct RectNamer;

impl egui::Plugin for RectNamer {
    fn debug_name(&self) -> &'static str {
        "wandur-a11y-rect-names"
    }

    fn output_hook(&mut self, ctx: &egui::Context, output: &mut egui::FullOutput) {
        let names = ctx
            .data_mut(|d| d.remove_temp::<RectNames>(rect_names_id()))
            .unwrap_or_default();
        let Some(update) = output.platform_output.accesskit_update.as_mut() else {
            return;
        };
        if names.0.is_empty() {
            return;
        }
        for (_, node) in &mut update.nodes {
            if node.label().is_some() || node.role() != Role::Unknown {
                continue;
            }
            let Some(b) = node.bounds() else { continue };
            let found = names.0.iter().find(|(r, _, _, how)| match how {
                Match::Exact => {
                    (f64::from(r.min.x) - b.x0).abs() < 0.6
                        && (f64::from(r.min.y) - b.y0).abs() < 0.6
                        && (f64::from(r.max.x) - b.x1).abs() < 0.6
                        && (f64::from(r.max.y) - b.y1).abs() < 0.6
                }
                Match::ThinInside => {
                    b.y1 - b.y0 <= 8.5
                        && b.x0 >= f64::from(r.min.x) - 0.6
                        && b.x1 <= f64::from(r.max.x) + 0.6
                        && b.y0 >= f64::from(r.min.y) - 0.6
                        && b.y1 <= f64::from(r.max.y) + 0.6
                }
            });
            if let Some((_, role, label, _)) = found {
                node.set_role(*role);
                node.set_label(label.as_str());
            }
        }
    }
}

/// One control a screen reader cannot name.
#[derive(Clone, Debug, PartialEq)]
pub struct Unnamed {
    pub role: Role,
    /// Where it is on screen (points): x, y, width, height.
    pub bounds: [i32; 4],
    /// What it shows, as far as the tree tells (its value or its children's text).
    pub hint: String,
}

impl std::fmt::Display for Unnamed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [x, y, w, h] = self.bounds;
        write!(f, "{:?} at {x},{y} {w}x{h}", self.role)?;
        if !self.hint.is_empty() {
            write!(f, " ({})", self.hint)?;
        }
        Ok(())
    }
}

/// Roles that are always something to operate.
fn interactive_role(role: Role) -> bool {
    matches!(
        role,
        Role::Button
            | Role::CheckBox
            | Role::RadioButton
            | Role::ComboBox
            | Role::TextInput
            | Role::MultilineTextInput
            | Role::SearchInput
            | Role::PasswordInput
            | Role::Slider
            | Role::SpinButton
            | Role::Link
            | Role::Tab
            | Role::MenuItem
            | Role::MenuItemCheckBox
            | Role::MenuItemRadio
            | Role::Switch
            | Role::ColorWell
    )
}

/// Roles whose kind is their name for a screen reader (scroll bars, resize handles, windows and
/// panes): VoiceOver says "scroll bar" or "splitter" and nothing more is expected.
fn named_by_role(role: Role) -> bool {
    matches!(
        role,
        Role::ScrollBar | Role::Splitter | Role::Window | Role::Pane | Role::ScrollView | Role::Log
    )
}

/// Every interactive node of a tree (one that can be clicked or focused, or has a control's
/// role) that has no accessible name: no label, no `labelled_by`, and for text fields no
/// placeholder either. Hidden nodes are skipped.
pub fn unnamed(update: &egui::accesskit::TreeUpdate) -> Vec<Unnamed> {
    use egui::accesskit::{Action, Node, NodeId};
    let nodes: std::collections::HashMap<NodeId, &Node> = update.nodes.iter().map(|(id, n)| (*id, n)).collect();
    let text_of = |node: &Node| {
        let mut parts: Vec<String> = Vec::new();
        if let Some(v) = node.value().filter(|v| !v.trim().is_empty()) {
            parts.push(v.chars().take(40).collect());
        }
        for child in node.children() {
            if let Some(c) = nodes.get(child)
                && let Some(t) = c.label().or(c.value()).filter(|t| !t.trim().is_empty())
            {
                parts.push(t.chars().take(40).collect());
            }
        }
        parts.join(" / ")
    };
    let mut out = Vec::new();
    for (_, node) in &update.nodes {
        let role = node.role();
        let interactive =
            node.supports_action(Action::Click) || node.supports_action(Action::Focus) || interactive_role(role);
        if !interactive || node.is_hidden() || named_by_role(role) {
            continue;
        }
        let labelled = node.label().is_some_and(|l| !l.trim().is_empty())
            || !node.labelled_by().is_empty()
            || (matches!(
                role,
                Role::TextInput | Role::MultilineTextInput | Role::SearchInput | Role::PasswordInput
            ) && node.placeholder().is_some_and(|p| !p.trim().is_empty()))
            || (role == Role::Label && node.value().is_some_and(|v| !v.trim().is_empty()));
        if labelled {
            continue;
        }
        let bounds = node.bounds().map_or([0; 4], |b| {
            [b.x0 as i32, b.y0 as i32, (b.x1 - b.x0) as i32, (b.y1 - b.y0) as i32]
        });
        out.push(Unnamed {
            role,
            bounds,
            hint: text_of(node),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wandur_term::TermSize;

    #[test]
    fn the_audit_finds_controls_without_a_name() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let _ = ui.button("Named");
                let mut on = true;
                ui.checkbox(&mut on, "Also named");
                // A painted icon with a click sense and nothing else: unnamed.
                let _ = ui.allocate_response(egui::vec2(16.0, 16.0), egui::Sense::click());
                // The same icon named for screen readers.
                let icon = ui.allocate_response(egui::vec2(16.0, 16.0), egui::Sense::click());
                crate::widgets::name(&icon, egui::WidgetType::Button, "Close");
                let mut text = String::new();
                ui.add(egui::TextEdit::singleline(&mut text).hint_text("Search"));
                let mut bare = String::new();
                ui.text_edit_singleline(&mut bare);
            });
        });
        out.textures_delta.clear();
        let found = unnamed(&out.platform_output.accesskit_update.unwrap());
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().any(|u| u.role == Role::Unknown));
        assert!(found.iter().any(|u| u.role == Role::TextInput));
    }

    #[test]
    fn screen_and_latest_text_come_from_the_grid() {
        let mut t = Terminal::new(TermSize::new(20, 4), 10);
        t.feed("one\r\n\x1b[31mtwo\x1b[0m\r\n漢字 ok\r\n".as_bytes());
        assert_eq!(screen_text(&t), "one\ntwo\n漢字 ok");
        assert_eq!(latest_lines(&t, 2), "two\n漢字 ok");
    }

    #[test]
    fn nodes_are_built_only_while_accesskit_is_on() {
        let ctx = egui::Context::default();
        let mut t = Terminal::new(TermSize::new(20, 4), 10);
        t.feed(b"hello there\r\n");
        let run = |ctx: &egui::Context, t: &Terminal| {
            let out = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let r = ui.allocate_response(egui::vec2(100.0, 50.0), egui::Sense::click());
                    expose_terminal(ui.ctx(), r.id, t);
                });
            });
            let mut out = out;
            out.textures_delta.clear();
            out.platform_output.accesskit_update
        };
        assert!(run(&ctx, &t).is_none(), "off unless a screen reader asks");
        ctx.enable_accesskit();
        let update = run(&ctx, &t).expect("a tree update");
        let values: Vec<String> = update
            .nodes
            .iter()
            .filter_map(|(_, n)| n.value().map(str::to_string))
            .collect();
        assert!(values.iter().any(|v| v == "hello there"), "{values:?}");
        let live = update.nodes.iter().find(|(_, n)| n.live() == Some(Live::Polite));
        assert!(live.is_some(), "a live region for new output");
    }
}
