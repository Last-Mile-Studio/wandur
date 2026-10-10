//! The script code editor (the C# `ScriptCodeEditor`): a monospace editor with line numbers,
//! JavaScript colouring, four-space indentation and the API completion list.
//!
//! The colouring is a lexical grammar, as the C# one is, not a JavaScript parser: comments,
//! strings (template literals re-enter expression colouring inside `${ }`), regex literals,
//! keywords, numbers and the scripting API (`mud`, `Events`, and `send`, `echo`, `alias`,
//! `trigger`, `every`, `on`, `Line`, `Gmcp` after a dot). Colours follow the C# grammar's light
//! and dark sets. A Lua script gets the C# Lua grammar instead (comments, short and long
//! strings, keywords, numbers and the same API names, plus `after`, `remove`, `regex` and
//! `match` after a dot or a colon).
//!
//! Completion opens after typing `.` or with Ctrl+Space; Up and Down choose, Tab or Enter accept,
//! Escape cancels, and typing a character that cannot continue a name accepts first (as C#).
//!
//! A JavaScript script is shown formatted ([`Shown`], `wandur-format`) without changing what is
//! stored until the person edits it.

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{Color32, FontId, Key, Modifiers, Ui};
use wandur_core::scripting::completion::{self, Suggestion};

use crate::theme::Theme;

/// What a stretch of source is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Plain,
    Keyword,
    Comment,
    String,
    Number,
    Regex,
    Api,
    Escape,
}

const KEYWORDS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "null",
    "of",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// Names coloured as the API after a dot.
const API_MEMBERS: &[&str] = &["send", "echo", "alias", "trigger", "every", "on", "Line", "Gmcp"];

const LUA_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in", "local", "nil",
    "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Names coloured as the API after a dot or a colon in Lua.
const LUA_API_MEMBERS: &[&str] = &[
    "send", "echo", "alias", "trigger", "every", "after", "remove", "on", "regex", "match", "Line", "Gmcp",
];

/// The colour of a token, in the C# grammar's light (paper) or dark set.
pub fn color(token: Token, light: bool, plain: Color32) -> Color32 {
    let pick = |dark: u32, paper: u32| {
        let c = if light { paper } else { dark };
        Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
    };
    match token {
        Token::Plain => plain,
        Token::Keyword => pick(0xD2A8FF, 0x8250DF),
        Token::Comment => pick(0x8B949E, 0x57606A),
        Token::String => pick(0xA5D6FF, 0x0A3069),
        Token::Number => pick(0x79C0FF, 0x0550AE),
        Token::Regex => pick(0x7EE787, 0x116329),
        Token::Api | Token::Escape => pick(0xFFA657, 0x953800),
    }
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c >= 0x80
}

/// The source's tokens as (start, end, kind) byte ranges, in order, covering all of it.
pub fn tokens(text: &str) -> Vec<(usize, usize, Token)> {
    tokens_in(text, false)
}

/// The tokens of JavaScript, or of Lua when `lua`.
pub fn tokens_in(text: &str, lua: bool) -> Vec<(usize, usize, Token)> {
    let mut out = Vec::new();
    if lua {
        lex_lua(text.as_bytes(), &mut out);
    } else {
        lex(text.as_bytes(), 0, text.len(), &mut out);
    }
    // Merge neighbours of one kind and fill gaps with plain text.
    let mut merged: Vec<(usize, usize, Token)> = Vec::with_capacity(out.len());
    let mut at = 0;
    for (start, end, kind) in out {
        if start > at {
            push(&mut merged, at, start, Token::Plain);
        }
        push(&mut merged, start, end, kind);
        at = end;
    }
    if at < text.len() {
        push(&mut merged, at, text.len(), Token::Plain);
    }
    // Ranges must fall on character boundaries for the layout.
    merged.retain(|(s, e, _)| s < e);
    merged
}

fn push(out: &mut Vec<(usize, usize, Token)>, start: usize, end: usize, kind: Token) {
    if let Some(last) = out.last_mut()
        && last.2 == kind
        && last.1 == start
    {
        last.1 = end;
        return;
    }
    out.push((start, end, kind));
}

fn lex(b: &[u8], from: usize, to: usize, out: &mut Vec<(usize, usize, Token)>) {
    let mut i = from;
    while i < to {
        let c = b[i];
        let next = if i + 1 < to { b[i + 1] } else { 0 };
        if c == b'/' && next == b'/' {
            let end = b[i..to].iter().position(|&x| x == b'\n').map_or(to, |p| i + p);
            out.push((i, end, Token::Comment));
            i = end;
        } else if c == b'/' && next == b'*' {
            let end = find(b, i + 2, to, b"*/").map_or(to, |p| p + 2);
            out.push((i, end, Token::Comment));
            i = end;
        } else if c == b'"' || c == b'\'' {
            i = string(b, i, to, c, out);
        } else if c == b'`' {
            i = template(b, i, to, out);
        } else if c == b'/'
            && let Some(end) = regex_literal(b, i, to)
        {
            out.push((i, end, Token::Regex));
            i = end;
        } else if c.is_ascii_digit() && (i == 0 || !is_word(b[i - 1])) {
            let end = number(b, i, to);
            out.push((i, end, Token::Number));
            i = end;
        } else if is_ident_start(c) && (i == 0 || !(is_word(b[i - 1]) || b[i - 1] == b'$')) {
            let mut end = i + 1;
            while end < to && (is_word(b[end]) || b[end] == b'$') {
                end += 1;
            }
            let word = std::str::from_utf8(&b[i..end]).unwrap_or("");
            let kind = if KEYWORDS.contains(&word) {
                Token::Keyword
            } else if word == "mud" || word == "Events" || (i > 0 && b[i - 1] == b'.' && API_MEMBERS.contains(&word)) {
                Token::Api
            } else {
                Token::Plain
            };
            if kind != Token::Plain {
                out.push((i, end, kind));
            }
            i = end;
        } else {
            i += 1;
            while i < to && !b.is_char_boundary_at(i) {
                i += 1;
            }
        }
    }
}

/// The level of a Lua long bracket at `at` (`[[` is 0, `[=[` is 1), if one opens there.
fn long_level(b: &[u8], at: usize) -> Option<usize> {
    let mut i = at + 1;
    while i < b.len() && b[i] == b'=' {
        i += 1;
    }
    (b.get(at) == Some(&b'[') && b.get(i) == Some(&b'[')).then_some(i - at - 1)
}

/// The end of a long bracket opened at `at` with `level`: after its closing bracket, or the end.
fn long_end(b: &[u8], at: usize, level: usize) -> usize {
    let mut close = vec![b']'];
    close.extend(std::iter::repeat_n(b'=', level));
    close.push(b']');
    find(b, at + level + 2, b.len(), &close).map_or(b.len(), |p| p + close.len())
}

fn lex_lua(b: &[u8], out: &mut Vec<(usize, usize, Token)>) {
    let to = b.len();
    let mut i = 0;
    while i < to {
        let c = b[i];
        let next = if i + 1 < to { b[i + 1] } else { 0 };
        if c == b'-' && next == b'-' {
            let end = match long_level(b, i + 2) {
                Some(level) => long_end(b, i + 2, level),
                None => b[i..].iter().position(|&x| x == b'\n').map_or(to, |p| i + p),
            };
            out.push((i, end, Token::Comment));
            i = end;
        } else if c == b'"' || c == b'\'' {
            i = string(b, i, to, c, out);
        } else if let Some(level) = long_level(b, i) {
            let end = long_end(b, i, level);
            out.push((i, end, Token::String));
            i = end;
        } else if c.is_ascii_digit() && (i == 0 || !is_word(b[i - 1])) {
            let end = number(b, i, to);
            out.push((i, end, Token::Number));
            i = end;
        } else if (c.is_ascii_alphabetic() || c == b'_' || c >= 0x80) && (i == 0 || !is_word(b[i - 1])) {
            let mut end = i + 1;
            while end < to && is_word(b[end]) {
                end += 1;
            }
            let word = std::str::from_utf8(&b[i..end]).unwrap_or("");
            let after_member = i > 0 && (b[i - 1] == b'.' || b[i - 1] == b':');
            let kind = if LUA_KEYWORDS.contains(&word) {
                Token::Keyword
            } else if word == "mud" || word == "Events" || (after_member && LUA_API_MEMBERS.contains(&word)) {
                Token::Api
            } else {
                Token::Plain
            };
            if kind != Token::Plain {
                out.push((i, end, kind));
            }
            i = end;
        } else {
            i += 1;
            while i < to && !b.is_char_boundary_at(i) {
                i += 1;
            }
        }
    }
}

trait Boundary {
    fn is_char_boundary_at(&self, i: usize) -> bool;
}

impl Boundary for [u8] {
    fn is_char_boundary_at(&self, i: usize) -> bool {
        i >= self.len() || (self[i] as i8) >= -0x40
    }
}

fn find(b: &[u8], from: usize, to: usize, needle: &[u8]) -> Option<usize> {
    (from..to.saturating_sub(needle.len() - 1)).find(|&p| b[p..].starts_with(needle))
}

/// A quoted string that ends at its quote or at the end of the line; escapes stand out.
fn string(b: &[u8], start: usize, to: usize, quote: u8, out: &mut Vec<(usize, usize, Token)>) -> usize {
    let mut i = start + 1;
    let mut run = start;
    while i < to && b[i] != quote && b[i] != b'\n' {
        if b[i] == b'\\' && i + 1 < to {
            out.push((run, i, Token::String));
            let mut end = i + 2;
            while end < to && !b.is_char_boundary_at(end) {
                end += 1;
            }
            out.push((i, end, Token::Escape));
            i = end;
            run = i;
        } else {
            i += 1;
        }
    }
    let end = if i < to && b[i] == quote { i + 1 } else { i };
    out.push((run, end, Token::String));
    end
}

/// A template literal: may span lines; `${ }` holds an expression coloured as code.
fn template(b: &[u8], start: usize, to: usize, out: &mut Vec<(usize, usize, Token)>) -> usize {
    let mut i = start + 1;
    let mut run = start;
    while i < to && b[i] != b'`' {
        if b[i] == b'\\' && i + 1 < to {
            out.push((run, i, Token::String));
            let mut end = i + 2;
            while end < to && !b.is_char_boundary_at(end) {
                end += 1;
            }
            out.push((i, end, Token::Escape));
            i = end;
            run = i;
        } else if b[i] == b'$' && i + 1 < to && b[i + 1] == b'{' {
            out.push((run, i + 2, Token::String));
            let mut depth = 1;
            let mut end = i + 2;
            while end < to && depth > 0 {
                match b[end] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                if depth > 0 {
                    end += 1;
                }
            }
            lex(b, i + 2, end, out);
            if end < to {
                out.push((end, end + 1, Token::String));
                end += 1;
            }
            i = end;
            run = i;
        } else {
            i += 1;
        }
    }
    let end = if i < to { i + 1 } else { i };
    out.push((run, end, Token::String));
    end
}

/// The C# rule: a `/` not right after a word character, `]` or `)`, not starting a comment,
/// whose body (escapes and classes allowed) closes on the same line, then its flags.
fn regex_literal(b: &[u8], start: usize, to: usize) -> Option<usize> {
    if start > 0 && (is_word(b[start - 1]) || b[start - 1] == b']' || b[start - 1] == b')') {
        return None;
    }
    let mut i = start + 1;
    if i >= to || b[i] == b'/' || b[i] == b'*' {
        return None;
    }
    let mut body = 0;
    loop {
        if i >= to || b[i] == b'\n' || b[i] == b'\r' {
            return None;
        }
        match b[i] {
            b'\\' => {
                if i + 1 >= to || b[i + 1] == b'\n' {
                    return None;
                }
                i += 2;
            }
            b'[' => {
                i += 1;
                while i < to && b[i] != b']' {
                    if b[i] == b'\n' || b[i] == b'\r' {
                        return None;
                    }
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                if i >= to {
                    return None;
                }
                i += 1;
            }
            b'/' => {
                if body == 0 {
                    return None;
                }
                i += 1;
                while i < to && b"dgimsuvy".contains(&b[i]) {
                    i += 1;
                }
                return Some(i.min(to));
            }
            _ => i += 1,
        }
        body += 1;
    }
}

fn number(b: &[u8], start: usize, to: usize) -> usize {
    let mut i = start + 1;
    let digit = |c: u8| c.is_ascii_hexdigit() || c == b'_';
    if b[start] == b'0' && i < to && matches!(b[i], b'x' | b'X' | b'b' | b'B' | b'o' | b'O') {
        i += 1;
        while i < to && digit(b[i]) {
            i += 1;
        }
        return i;
    }
    while i < to && (b[i].is_ascii_digit() || b[i] == b'_') {
        i += 1;
    }
    if i + 1 < to && b[i] == b'.' && b[i + 1].is_ascii_digit() {
        i += 1;
        while i < to && (b[i].is_ascii_digit() || b[i] == b'_') {
            i += 1;
        }
    }
    if i < to && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < to && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < to && b[j].is_ascii_digit() {
            i = j;
            while i < to && (b[i].is_ascii_digit() || b[i] == b'_') {
                i += 1;
            }
        }
    }
    if i < to && b[i] == b'n' {
        i += 1;
    }
    i
}

/// The coloured layout of `text` (Lua when `lua`).
pub fn layout_job(text: &str, lua: bool, light: bool, plain: Color32, font: FontId, wrap: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    for (start, end, kind) in tokens_in(text, lua) {
        let format = TextFormat {
            font_id: font.clone(),
            color: color(kind, light, plain),
            ..TextFormat::default()
        };
        // Each operator character is a section of its own, so the monospace font's ligatures
        // (`=>` as an arrow) do not join them: code shows what was typed, as in C#.
        let piece = &text[start..end];
        let mut from = 0;
        for (i, c) in piece.char_indices() {
            if "=<>!-+*/|&:.~^%?".contains(c) {
                if from < i {
                    section(&mut job, &piece[from..i], &format);
                }
                section(&mut job, &piece[i..i + 1], &format);
                from = i + 1;
            }
        }
        if from < piece.len() {
            section(&mut job, &piece[from..], &format);
        }
    }
    job
}

/// A section of its own (`LayoutJob::append` merges equal neighbours, which would let the font
/// shape them together).
fn section(job: &mut LayoutJob, text: &str, format: &TextFormat) {
    let start = job.text.len();
    job.text.push_str(text);
    job.sections.push(egui::text::LayoutSection {
        leading_space: 0.0,
        byte_range: egui::text::ByteIndex(start)..egui::text::ByteIndex(job.text.len()),
        format: format.clone(),
    });
}

/// Whether the caret at byte `at` sits in a comment, a string or a regex (no completion there).
pub fn in_literal(text: &str, at: usize) -> bool {
    in_literal_of(text, at, false)
}

fn in_literal_of(text: &str, at: usize, lua: bool) -> bool {
    tokens_in(text, lua).into_iter().any(|(s, e, k)| {
        s < at && at <= e && matches!(k, Token::Comment | Token::String | Token::Regex | Token::Escape)
    })
}

/// The text the editor shows for a script: its source formatted for reading, so a script stored
/// on one long line reads with line breaks. Showing it changes nothing: the draft keeps the stored
/// source until the person edits, and from then on holds the edited text (formatted, with the
/// edit). A Lua script, or a source that does not parse, is too large or too slow to format, is
/// shown as stored.
#[derive(Clone, Debug, Default)]
pub struct Shown {
    /// The script's id.
    pub id: String,
    /// The source as the draft held it when this was made.
    pub stored: String,
    /// The formatted source, when formatting applied.
    pub formatted: Option<std::sync::Arc<str>>,
    /// The editor's buffer: the formatted source, then the person's edits of it.
    pub text: String,
}

impl Shown {
    pub fn new(id: &str, source: &str, lua: bool) -> Self {
        let formatted = formatted(source, lua);
        Self {
            id: id.to_string(),
            stored: source.to_string(),
            text: formatted.as_deref().unwrap_or(source).to_string(),
            formatted,
        }
    }

    /// Whether this still shows script `id` whose draft source is `source` (as stored, or as
    /// edited here).
    pub fn shows(&self, id: &str, source: &str) -> bool {
        self.id == id && (source == self.stored || source == self.text)
    }

    /// The source the draft should hold: the stored one while the text is still the untouched
    /// formatted view (viewing is not a change), else the text.
    pub fn source(&self) -> &str {
        if self.formatted.as_deref() == Some(self.text.as_str()) {
            &self.stored
        } else {
            &self.text
        }
    }
}

/// `source` formatted for the editor (cached for the session), or `None` to show it as stored.
pub fn formatted(source: &str, lua: bool) -> Option<std::sync::Arc<str>> {
    if lua {
        return None;
    }
    wandur_format::shown_javascript(source)
}

/// The text the editor shows for `source`.
pub fn shown_text(source: &str, lua: bool) -> String {
    formatted(source, lua).as_deref().unwrap_or(source).to_string()
}

/// The open completion list.
#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    pub suggestions: Vec<Suggestion>,
    pub selected: usize,
    /// Characters before the caret that the chosen name replaces.
    pub prefix_len: usize,
}

/// The editor's state between frames.
#[derive(Clone, Debug, Default)]
pub struct EditorState {
    pub completion: Option<Completion>,
    /// Open the completion list at the caret in the next frame (a scene, or Ctrl+Space).
    pub request_completion: bool,
    /// Put the caret here (a character index) in the next frame.
    pub set_caret: Option<usize>,
    /// Where the caret was last frame (a character index).
    pub caret: usize,
    /// The source is Lua (its colouring and where completion opens).
    pub lua: bool,
    /// A read-only editor was typed, pasted or deleted in this frame: the events that tried.
    pub blocked_edit: Option<Vec<egui::Event>>,
    /// Events to give the editor once it has the focus again (the keystroke that asked for an
    /// editable copy of a pack script, replayed into the copy).
    pub replay: Vec<egui::Event>,
}

/// The events in `events` that would change the text of a focused editor: typing, pasting,
/// cutting, Backspace, Delete and Enter (Tab only moves the focus out of a read-only editor).
pub fn edit_events(events: &[egui::Event]) -> Vec<egui::Event> {
    events
        .iter()
        .filter(|e| match e {
            egui::Event::Text(text) => !text.is_empty(),
            egui::Event::Paste(text) => !text.is_empty(),
            egui::Event::Cut => true,
            egui::Event::Key {
                key: Key::Backspace | Key::Delete | Key::Enter,
                pressed: true,
                modifiers,
                ..
            } => !modifiers.alt && !modifiers.ctrl,
            _ => false,
        })
        .cloned()
        .collect()
}

/// The suggestions for the caret at character `caret`, or `None` when there are none (or the
/// caret is in a comment, string or regex).
pub fn suggestions_at(text: &str, caret: usize) -> Option<Completion> {
    suggestions_in(text, caret, false)
}

fn suggestions_in(text: &str, caret: usize, lua: bool) -> Option<Completion> {
    let at = text.char_indices().nth(caret).map_or(text.len(), |(i, _)| i);
    if at > 0 && in_literal_of(text, at, lua) {
        return None;
    }
    let context = completion::complete(&text[..at]);
    (!context.suggestions.is_empty()).then_some(Completion {
        suggestions: context.suggestions,
        selected: 0,
        prefix_len: context.prefix_len,
    })
}

/// Replace the prefix before the caret with `name`; returns the new caret (characters).
pub fn accept(text: &mut String, caret: usize, prefix_len: usize, name: &str) -> usize {
    let at = text.char_indices().nth(caret).map_or(text.len(), |(i, _)| i);
    let start = at.saturating_sub(prefix_len);
    text.replace_range(start..at, name);
    text[..start].chars().count() + name.chars().count()
}

/// Height of a completion row.
const ROW: f32 = 37.0;
/// Width of the completion list (C# 260 to 420).
const POPUP_WIDTH: f32 = 260.0;

/// The code editor. Returns the text edit's response (`changed` when the text changed).
pub fn code_editor(
    ui: &mut Ui,
    id: egui::Id,
    text: &mut String,
    read_only: bool,
    state: &mut EditorState,
    theme: &Theme,
) -> egui::Response {
    let font = FontId::monospace(13.0);
    let light = theme.light;
    let background = if read_only {
        read_only_background(theme)
    } else {
        editor_background(theme)
    };
    state.blocked_edit = None;
    let focused = ui.memory(|m| m.has_focus(id));
    if read_only && focused {
        // A read-only editor keeps its caret and selection (to read and copy), but typing in it
        // is reported so the caller can offer an editable copy.
        let tried = ui.input(|i| edit_events(&i.events));
        if !tried.is_empty() {
            state.blocked_edit = Some(tried);
        }
    } else if !read_only && focused && state.set_caret.is_none() && !state.replay.is_empty() {
        let events = std::mem::take(&mut state.replay);
        ui.input_mut(|i| i.events.extend(events));
    }
    let gutter_color = theme.muted;
    let rect = ui.available_rect_before_wrap();
    ui.painter().rect_filled(rect, 0.0, background);

    // Keys the completion list takes before the text edit sees them.
    let mut accepted: Option<(usize, String)> = None;
    if let Some(open) = &mut state.completion {
        let count = open.suggestions.len();
        ui.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                open.selected = (open.selected + 1).min(count - 1);
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                open.selected = open.selected.saturating_sub(1);
            }
            if i.consume_key(Modifiers::NONE, Key::Tab) || i.consume_key(Modifiers::NONE, Key::Enter) {
                accepted = Some((open.prefix_len, open.suggestions[open.selected].name.to_string()));
            }
        });
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            state.completion = None;
        }
        // A character that cannot continue a name accepts the choice first (as C#).
        if let Some(open) = &state.completion
            && accepted.is_none()
        {
            let typed = ui.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Text(t) => t.chars().next(),
                    _ => None,
                })
            });
            if let Some(c) = typed
                && !(c.is_alphanumeric() || c == '_' || c == '$')
            {
                accepted = Some((open.prefix_len, open.suggestions[open.selected].name.to_string()));
            }
        }
    }
    if ui.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::Space)) {
        state.request_completion = true;
    }
    if let Some((prefix_len, name)) = accepted
        && !read_only
    {
        state.set_caret = Some(accept(text, state.caret, prefix_len, &name));
        state.completion = None;
    }

    let lines = text.split('\n').count().max(1);
    let digits = lines.to_string().len().max(2);
    let char_width = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let gutter = digits as f32 * char_width + 22.0;
    let lua = state.lua;
    let mut layouter = |ui: &Ui, source: &dyn egui::TextBuffer, wrap: f32| {
        let job = layout_job(source.as_str(), lua, light, theme.text, font.clone(), wrap);
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let mut output = None;
    egui::ScrollArea::both()
        .id_salt(id.with("scroll"))
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let (gutter_rect, _) =
                    ui.allocate_exact_size(egui::vec2(gutter, ui.available_height()), egui::Sense::hover());
                // Read only: a `&str` buffer keeps the caret, selection and copy but takes no edits.
                let mut view: &str = text.as_str();
                let buffer: &mut dyn egui::TextBuffer = if read_only { &mut view } else { &mut *text };
                let edit = egui::TextEdit::multiline(buffer)
                    .id(id)
                    .font(font.clone())
                    .code_editor()
                    // Tab indents in an editable script; in a read-only one it moves on.
                    .lock_focus(!read_only)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::symmetric(4, 10))
                    .desired_width(f32::INFINITY)
                    .desired_rows(24)
                    .layouter(&mut layouter);
                let shown = edit.show(ui);
                crate::a11y::label(
                    &shown.response,
                    wandur_core::l10n::t(wandur_core::l10n::S::ScriptEditorTitle),
                );
                if read_only {
                    ui.ctx()
                        .accesskit_node_builder(shown.response.id, |node| node.set_read_only());
                }
                // Line numbers, right-aligned in the gutter, beside each logical line.
                let row_height = ui.fonts_mut(|f| f.row_height(&font));
                let painter = ui.painter_at(gutter_rect.expand2(egui::vec2(0.0, 4000.0)));
                let mut line = 1;
                let mut new_line = true;
                for row in &shown.galley.rows {
                    if new_line {
                        let y = shown.galley_pos.y + row.min_y();
                        painter.text(
                            egui::pos2(gutter_rect.right() - 12.0, y + row_height / 2.0),
                            egui::Align2::RIGHT_CENTER,
                            line.to_string(),
                            font.clone(),
                            gutter_color,
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
                output = Some(shown);
            });
        });
    let mut output = output.expect("the editor was shown");
    if let Some(caret) = state.set_caret.take() {
        let cursor = CCursorRange::one(CCursor::new(caret));
        output.state.cursor.set_char_range(Some(cursor));
        output.state.clone().store(ui.ctx(), id);
        output.response.request_focus();
        state.caret = caret;
    } else if let Some(range) = output.cursor_range {
        state.caret = range.primary.index.into();
    }
    let caret = state.caret;
    // Typing a dot opens the list; typing more narrows it; a list with nothing left closes.
    let before = text.chars().take(caret).last();
    if output.response.changed() && before == Some('.') && !read_only {
        state.request_completion = true;
    }
    if state.request_completion {
        state.request_completion = false;
        state.completion = suggestions_in(text, caret, state.lua);
    } else if state.completion.is_some() && output.response.changed() {
        let selected = state.completion.as_ref().map_or(0, |c| c.selected);
        state.completion = suggestions_in(text, caret, state.lua).map(|mut c| {
            c.selected = selected.min(c.suggestions.len() - 1);
            c
        });
    }
    if !output.response.has_focus() && state.completion.is_some() && ui.input(|i| i.pointer.any_pressed()) {
        state.completion = None;
    }
    if let Some(open) = &mut state.completion {
        let galley_caret = output.galley.pos_from_cursor(CCursor::new(caret));
        let anchor = output.galley_pos + galley_caret.left_bottom().to_vec2() + egui::vec2(0.0, 6.0);
        let mut chosen = None;
        egui::Area::new(id.with("completion"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(background)
                    .stroke(egui::Stroke::new(1.0, theme.border))
                    .shadow(egui::Shadow {
                        offset: [0, 3],
                        blur: 10,
                        spread: 0,
                        color: Color32::from_black_alpha(40),
                    })
                    .show(ui, |ui| {
                        ui.set_width(POPUP_WIDTH);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        egui::ScrollArea::vertical().max_height(ROW * 6.0).show(ui, |ui| {
                            for (i, suggestion) in open.suggestions.iter().enumerate() {
                                let (rect, response) =
                                    ui.allocate_exact_size(egui::vec2(POPUP_WIDTH, ROW), egui::Sense::click());
                                crate::a11y::toggle(
                                    &response,
                                    egui::accesskit::Role::ListBoxOption,
                                    suggestion.signature,
                                    i == open.selected,
                                );
                                if i == open.selected {
                                    ui.painter().rect_filled(rect, 0.0, theme.selection_fill());
                                }
                                ui.painter().text(
                                    egui::pos2(rect.left() + 28.0, rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    suggestion.signature,
                                    FontId::proportional(14.0),
                                    theme.text,
                                );
                                let response = response.on_hover_text(suggestion.description);
                                if i == open.selected {
                                    response.scroll_to_me(None);
                                }
                                if response.clicked() {
                                    chosen = Some(i);
                                }
                            }
                        });
                    });
            });
        if let Some(i) = chosen
            && !read_only
        {
            let name = open.suggestions[i].name;
            state.set_caret = Some(accept(text, caret, open.prefix_len, name));
            state.completion = None;
            ui.ctx().request_repaint();
        }
    }
    output.response.response
}

/// A read-only editor's background: the editor's, drawn a little toward the muted text so a
/// pack script reads as not editable.
pub fn read_only_background(theme: &Theme) -> Color32 {
    crate::theme::mix(editor_background(theme), theme.muted, 0.10)
}

/// The editor's background: lighter than the panel on light themes, darker on dark ones.
pub fn editor_background(theme: &Theme) -> Color32 {
    if theme.light {
        crate::theme::mix(theme.panel, Color32::WHITE, 0.55)
    } else {
        crate::theme::mix(theme.panel, Color32::BLACK, 0.25)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(&str, Token)> {
        tokens(text)
            .into_iter()
            .filter(|(_, _, k)| *k != Token::Plain)
            .map(|(s, e, k)| (&text[s..e], k))
            .collect()
    }

    #[test]
    fn the_grammar_colours_the_lantern_watch_script() {
        let text = r#"// Lantern watch
const watch = mud.panel("lantern-watch", { title: "Lantern \"watch\"", dock: "right" });
let oil = 8;
mud.trigger(/^Your lantern gutters/, () => { oil = Math.max(0, oil - 1); draw(); });
const half = oil/2/1;
mud.on(Events.Line, e => mud.send(`say ${e.text + 1}`));
"#;
        let k = kinds(text);
        assert_eq!(k[0], ("// Lantern watch", Token::Comment));
        assert!(k.contains(&("const", Token::Keyword)));
        assert!(k.contains(&("mud", Token::Api)));
        assert!(k.contains(&("\"lantern-watch\"", Token::String)));
        assert!(k.contains(&("\\\"", Token::Escape)));
        assert!(k.contains(&("8", Token::Number)));
        assert!(k.contains(&("trigger", Token::Api)));
        assert!(k.contains(&("/^Your lantern gutters/", Token::Regex)));
        assert!(
            !k.iter().any(|(t, _)| *t == "/2/"),
            "a division right after a word is not a regex"
        );
        assert!(k.contains(&("Events", Token::Api)));
        assert!(k.contains(&("Line", Token::Api)));
        assert!(k.contains(&("`say ${", Token::String)));
        assert!(k.contains(&("1", Token::Number)), "code inside ${{}} is coloured");
        assert!(!k.iter().any(|(t, _)| *t == "panel"), "panel is not in the C# API list");
        // Every byte is covered once, in order.
        let mut at = 0;
        for (s, e, _) in tokens(text) {
            assert_eq!(s, at);
            at = e;
        }
        assert_eq!(at, text.len());
    }

    #[test]
    fn unterminated_pieces_and_unicode_do_not_break_the_grammar() {
        for text in ["'open", "/* open", "`open ${ x", "x = /[a-", "é = \"ü\\é\"", "1e", "0x"] {
            let mut at = 0;
            for (s, e, _) in tokens(text) {
                assert_eq!(s, at, "{text}");
                assert!(text.is_char_boundary(e), "{text}");
                at = e;
            }
            assert_eq!(at, text.len(), "{text}");
        }
    }

    #[test]
    fn completion_opens_outside_literals_and_accepts_over_the_prefix() {
        let text = "mud.tr";
        let open = suggestions_at(text, 6).unwrap();
        assert_eq!(open.suggestions[0].name, "trigger");
        let mut edited = text.to_string();
        let caret = accept(&mut edited, 6, open.prefix_len, "trigger");
        assert_eq!((edited.as_str(), caret), ("mud.trigger", 11));
        assert!(suggestions_at("// mud.", 7).is_none());
        assert!(suggestions_at("'mud.", 5).is_none());
        assert_eq!(suggestions_at("é mud.", 6).unwrap().suggestions.len(), 11);
    }

    #[test]
    fn a_shown_script_keeps_its_stored_source_until_edited() {
        let stored = "mud.alias(/^lh$/,()=>mud.send('look'))";
        let mut shown = Shown::new("a", stored, false);
        assert_eq!(shown.text, "mud.alias(/^lh$/, () => mud.send(\"look\"));");
        assert!(shown.shows("a", stored));
        assert!(!shown.shows("b", stored));
        assert_eq!(shown.source(), stored, "viewing is not a change");
        // An edit: the draft takes the formatted text with the edit, and still matches.
        shown.text.push_str("\n// mine");
        assert_eq!(shown.source(), shown.text);
        assert!(shown.shows("a", &shown.text.clone()));
        // Undone back to the formatted view: the stored source again.
        shown.text.truncate(shown.text.len() - "\n// mine".len());
        assert_eq!(shown.source(), stored);
        // Lua and sources that do not parse are shown as stored.
        for (source, lua) in [("local x   =  1", true), ("mud.echo(", false)] {
            let shown = Shown::new("c", source, lua);
            assert_eq!(shown.text, source);
            assert!(shown.formatted.is_none());
            assert_eq!(shown.source(), source);
        }
    }

    #[test]
    fn the_lua_grammar_colours_a_lua_script() {
        let source = "-- Greet a guildmate\nmud.trigger(\"^(%w+)\", function(m)\n  mud.send('wave ' .. m[2]) --[[ long\n]] local s = [==[x]==] end)";
        let kinds: Vec<(&str, Token)> = tokens_in(source, true)
            .into_iter()
            .filter(|(_, _, k)| *k != Token::Plain)
            .map(|(s, e, k)| (&source[s..e], k))
            .collect();
        assert_eq!(kinds[0], ("-- Greet a guildmate", Token::Comment));
        assert!(kinds.contains(&("mud", Token::Api)));
        assert!(kinds.contains(&("trigger", Token::Api)));
        assert!(kinds.contains(&("function", Token::Keyword)));
        assert!(kinds.contains(&("2", Token::Number)));
        assert!(kinds.contains(&("--[[ long\n]]", Token::Comment)));
        assert!(kinds.contains(&("local", Token::Keyword)));
        assert!(kinds.contains(&("[==[x]==]", Token::String)));
        assert!(kinds.contains(&("end", Token::Keyword)));
        // JavaScript keywords are plain words in Lua, and Lua's are plain in JavaScript.
        assert!(
            tokens_in("const x = 1", true)
                .iter()
                .all(|(_, _, k)| *k != Token::Keyword)
        );
        assert!(
            tokens_in("local x = nil", false)
                .iter()
                .all(|(_, _, k)| *k != Token::Keyword)
        );
    }
}
