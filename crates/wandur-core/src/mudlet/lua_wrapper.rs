//! Builds a Mudlet-compatible Lua script from the items an import kept for conversion (the C#
//! `MudletLuaWrapper`, behind Run as Lua): each trigger, alias, timer and script registers by
//! name with the Mudlet layer, and its original Lua runs unchanged as the body. Items the layer
//! cannot run (chains, multi-line and colour triggers, keys, buttons, patterns JavaScript lacks)
//! are listed as comments, so the script still says what it is missing.

use super::converter::{parse_time, seconds_text};
use super::model::ItemKind;
use super::regex;
use super::summary::{describe, reason};
use crate::db::scripts::{ImportInfo, ImportedItem};
use crate::l10n::{S, t, tf};

const RUNNABLE: &[&str] = &[
    "",
    reason::LUA,
    reason::GEYSER,
    reason::MAPPER,
    reason::SCRIPT,
    reason::ALIAS_CHAIN,
    reason::UNSENDABLE,
    reason::TIMER,
];

/// The Lua source for an imported record.
pub fn build(import: &ImportInfo, separator: &str) -> String {
    let group = if import.group.is_empty() {
        t(S::MudletLooseItems).to_string()
    } else {
        import.group.clone()
    };
    let mut text = String::new();
    comment(&mut text, &tf(S::MudletLuaHeader, &[&import.source, &group]));
    comment(&mut text, t(S::MudletLuaHeaderDetail));
    text.push_str(&format!("__mudlet.separator({})\n", quote(separator)));
    for item in &import.items {
        text.push('\n');
        let name = [item.path.as_str(), item.name.as_str()]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" / ");
        let Some((open, close)) = registration(item, &name) else {
            comment(
                &mut text,
                &tf(
                    S::MudletLuaNotRun,
                    &[&kind_label(&item.kind), &name, &describe(&item.reason)],
                ),
            );
            continue;
        };
        comment(&mut text, &format!("{}: {name}", kind_label(&item.kind)));
        text.push_str(&open);
        text.push_str("function()\n");
        if !item.command.is_empty() && item.kind != "script" {
            text.push_str(&format!("expandAlias({})\n", quote(&item.command)));
        }
        text.push_str(item.code.trim_end_matches(['\r', '\n']));
        text.push('\n');
        text.push_str("end");
        text.push_str(&close);
        text.push('\n');
    }
    text
}

fn registration(item: &ImportedItem, name: &str) -> Option<(String, String)> {
    if !RUNNABLE.contains(&item.reason.as_str()) {
        return None;
    }
    let active = if item.active { "true" } else { "false" };
    match item.kind.as_str() {
        "trigger" => {
            let mut patterns = Vec::new();
            for pattern in &item.patterns {
                let (kind, text) = pattern.split_once(':').unwrap_or(("", pattern));
                if !matches!(kind, "substring" | "regex" | "begin" | "exact" | "prompt") {
                    return None;
                }
                if kind != "prompt" && text.is_empty() {
                    continue;
                }
                if kind == "regex" && regex::from_perl(text).is_none() {
                    return None;
                }
                patterns.push(format!("{{ {}, {} }}", quote(kind), quote(text)));
            }
            if patterns.is_empty() {
                return None;
            }
            Some((
                format!("__mudlet.trigger({}, {{ {} }}, ", quote(name), patterns.join(", ")),
                format!(", {active})"),
            ))
        }
        "alias" => {
            let pattern = item.patterns.iter().find_map(|p| p.strip_prefix("regex:"))?;
            regex::from_perl(pattern)?;
            Some((
                format!("__mudlet.alias({}, {}, ", quote(name), quote(pattern)),
                format!(", {active})"),
            ))
        }
        "timer" => {
            let time = item.patterns.iter().find_map(|p| p.strip_prefix("time:"))?;
            let seconds = parse_time(time).filter(|s| *s > 0.0)?;
            Some((
                format!("__mudlet.timer({}, {}, ", quote(name), seconds_text(seconds)),
                format!(", {active})"),
            ))
        }
        "script" => {
            let events: Vec<String> = item
                .patterns
                .iter()
                .filter_map(|p| p.strip_prefix("event:"))
                .map(quote)
                .collect();
            // Mudlet calls the function named after the script for its events.
            Some((
                format!("__mudlet.script({}, {{ {} }}, ", quote(&item.name), events.join(", ")),
                ")".to_string(),
            ))
        }
        _ => None,
    }
}

fn kind_label(kind: &str) -> String {
    ItemKind::from_key(kind).map_or_else(|| kind.to_string(), |k| k.label().to_string())
}

fn comment(text: &mut String, value: &str) {
    text.push_str("-- ");
    text.extend(value.chars().map(|c| if c.is_control() { ' ' } else { c }));
    text.push('\n');
}

/// A Lua string literal: quotes and backslashes escaped, control characters as decimal escapes.
pub(crate) fn quote(value: &str) -> String {
    let mut text = String::from("\"");
    for c in value.chars() {
        if c == '"' || c == '\\' {
            text.push('\\');
            text.push(c);
        } else if c.is_control() {
            text.push_str(&format!("\\{:03}", c as u32));
        } else {
            text.push(c);
        }
    }
    text.push('"');
    text
}
