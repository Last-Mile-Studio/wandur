//! Guesses the shape of a channel line from one example, or two (the C# `ChannelRuleProposer`,
//! the heuristic behind the "Mark as channel" dialog).
//!
//! Worlds print channels as a literal head (`(OOC) `, `[CHAT] `, `CommNet 0 `), a speaker (a
//! name such as `@Nield` or `Aldric`, possibly followed by a role tag `[IMM]`, or a bracketed
//! description `[A Human male]` with an optional tone), a separator (`: `, ` tells you '`,
//! `, '`) and the text. The proposal keeps the head as an escaped literal with its digits
//! generalised, the speaker as a name or description class, and the text as everything to the
//! end of the line inside whatever quote opened it. With a second example the pieces are
//! narrowed to what both lines share. The result is a starting point the reader corrects.
//!
//! The C# reads the pieces with small .NET regular expressions anchored with `\G`; here they
//! are small hand-written matchers that give the same results (the separator's backtracking
//! is spelled out in [`separator_shape`]).

use super::rules::{ChannelRule, strip};

const TONE_PATTERN: &str = r"\( ?[^)]*? ?\)";
const NAME_PATTERN: &str = "@?[A-Za-z]+";
const WIDE_NAME_PATTERN: &str = "@?[A-Za-z'-]+";
const DESCRIPTION_PATTERN: &str = r"\[[^\]]+\]";
const ROLE_TAG_PATTERN: &str = r"(?: \[[A-Za-z]+\])?";

/// One line read as a channel line: the literal pieces and the patterns proposed for them,
/// which compose into `rule`. An empty `speaker_pattern` means no speaker was recognised and
/// the rule carries a text group only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    pub rule: ChannelRule,
    pub head: String,
    pub speaker: String,
    pub separator: String,
    pub text: String,
    pub head_pattern: String,
    pub speaker_pattern: String,
    pub separator_pattern: String,
    pub closing: String,
}

impl Proposal {
    pub fn has_speaker(&self) -> bool {
        !self.speaker_pattern.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    None,
    Name,
    Description,
}

/// The pieces one line was cut into, and the patterns they became.
#[derive(Clone, Debug)]
struct Parts {
    head: String,
    speaker: String,
    separator: String,
    text: String,
    closing: String,
    kind: Kind,
    role_tag: bool,
    tone: bool,
    head_pattern: String,
    speaker_pattern: String,
    separator_pattern: String,
}

impl Parts {
    #[allow(clippy::too_many_arguments)]
    fn new(
        head: &str,
        speaker: &str,
        separator: &str,
        text: &str,
        closing: &str,
        kind: Kind,
        role_tag: bool,
        tone: bool,
    ) -> Self {
        let speaker_pattern = match kind {
            Kind::Name => {
                let base = if speaker.contains(['\'', '-']) {
                    WIDE_NAME_PATTERN
                } else {
                    NAME_PATTERN
                };
                format!("{base}{}", if role_tag { ROLE_TAG_PATTERN } else { "" })
            }
            Kind::Description => DESCRIPTION_PATTERN.into(),
            Kind::None => String::new(),
        };
        Self {
            head_pattern: generalize(head),
            speaker_pattern,
            separator_pattern: format!("{}{}", if tone { TONE_PATTERN } else { "" }, escape(separator)),
            head: head.into(),
            speaker: speaker.into(),
            separator: separator.into(),
            text: text.into(),
            closing: closing.into(),
            kind,
            role_tag,
            tone,
        }
    }
}

/// The proposal for one line, narrowed by a second example of the same channel when given.
pub fn propose(line: &str, second: Option<&str>) -> Proposal {
    let mut parts = read(strip(line).trim());
    if let Some(second) = second.filter(|s| !s.trim().is_empty()) {
        parts = narrow(parts, read(strip(second).trim()));
    }
    let channel = guess_channel(&parts.head, &parts.separator);
    let pattern = compose(
        &parts.head_pattern,
        &parts.speaker_pattern,
        &parts.separator_pattern,
        &parts.closing,
    );
    let rule = ChannelRule {
        reply_command: guess_reply(&channel),
        private: is_private_channel(&channel),
        channel,
        pattern,
        ..ChannelRule::default()
    };
    Proposal {
        rule,
        head: parts.head,
        speaker: parts.speaker,
        separator: parts.separator,
        text: parts.text,
        head_pattern: parts.head_pattern,
        speaker_pattern: parts.speaker_pattern,
        separator_pattern: parts.separator_pattern,
        closing: parts.closing,
    }
}

/// The rule pattern the pieces compose into; the dialog calls this again after every edit.
pub fn compose(head: &str, speaker: &str, separator: &str, closing: &str) -> String {
    let mut pattern = format!("^{head}");
    if !speaker.is_empty() {
        pattern.push_str("(?<speaker>");
        pattern.push_str(speaker);
        pattern.push(')');
        pattern.push_str(separator);
    }
    pattern.push_str("(?<text>.*)");
    pattern.push_str(&escape(closing));
    pattern.push('$');
    pattern
}

/// The channel a head or a separator names: the first word of the head (`ooc`, `chat`,
/// `commnet`), else the verb in the separator with its plural s dropped (`tells you` is
/// `tell`, `gossips` is `gossip`), else `channel`.
pub fn guess_channel(head: &str, separator: &str) -> String {
    for source in [head, separator] {
        for word in ascii_words(source) {
            let mut name = word.to_ascii_lowercase();
            if matches!(name.as_str(), "you" | "the" | "to" | "a" | "an") {
                continue;
            }
            if name.len() > 3 && name.ends_with('s') && !name.ends_with("ss") {
                name.pop();
            }
            return name;
        }
    }
    "channel".into()
}

fn ascii_words(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !c.is_ascii_alphabetic()).filter(|w| !w.is_empty())
}

/// Tells, pages and whispers answer one person; everything else is a room of people.
pub fn is_private_channel(channel: &str) -> bool {
    let c = channel.to_lowercase();
    ["tell", "whisper", "page", "reply", "msg"]
        .iter()
        .any(|n| c.contains(n))
}

/// Worlds name the command after the channel, which is the only guess worth making.
pub fn guess_reply(channel: &str) -> Option<String> {
    let name: String = channel
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    if name.is_empty() {
        None
    } else if is_private_channel(&name) {
        Some(format!("{name} {{speaker}}"))
    } else {
        Some(name)
    }
}

/// Escapes a literal for a pattern, leaving spaces readable and generalising runs of digits.
pub fn generalize(literal: &str) -> String {
    let mut out = String::new();
    let mut chars = literal.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            while chars.peek().is_some_and(char::is_ascii_digit) {
                chars.next();
            }
            out.push_str("[0-9]+");
        } else {
            push_escaped(&mut out, c);
        }
    }
    out
}

fn escape(literal: &str) -> String {
    let mut out = String::with_capacity(literal.len());
    for c in literal.chars() {
        push_escaped(&mut out, c);
    }
    out
}

fn push_escaped(out: &mut String, c: char) {
    if matches!(
        c,
        '\\' | '*' | '+' | '?' | '|' | '{' | '}' | '[' | ']' | '(' | ')' | '^' | '$' | '.' | '#'
    ) {
        out.push('\\');
    }
    out.push(c);
}

fn read(plain: &str) -> Parts {
    // Names first, left to right, then descriptions: a bracketed tag at the start of the line
    // ("[CHAT] Anka: ...") is a head, and only when no name follows it is a bracket the speaker.
    let mut previous: Option<char> = None;
    for (i, c) in plain.char_indices() {
        if !previous.is_some_and(char::is_alphanumeric)
            && let Some(parts) = try_name(plain, i)
        {
            return parts;
        }
        previous = Some(c);
    }
    for (i, _) in plain.char_indices() {
        if let Some(parts) = try_description(plain, i) {
            return parts;
        }
    }
    // No speaker: the leading tag or word is the head and the rest is the text.
    let head = match leading_tag(plain) {
        Some(n) if n < plain.len() => &plain[..n],
        _ => "",
    };
    Parts::new(head, "", "", &plain[head.len()..], "", Kind::None, false, false)
}

fn try_name(plain: &str, at: usize) -> Option<Parts> {
    let name_end = name(plain, at)?;
    let tag_end = role_tag(plain, name_end);
    let end = tag_end.unwrap_or(name_end);
    let (separator, text, closing) = separator(plain, end)?;
    Some(Parts::new(
        &plain[..at],
        &plain[at..end],
        &separator,
        &text,
        &closing,
        Kind::Name,
        tag_end.is_some(),
        false,
    ))
}

fn try_description(plain: &str, at: usize) -> Option<Parts> {
    let end = description(plain, at)?;
    let tone_end = tone(plain, end);
    let (separator, text, closing) = separator(plain, tone_end.unwrap_or(end))?;
    Some(Parts::new(
        &plain[..at],
        &plain[at..end],
        &separator,
        &text,
        &closing,
        Kind::Description,
        false,
        tone_end.is_some(),
    ))
}

/// `@?[A-Z][a-z]+(?:['-][A-Za-z]+)*\b` at `at`: the end of the name.
fn name(s: &str, at: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = at;
    if b.get(i) == Some(&b'@') {
        i += 1;
    }
    if !b.get(i).is_some_and(u8::is_ascii_uppercase) {
        return None;
    }
    i += 1;
    let lower = i;
    while b.get(i).is_some_and(u8::is_ascii_lowercase) {
        i += 1;
    }
    if i == lower {
        return None;
    }
    // Every place a group can end, longest first; the first followed by a word boundary wins.
    let mut ends = vec![i];
    loop {
        if !matches!(b.get(i), Some(b'\'' | b'-')) || !b.get(i + 1).is_some_and(u8::is_ascii_alphabetic) {
            break;
        }
        i += 1;
        while b.get(i).is_some_and(u8::is_ascii_alphabetic) {
            i += 1;
        }
        ends.push(i);
    }
    ends.into_iter()
        .rev()
        .find(|&end| !s[end..].chars().next().is_some_and(is_word))
}

/// .NET's `\w`: letters, digits, marks, connector punctuation.
fn is_word(c: char) -> bool {
    use unicode_general_category::{GeneralCategory as G, get_general_category};
    c.is_alphanumeric()
        || matches!(
            get_general_category(c),
            G::NonspacingMark | G::SpacingMark | G::EnclosingMark | G::ConnectorPunctuation
        )
}

/// ` \[[A-Za-z]+\]` at `at`.
fn role_tag(s: &str, at: usize) -> Option<usize> {
    let b = s.as_bytes();
    if b.get(at) != Some(&b' ') || b.get(at + 1) != Some(&b'[') {
        return None;
    }
    let mut i = at + 2;
    while b.get(i).is_some_and(u8::is_ascii_alphabetic) {
        i += 1;
    }
    (i > at + 2 && b.get(i) == Some(&b']')).then_some(i + 1)
}

/// `\[[A-Za-z][A-Za-z' -]*\]` at `at`.
fn description(s: &str, at: usize) -> Option<usize> {
    let b = s.as_bytes();
    if b.get(at) != Some(&b'[') || !b.get(at + 1).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let mut i = at + 2;
    while b
        .get(i)
        .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, b'\'' | b' ' | b'-'))
    {
        i += 1;
    }
    (b.get(i) == Some(&b']')).then_some(i + 1)
}

/// `\( ?[^()]*? ?\)` at `at`: an opening parenthesis and the first closing one, with no other
/// opening one between them.
fn tone(s: &str, at: usize) -> Option<usize> {
    let b = s.as_bytes();
    if b.get(at) != Some(&b'(') {
        return None;
    }
    let close = b[at + 1..].iter().position(|&c| c == b'(' || c == b')')?;
    (b[at + 1 + close] == b')').then_some(at + close + 2)
}

/// `^(?:\[[^\]]{1,20}\]|\([^)]{1,20}\)|[A-Za-z]+)[: ]*`: the length of the leading tag.
fn leading_tag(s: &str) -> Option<usize> {
    let bracketed = |open: char, close: char| -> Option<usize> {
        let rest = s.strip_prefix(open)?;
        let inner = rest.find(close)?;
        let count = rest[..inner].chars().count();
        (1..=20).contains(&count).then_some(1 + inner + close.len_utf8())
    };
    let mut end = bracketed('[', ']').or_else(|| bracketed('(', ')')).or_else(|| {
        let n = s.bytes().take_while(u8::is_ascii_alphabetic).count();
        (n > 0).then_some(n)
    })?;
    end += s[end..].bytes().take_while(|&c| c == b':' || c == b' ').count();
    Some(end)
}

/// `(?: ?[A-Za-z]+){0,3}(?:,? ?['"]|: ?['"]?|> ?)(?=\S)` at `at`: up to three words of verb
/// ("tells you", "says", "OOC"), then what opens the text: a colon, a quote, a comma and a
/// quote, or a prompt arrow, followed by something that is not a space. A bare space is not a
/// separator, or every capitalised sentence would read as a speaker. The candidates are tried
/// in the order .NET's backtracking tries them: most words first, then each alternative with
/// its optional parts taken before they are left out.
fn separator_shape(s: &str, at: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut word_ends = vec![at];
    let mut i = at;
    for _ in 0..3 {
        let mut j = i;
        if b.get(j) == Some(&b' ') {
            j += 1;
        }
        let start = j;
        while b.get(j).is_some_and(u8::is_ascii_alphabetic) {
            j += 1;
        }
        if j == start {
            break;
        }
        word_ends.push(j);
        i = j;
    }
    let followed = |end: usize| s[end..].chars().next().is_some_and(|c| !c.is_whitespace());
    let quote = |p: usize| matches!(b.get(p), Some(b'\'' | b'"'));
    for &p in word_ends.iter().rev() {
        // ,? ?['"]
        for comma in [true, false] {
            if comma && b.get(p) != Some(&b',') {
                continue;
            }
            let q = p + usize::from(comma);
            for space in [true, false] {
                if space && b.get(q) != Some(&b' ') {
                    continue;
                }
                let r = q + usize::from(space);
                if quote(r) && followed(r + 1) {
                    return Some(r + 1);
                }
            }
        }
        // : ?['"]?
        if b.get(p) == Some(&b':') {
            for space in [true, false] {
                if space && b.get(p + 1) != Some(&b' ') {
                    continue;
                }
                let q = p + 1 + usize::from(space);
                if quote(q) && followed(q + 1) {
                    return Some(q + 1);
                }
                if followed(q) {
                    return Some(q);
                }
            }
        }
        // > ?
        if b.get(p) == Some(&b'>') {
            if b.get(p + 1) == Some(&b' ') && followed(p + 2) {
                return Some(p + 2);
            }
            if followed(p + 1) {
                return Some(p + 1);
            }
        }
    }
    None
}

/// The separator after a speaker and the text it opens; an opening quote counts only when the
/// line closes it.
fn separator(plain: &str, at: usize) -> Option<(String, String, String)> {
    let end = separator_shape(plain, at)?;
    let mut separator = &plain[at..end];
    let last = separator.trim_end().chars().last();
    if let Some(quote @ ('\'' | '"')) = last {
        let start = end;
        if plain.len() > start + 1 && plain.ends_with(quote) {
            return Some((
                separator.to_string(),
                plain[start..plain.len() - 1].to_string(),
                quote.to_string(),
            ));
        }
        // The quote opened something the line never closed, so it belongs to the text.
        separator = &separator[..separator.rfind(quote).unwrap_or(separator.len())];
        if separator.trim().is_empty() {
            return None;
        }
    }
    Some((
        separator.to_string(),
        plain[at + separator.len()..].to_string(),
        String::new(),
    ))
}

/// Keeps what two examples share and widens each piece where they differ.
fn narrow(first: Parts, second: Parts) -> Parts {
    if first.kind == Kind::None || second.kind == Kind::None {
        return if first.kind == Kind::None { second } else { first };
    }
    let head = if first.head_pattern == second.head_pattern {
        first.head_pattern.clone()
    } else if first.head.is_empty() || second.head.is_empty() {
        format!("(?:{})?", generalize(&format!("{}{}", first.head, second.head)))
    } else {
        format!("(?:{}|{})", first.head_pattern, second.head_pattern)
    };
    let speaker = if first.kind == Kind::Name && second.kind == Kind::Name {
        let wide = first.speaker.contains(['\'', '-']) || second.speaker.contains(['\'', '-']);
        format!(
            "{}{}",
            if wide { WIDE_NAME_PATTERN } else { NAME_PATTERN },
            if first.role_tag || second.role_tag {
                ROLE_TAG_PATTERN
            } else {
                ""
            }
        )
    } else if first.speaker_pattern == second.speaker_pattern {
        first.speaker_pattern.clone()
    } else {
        format!("(?:{}|{})", first.speaker_pattern, second.speaker_pattern)
    };
    let tone = if first.tone != second.tone {
        format!("(?:{TONE_PATTERN})?")
    } else if first.tone {
        TONE_PATTERN.into()
    } else {
        String::new()
    };
    let separator = if first.separator == second.separator {
        escape(&first.separator)
    } else {
        format!("(?:{}|{})", escape(&first.separator), escape(&second.separator))
    };
    // The literal pieces stay those of the first example; only the patterns widen.
    Parts {
        closing: if first.closing == second.closing {
            first.closing.clone()
        } else {
            String::new()
        },
        head_pattern: head,
        speaker_pattern: speaker,
        separator_pattern: format!("{tone}{separator}"),
        ..first
    }
}

#[cfg(test)]
mod tests {
    //! The C# `ChannelRuleProposerTests`, ported.
    use super::super::rules::{Matched, apply};
    use super::*;

    fn matches(rule: &ChannelRule, line: &str) -> Matched {
        apply(rule, line).unwrap_or_else(|| panic!("{} should match {line}", rule.pattern))
    }

    #[test]
    fn an_ooc_line_with_an_account_mark_and_a_role_tag() {
        let p = propose("(OOC) @Nield [IMM]: Now I'm hungry.", None);
        assert_eq!(p.head, "(OOC) ");
        assert_eq!(p.speaker, "@Nield [IMM]");
        assert_eq!(p.separator, ": ");
        assert_eq!(p.text, "Now I'm hungry.");
        assert_eq!(p.head_pattern, r"\(OOC\) ");
        assert_eq!(p.speaker_pattern, r"@?[A-Za-z]+(?: \[[A-Za-z]+\])?");
        assert_eq!(p.separator_pattern, ": ");
        assert_eq!(p.closing, "");
        assert_eq!(
            p.rule.pattern,
            r"^\(OOC\) (?<speaker>@?[A-Za-z]+(?: \[[A-Za-z]+\])?): (?<text>.*)$"
        );
        assert_eq!(p.rule.channel, "ooc");
        assert_eq!(p.rule.reply_command.as_deref(), Some("ooc"));
        assert!(!p.rule.private);
        let m = matches(&p.rule, "(OOC) @Nield [IMM]: Now I'm hungry.");
        assert_eq!(m.speaker, "Nield");
        assert_eq!(m.text, "Now I'm hungry.");
        assert_eq!(matches(&p.rule, "(OOC) Aldric: plain name, no tag").speaker, "Aldric");
        assert!(apply(&p.rule, "[OOC] Aldric: a different head").is_none());
    }

    #[test]
    fn a_commnet_line_generalises_the_frequency_and_keeps_the_description() {
        let p = propose("CommNet 0 [A Human male]( warmly ): Well played everyone!", None);
        assert_eq!(p.head, "CommNet 0 ");
        assert_eq!(p.head_pattern, "CommNet [0-9]+ ");
        assert_eq!(p.speaker, "[A Human male]");
        assert_eq!(p.speaker_pattern, r"\[[^\]]+\]");
        assert_eq!(p.separator, ": ");
        // One example: the tone is part of the shape.
        assert_eq!(p.separator_pattern, r"\( ?[^)]*? ?\): ");
        assert_eq!(p.text, "Well played everyone!");
        assert_eq!(p.rule.channel, "commnet");
        assert_eq!(p.rule.reply_command.as_deref(), Some("commnet"));
        let m = matches(&p.rule, "CommNet 1250 [A Wookiee]( growling ): Rrrr");
        assert_eq!(m.speaker, "A Wookiee");
        assert_eq!(m.text, "Rrrr");
        assert!(apply(&p.rule, "CommNet 0 [A Human male]: no tone").is_none());

        // A second example without the tone makes it optional.
        let n = propose(
            "CommNet 0 [A Human male]( warmly ): Well played everyone!",
            Some("CommNet 12 [A Twi'lek female]: Thanks!"),
        );
        assert_eq!(n.separator_pattern, r"(?:\( ?[^)]*? ?\))?: ");
        assert_eq!(n.head_pattern, "CommNet [0-9]+ ");
        assert_eq!(
            matches(&n.rule, "CommNet 0 [A Human male]( warmly ): Well played everyone!").speaker,
            "A Human male"
        );
        assert_eq!(
            matches(&n.rule, "CommNet 12 [A Twi'lek female]: Thanks!").speaker,
            "A Twi'lek female"
        );
        assert_eq!(
            matches(&n.rule, "CommNet 12 [A Twi'lek female]: Thanks!").text,
            "Thanks!"
        );
    }

    #[test]
    fn a_tell_is_private_and_answers_the_person_who_spoke() {
        let p = propose("Aldric tells you 'the gate is open'", None);
        assert_eq!(p.head, "");
        assert_eq!(p.speaker, "Aldric");
        assert_eq!(p.separator, " tells you '");
        assert_eq!(p.text, "the gate is open");
        assert_eq!(p.closing, "'");
        assert_eq!(p.rule.pattern, "^(?<speaker>@?[A-Za-z]+) tells you '(?<text>.*)'$");
        assert_eq!(p.rule.channel, "tell");
        assert_eq!(p.rule.reply_command.as_deref(), Some("tell {speaker}"));
        assert!(p.rule.private);
        assert_eq!(
            matches(&p.rule, "Aldric tells you 'the gate is open'").text,
            "the gate is open"
        );
        assert_eq!(matches(&p.rule, "Brenna tells you 'it's shut again'").speaker, "Brenna");
    }

    #[test]
    fn a_bracketed_head_names_the_channel() {
        let p = propose("[CHAT] Anka: who is flying tonight?", None);
        assert_eq!(p.head, "[CHAT] ");
        assert_eq!(p.head_pattern, r"\[CHAT\] ");
        assert_eq!(p.speaker, "Anka");
        assert_eq!(p.speaker_pattern, "@?[A-Za-z]+");
        assert_eq!(p.separator, ": ");
        assert_eq!(p.text, "who is flying tonight?");
        assert_eq!(p.rule.channel, "chat");
        assert_eq!(p.rule.reply_command.as_deref(), Some("chat"));
        assert_eq!(p.rule.pattern, r"^\[CHAT\] (?<speaker>@?[A-Za-z]+): (?<text>.*)$");
        assert_eq!(matches(&p.rule, "[CHAT] Anka: who is flying tonight?").speaker, "Anka");
    }

    #[test]
    fn a_verb_separator_with_a_quote_names_the_channel_and_closes_the_text() {
        let gossip = propose("Nessa gossips, 'good morning'", None);
        assert_eq!(gossip.rule.channel, "gossip");
        assert_eq!(&gossip.separator[gossip.separator.len() - 3..], ", '");
        assert_eq!(
            matches(&gossip.rule, "Nessa gossips, 'good morning'").text,
            "good morning"
        );
        let ooc = propose("Cadoc OOC: 'the east gate is closed again'", None);
        assert_eq!(ooc.rule.channel, "ooc");
        assert_eq!(
            matches(&ooc.rule, "Cadoc OOC: 'the east gate is closed again'").text,
            "the east gate is closed again"
        );
        // A quote the line never closes belongs to the text.
        let unclosed = propose("Bob says: 'tis a fine day", None);
        assert_eq!(unclosed.separator, " says: ");
        assert_eq!(unclosed.text, "'tis a fine day");
        assert_eq!(unclosed.rule.channel, "say");
    }

    #[test]
    fn two_examples_keep_only_what_they_share() {
        let p = propose("[OOC] Aldric: hello", Some("(OOC) @Brenna [IMM]: hi there"));
        assert_eq!(p.head_pattern, r"(?:\[OOC\] |\(OOC\) )");
        assert_eq!(p.speaker_pattern, r"@?[A-Za-z]+(?: \[[A-Za-z]+\])?");
        assert_eq!(p.separator_pattern, ": ");
        assert_eq!(p.rule.channel, "ooc");
        assert_eq!(matches(&p.rule, "[OOC] Aldric: hello").speaker, "Aldric");
        assert_eq!(matches(&p.rule, "(OOC) @Brenna [IMM]: hi there").speaker, "Brenna");
        assert!(apply(&p.rule, "[CHAT] Aldric: hello").is_none());
        // The same shape twice narrows nothing.
        let same = propose("[CHAT] Anka: one", Some("[CHAT] Bix: two"));
        assert_eq!(same.rule.pattern, r"^\[CHAT\] (?<speaker>@?[A-Za-z]+): (?<text>.*)$");
    }

    #[test]
    fn a_line_with_no_speaker_yields_a_text_group_only() {
        let p = propose("You are standing in a wide green field.", None);
        assert!(!p.has_speaker());
        assert_eq!(p.speaker_pattern, "");
        assert_eq!(p.speaker, "");
        assert!(!p.rule.pattern.contains("(?<speaker>"));
        assert!(p.rule.pattern.ends_with("(?<text>.*)$"));
        let m = matches(&p.rule, "You are standing in a wide green field.");
        assert_eq!(m.text, "are standing in a wide green field.");
        assert_eq!(m.speaker, "");
        let tagged = propose("[INFO] The gates close at dusk.", None);
        assert_eq!(tagged.head, "[INFO] ");
        assert_eq!(tagged.rule.channel, "info");
        assert_eq!(tagged.rule.pattern, r"^\[INFO\] (?<text>.*)$");
        // Colour is not shape.
        assert_eq!(
            propose("\u{1b}[32m[INFO]\u{1b}[0m The gates close at dusk.\r\n", None)
                .rule
                .pattern,
            r"^\[INFO\] (?<text>.*)$"
        );
    }

    #[test]
    fn compose_rebuilds_the_pattern_from_edited_pieces() {
        assert_eq!(
            compose(r"\[X\] ", "[A-Z]+", ": ", ""),
            r"^\[X\] (?<speaker>[A-Z]+): (?<text>.*)$"
        );
        assert_eq!(
            compose("", "[A-Za-z]+", " says '", "'"),
            "^(?<speaker>[A-Za-z]+) says '(?<text>.*)'$"
        );
        assert_eq!(compose("Note: ", "", ": ", ""), "^Note: (?<text>.*)$");
        assert_eq!(generalize("Line 12 (of 30)."), r"Line [0-9]+ \(of [0-9]+\)\.");
        assert_eq!(guess_channel("", " tells you '"), "tell");
        assert_eq!(guess_channel("[SHIPCHANNEL] ", ": "), "shipchannel");
        assert_eq!(guess_channel("", ": "), "channel");
        assert_eq!(guess_reply("whisper").as_deref(), Some("whisper {speaker}"));
        assert_eq!(guess_reply("..."), None);
    }

    #[test]
    fn the_separator_backtracks_as_net_does() {
        // Two spaces after the colon: ": " is followed by a space, so ":" alone is tried, which
        // is followed by a space too: no separator after the name.
        assert_eq!(separator_shape("A:  x", 1), None);
        assert_eq!(separator_shape("A tells you, 'x'", 1), Some(14));
        assert_eq!(separator_shape("A> go", 1), Some(3));
        assert_eq!(separator_shape("A says", 1), None, "a bare word is not a separator");
        // A name followed by a word character is not a name there.
        assert_eq!(name("Anne-Marie5", 0), Some(4));
        assert_eq!(name("McDonald", 0), None);
    }
}
