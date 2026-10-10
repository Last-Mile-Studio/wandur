//! The shipped channel shapes, one rule set per codebase family (`channels/families.json`, the
//! C# client's file). A world's own rules come first, then its family's (or the generic set),
//! so a rule taught on a world wins over the shipped shape for the same line and an exclusion
//! taught there wins over both.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use serde::Deserialize;

use super::rules::{ChannelRule, RuleSet};

/// The family for codebases Wandur does not know.
pub const GENERIC: &str = "generic";

const FAMILIES: &str = include_str!("../../channels/families.json");

/// One entry of the shipped file: an include, or a rule in the shape a world carries.
#[derive(Deserialize)]
struct Entry {
    #[serde(default)]
    include: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    reply_command: Option<String>,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    exclude: bool,
}

type Raw = indexmap::IndexMap<String, Vec<Entry>>;

struct Families {
    /// Names in the order the file lists them.
    names: Vec<String>,
    rules: HashMap<String, Vec<ChannelRule>>,
    sets: HashMap<String, Arc<RuleSet>>,
}

fn families() -> &'static Families {
    static LOADED: OnceLock<Families> = OnceLock::new();
    LOADED.get_or_init(|| {
        // serde_json keeps the file's order (`preserve_order`).
        let file: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(FAMILIES).expect("the shipped channel families are valid JSON");
        let raw: Raw = file
            .into_iter()
            .map(|(k, v)| (k.to_lowercase(), serde_json::from_value(v).unwrap_or_default()))
            .collect();
        let mut names: Vec<String> = raw.keys().cloned().collect();
        let mut rules = HashMap::new();
        for name in &names {
            let mut out = Vec::new();
            expand(&raw, &raw[name], 0, &mut out);
            rules.insert(name.clone(), out);
        }
        if !rules.contains_key(GENERIC) {
            names.push(GENERIC.into());
            rules.insert(GENERIC.into(), Vec::new());
        }
        let sets = rules
            .iter()
            .map(|(name, list)| (name.clone(), Arc::new(RuleSet::new(list.iter().cloned()))))
            .collect();
        Families { names, rules, sets }
    })
}

/// A family may open with the rules of the family it derives from, keeping the file short.
fn expand(raw: &Raw, entries: &[Entry], depth: usize, out: &mut Vec<ChannelRule>) {
    for entry in entries {
        if let Some(include) = entry.include.as_deref().filter(|i| !i.is_empty()) {
            if depth < 4
                && let Some(target) = raw.get(&include.to_lowercase())
            {
                expand(raw, target, depth + 1, out);
            }
            continue;
        }
        if let (Some(channel), Some(pattern)) = (&entry.channel, &entry.pattern)
            && !channel.is_empty()
            && !pattern.is_empty()
        {
            out.push(ChannelRule {
                channel: channel.clone(),
                pattern: pattern.clone(),
                reply_command: entry.reply_command.clone(),
                private: entry.private,
                exclude: entry.exclude,
                disabled: false,
            });
        }
    }
}

/// The family names, in the order the shipped file lists them.
pub fn names() -> &'static [String] {
    &families().names
}

/// The rules of one family, or the generic set when the name is not one Wandur ships.
pub fn family(name: Option<&str>) -> Arc<RuleSet> {
    let f = families();
    let key = name.map(|n| n.trim().to_lowercase()).unwrap_or_default();
    Arc::clone(f.sets.get(&key).unwrap_or_else(|| &f.sets[GENERIC]))
}

fn family_rules(name: &str) -> &'static [ChannelRule] {
    let f = families();
    f.rules.get(name).unwrap_or_else(|| &f.rules[GENERIC])
}

/// The family a free-text codebase belongs to ("SMAUG 1.4a", "ROM 2.4b6", "Custom LPMud"):
/// a containment test, most specific first, as C# `ChannelFamilies.Match`.
pub fn match_codebase(codebase: Option<&str>) -> &'static str {
    let text = codebase.unwrap_or("").to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| text.contains(w));
    if text.is_empty() {
        GENERIC
    } else if has(&["swr", "star wars reality"]) {
        "swr"
    } else if has(&["smaug", "aftermath"]) {
        "smaug"
    } else if has(&["circle", "tba"]) {
        "circle"
    } else if has(&["rom", "envy", "merc", "godwars"]) {
        "rom"
    } else if has(&["lp", "ldmud", "dgd", "mudos"]) {
        "lp"
    } else if has(&["diku"]) {
        "diku"
    } else {
        GENERIC
    }
}

/// The rule set for a world: its own rules first, then its family's (by codebase).
pub fn for_world(world_rules: &[ChannelRule], codebase: Option<&str>) -> Arc<RuleSet> {
    let name = match_codebase(codebase);
    if world_rules.is_empty() {
        return family(Some(name));
    }
    Arc::new(RuleSet::new(
        world_rules.iter().cloned().chain(family_rules(name).iter().cloned()),
    ))
}
