//! The Markdown editor of agent goals (the C# `MarkdownCodeEditor`): a monospace editor with
//! line numbers, word wrap, undo and redo (egui's), and Markdown colouring for headings, list
//! and quote markers, emphasis, strong text, links, inline code and fenced code.
//!
//! The colouring is a small lexical pass per line, as the C# highlighting definition is, not a
//! Markdown parser: what it colours is what a person reads as Markdown while typing.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Ui};

use crate::theme::Theme;

/// What a stretch of Markdown is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Md {
    Plain,
    Heading,
    /// A list bullet or number, or a quote mark.
    Marker,
    Emphasis,
    Strong,
    Code,
    Link,
}

/// The colour of a token (light and dark sets, after the C# definition).
pub fn color(token: Md, light: bool, plain: Color32) -> Color32 {
    match (token, light) {
        (Md::Plain, _) => plain,
        (Md::Heading, true) => Color32::from_rgb(0x1f, 0x4e, 0x9c),
        (Md::Heading, false) => Color32::from_rgb(0x8c, 0xb4, 0xff),
        (Md::Marker, true) => Color32::from_rgb(0xb3, 0x5c, 0x00),
        (Md::Marker, false) => Color32::from_rgb(0xff, 0xb8, 0x6c),
        (Md::Emphasis | Md::Strong, true) => Color32::from_rgb(0x7a, 0x3e, 0x9d),
        (Md::Emphasis | Md::Strong, false) => Color32::from_rgb(0xd2, 0xa8, 0xff),
        (Md::Code, true) => Color32::from_rgb(0x2e, 0x7d, 0x32),
        (Md::Code, false) => Color32::from_rgb(0x9c, 0xd6, 0x8b),
        (Md::Link, true) => Color32::from_rgb(0x00, 0x6b, 0xa6),
        (Md::Link, false) => Color32::from_rgb(0x6c, 0xc8, 0xff),
    }
}

/// The tokens of `text` as (start, end, kind) byte ranges covering it all, in order.
pub fn tokens(text: &str) -> Vec<(usize, usize, Md)> {
    let mut out = Vec::new();
    let mut fenced = false;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let body = line.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            push(&mut out, start, at, Md::Code);
            continue;
        }
        if fenced {
            push(&mut out, start, at, Md::Code);
            continue;
        }
        let indent = body.len() - trimmed.len();
        let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
        if indent <= 3 && (1..=6).contains(&hashes) && trimmed[hashes..].chars().next().is_none_or(char::is_whitespace)
        {
            push(&mut out, start, at, Md::Heading);
            continue;
        }
        let marker = marker_len(trimmed);
        if marker > 0 {
            push(&mut out, start, start + indent, Md::Plain);
            push(&mut out, start + indent, start + indent + marker, Md::Marker);
            inline(&mut out, line, start + indent + marker, start);
        } else {
            inline(&mut out, line, start, start);
        }
    }
    out
}

/// A list bullet (`- `, `* `, `+ `), a number (`1. `, `2) `) or a quote mark (`> `), with its
/// space.
fn marker_len(line: &str) -> usize {
    let b = line.as_bytes();
    match b.first() {
        Some(b'-' | b'*' | b'+') if matches!(b.get(1), Some(b' ' | b'\t')) => 2,
        Some(b'>') => 1 + usize::from(matches!(b.get(1), Some(b' '))),
        Some(c) if c.is_ascii_digit() => {
            let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
            if digits <= 9
                && matches!(b.get(digits), Some(b'.' | b')'))
                && matches!(b.get(digits + 1), Some(b' ' | b'\t'))
            {
                digits + 2
            } else {
                0
            }
        }
        _ => 0,
    }
}

fn push(out: &mut Vec<(usize, usize, Md)>, start: usize, end: usize, kind: Md) {
    if start >= end {
        return;
    }
    if let Some(last) = out.last_mut()
        && last.1 == start
        && last.2 == kind
    {
        last.1 = end;
        return;
    }
    out.push((start, end, kind));
}

/// Inline spans of `line` (which starts at byte `base` of the text) from byte `from`.
fn inline(out: &mut Vec<(usize, usize, Md)>, line: &str, from: usize, base: usize) {
    let bytes = line.as_bytes();
    let mut i = from - base;
    let mut plain = i;
    let end = line.len();
    while i < end {
        let found = match bytes[i] {
            b'`' => line[i + 1..].find('`').map(|j| (i + 1 + j + 1, Md::Code)),
            b'*' | b'_' if bytes.get(i + 1) == Some(&bytes[i]) => {
                let close = if bytes[i] == b'*' { "**" } else { "__" };
                line[i + 2..]
                    .find(close)
                    .filter(|&j| j > 0)
                    .map(|j| (i + 2 + j + 2, Md::Strong))
            }
            b'*' | b'_' => {
                let close = bytes[i] as char;
                line[i + 1..]
                    .find(close)
                    .filter(|&j| j > 0 && !line[i + 1..i + 1 + j].contains('\n'))
                    .map(|j| (i + 1 + j + 1, Md::Emphasis))
            }
            b'[' => line[i..].find("](").and_then(|j| {
                let after = i + j + 2;
                line[after..].find(')').map(|k| (after + k + 1, Md::Link))
            }),
            _ => None,
        };
        match found {
            Some((stop, kind)) => {
                push(out, base + plain, base + i, Md::Plain);
                push(out, base + i, base + stop, kind);
                i = stop;
                plain = stop;
            }
            None => {
                i += line[i..].chars().next().map_or(1, char::len_utf8);
            }
        }
    }
    push(out, base + plain, base + end, Md::Plain);
}

/// The coloured layout of `text`.
pub fn layout_job(text: &str, light: bool, plain: Color32, font: FontId, wrap: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    for (start, end, kind) in tokens(text) {
        job.append(
            &text[start..end],
            0.0,
            TextFormat {
                font_id: font.clone(),
                color: color(kind, light, plain),
                italics: kind == Md::Emphasis,
                ..TextFormat::default()
            },
        );
    }
    job
}

/// The editor, `height` points tall. Returns the text edit's response.
pub fn markdown_editor(ui: &mut Ui, id: egui::Id, text: &mut String, theme: &Theme, height: f32) -> egui::Response {
    let font = FontId::monospace(13.0);
    let light = theme.light;
    let background = crate::script_editor::editor_background(theme);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, background);
    let lines = text.split('\n').count().max(1);
    let digits = lines.to_string().len().max(1);
    let char_width = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let gutter = digits as f32 * char_width + 18.0;
    let plain = theme.text;
    let layout_font = font.clone();
    let mut layouter = move |ui: &Ui, source: &dyn egui::TextBuffer, wrap: f32| {
        let job = layout_job(source.as_str(), light, plain, layout_font.clone(), wrap);
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let mut response = None;
    let inner = egui::UiBuilder::new()
        .max_rect(rect)
        .layout(egui::Layout::top_down(egui::Align::Min));
    ui.scope_builder(inner, |ui| {
        egui::ScrollArea::vertical()
            .id_salt(id.with("scroll"))
            .auto_shrink(false)
            .max_height(height)
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let (gutter_rect, _) = ui.allocate_exact_size(
                        egui::vec2(gutter, ui.available_height().max(height)),
                        egui::Sense::hover(),
                    );
                    let shown = egui::TextEdit::multiline(text)
                        .id(id)
                        .font(font.clone())
                        .lock_focus(true)
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::symmetric(6, 8))
                        .desired_width((width - gutter - 16.0).max(40.0))
                        .desired_rows(3)
                        .layouter(&mut layouter)
                        .show(ui);
                    crate::a11y::label(&shown.response, &crate::a11y::pending(ui));
                    let row_height = ui.fonts_mut(|f| f.row_height(&font));
                    let painter = ui.painter_at(gutter_rect.expand2(egui::vec2(0.0, 4000.0)));
                    let mut line = 1;
                    let mut new_line = true;
                    for row in &shown.galley.rows {
                        if new_line {
                            let y = shown.galley_pos.y + row.min_y();
                            painter.text(
                                egui::pos2(gutter_rect.right() - 8.0, y + row_height / 2.0),
                                egui::Align2::RIGHT_CENTER,
                                line.to_string(),
                                font.clone(),
                                theme.muted,
                            );
                            line += 1;
                        }
                        new_line = row.ends_with_newline;
                    }
                    painter.vline(
                        gutter_rect.right() - 2.0,
                        gutter_rect.y_range(),
                        egui::Stroke::new(1.0, theme.border.gamma_multiply(0.6)),
                    );
                    response = Some(shown.response.response);
                });
            });
    });
    response.expect("the editor was shown")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(&str, Md)> {
        tokens(text)
            .into_iter()
            .filter(|(_, _, k)| *k != Md::Plain)
            .map(|(s, e, k)| (&text[s..e], k))
            .collect()
    }

    #[test]
    fn the_tokens_cover_the_text_in_order() {
        let text = "# Custom objective\nFind **a room** with `look`.\n- one\n";
        let tokens = tokens(text);
        let mut at = 0;
        for (s, e, _) in &tokens {
            assert_eq!(*s, at);
            at = *e;
        }
        assert_eq!(at, text.len());
    }

    /// AgentSettingsTests: headings, emphasis and inline code are highlighted.
    #[test]
    fn markdown_is_coloured() {
        assert_eq!(
            kinds("# Custom objective\nFind **a room** with `look`."),
            [
                ("# Custom objective\n", Md::Heading),
                ("**a room**", Md::Strong),
                ("`look`", Md::Code)
            ]
        );
        assert_eq!(
            kinds("- **Do not move**, attack.\n2. Then *stop*.\n> note"),
            [
                ("- ", Md::Marker),
                ("**Do not move**", Md::Strong),
                ("2. ", Md::Marker),
                ("*stop*", Md::Emphasis),
                ("> ", Md::Marker),
            ]
        );
        assert_eq!(
            kinds("See [the map](https://example.org/map)."),
            [("[the map](https://example.org/map)", Md::Link)]
        );
        assert_eq!(
            kinds("```json\n{\"a\": 1}\n```\nafter"),
            [("```json\n{\"a\": 1}\n```\n", Md::Code)]
        );
        assert!(kinds("#hashtag and 3 * 4 * 5").iter().all(|(_, k)| *k == Md::Emphasis));
        assert!(kinds("plain words, nothing more").is_empty());
        // Every template's Markdown has something coloured.
        for (key, _) in wandur_core::agent::profile::TEMPLATES {
            let goal = wandur_core::agent::profile::template(key).unwrap();
            assert!(!kinds(&goal.text).is_empty());
            assert!(!kinds(&goal.rules).is_empty());
        }
    }
}
