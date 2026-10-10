//! The rule engine: saved macros compiled once into lookups that cost little per line.
//!
//! Every trigger pattern is literal (the C# compiler escapes all regular expression
//! metacharacters), so a line is matched against all triggers in one Aho-Corasick pass (a DFA
//! that folds ASCII case; hits of case-sensitive patterns are then checked exactly), plus, only
//! when a pattern that ignores case has non-ASCII letters, a pass over a case-folded copy of the
//! line. A hit then checks where it is: anywhere (contains), at the start (starts with) or the
//! whole line (exact). Aliases are hash lookups of the whole command.
//!
//! Case folding follows ECMAScript's rule for a regular expression with the `i` flag and no `u`
//! flag (what the C# client's generated `new RegExp(pattern, "i")` does): each UTF-16 unit is
//! compared by its simple upper-case form, unless that form is several characters, outside the
//! Basic Multilingual Plane, or would turn a non-ASCII character into an ASCII one.

use std::collections::HashMap;
use std::time::Duration;

use aho_corasick::{AhoCorasick, AhoCorasickKind};

use super::definition::{MacroDefinition, MacroKind, MacroMatch};

/// One runnable macro.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The library entry's id.
    pub id: String,
    pub kind: MacroKind,
    pub commands: Vec<String>,
}

/// What a distinct pattern of an automaton stands for.
struct Target {
    rule: u32,
    how: MacroMatch,
    /// For a case-sensitive pattern in the ASCII case-insensitive automaton: the exact text a
    /// hit must have.
    exact: Option<Box<str>>,
}

/// Patterns searched by one automaton and, per distinct pattern, the rules using it.
#[derive(Default)]
struct Literals {
    automaton: Option<AhoCorasick>,
    targets: Vec<Vec<Target>>,
}

/// A pattern for [`Literals::build`]: text, rule, placement, and whether case must match.
type PatternSpec = (String, u32, MacroMatch, bool);

impl Literals {
    /// With `ascii_case_insensitive`, patterns are told apart by their ASCII lower case and
    /// case-sensitive ones are checked against the hit's text.
    fn build(patterns: Vec<PatternSpec>, ascii_case_insensitive: bool) -> Self {
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut distinct: Vec<String> = Vec::new();
        let mut targets: Vec<Vec<Target>> = Vec::new();
        for (pattern, rule, how, case_sensitive) in patterns {
            let key = if ascii_case_insensitive {
                pattern.to_ascii_lowercase()
            } else {
                pattern.clone()
            };
            let at = *index.entry(key.clone()).or_insert_with(|| {
                distinct.push(key);
                targets.push(Vec::new());
                distinct.len() - 1
            });
            let exact = (ascii_case_insensitive && case_sensitive).then(|| pattern.into_boxed_str());
            targets[at].push(Target { rule, how, exact });
        }
        let automaton = if distinct.is_empty() {
            None
        } else {
            // Literal strings of bounded length: building cannot fail. A DFA is the fastest kind
            // to search; at the library's 64 entries it is small.
            AhoCorasick::builder()
                .kind(Some(AhoCorasickKind::DFA))
                .ascii_case_insensitive(ascii_case_insensitive)
                .build(&distinct)
                .ok()
        };
        Self { automaton, targets }
    }

    fn search(&self, haystack: &str, hits: &mut Vec<u32>) {
        let Some(ac) = &self.automaton else { return };
        for m in ac.find_overlapping_iter(haystack) {
            for target in &self.targets[m.pattern().as_usize()] {
                let placed = match target.how {
                    MacroMatch::Contains => true,
                    MacroMatch::StartsWith => m.start() == 0,
                    MacroMatch::Exact => m.start() == 0 && m.end() == haystack.len(),
                };
                let cased = target
                    .exact
                    .as_deref()
                    .is_none_or(|exact| &haystack[m.start()..m.end()] == exact);
                if placed && cased {
                    hits.push(target.rule);
                }
            }
        }
    }
}

/// A unit-by-unit case fold (see the module notes).
fn fold_char(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_uppercase();
    }
    if c as u32 > 0xFFFF {
        return c;
    }
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(u), None) if (u as u32) >= 128 && (u as u32) <= 0xFFFF => u,
        _ => c,
    }
}

/// `text` case-folded into `out` (cleared first).
pub fn fold_into(text: &str, out: &mut String) {
    out.clear();
    if text.is_ascii() {
        out.push_str(text);
        out.make_ascii_uppercase();
    } else {
        out.extend(text.chars().map(fold_char));
    }
}

pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    fold_into(text, &mut out);
    out
}

/// Saved macros compiled for matching. Rules keep the library's order, which is the order their
/// commands go out when several match one line.
#[derive(Default)]
pub struct RuleSet {
    rules: Vec<Rule>,
    /// Case-sensitive patterns and ASCII patterns that ignore case, in one automaton that folds
    /// ASCII case (for ASCII patterns ECMAScript folding is ASCII folding, since a non-ASCII
    /// character never folds to an ASCII one); case-sensitive hits are checked exactly.
    literals: Literals,
    /// Patterns that ignore case and have non-ASCII characters, searched over a folded line.
    folded_insensitive: Literals,
    aliases: HashMap<String, u32>,
    folded_aliases: HashMap<String, u32>,
    timers: Vec<(u32, Duration)>,
    shortcuts: Vec<(String, u32)>,
    hits: Vec<u32>,
    folded: String,
}

impl RuleSet {
    /// Compile `macros` (id and definition, in library order). Invalid definitions are skipped:
    /// the library never stores one, and a rule that cannot run must not half run.
    pub fn build<'a>(macros: impl IntoIterator<Item = (&'a str, &'a MacroDefinition)>) -> Self {
        let mut set = RuleSet::default();
        let mut literals = Vec::new();
        let mut folded_insensitive = Vec::new();
        for (id, def) in macros {
            if def.validate().is_err() {
                continue;
            }
            let index = set.rules.len() as u32;
            set.rules.push(Rule {
                id: id.to_string(),
                kind: def.kind,
                commands: def.command_lines(),
            });
            match def.kind {
                MacroKind::Trigger if def.ignore_case && !def.pattern.is_ascii() => {
                    folded_insensitive.push((fold(&def.pattern), index, def.match_kind, true));
                }
                MacroKind::Trigger => {
                    literals.push((def.pattern.clone(), index, def.match_kind, !def.ignore_case));
                }
                MacroKind::Alias => {
                    let map = if def.ignore_case {
                        &mut set.folded_aliases
                    } else {
                        &mut set.aliases
                    };
                    let key = if def.ignore_case {
                        fold(&def.pattern)
                    } else {
                        def.pattern.clone()
                    };
                    // The first alias in library order wins (C# stops at the first handler).
                    map.entry(key).or_insert(index);
                }
                MacroKind::Timer => set
                    .timers
                    .push((index, Duration::from_secs(u64::from(def.interval_seconds)))),
                MacroKind::Shortcut => set.shortcuts.push((def.pattern.clone(), index)),
            }
        }
        set.literals = Literals::build(literals, true);
        set.folded_insensitive = Literals::build(folded_insensitive, false);
        set
    }

    pub fn rule(&self, index: u32) -> &Rule {
        &self.rules[index as usize]
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Heap bytes of the trigger automata.
    pub fn automata_bytes(&self) -> usize {
        [&self.literals, &self.folded_insensitive]
            .iter()
            .filter_map(|l| l.automaton.as_ref())
            .map(AhoCorasick::memory_usage)
            .sum()
    }

    pub fn has_triggers(&self) -> bool {
        self.literals.automaton.is_some() || self.folded_insensitive.automaton.is_some()
    }

    /// The triggers a complete line sets off, in library order, each once.
    pub fn match_line(&mut self, line: &str, out: &mut Vec<u32>) {
        self.hits.clear();
        self.literals.search(line, &mut self.hits);
        if self.folded_insensitive.automaton.is_some() {
            fold_into(line, &mut self.folded);
            self.folded_insensitive.search(&self.folded, &mut self.hits);
        }
        if self.hits.is_empty() {
            return;
        }
        self.hits.sort_unstable();
        self.hits.dedup();
        out.extend_from_slice(&self.hits);
    }

    /// The alias that replaces `command`: the first in library order whose pattern is the whole
    /// command.
    pub fn match_alias(&self, command: &str) -> Option<u32> {
        let exact = self.aliases.get(command).copied();
        let folded = if self.folded_aliases.is_empty() {
            None
        } else {
            self.folded_aliases.get(&fold(command)).copied()
        };
        match (exact, folded) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Every shortcut on function key `key` (`"F1"` to `"F12"`).
    pub fn match_key<'a>(&'a self, key: &'a str) -> impl Iterator<Item = u32> + 'a {
        self.shortcuts.iter().filter(move |(k, _)| k == key).map(|(_, i)| *i)
    }

    /// The timers: rule and interval.
    pub fn timers(&self) -> &[(u32, Duration)] {
        &self.timers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(defs: &[MacroDefinition]) -> RuleSet {
        let ids: Vec<String> = (0..defs.len()).map(|i| format!("m{i}")).collect();
        RuleSet::build(ids.iter().map(String::as_str).zip(defs))
    }

    fn lines(set: &mut RuleSet, line: &str) -> Vec<u32> {
        let mut out = Vec::new();
        set.match_line(line, &mut out);
        out
    }

    /// C# `TriggerMatchesLiteralTextAndSafelySendsCommandsInOrder`: brackets are literal, case is
    /// ignored on request, the commands come back as typed and in order.
    #[test]
    fn trigger_matches_literal_text_and_keeps_commands_in_order() {
        let mut s = set(&[MacroDefinition::new(
            MacroKind::Trigger,
            "[Hungry]",
            "eat bread\nsay \"thanks\"; mud.send('oops')",
        )
        .ignoring_case()]);
        assert!(lines(&mut s, "Hungry").is_empty());
        assert_eq!(lines(&mut s, "You are [HUNGRY] now"), [0]);
        assert_eq!(s.rule(0).commands, ["eat bread", "say \"thanks\"; mud.send('oops')"]);
    }

    #[test]
    fn contains_starts_with_and_exact() {
        let mut s = set(&[
            MacroDefinition::new(MacroKind::Trigger, "lantern", "a"),
            MacroDefinition::new(MacroKind::Trigger, "Your lantern", "b").with_match(MacroMatch::StartsWith),
            MacroDefinition::new(MacroKind::Trigger, "Your lantern gutters.", "c").with_match(MacroMatch::Exact),
            MacroDefinition::new(MacroKind::Trigger, "LANTERN", "d"),
            MacroDefinition::new(MacroKind::Trigger, "your LANTERN", "e")
                .with_match(MacroMatch::StartsWith)
                .ignoring_case(),
        ]);
        assert_eq!(lines(&mut s, "Your lantern gutters."), [0, 1, 2, 4]);
        assert_eq!(lines(&mut s, "Your lantern gutters. Again."), [0, 1, 4]);
        assert_eq!(lines(&mut s, "Look: Your lantern gutters."), [0]);
        assert_eq!(lines(&mut s, "A LANTERN lantern lantern"), [0, 3], "each rule once");
        assert!(lines(&mut s, "").is_empty());
    }

    #[test]
    fn ignore_case_folds_like_ecmascript() {
        let mut s = set(&[
            MacroDefinition::new(MacroKind::Trigger, "straße", "a").ignoring_case(),
            MacroDefinition::new(MacroKind::Trigger, "ÉCLAIR", "b").ignoring_case(),
            MacroDefinition::new(MacroKind::Trigger, "k", "c")
                .with_match(MacroMatch::Exact)
                .ignoring_case(),
        ]);
        assert_eq!(
            lines(&mut s, "STRAßE"),
            [0],
            "ß has no single upper case and stays itself"
        );
        assert!(lines(&mut s, "STRASSE").is_empty());
        assert_eq!(lines(&mut s, "un éclair"), [1]);
        assert_eq!(lines(&mut s, "K"), [2]);
        // The Kelvin sign upper-cases to itself, not to ASCII K.
        assert!(lines(&mut s, "\u{212A}").is_empty());
    }

    /// C# `AliasConsumesOnlyTheExactCommand`.
    #[test]
    fn alias_consumes_only_the_exact_command() {
        let s = set(&[
            MacroDefinition::new(MacroKind::Alias, "h+", "look"),
            MacroDefinition::new(MacroKind::Alias, "FORD", "east").ignoring_case(),
            MacroDefinition::new(MacroKind::Alias, "h+", "second"),
        ]);
        assert_eq!(s.match_alias("hhh"), None);
        assert_eq!(s.match_alias("h+ now"), None);
        assert_eq!(s.match_alias("h+"), Some(0), "the first in library order");
        assert_eq!(s.match_alias("ford"), Some(1));
        assert_eq!(s.match_alias("Ford"), Some(1));
        assert_eq!(s.match_alias(" ford"), None);
    }

    /// C# `ShortcutRespondsOnlyToItsFunctionKey`.
    #[test]
    fn shortcut_responds_only_to_its_function_key() {
        let s = set(&[
            MacroDefinition::new(MacroKind::Shortcut, "F4", "score"),
            MacroDefinition::new(MacroKind::Shortcut, "F4", "look"),
        ]);
        assert_eq!(s.match_key("F5").count(), 0);
        assert_eq!(s.match_key("F4").collect::<Vec<_>>(), [0, 1], "every macro on the key");
    }

    #[test]
    fn invalid_definitions_are_left_out() {
        let s = set(&[
            MacroDefinition::new(MacroKind::Trigger, "", "look"),
            MacroDefinition::new(MacroKind::Timer, "", "score").every(10),
        ]);
        assert_eq!(s.len(), 1);
        assert_eq!(s.rule(0).id, "m1");
        assert_eq!(s.timers(), [(0, Duration::from_secs(10))]);
        assert!(!s.has_triggers());
    }
}
