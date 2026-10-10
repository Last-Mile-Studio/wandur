//! Channel rules: one recognized channel shape each, and ordered sets of them (the C#
//! `ChannelRule` and `ChannelRuleSet`).
//!
//! A rule is a channel name, a regular expression over the plain (colour stripped) line, anchored
//! at its start and matched without regard to case, with the optional named groups `speaker` and
//! `text`, the command that speaks on the channel (`{speaker}` stands for the person answered),
//! and three flags: private (it answers one person), exclude (a line it matches is not a channel,
//! whatever else claims it) and disabled (kept on the world, never consulted).
//!
//! In a set the first rule that matches wins, so a line is never two messages, and an exclusion
//! anywhere in the set wins over every other rule. An unreadable pattern is dropped, never fatal.
//! Patterns use the `regex` crate's syntax, which matches in linear time: the C# 50 ms match
//! timeout has nothing to guard here. Lookaround and backreferences are not supported; such a
//! pattern counts as unreadable.

use std::ops::Range;

use regex::{Regex, RegexBuilder, RegexSet, RegexSetBuilder};
use serde::{Deserialize, Serialize};

/// Longest plain line a rule is tried on (the C# limit).
pub const MAX_LINE: usize = 2048;
/// Longest channel name a world may save.
pub const MAX_CHANNEL: usize = 40;
/// Longest pattern a world may save.
pub const MAX_PATTERN: usize = 400;
/// Longest reply command a world may save.
pub const MAX_REPLY: usize = 80;
/// Most rules one world may carry.
pub const MAX_RULES: usize = 200;
/// Longest speaker name: anything longer is prose.
const MAX_SPEAKER: usize = 40;
/// Compiled size limit of one pattern, so a hand-written monster is refused rather than slow.
const SIZE_LIMIT: usize = 1 << 20;

/// One channel shape, in the JSON shape a world and a channel pack carry:
/// `{"channel", "pattern", "reply_command", "private", "exclude", "disabled"}`, the last three
/// left out when false and `reply_command` when there is none.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChannelRule {
    pub channel: String,
    pub pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_command: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub private: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub exclude: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl ChannelRule {
    /// A plain rule: channel, pattern and reply command.
    pub fn new(channel: &str, pattern: &str, reply: Option<&str>) -> Self {
        Self {
            channel: channel.into(),
            pattern: pattern.into(),
            reply_command: reply.map(Into::into),
            ..Self::default()
        }
    }

    /// An exclusion for `channel` (what "Not a channel" teaches).
    pub fn exclusion(channel: &str, pattern: &str) -> Self {
        Self {
            channel: channel.into(),
            pattern: pattern.into(),
            exclude: true,
            ..Self::default()
        }
    }

    /// Whether a world may save this rule: a name, a readable pattern, the C# length limits.
    pub fn is_valid(&self) -> bool {
        !self.channel.trim().is_empty()
            && self.channel.chars().count() <= MAX_CHANNEL
            && !self.pattern.is_empty()
            && self.pattern.chars().count() <= MAX_PATTERN
            && self
                .reply_command
                .as_ref()
                .is_none_or(|r| r.chars().count() <= MAX_REPLY)
            && compile(&self.pattern).is_some()
    }
}

/// Whether a world's rules can be saved (the C# profile validation).
pub fn validate(rules: &[ChannelRule]) -> Result<(), String> {
    if rules.len() > MAX_RULES || !rules.iter().all(ChannelRule::is_valid) {
        return Err(crate::l10n::t(crate::l10n::S::ChannelRulesInvalid).into());
    }
    Ok(())
}

/// The expression a pattern runs as (anchored at the line's start, case-insensitive), or `None`
/// when it cannot be read.
pub fn compile(pattern: &str) -> Option<Regex> {
    RegexBuilder::new(&anchored(pattern))
        .case_insensitive(true)
        .size_limit(SIZE_LIMIT)
        .build()
        .ok()
}

fn anchored(pattern: &str) -> String {
    if pattern.starts_with('^') {
        pattern.to_string()
    } else {
        format!("^{pattern}")
    }
}

/// What a rule made of a line. Ranges are byte ranges in the plain line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matched {
    pub channel: String,
    /// The speaker, cleaned (no account mark, role tag or brackets); empty when none.
    pub speaker: String,
    /// The message text, trimmed.
    pub text: String,
    /// Where `text` is in the plain line.
    pub text_range: Range<usize>,
    pub private: bool,
    pub reply_command: Option<String>,
}

/// An ordered set of rules, compiled. Disabled rules are listed but never consulted.
#[derive(Debug, Default)]
pub struct RuleSet {
    /// Every readable rule, in order, including the disabled ones.
    rules: Vec<ChannelRule>,
    /// The enabled rules that claim lines: index into `rules` and the expression.
    compiled: Vec<(usize, Regex)>,
    exclusions: Vec<Regex>,
    /// All of `compiled` in one automaton: one pass over a line says which rules can match, so
    /// ordinary output (most lines) costs one scan however many rules there are.
    set: Option<RegexSet>,
    exclusion_set: Option<RegexSet>,
}

impl RuleSet {
    pub fn new(rules: impl IntoIterator<Item = ChannelRule>) -> Self {
        let mut set = Self::default();
        for rule in rules {
            if rule.channel.trim().is_empty() || rule.pattern.is_empty() {
                continue;
            }
            let Some(expression) = compile(&rule.pattern) else {
                continue;
            };
            set.rules.push(rule);
            let rule = set.rules.last().expect("just pushed");
            if rule.disabled {
                continue;
            }
            if rule.exclude {
                set.exclusions.push(expression);
            } else {
                set.compiled.push((set.rules.len() - 1, expression));
            }
        }
        set.set = build_set(set.compiled.iter().map(|(i, _)| set.rules[*i].pattern.as_str()));
        set.exclusion_set = build_set(
            set.rules
                .iter()
                .filter(|r| r.exclude && !r.disabled)
                .map(|r| r.pattern.as_str()),
        );
        set
    }

    /// Every readable rule, in order, including the disabled ones.
    pub fn rules(&self) -> &[ChannelRule] {
        &self.rules
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The channel names this set knows, first seen first, exclusions and disabled rules left out.
    pub fn channels(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for (i, _) in &self.compiled {
            let name = &self.rules[*i].channel;
            if !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                names.push(name.clone());
            }
        }
        names
    }

    /// The first enabled rule for a channel (its reply command and privacy), for a channel named
    /// by a structured message rather than by a pattern.
    pub fn for_channel(&self, channel: &str) -> Option<&ChannelRule> {
        self.compiled
            .iter()
            .map(|(i, _)| &self.rules[*i])
            .find(|r| r.channel.eq_ignore_ascii_case(channel))
    }

    /// The first rule that claims a plain line, or `None` when it is ordinary output (or an
    /// exclusion matches it).
    pub fn match_plain(&self, plain: &str) -> Option<Matched> {
        if plain.is_empty() || too_long(plain) || self.compiled.is_empty() {
            return None;
        }
        let excluded = match &self.exclusion_set {
            Some(set) => set.is_match(plain),
            None => self.exclusions.iter().any(|e| e.is_match(plain)),
        };
        if excluded {
            return None;
        }
        match &self.set {
            Some(set) => {
                let hits = set.matches(plain);
                if !hits.matched_any() {
                    return None;
                }
                hits.iter().find_map(|k| {
                    let (i, expression) = &self.compiled[k];
                    message(&self.rules[*i], expression, plain)
                })
            }
            None => self
                .compiled
                .iter()
                .find_map(|(i, expression)| message(&self.rules[*i], expression, plain)),
        }
    }

    /// The same for a line that may carry colour codes.
    pub fn match_line(&self, line: &str) -> Option<Matched> {
        self.match_plain(&strip(line))
    }

    /// Whether a line is channel traffic (what the agent feed asks).
    pub fn is_channel_line(&self, line: &str) -> bool {
        self.match_line(line).is_some()
    }
}

fn build_set<'a>(patterns: impl Iterator<Item = &'a str>) -> Option<RegexSet> {
    let patterns: Vec<String> = patterns.map(anchored).collect();
    if patterns.is_empty() {
        return None;
    }
    RegexSetBuilder::new(patterns)
        .case_insensitive(true)
        .size_limit(SIZE_LIMIT * 4)
        .build()
        .ok()
}

fn too_long(plain: &str) -> bool {
    plain.len() > MAX_LINE && plain.chars().count() > MAX_LINE
}

/// What one rule makes of a line, exclusions and the rest of the set aside: the teaching
/// dialog's preview.
pub fn apply(rule: &ChannelRule, line: &str) -> Option<Matched> {
    let plain = strip(line);
    if plain.is_empty() || too_long(&plain) {
        return None;
    }
    let expression = compile(&rule.pattern)?;
    message(rule, &expression, &plain)
}

/// Apply with an expression compiled once (the preview runs one rule over 200 lines).
pub fn apply_compiled(rule: &ChannelRule, expression: &Regex, plain: &str) -> Option<Matched> {
    if plain.is_empty() || too_long(plain) {
        return None;
    }
    message(rule, expression, plain)
}

fn message(rule: &ChannelRule, expression: &Regex, plain: &str) -> Option<Matched> {
    let captures = expression.captures(plain)?;
    let speaker = clean_speaker(captures.name("speaker").map_or("", |m| m.as_str()));
    // A speaker is a short name: letters, and for worlds that speak through a description ("A
    // Human male" on a CommNet) spaces, apostrophes and hyphens, never digits or punctuation.
    // Anything else is prose that happens to read like a channel.
    if speaker.chars().count() > MAX_SPEAKER
        || speaker.chars().next().is_some_and(|c| !c.is_alphabetic())
        || !speaker
            .chars()
            .all(|c| c.is_alphabetic() || matches!(c, ' ' | '\'' | '-'))
    {
        return None;
    }
    let range = captures.name("text").map_or(0..plain.len(), |m| m.range());
    let range = trimmed(plain, range);
    Some(Matched {
        channel: rule.channel.clone(),
        speaker: speaker.to_string(),
        text: plain[range.clone()].to_string(),
        text_range: range,
        private: rule.private,
        reply_command: rule.reply_command.clone(),
    })
}

/// A byte range with the whitespace at both ends left out.
fn trimmed(s: &str, range: Range<usize>) -> Range<usize> {
    let part = &s[range.clone()];
    let start = range.start + (part.len() - part.trim_start().len());
    let end = range.end - (part.len() - part.trim_end().len());
    start..end.max(start)
}

/// The name inside what a taught pattern captured: an account mark (`@Nield`), a role tag
/// (`Nield [IMM]`) and the brackets of a description (`[A Human male]`) are decoration.
pub fn clean_speaker(speaker: &str) -> &str {
    let mut name = speaker.trim();
    if let Some(rest) = name.strip_prefix('@') {
        name = rest.trim_start();
    }
    if name.len() > 1 && name.starts_with('[') && name.ends_with(']') {
        name = name[1..name.len() - 1].trim();
    }
    if name.ends_with(']')
        && let Some(tag) = name.rfind(" [")
        && tag > 0
    {
        name = name[..tag].trim_end();
    }
    name
}

/// The plain text of a line: escape sequences (CSI, OSC, two-byte escapes) and control
/// characters other than tab removed. Recognition works on the text under the colours.
pub fn strip(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    strip_into(line, &mut out);
    out
}

/// [`strip`] into a buffer the caller reuses (the classifier's hot path). Text between control
/// characters is copied in runs; the only control characters outside ASCII (U+0080 to U+009F)
/// are two bytes starting with 0xC2.
pub fn strip_into(line: &str, out: &mut String) {
    out.clear();
    let bytes = line.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        // Eight plain bytes at a time: most of a line lies between its colour codes.
        if let Some(chunk) = bytes.get(i..i + 8)
            && !maybe_special(u64::from_le_bytes(chunk.try_into().expect("eight bytes")))
        {
            i += 8;
            continue;
        }
        let b = bytes[i];
        let c1 = b == 0xc2 && matches!(bytes.get(i + 1), Some(0x80..=0x9f));
        if b >= 0x20 && b != 0x7f && !c1 {
            i += 1;
            continue;
        }
        out.push_str(&line[start..i]);
        i += if b == 0x1b {
            escape_len(&bytes[i..])
        } else if c1 {
            2
        } else {
            if b == b'\t' {
                out.push('\t');
            }
            1
        };
        start = i;
    }
    out.push_str(&line[start..]);
}

/// Whether eight bytes may hold a control character's first byte: one below 0x20, 0x7F or
/// 0xC2 (the word tricks for "has a byte less than" and "has a byte equal to"; no false
/// negatives).
fn maybe_special(x: u64) -> bool {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;
    let below = x.wrapping_sub(LO * 0x20) & !x & HI;
    let del = x ^ (LO * 0x7f);
    let c2 = x ^ (LO * 0xc2);
    below | (del.wrapping_sub(LO) & !del & HI) | (c2.wrapping_sub(LO) & !c2 & HI) != 0
}

/// Length in bytes of the escape sequence at the start of `b` (which starts with ESC).
pub(crate) fn escape_len(b: &[u8]) -> usize {
    match b.get(1) {
        Some(b'[') => b[2..]
            .iter()
            .position(|c| (0x40..=0x7e).contains(c))
            .map_or(b.len(), |p| p + 3),
        Some(b']') => {
            let mut i = 2;
            while i < b.len() {
                if b[i] == 0x07 {
                    return i + 1;
                }
                if b[i] == 0x1b && b.get(i + 1) == Some(&b'\\') {
                    return i + 2;
                }
                i += 1;
            }
            b.len()
        }
        Some(c) if c.is_ascii() => 2,
        Some(_) => 1,
        None => 1,
    }
}

/// The part of a raw line (colour codes kept) whose plain text is `range` of [`strip`]'s
/// result, with the colour codes that came before it, so the panel shows the words in the
/// colours the world gave them. Only SGR sequences (`ESC [ ... m`) are kept.
pub fn styled_slice(raw: &str, range: Range<usize>) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(range.len() + 16);
    let mut plain = 0;
    let mut i = 0;
    while i < raw.len() && plain < range.end {
        let b = bytes[i];
        if b == 0x1b {
            let n = escape_len(&bytes[i..]);
            if bytes.get(i + 1) == Some(&b'[') && bytes.get(i + n - 1) == Some(&b'm') {
                out.push_str(&raw[i..i + n]);
            }
            i += n;
            continue;
        }
        let c = raw[i..].chars().next().unwrap_or('\u{fffd}');
        let len = c.len_utf8();
        let kept = c == '\t' || !c.is_control();
        if kept {
            if plain >= range.start {
                out.push(c);
            }
            plain += len;
        }
        i += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_removes_csi_osc_and_controls() {
        assert_eq!(strip("a\u{1b}]0;title\u{7}b\u{1b}[1;31mc\u{0}d\r"), "abcd");
        assert_eq!(strip("plain\ttab"), "plain\ttab");
        assert_eq!(strip("é\u{1b}[0mé"), "éé");
        assert_eq!(strip("a\u{85}b\u{1b}[31mc"), "abc");
        // Long plain runs (the eight-byte steps) around every kind of control character.
        let long = "plain text of some length ".repeat(3);
        for control in ["\u{1b}[1;32m", "\u{7f}", "\u{85}", "\u{0}", "\t", "\r"] {
            let line = format!("{long}{control}{long}é{long}");
            let kept = if control == "\t" { "\t" } else { "" };
            assert_eq!(strip(&line), format!("{long}{kept}{long}é{long}"), "{control:?}");
        }
    }

    #[test]
    fn a_styled_slice_keeps_the_colours_before_and_inside_it() {
        let raw = "\u{1b}[1;33m[OOC] Bora:\u{1b}[0;32m hello there\u{1b}[0m";
        let plain = strip(raw);
        let start = plain.find("hello").unwrap();
        let s = styled_slice(raw, start..plain.len());
        assert_eq!(strip(&s), "hello there");
        assert!(s.contains("\u{1b}[0;32m"));
        assert!(s.starts_with("\u{1b}[1;33m"), "{s:?}");
    }

    #[test]
    fn patterns_are_anchored_and_ignore_case() {
        let set = RuleSet::new([ChannelRule::new(
            "ooc",
            r"\[OOC\] (?<speaker>[A-Za-z]+): (?<text>.*)$",
            Some("ooc"),
        )]);
        assert!(set.match_plain("[ooc] Ann: hi").is_some());
        assert!(set.match_plain("x [OOC] Ann: hi").is_none());
        let m = set.match_plain("[OOC] Ann:   padded  ").unwrap();
        assert_eq!(m.text, "padded");
        assert_eq!(&"[OOC] Ann:   padded  "[m.text_range], "padded");
    }

    #[test]
    fn escaped_punctuation_from_the_proposer_compiles() {
        assert!(compile(r"^\#\[x\]\(y\)\.\$\^\|\{\}\*\+\?\\").is_some());
    }
}
