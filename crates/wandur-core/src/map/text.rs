//! Rooms read from plain text, for worlds that send no room data (the C# `TextRoomObserver`): a
//! conservative English room block, a title line (bold preferred), description lines and an
//! `Exits:` line or a list of `North - ...` lines. Arbitrary transcript text is never taken as
//! a room: no exits, no room. Movement failures ("You can't go that way") are noticed too.

use std::sync::LazyLock;

use regex::Regex;

use super::model::{RoomObservation, normalize_direction};

fn pattern(expression: &str) -> Regex {
    Regex::new(&format!("(?i){expression}")).expect("a valid pattern")
}

static DIRECTION_LINE: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^\s*(north(?:east|west)?|south(?:east|west)?|east|west|up|down|in|out)\s*[-–:]\s*.+$"));
static PROMPT: LazyLock<Regex> = LazyLock::new(|| pattern(r"^(?:>\s*$|(?:HP|Health)\s*[:=]\s*\d|\*\(\()"));
static FAILURE: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?:you (?:can't|cannot|can not) go (?:that way|there)|(?:door|gate) is (?:closed|locked)|you (?:fail|are unable) to (?:move|go)|alas, you cannot go)",
    )
});
static OCCUPANT: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?:\b(?:is|are) (?:standing|sitting|resting|sleeping|here)\b|\b(?:stands|sits|lies) here\b|^you see\b|^\[.*\]|\bsays[:,])",
    )
});

/// Words every failure phrase holds; most lines are ruled out by one fast search (this runs on
/// every line of a flood) before the pattern.
static FAILURE_WORDS: LazyLock<aho_corasick::AhoCorasick> = LazyLock::new(|| {
    aho_corasick::AhoCorasick::builder()
        .ascii_case_insensitive(true)
        .build([
            "can't go",
            "cannot go",
            "can not go",
            "door is",
            "gate is",
            "fail to",
            "unable to",
            "alas,",
        ])
        .expect("valid words")
});

/// Whether `text` says a move failed.
pub fn is_movement_failure(text: &str) -> bool {
    text.len() <= 32_768 && FAILURE_WORDS.is_match(text) && FAILURE.is_match(text)
}

/// The list after an exits heading (`Exits:` or `Obvious exits:`, any case), or `None` when the
/// line is not one (the C# pattern `^\s*(?:obvious\s+)?exits\s*:\s*(.*)$`, by hand: this runs
/// on every line).
fn exit_header(text: &str) -> Option<&str> {
    fn strip<'a>(s: &'a str, word: &str) -> Option<&'a str> {
        (s.len() >= word.len() && s.as_bytes()[..word.len()].eq_ignore_ascii_case(word.as_bytes()))
            .then(|| &s[word.len()..])
    }
    let mut rest = text.trim_start();
    if let Some(after) = strip(rest, "obvious") {
        let trimmed = after.trim_start();
        if trimmed.len() == after.len() {
            return None;
        }
        rest = trimmed;
    }
    let rest = strip(rest, "exits")?.trim_start();
    Some(rest.strip_prefix(':')?.trim_start())
}

/// A compass word, up, down, in or out, with the short forms, ignoring case (no allocation).
fn direction_word(word: &str) -> Option<&'static str> {
    const WORDS: [(&str, &str); 22] = [
        ("north", "north"),
        ("south", "south"),
        ("east", "east"),
        ("west", "west"),
        ("northeast", "northeast"),
        ("northwest", "northwest"),
        ("southeast", "southeast"),
        ("southwest", "southwest"),
        ("up", "up"),
        ("down", "down"),
        ("in", "in"),
        ("out", "out"),
        ("ne", "northeast"),
        ("nw", "northwest"),
        ("se", "southeast"),
        ("sw", "southwest"),
        ("n", "north"),
        ("s", "south"),
        ("e", "east"),
        ("w", "west"),
        ("u", "up"),
        ("d", "down"),
    ];
    WORDS
        .iter()
        .find(|(w, _)| w.eq_ignore_ascii_case(word))
        .map(|(_, d)| *d)
}

#[derive(Clone, Debug, Default)]
struct Line {
    text: String,
    /// Some of it was bold.
    bold: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Escape {
    #[default]
    None,
    Esc,
    Csi,
    Osc,
    OscEsc,
}

/// Most finished blocks held for the session between two feeds.
const MAX_BLOCKS: usize = 8;
/// Longest description taken from text (the tracker's limit, in UTF-16 units).
pub const MAX_DESCRIPTION: usize = 16_000;

/// A finished room block: its lines up to the exits (trimmed, non-empty) and the exits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextBlock {
    pub lines: Vec<String>,
    pub exits: Vec<String>,
}

impl TextBlock {
    /// The line that titles the room `name` (the last one, nearest the exits), if any.
    fn title_for(&self, name: &str) -> Option<usize> {
        self.lines
            .iter()
            .rposition(|line| is_title(line) && title_matches(line, name))
    }

    /// Whether the block shows the room `name`, with or without a description (brief mode).
    pub fn shows(&self, name: &str) -> bool {
        self.title_for(name).is_some()
    }

    /// The description under the title of the room `name`: the lines after it, without
    /// occupants and rules, joined, whitespace collapsed. `None` when no title matches, no
    /// description was shown (brief mode) or it is too long.
    pub fn description_for(&self, name: &str) -> Option<String> {
        let title = self.title_for(name)?;
        let description = collapse_whitespace(&description_lines(self.lines[title + 1..].iter().map(String::as_str)));
        (!description.is_empty() && description.encode_utf16().count() <= MAX_DESCRIPTION).then_some(description)
    }
}

/// Description lines joined by spaces: empty lines, rules (`----`) and occupants are left out.
fn description_lines<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    lines
        .map(str::trim)
        .filter(|t| !t.is_empty() && !t.chars().all(|c| matches!(c, '─' | '-' | '=')) && !OCCUPANT.is_match(t))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` trimmed, every run of whitespace inside it one space (the tracker's normalizer
/// without the lowercasing: this is what the person reads).
pub fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` without its trailing bracketed groups (`TRAINING: Bacta Tanks [Bacta]`: LOTJ's room
/// flags, which its `Room.Info` name leaves out).
fn without_flags(text: &str) -> &str {
    let mut s = text.trim_end();
    while s.ends_with(']')
        && let Some(open) = s.rfind('[')
    {
        let rest = s[..open].trim_end();
        if rest.is_empty() {
            break;
        }
        s = rest;
    }
    s
}

/// Whether a text title names the protocol room `name`: equal ignoring case and spacing, or
/// equal once trailing bracketed flags are left out of both.
pub fn title_matches(title: &str, name: &str) -> bool {
    let same = |a: &str, b: &str| {
        let (a, b) = (collapse_whitespace(a), collapse_whitespace(b));
        !a.is_empty() && a.to_lowercase() == b.to_lowercase()
    };
    same(title, name) || same(without_flags(title), without_flags(name))
}

#[derive(Debug, Default)]
pub struct TextRoomObserver {
    partial: Line,
    bold: bool,
    escape: Escape,
    params: String,
    block: std::collections::VecDeque<Line>,
    /// Emptied line buffers, reused for new lines.
    spare: Vec<String>,
    exits: Option<indexmap::IndexMap<String, Option<String>>>,
    /// The last feed held a movement failure.
    pub movement_failed: bool,
    /// Look for movement failures (only needed while a move or a walk waits for its answer;
    /// off, a flood's lines skip that search).
    pub watch_failures: bool,
    /// Keep each finished room block in [`Self::blocks`] (for protocol rooms without a
    /// description).
    pub keep_blocks: bool,
    /// Finished room blocks since the caller last took them (at most a few).
    pub blocks: Vec<TextBlock>,
}

impl TextRoomObserver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget everything seen (a command was sent: a new answer starts).
    pub fn reset(&mut self) {
        let (watch, keep) = (self.watch_failures, self.keep_blocks);
        *self = Self::default();
        self.watch_failures = watch;
        self.keep_blocks = keep;
    }

    /// End the block being read, as a prompt or a room change does: a block whose exits are
    /// listed is finished now rather than at the next line. Returns the rooms it completed.
    pub fn flush(&mut self) -> Vec<RoomObservation> {
        let mut result = Vec::new();
        if self.exits.as_ref().is_some_and(|e| !e.is_empty()) {
            self.complete(&mut result);
        }
        result
    }

    /// Read more server text; returns the rooms it completed.
    pub fn feed(&mut self, text: &str) -> Vec<RoomObservation> {
        self.movement_failed = false;
        let mut result = Vec::new();
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            match self.escape {
                Escape::Esc => {
                    self.escape = match b {
                        b'[' => {
                            self.params.clear();
                            Escape::Csi
                        }
                        b']' => Escape::Osc,
                        _ => Escape::None,
                    };
                    i += 1;
                    continue;
                }
                Escape::Csi => {
                    if (b'@'..=b'~').contains(&b) {
                        if b == b'm' {
                            self.apply_sgr();
                        }
                        self.escape = Escape::None;
                    } else if self.params.len() < 64 {
                        self.params.push(b as char);
                    }
                    i += 1;
                    continue;
                }
                Escape::Osc => {
                    self.escape = match b {
                        0x07 => Escape::None,
                        0x1b => Escape::OscEsc,
                        _ => Escape::Osc,
                    };
                    i += 1;
                    continue;
                }
                Escape::OscEsc => {
                    self.escape = if b == b'\\' { Escape::None } else { Escape::Osc };
                    i += 1;
                    continue;
                }
                Escape::None => {}
            }
            // Plain text up to the next control byte goes in at once (control bytes are ASCII,
            // so this always ends on a character boundary).
            // Lines end at a newline and colours start at an escape; any other control byte in
            // the run (a carriage return, a tab) is found by the second, shorter search.
            let stop = memchr::memchr2(0x1b, b'\n', &bytes[i..]).map_or(bytes.len(), |p| i + p);
            let end = bytes[i..stop]
                .iter()
                .position(|&c| c < 0x20 || c == 0x7f)
                .map_or(stop, |p| i + p);
            if end > i {
                self.push_str(&text[i..end]);
                i = end;
                continue;
            }
            match b {
                0x1b => {
                    // A whole CSI sequence in this text is read at once (the common case).
                    if bytes.get(i + 1) == Some(&b'[')
                        && let Some(len) = bytes[i + 2..].iter().position(|c| (b'@'..=b'~').contains(c))
                    {
                        let end = i + 2 + len;
                        if bytes[end] == b'm' {
                            self.apply_sgr_params(&text[i + 2..end]);
                        }
                        i = end + 1;
                        continue;
                    }
                    self.escape = Escape::Esc;
                }
                b'\n' => {
                    let spare = self
                        .spare
                        .pop()
                        .map(|text| Line { text, bold: false })
                        .unwrap_or_default();
                    let line = std::mem::replace(&mut self.partial, spare);
                    self.consume(line, &mut result);
                }
                b'\t' => self.push_str(" "),
                _ => {}
            }
            i += 1;
        }
        // LOTJ often ends the exits with an unterminated HP prompt.
        if self.exits.as_ref().is_some_and(|e| !e.is_empty()) && PROMPT.is_match(self.partial.text.trim_start()) {
            self.complete(&mut result);
        }
        result
    }

    fn push_str(&mut self, run: &str) {
        let room = 4096usize.saturating_sub(self.partial.text.len());
        if room == 0 {
            return;
        }
        let mut cut = run.len().min(room);
        while !run.is_char_boundary(cut) {
            cut -= 1;
        }
        let run = &run[..cut];
        self.partial.text.push_str(run);
        self.partial.bold |= self.bold && run.chars().any(|c| !c.is_whitespace());
    }

    fn apply_sgr(&mut self) {
        let params = std::mem::take(&mut self.params);
        self.apply_sgr_params(&params);
        self.params = params;
    }

    fn apply_sgr_params(&mut self, params: &str) {
        // Numbers separated by `;` or `:`, read without allocating; an empty one is 0.
        let mut numbers = params.split([';', ':']).map(|p| {
            p.bytes()
                .try_fold(0u32, |n, b| {
                    b.is_ascii_digit()
                        .then(|| n.saturating_mul(10).saturating_add(u32::from(b - b'0')))
                })
                .unwrap_or(0)
        });
        while let Some(p) = numbers.next() {
            match p {
                0 | 22 => self.bold = false,
                1 => self.bold = true,
                38 | 48 | 58 => match numbers.next() {
                    Some(5) => {
                        numbers.next();
                    }
                    Some(2) => {
                        numbers.next();
                        numbers.next();
                        numbers.next();
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    fn consume(&mut self, line: Line, result: &mut Vec<RoomObservation>) {
        let text = line.text.trim();
        if self.watch_failures && is_movement_failure(text) {
            self.movement_failed = true;
            self.recycle_block();
            self.exits = None;
            return;
        }
        if let Some(exits) = &mut self.exits {
            // Only a line that starts with a direction word can be one (checked before the
            // pattern).
            let first = text.split(|c: char| !c.is_alphabetic()).next().unwrap_or("");
            if let Some(m) = direction_word(first).and_then(|_| DIRECTION_LINE.captures(text)) {
                if let Some(d) = normalize_direction(&m[1]) {
                    exits.insert(d.to_string(), None);
                }
                return;
            }
            self.complete(result);
        }
        if let Some(list) = exit_header(text) {
            let mut exits = indexmap::IndexMap::new();
            for word in list.split(|c: char| !c.is_alphanumeric() && c != '_') {
                if let Some(d) = direction_word(word) {
                    exits.insert(d.to_string(), None);
                }
            }
            self.exits = Some(exits);
            if !list.is_empty() {
                self.complete(result);
            }
            return;
        }
        if self.block.len() >= 128
            && let Some(mut old) = self.block.pop_front()
        {
            // Keep the oldest line's buffer for a line to come (one allocation less per line).
            old.text.clear();
            self.spare.push(old.text);
        }
        self.block.push_back(line);
    }

    fn complete(&mut self, result: &mut Vec<RoomObservation>) {
        let candidates: Vec<usize> = (0..self.block.len())
            .filter(|&i| is_title(&self.block[i].text))
            .collect();
        // A styled title first; else the first plausible line.
        let chosen = candidates
            .iter()
            .copied()
            .find(|&i| self.block[i].bold)
            .or_else(|| candidates.first().copied());
        if let (Some(chosen), Some(exits)) = (chosen, &self.exits) {
            let description = description_lines(self.block.range(chosen + 1..).map(|l| l.text.as_str()));
            if !description.is_empty() && description.encode_utf16().count() <= MAX_DESCRIPTION {
                result.push(RoomObservation {
                    name: self.block[chosen].text.trim().to_string(),
                    description,
                    exits: exits.clone(),
                    ..RoomObservation::default()
                });
            }
        }
        if self.keep_blocks
            && let Some(exits) = &self.exits
            && !exits.is_empty()
            && !candidates.is_empty()
        {
            if self.blocks.len() >= MAX_BLOCKS {
                self.blocks.remove(0);
            }
            self.blocks.push(TextBlock {
                lines: self
                    .block
                    .iter()
                    .map(|l| l.text.trim())
                    .filter(|t| !t.is_empty())
                    .map(str::to_string)
                    .collect(),
                exits: exits.keys().cloned().collect(),
            });
        }
        self.recycle_block();
        self.exits = None;
    }

    /// Empty the block, keeping its line buffers for the lines to come.
    fn recycle_block(&mut self) {
        for mut line in self.block.drain(..) {
            if self.spare.len() < 128 {
                line.text.clear();
                self.spare.push(line.text);
            }
        }
    }
}

fn is_title(value: &str) -> bool {
    let text = value.trim();
    // The cheap checks first: most lines fail them.
    if !text.chars().next().is_some_and(char::is_alphabetic) || text.ends_with(['.', ':', '!']) {
        return false;
    }
    let length = text.encode_utf16().count();
    if !(2..=110).contains(&length) {
        return false;
    }
    let lower = text.to_lowercase();
    !lower.contains("says") && !lower.contains("password") && !lower.starts_with("exits") && !lower.starts_with("you ")
}
