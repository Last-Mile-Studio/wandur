//! What an import did (the C# `MudletImportSummary` and `MudletReason`): counts of what works
//! now, what came in switched off and why, and what was left out and why.

use std::collections::BTreeMap;

use super::model::ItemKind;
use crate::l10n::{S, t, tf};

/// Why an item was not converted, or was left out. The values are stored with kept items, so
/// they stay stable; the text shown for each comes from the string table.
pub mod reason {
    pub const LUA: &str = "lua";
    pub const GEYSER: &str = "geyser";
    pub const MAPPER: &str = "mapper";
    pub const CHAIN: &str = "chain";
    pub const MULTILINE: &str = "multiline";
    pub const PATTERN_TYPE: &str = "pattern-type";
    pub const REGEX: &str = "regex";
    pub const EFFECTS: &str = "effects";
    pub const KEY: &str = "key";
    pub const TIMER: &str = "timer";
    pub const SCRIPT: &str = "script";
    pub const BUTTON: &str = "button";
    pub const ALIAS_CHAIN: &str = "alias-chain";
    pub const UNSENDABLE: &str = "unsendable";
    pub const EMPTY: &str = "empty";
    pub const NO_PATTERN: &str = "no-pattern";
    pub const TEMPORARY: &str = "temporary";
    pub const VARIABLES: &str = "variables";
    pub const MODULES: &str = "modules";
    pub const NO_ROOM: &str = "no-room";
    pub const EDITED: &str = "edited";
}

/// The text shown for a reason.
pub fn describe(reason: &str) -> String {
    let key = match reason {
        reason::LUA => S::MudletReasonLua,
        reason::GEYSER => S::MudletReasonGeyser,
        reason::MAPPER => S::MudletReasonMapper,
        reason::CHAIN => S::MudletReasonChain,
        reason::MULTILINE => S::MudletReasonMultiline,
        reason::PATTERN_TYPE => S::MudletReasonPatternType,
        reason::REGEX => S::MudletReasonRegex,
        reason::EFFECTS => S::MudletReasonEffects,
        reason::KEY => S::MudletReasonKey,
        reason::TIMER => S::MudletReasonTimer,
        reason::SCRIPT => S::MudletReasonScript,
        reason::BUTTON => S::MudletReasonButton,
        reason::ALIAS_CHAIN => S::MudletReasonAliasChain,
        reason::UNSENDABLE => S::MudletReasonUnsendable,
        reason::EMPTY => S::MudletReasonEmpty,
        reason::NO_PATTERN => S::MudletReasonNoPattern,
        reason::TEMPORARY => S::MudletReasonTemporary,
        reason::VARIABLES => S::MudletReasonVariables,
        reason::MODULES => S::MudletReasonModules,
        reason::NO_ROOM => S::MudletReasonNoRoom,
        reason::EDITED => S::MudletReasonEdited,
        other => return other.to_string(),
    };
    t(key).to_string()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub kind: Option<ItemKind>,
    pub path: String,
    pub reason: &'static str,
}

impl Entry {
    pub fn new(kind: Option<ItemKind>, path: impl Into<String>, reason: &'static str) -> Self {
        Self {
            kind,
            path: path.into(),
            reason,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub world_name: String,
    pub world_created: bool,
    pub password_skipped: bool,
    pub scripts_written: usize,
    pub working: BTreeMap<ItemKind, usize>,
    pub off_in_mudlet: BTreeMap<ItemKind, usize>,
    pub needs_conversion: Vec<Entry>,
    pub left_out: Vec<Entry>,
    /// Saved variables, counted rather than listed.
    pub variables_left_out: usize,
}

impl Summary {
    pub fn working_count(&self) -> usize {
        self.working.values().sum()
    }

    pub fn off_count(&self) -> usize {
        self.off_in_mudlet.values().sum()
    }

    pub(crate) fn add(counts: &mut BTreeMap<ItemKind, usize>, kind: ItemKind) {
        *counts.entry(kind).or_default() += 1;
    }

    fn counts(counts: &BTreeMap<ItemKind, usize>) -> String {
        counts
            .iter()
            .filter(|(_, n)| **n > 0)
            .map(|(kind, n)| tf(S::MudletSummaryCount, &[&kind.label(), n]))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The summary as plain text for the dialog, at most `examples` item names per reason.
    pub fn to_text(&self, examples: usize) -> String {
        let mut lines: Vec<String> = Vec::new();
        lines.push(tf(
            if self.world_created {
                S::MudletSummaryWorldCreated
            } else {
                S::MudletSummaryWorldUpdated
            },
            &[&self.world_name],
        ));
        lines.push(String::new());
        lines.push(if self.working_count() > 0 {
            tf(S::MudletSummaryWorking, &[&Self::counts(&self.working)])
        } else {
            t(S::MudletSummaryNothingWorking).to_string()
        });
        if self.off_count() > 0 {
            lines.push(tf(S::MudletSummaryOffInMudlet, &[&Self::counts(&self.off_in_mudlet)]));
        }
        if !self.needs_conversion.is_empty() {
            lines.push(String::new());
            lines.push(tf(S::MudletSummaryNeedsConversion, &[&self.needs_conversion.len()]));
            reasons(&mut lines, &self.needs_conversion, examples);
        }
        if !self.left_out.is_empty() || self.variables_left_out > 0 {
            lines.push(String::new());
            lines.push(tf(
                S::MudletSummaryLeftOut,
                &[&(self.left_out.len() + self.variables_left_out)],
            ));
            reasons(&mut lines, &self.left_out, examples);
            if self.variables_left_out > 0 {
                lines.push(indent(
                    2,
                    &tf(
                        S::MudletSummaryReason,
                        &[&describe(reason::VARIABLES), &self.variables_left_out],
                    ),
                ));
            }
        }
        if self.password_skipped {
            lines.push(String::new());
            lines.push(t(S::MudletSummaryPassword).to_string());
        }
        lines.join("\n").trim_end().to_string()
    }
}

fn indent(spaces: usize, text: &str) -> String {
    let mut line = " ".repeat(spaces);
    line.push_str(text);
    line
}

/// Each reason with its count (most first, ties in order of appearance) and up to `examples`
/// item names.
fn reasons(lines: &mut Vec<String>, entries: &[Entry], examples: usize) {
    let mut groups: Vec<(&str, Vec<&Entry>)> = Vec::new();
    for entry in entries {
        match groups.iter_mut().find(|(r, _)| *r == entry.reason) {
            Some((_, list)) => list.push(entry),
            None => groups.push((entry.reason, vec![entry])),
        }
    }
    groups.sort_by_key(|(_, list)| std::cmp::Reverse(list.len()));
    for (reason, list) in groups {
        lines.push(indent(
            2,
            &tf(S::MudletSummaryReason, &[&describe(reason), &list.len()]),
        ));
        let mut names: Vec<&str> = Vec::new();
        for entry in &list {
            if !entry.path.is_empty() && !names.contains(&entry.path.as_str()) {
                names.push(&entry.path);
            }
        }
        if names.is_empty() {
            continue;
        }
        let shown = names.iter().take(examples).copied().collect::<Vec<_>>().join(", ");
        lines.push(indent(
            4,
            &if names.len() > examples {
                tf(S::MudletSummaryMore, &[&shown, &(names.len() - examples)])
            } else {
                shown
            },
        ));
    }
}
