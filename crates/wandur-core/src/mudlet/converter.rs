//! Maps a Mudlet profile or package onto a world's script library (the C# `MudletConverter`).
//! Items that only send commands become JavaScript, one script per top-level Mudlet folder, and
//! F1 to F12 keys become key macros; everything else is kept unchanged in switched-off scripts
//! marked as needing conversion, so nothing is lost. The library's own limits (entries per
//! world, hooks per script, source size) decide how items are grouped into scripts.

use std::cmp::Ordering;

use super::model::{Item, ItemKind, Pattern, Source};
use super::plain_send::{self, SendPart, SendStep};
use super::regex::{self, ScriptPattern};
use super::summary::{Entry, Summary, reason};
use crate::db::scripts::{ImportInfo, ImportedItem, LibraryEntry};
use crate::l10n::{S, t, tf};
use crate::macros::{MacroDefinition, MacroKind};
use crate::scripting::engine::RegexCheck;

/// Hooks one generated script may register, below the engine's 256, leaving room for its two
/// listeners.
pub const MAX_HOOKS_PER_SCRIPT: usize = 240;
/// Source characters per generated script, below the 256 KiB entry limit so the kept items fit
/// beside it.
pub const MAX_SOURCE_PER_SCRIPT: usize = 120_000;
const MAX_NESTED_ALIAS_EXPANSION: usize = 8;
const MAPPER_CALLS: &[&str] = &[
    "addRoom",
    "setExit",
    "setRoomCoordinates",
    "setRoomArea",
    "addAreaName",
    "createRoomID",
    "centerview",
    "getRoomArea",
    "setRoomEnv",
    "addSpecialExit",
];

/// Where an imported item ends up: converted and running, converted but off (it was off in
/// Mudlet), or kept with its original Lua, switched off, needing conversion by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BucketState {
    On,
    Off,
    NeedsConversion,
}

/// The scripts an import would write to one world's library, and what it did.
#[derive(Clone, Debug)]
pub struct ImportPlan {
    pub source_name: String,
    pub scripts: Vec<LibraryEntry>,
    pub summary: Summary,
}

struct Candidate {
    item: Item,
    folders: Vec<String>,
    separator: String,
    active: bool,
    reason: &'static str,
    left_out: bool,
    line_patterns: Vec<ScriptPattern>,
    on_prompt: bool,
    alias_pattern: Option<ScriptPattern>,
    timer_seconds: f64,
    key_name: String,
    steps: Vec<SendStep>,
}

impl Candidate {
    fn group(&self) -> &str {
        self.folders.first().map_or("", String::as_str)
    }

    fn path(&self) -> String {
        let name = self.item.name.trim();
        let mut parts: Vec<&str> = self.folders.iter().map(String::as_str).collect();
        parts.push(if name.is_empty() { "?" } else { name });
        parts.join(" / ")
    }

    fn converted(&self) -> bool {
        self.reason.is_empty() && !self.left_out
    }

    fn state(&self) -> BucketState {
        if !self.converted() {
            BucketState::NeedsConversion
        } else if self.active {
            BucketState::On
        } else {
            BucketState::Off
        }
    }
}

struct Bucket {
    state: BucketState,
    group: String,
    items: Vec<usize>,
    merged: bool,
}

pub struct Converter {
    world_key: String,
    available: usize,
    regex: Option<RegexCheck>,
}

impl Converter {
    /// `world_key` seeds the stable ids; `available` is how many entries the library can still
    /// take for this import.
    pub fn new(world_key: &str, available: usize) -> Self {
        Self {
            world_key: world_key.to_string(),
            available,
            regex: RegexCheck::new(),
        }
    }

    pub fn convert(&self, source: &Source) -> ImportPlan {
        let mut plan = ImportPlan {
            source_name: clean(&source.name, 200, t(S::MudletImportUnnamed)),
            scripts: Vec::new(),
            summary: Summary {
                password_skipped: source.password_skipped,
                ..Summary::default()
            },
        };
        let mut candidates = Vec::new();
        for package in &source.packages {
            let separator = &package.command_separator;
            for list in [
                &package.triggers,
                &package.aliases,
                &package.timers,
                &package.keys,
                &package.scripts,
                &package.buttons,
            ] {
                walk(list, &[], true, false, separator, &mut candidates);
            }
            plan.summary.variables_left_out += package.variable_count;
            for module in &package.modules {
                plan.summary
                    .left_out
                    .push(Entry::new(None, clean(module, 200, "?"), reason::MODULES));
            }
        }
        for candidate in candidates.iter_mut().filter(|c| c.reason.is_empty() && !c.left_out) {
            self.classify(candidate);
        }
        self.check_alias_expansion(&mut candidates);
        self.build_scripts(&mut plan, &candidates);
        plan
    }

    fn valid(&self, pattern: &ScriptPattern) -> bool {
        self.regex
            .as_ref()
            .is_some_and(|r| r.is_valid(&pattern.source, &pattern.flags))
    }

    fn matches(&self, pattern: &ScriptPattern, text: &str) -> bool {
        self.regex
            .as_ref()
            .is_some_and(|r| r.matches(&pattern.source, &pattern.flags, text))
    }

    fn classify(&self, candidate: &mut Candidate) {
        let item = &candidate.item;
        match item.kind {
            ItemKind::Script => {
                if item.script.trim().is_empty() && item.event_handlers.is_empty() {
                    return leave(candidate, reason::EMPTY);
                }
                candidate.reason = lua_reason(&item.script, reason::SCRIPT);
                return;
            }
            ItemKind::Button => {
                if item.script.trim().is_empty() && item.command.trim().is_empty() {
                    return leave(candidate, reason::EMPTY);
                }
                candidate.reason = reason::BUTTON;
                return;
            }
            ItemKind::Trigger => {
                if item.is_multiline {
                    candidate.reason = reason::MULTILINE;
                    return;
                }
                if item.is_filter || item.is_highlight || item.is_sound || item.is_colour_trigger {
                    candidate.reason = reason::EFFECTS;
                    return;
                }
                if item
                    .patterns
                    .iter()
                    .all(|p| p.text.is_empty() && p.kind != Pattern::PROMPT)
                {
                    return leave(candidate, reason::NO_PATTERN);
                }
                let mut lines = Vec::new();
                let mut on_prompt = false;
                for pattern in &item.patterns {
                    if pattern.text.is_empty() && pattern.kind != Pattern::PROMPT {
                        continue;
                    }
                    let converted = match pattern.kind {
                        Pattern::SUBSTRING => Some(regex::literal(&pattern.text, false, false)),
                        Pattern::BEGINNING_OF_LINE => Some(regex::literal(&pattern.text, true, false)),
                        Pattern::EXACT => Some(regex::literal(&pattern.text, true, true)),
                        Pattern::REGEX => regex::from_perl(&pattern.text),
                        Pattern::PROMPT => {
                            on_prompt = true;
                            continue;
                        }
                        _ => {
                            candidate.reason = reason::PATTERN_TYPE;
                            return;
                        }
                    };
                    match converted.filter(|p| self.valid(p)) {
                        Some(p) => lines.push(p),
                        None => {
                            candidate.reason = reason::REGEX;
                            return;
                        }
                    }
                }
                candidate.line_patterns = lines;
                candidate.on_prompt = on_prompt;
            }
            ItemKind::Alias => {
                if item.alias_pattern.is_empty() {
                    return leave(candidate, reason::NO_PATTERN);
                }
                match regex::from_perl(&item.alias_pattern).filter(|p| self.valid(p)) {
                    Some(p) => candidate.alias_pattern = Some(p),
                    None => {
                        candidate.reason = reason::REGEX;
                        return;
                    }
                }
            }
            ItemKind::Timer => match parse_time(&item.time) {
                Some(seconds) if !item.is_offset_timer && seconds >= 1.0 => candidate.timer_seconds = seconds,
                _ => {
                    candidate.reason = reason::TIMER;
                    return;
                }
            },
            ItemKind::Key => {
                // Qt numbers F1 to F12 from 0x01000030; Wandur binds those keys, without
                // modifiers, to key macros.
                if item.key_modifier != 0 || !(0x0100_0030..=0x0100_003B).contains(&item.key_code) {
                    candidate.reason = reason::KEY;
                    return;
                }
                candidate.key_name = format!("F{}", item.key_code - 0x0100_0030 + 1);
            }
        }
        let item = &candidate.item;
        // The command field goes through the aliases, as typed input would; the script follows.
        let mut steps = Vec::new();
        if !item.command.is_empty() {
            steps.push(SendStep::literal(&item.command, true));
        }
        if !item.script.trim().is_empty() {
            match plain_send::parse(&item.script) {
                Some(parsed) => steps.extend(parsed),
                None => {
                    candidate.reason = lua_reason(&item.script, reason::LUA);
                    return;
                }
            }
        }
        let kind = item.kind;
        // Captures only exist where something matched; a timer or a key has nothing to capture.
        if matches!(kind, ItemKind::Timer | ItemKind::Key) && steps.iter().any(|s| !s.is_literal()) {
            candidate.reason = reason::LUA;
            return;
        }
        if kind == ItemKind::Alias && steps.iter().any(|s| s.parts.contains(&SendPart::Line)) {
            candidate.reason = reason::LUA;
            return;
        }
        if steps.is_empty() && kind != ItemKind::Alias {
            return leave(candidate, reason::EMPTY);
        }
        candidate.steps = steps;
        if !sendable(candidate) {
            candidate.reason = reason::UNSENDABLE;
        }
    }

    /// A literal command sent through the aliases must land on an alias the same generated
    /// script holds, since each script only sees its own aliases. A key macro sends without
    /// aliases at all, so any match is a miss.
    fn check_alias_expansion(&self, candidates: &mut [Candidate]) {
        for _ in 0..4 {
            let aliases: Vec<(ScriptPattern, String, BucketState)> = candidates
                .iter()
                .filter(|c| c.converted())
                .filter_map(|c| c.alias_pattern.clone().map(|p| (p, c.group().to_string(), c.state())))
                .collect();
            let mut changed = false;
            for candidate in candidates.iter_mut().filter(|c| c.converted()) {
                let commands: Vec<String> = candidate
                    .steps
                    .iter()
                    .filter(|s| s.through_aliases && s.is_literal())
                    .flat_map(|s| split(&s.literal_text(), &candidate.separator))
                    .collect();
                for command in commands {
                    let Some((_, group, state)) = aliases.iter().find(|(p, _, _)| self.matches(p, &command)) else {
                        continue;
                    };
                    if candidate.item.kind != ItemKind::Key && group == candidate.group() && *state == candidate.state()
                    {
                        continue;
                    }
                    candidate.reason = reason::ALIAS_CHAIN;
                    changed = true;
                    break;
                }
            }
            if !changed {
                return;
            }
        }
    }

    fn build_scripts(&self, plan: &mut ImportPlan, candidates: &[Candidate]) {
        let summary = &mut plan.summary;
        for left in candidates.iter().filter(|c| c.left_out) {
            summary
                .left_out
                .push(Entry::new(Some(left.item.kind), left.path(), left.reason));
        }
        let placed: Vec<usize> = (0..candidates.len()).filter(|&i| !candidates[i].left_out).collect();
        let is_key_macro = |c: &Candidate| c.converted() && c.item.kind == ItemKind::Key;
        let mut keys: Vec<usize> = placed
            .iter()
            .copied()
            .filter(|&i| is_key_macro(&candidates[i]))
            .collect();
        let mut buckets: Vec<Bucket> = Vec::new();
        for &i in placed.iter().filter(|&&i| !is_key_macro(&candidates[i])) {
            let (state, group) = (candidates[i].state(), candidates[i].group());
            match buckets.iter_mut().find(|b| b.state == state && b.group == group) {
                Some(bucket) => bucket.items.push(i),
                None => buckets.push(Bucket {
                    state,
                    group: group.to_string(),
                    items: vec![i],
                    merged: false,
                }),
            }
        }
        buckets.sort_by(|a, b| {
            a.state
                .cmp(&b.state)
                .then((a.group.is_empty()).cmp(&b.group.is_empty()))
                .then_with(|| compare_ignoring_case(&a.group, &b.group))
        });

        // Fit the library: merge the smallest groups of one state into a shared script.
        let count = |buckets: &[Bucket], keys: &[usize]| -> usize {
            buckets.iter().map(|b| parts(candidates, &b.items).len()).sum::<usize>() + keys.len()
        };
        while count(&buckets, &keys) > self.available {
            let mut states: Vec<(BucketState, usize)> = Vec::new();
            for bucket in &buckets {
                match states.iter_mut().find(|(s, _)| *s == bucket.state) {
                    Some((_, n)) => *n += 1,
                    None => states.push((bucket.state, 1)),
                }
            }
            // The state with the most scripts (the first such, as C# orders them).
            let mut best: Option<(BucketState, usize)> = None;
            for &(state, n) in states.iter().filter(|(_, n)| *n > 1) {
                if best.is_none_or(|(_, most)| n > most) {
                    best = Some((state, n));
                }
            }
            let Some((state, _)) = best else {
                break;
            };
            let mut same: Vec<usize> = (0..buckets.len()).filter(|&i| buckets[i].state == state).collect();
            same.sort_by_key(|&i| buckets[i].items.len());
            let smallest: Vec<usize> = same.iter().copied().take(2).collect();
            let other = (0..buckets.len())
                .find(|&i| buckets[i].state == state && buckets[i].merged)
                .unwrap_or(smallest[0]);
            if let Some(&take) = smallest.iter().find(|&&i| i != other) {
                let items = std::mem::take(&mut buckets[take].items);
                buckets[other].items.extend(items);
                buckets[other].merged = true;
                buckets[other].group = t(S::MudletOtherGroups).to_string();
                buckets.remove(take);
            } else {
                buckets[other].merged = true;
                buckets[other].group = t(S::MudletOtherGroups).to_string();
            }
        }
        // Still too many: leave out keys first, then whole scripts, needing conversion first.
        while count(&buckets, &keys) > self.available
            && let Some(key) = keys.pop()
        {
            summary.left_out.push(Entry::new(
                Some(candidates[key].item.kind),
                candidates[key].path(),
                reason::NO_ROOM,
            ));
        }
        while count(&buckets, &keys) > self.available && !buckets.is_empty() {
            // The first script of the last state: what needs conversion goes first.
            let last = buckets.iter().map(|b| b.state).max().unwrap_or(BucketState::On);
            let drop = buckets.iter().position(|b| b.state == last).unwrap_or(0);
            let bucket = buckets.remove(drop);
            for i in bucket.items {
                summary.left_out.push(Entry::new(
                    Some(candidates[i].item.kind),
                    candidates[i].path(),
                    reason::NO_ROOM,
                ));
            }
        }

        for &key in &keys {
            plan.scripts.push(self.key_macro(&plan.source_name, &candidates[key]));
        }
        for bucket in &buckets {
            let split = parts(candidates, &bucket.items);
            for (index, items) in split.iter().enumerate() {
                let script = if bucket.state == BucketState::NeedsConversion {
                    self.lua_script(&plan.source_name, bucket, candidates, items, index, split.len())
                } else {
                    self.javascript(&plan.source_name, bucket, candidates, items, index, split.len())
                };
                plan.scripts.push(script);
            }
        }
        let summary = &mut plan.summary;
        for &i in &placed {
            let included = keys.contains(&i) || buckets.iter().any(|b| b.items.contains(&i));
            if !included {
                continue;
            }
            let candidate = &candidates[i];
            match candidate.state() {
                BucketState::NeedsConversion => summary.needs_conversion.push(Entry::new(
                    Some(candidate.item.kind),
                    candidate.path(),
                    candidate.reason,
                )),
                BucketState::On => Summary::add(&mut summary.working, candidate.item.kind),
                BucketState::Off => Summary::add(&mut summary.off_in_mudlet, candidate.item.kind),
            }
        }
    }

    fn script_name(&self, bucket: &Bucket, index: usize, count: usize) -> String {
        let group = if bucket.group.is_empty() {
            t(S::MudletLooseItems).to_string()
        } else {
            bucket.group.clone()
        };
        let key = match bucket.state {
            BucketState::On => S::MudletScriptName,
            BucketState::Off => S::MudletScriptNameOff,
            BucketState::NeedsConversion => S::MudletScriptNameLua,
        };
        let mut name = tf(key, &[&group]);
        if count > 1 {
            name.push(' ');
            name.push_str(&(index + 1).to_string());
        }
        clean(&name, 120, "Mudlet")
    }

    fn id(&self, source_name: &str, bucket_key: &str) -> String {
        ImportInfo::id_for(&self.world_key, ImportInfo::MUDLET, source_name, bucket_key)
    }

    fn key_macro(&self, source_name: &str, key: &Candidate) -> LibraryEntry {
        let commands: Vec<String> = key
            .steps
            .iter()
            .flat_map(|s| split(&s.literal_text(), &key.separator))
            .collect();
        let definition = MacroDefinition::new(MacroKind::Shortcut, &key.key_name, &commands.join("\n"));
        let source = definition.compile_javascript().unwrap_or_default();
        let label = if key.item.name.trim().is_empty() {
            key.path()
        } else {
            key.item.name.trim().to_string()
        };
        LibraryEntry {
            id: self.id(source_name, &format!("key\n{}\n{}", key.path(), key.key_name)),
            name: clean(
                &tf(S::MudletKeyScriptName, &[&key.key_name, &label]),
                120,
                &key.key_name,
            ),
            source: source.clone(),
            enabled: key.active,
            macro_def: Some(definition),
            import: Some(ImportInfo {
                origin: ImportInfo::MUDLET.into(),
                source: source_name.to_string(),
                group: clean(key.group(), 200, ""),
                needs_conversion: false,
                items: vec![kept(key)],
                source_hash: ImportInfo::hash(&source),
            }),
            ..LibraryEntry::default()
        }
    }

    fn lua_script(
        &self,
        source_name: &str,
        bucket: &Bucket,
        candidates: &[Candidate],
        items: &[usize],
        index: usize,
        count: usize,
    ) -> LibraryEntry {
        let group = group_label(&bucket.group);
        let mut text = String::new();
        line(&mut text, "-- ", &tf(S::MudletHeaderLua, &[&source_name, &group]));
        line(&mut text, "-- ", t(S::MudletHeaderLuaDetail));
        for &i in items {
            let item = &candidates[i];
            text.push('\n');
            line(
                &mut text,
                "-- ",
                &tf(
                    S::MudletItemHeader,
                    &[
                        &item.item.kind.label(),
                        &item.path(),
                        &super::summary::describe(item.reason),
                    ],
                ),
            );
            for pattern in patterns(&item.item) {
                line(&mut text, "--   ", &pattern);
            }
            if !item.item.command.is_empty() {
                line(&mut text, "--   command: ", &item.item.command);
            }
            text.push_str(item.item.script.trim_end_matches(['\r', '\n']));
            text.push('\n');
        }
        LibraryEntry {
            id: self.id(source_name, &format!("lua\n{}\n{index}", bucket.group)),
            name: self.script_name(bucket, index, count),
            source: text.clone(),
            enabled: false,
            import: Some(ImportInfo {
                origin: ImportInfo::MUDLET.into(),
                source: source_name.to_string(),
                group: clean(&bucket.group, 200, ""),
                needs_conversion: true,
                items: items.iter().map(|&i| kept(&candidates[i])).collect(),
                source_hash: ImportInfo::hash(&text),
            }),
            ..LibraryEntry::default()
        }
    }

    fn javascript(
        &self,
        source_name: &str,
        bucket: &Bucket,
        candidates: &[Candidate],
        items: &[usize],
        index: usize,
        count: usize,
    ) -> LibraryEntry {
        let separator = &candidates[items[0]].separator;
        let group = group_label(&bucket.group);
        let header = if bucket.state == BucketState::On {
            S::MudletHeaderConverted
        } else {
            S::MudletHeaderOff
        };
        let mut text = String::new();
        line(&mut text, "// ", &tf(header, &[&source_name, &group]));
        line(&mut text, "// ", t(S::MudletHeaderConvertedDetail));
        text.push_str("(() => {\n");
        text.push_str(&format!("    const separator = {};\n", js(separator)));
        text.push_str(&RUNTIME.replace("__DEPTH__", &MAX_NESTED_ALIAS_EXPANSION.to_string()));
        for &i in items {
            let item = &candidates[i];
            text.push('\n');
            line(
                &mut text,
                "    // ",
                &tf(S::MudletItemHeaderConverted, &[&item.item.kind.label(), &item.path()]),
            );
            let body = body(item);
            match item.item.kind {
                ItemKind::Trigger => {
                    if !item.line_patterns.is_empty() {
                        let list: Vec<String> = item.line_patterns.iter().map(pattern_js).collect();
                        text.push_str(&format!("    trigger([{}], (match, line) => {{\n", list.join(", ")));
                        text.push_str(&body);
                        text.push_str("    });\n");
                    }
                    if item.on_prompt {
                        text.push_str("    prompt((match, line) => {\n");
                        text.push_str(&body);
                        text.push_str("    });\n");
                    }
                }
                ItemKind::Alias => {
                    if let Some(pattern) = &item.alias_pattern {
                        text.push_str(&format!("    alias({}, (match, depth) => {{\n", pattern_js(pattern)));
                        text.push_str(&body);
                        text.push_str("    });\n");
                    }
                }
                ItemKind::Timer => {
                    text.push_str(&format!(
                        "    mud.every({}, () => {{\n",
                        seconds_text(item.timer_seconds)
                    ));
                    text.push_str(&body);
                    text.push_str("    });\n");
                }
                _ => {}
            }
        }
        text.push('\n');
        text.push_str("    if (lines.length > 0) mud.on(Events.Line, event => fire(lines, event.text));\n");
        text.push_str(
            "    if (prompts.length > 0) mud.on(Events.Prompt, event => { for (const run of prompts) run([event.text], event.text); });\n",
        );
        text.push_str("})();\n");
        let state = if bucket.state == BucketState::On { "on" } else { "off" };
        LibraryEntry {
            id: self.id(source_name, &format!("{state}\n{}\n{index}", bucket.group)),
            name: self.script_name(bucket, index, count),
            source: text.clone(),
            enabled: bucket.state == BucketState::On,
            import: Some(ImportInfo {
                origin: ImportInfo::MUDLET.into(),
                source: source_name.to_string(),
                group: clean(&bucket.group, 200, ""),
                needs_conversion: false,
                items: items.iter().map(|&i| kept(&candidates[i])).collect(),
                source_hash: ImportInfo::hash(&text),
            }),
            ..LibraryEntry::default()
        }
    }
}

/// The helpers every converted script starts with (the C# template).
const RUNTIME: &str = r#"    const aliases = [], lines = [], prompts = [];
    function commands(text) {
        const parts = separator.length > 0 ? text.split(separator) : [text];
        return parts.map(part => part.split("\n").join("")).filter(part => part.length > 0);
    }
    function send(text) { for (const command of commands(text)) mud.send(command); }
    function expand(text, depth) {
        for (const command of commands(text)) {
            let handled = false;
            if (depth < __DEPTH__) {
                for (const alias of aliases) {
                    alias.pattern.lastIndex = 0;
                    const match = alias.pattern.exec(command);
                    if (match) { alias.run(match, depth + 1); handled = true; break; }
                }
            }
            if (!handled) mud.send(command);
        }
    }
    function capture(match, index) { const value = match[index]; return value === undefined || value === null ? "" : String(value); }
    function fire(list, text) {
        for (const entry of list) {
            for (const pattern of entry.patterns) {
                pattern.lastIndex = 0;
                const match = pattern.exec(text);
                if (match) { entry.run(match, text); break; }
            }
        }
    }
    function alias(pattern, run) { aliases.push({ pattern, run }); mud.alias(pattern, match => run(match, 0)); }
    function trigger(patterns, run) { lines.push({ patterns, run }); }
    function prompt(run) { prompts.push(run); }
"#;

fn walk(items: &[Item], folders: &[String], active: bool, in_chain: bool, separator: &str, out: &mut Vec<Candidate>) {
    for item in items {
        let item_active = active && item.is_active;
        let acts = !item.is_folder || acts(item);
        let chain = in_chain || (item.kind == ItemKind::Trigger && acts && !item.children.is_empty());
        if acts {
            let mut candidate = Candidate {
                item: Item {
                    children: Vec::new(),
                    ..item.clone()
                },
                folders: folders.to_vec(),
                separator: separator.to_string(),
                active: item_active,
                reason: "",
                left_out: false,
                line_patterns: Vec::new(),
                on_prompt: false,
                alias_pattern: None,
                timer_seconds: 0.0,
                key_name: String::new(),
                steps: Vec::new(),
            };
            if item.is_temporary {
                candidate.left_out = true;
                candidate.reason = reason::TEMPORARY;
            } else if chain {
                candidate.reason = reason::CHAIN;
            }
            out.push(candidate);
        }
        if item.children.is_empty() {
            continue;
        }
        let mut child_folders = folders.to_vec();
        if item.is_folder || chain {
            child_folders.push(clean(&item.name, 120, "?"));
        }
        walk(&item.children, &child_folders, item_active, chain, separator, out);
    }
}

/// A folder that matches or runs something of its own, rather than only holding items.
fn acts(item: &Item) -> bool {
    let code = !item.script.trim().is_empty() || !item.command.trim().is_empty();
    match item.kind {
        ItemKind::Trigger => item.patterns.iter().any(|p| !p.text.is_empty()) || code,
        ItemKind::Alias => !item.alias_pattern.is_empty() || code,
        _ => code,
    }
}

fn leave(candidate: &mut Candidate, why: &'static str) {
    candidate.left_out = true;
    candidate.reason = why;
}

fn lua_reason(lua: &str, fallback: &'static str) -> &'static str {
    if lua.contains("Geyser") {
        reason::GEYSER
    } else if calls_mapper(lua) {
        reason::MAPPER
    } else {
        fallback
    }
}

/// A call of one of Mudlet's mapper functions (`\b(name)\s*\(`).
fn calls_mapper(lua: &str) -> bool {
    MAPPER_CALLS.iter().any(|name| {
        lua.match_indices(name).any(|(at, _)| {
            let before = lua[..at].chars().next_back();
            let boundary = before.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            boundary && lua[at + name.len()..].trim_start().starts_with('(')
        })
    })
}

/// Literal commands must be sendable: no control characters, and something to send after
/// Mudlet's own rules (split at the command separator, line feeds removed). A key macro has its
/// own limits too.
fn sendable(candidate: &Candidate) -> bool {
    let mut commands = Vec::new();
    for step in &candidate.steps {
        let literal: String = step
            .parts
            .iter()
            .filter_map(|p| match p {
                SendPart::Literal(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        if literal
            .chars()
            .filter(|&c| c != '\n')
            .any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}')
        {
            return false;
        }
        if step.is_literal() {
            let split = split(&step.literal_text(), &candidate.separator);
            if split.is_empty() {
                return false;
            }
            commands.extend(split);
        }
    }
    let units = |c: &String| c.encode_utf16().count();
    if candidate.item.kind == ItemKind::Key {
        return (1..=20).contains(&commands.len()) && commands.iter().all(|c| units(c) <= 1024 && !c.trim().is_empty());
    }
    commands.iter().all(|c| units(c) <= 4096)
}

/// Mudlet's command splitting: at the separator, line feeds removed, empty parts dropped.
pub(crate) fn split(text: &str, separator: &str) -> Vec<String> {
    let parts: Vec<&str> = if separator.is_empty() {
        vec![text]
    } else {
        text.split(separator).collect()
    };
    parts
        .into_iter()
        .map(|p| p.replace('\n', ""))
        .filter(|p| !p.is_empty())
        .collect()
}

/// `hh:mm:ss.zzz` (one or two digits each, up to three of milliseconds) in seconds, at most a day.
pub(crate) fn parse_time(text: &str) -> Option<f64> {
    let text = text.trim();
    let (clock, fraction) = match text.split_once('.') {
        Some((clock, fraction)) => (clock, Some(fraction)),
        None => (text, None),
    };
    let fields: Vec<&str> = clock.split(':').collect();
    if fields.len() != 3
        || fields
            .iter()
            .any(|f| f.is_empty() || f.len() > 2 || !f.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let milliseconds = match fraction {
        None => 0,
        Some(f) if !f.is_empty() && f.len() <= 3 && f.bytes().all(|b| b.is_ascii_digit()) => {
            format!("{f:0<3}").parse::<u32>().ok()?
        }
        Some(_) => return None,
    };
    let part = |i: usize| fields[i].parse::<u32>().unwrap_or(0);
    let seconds = f64::from(part(0) * 3600 + part(1) * 60 + part(2)) + f64::from(milliseconds) / 1000.0;
    (seconds <= 86_400.0).then_some(seconds)
}

/// Seconds as C# writes them with `0.###`.
pub(crate) fn seconds_text(seconds: f64) -> String {
    let rounded = (seconds * 1000.0).round() / 1000.0;
    let text = format!("{rounded:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The `type:text` lines kept for an item.
fn patterns(item: &Item) -> Vec<String> {
    match item.kind {
        ItemKind::Trigger => item
            .patterns
            .iter()
            .take(256)
            .map(|p| format!("{}:{}", p.type_name(), p.text))
            .collect(),
        ItemKind::Alias => vec![format!("regex:{}", item.alias_pattern)],
        ItemKind::Timer => vec![format!("time:{}", item.time)],
        ItemKind::Key => vec![format!("key:{}+{}", item.key_code, item.key_modifier)],
        ItemKind::Script => item.event_handlers.iter().map(|h| format!("event:{h}")).collect(),
        ItemKind::Button => Vec::new(),
    }
}

fn kept(candidate: &Candidate) -> ImportedItem {
    ImportedItem {
        kind: candidate.item.kind.key().to_string(),
        path: clean(&candidate.folders.join(" / "), 1000, ""),
        name: clean(&candidate.item.name, 400, ""),
        reason: if candidate.converted() {
            String::new()
        } else {
            candidate.reason.to_string()
        },
        code: candidate.item.script.clone(),
        patterns: patterns(&candidate.item),
        command: candidate.item.command.clone(),
        active: candidate.active,
    }
}

/// Splits a bucket so each script stays inside the hook limit and the source size limit.
fn parts(candidates: &[Candidate], items: &[usize]) -> Vec<Vec<usize>> {
    let mut parts = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let (mut hooks, mut size) = (0usize, 0usize);
    for &i in items {
        let item = &candidates[i];
        let item_hooks = usize::from(matches!(item.item.kind, ItemKind::Alias | ItemKind::Timer) && item.converted());
        let item_size = units(&item.item.script)
            + units(&item.item.command)
            + units(&item.item.alias_pattern)
            + item.item.patterns.iter().map(|p| units(&p.text)).sum::<usize>()
            + 400;
        if !current.is_empty()
            && (hooks + item_hooks > MAX_HOOKS_PER_SCRIPT
                || size + item_size > MAX_SOURCE_PER_SCRIPT
                || current.len() >= ImportInfo::MAX_ITEMS)
        {
            parts.push(std::mem::take(&mut current));
            hooks = 0;
            size = 0;
        }
        current.push(i);
        hooks += item_hooks;
        size += item_size;
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn units(text: &str) -> usize {
    text.encode_utf16().count()
}

fn body(item: &Candidate) -> String {
    let mut body = String::new();
    let depth = if item.item.kind == ItemKind::Alias {
        "depth"
    } else {
        "0"
    };
    for step in &item.steps {
        let expression = step
            .parts
            .iter()
            .map(|part| match part {
                SendPart::Line => "line".to_string(),
                SendPart::Capture(n) => format!("capture(match, {})", n - 1),
                SendPart::Literal(text) => js(text),
            })
            .collect::<Vec<_>>()
            .join(" + ");
        if step.through_aliases {
            body.push_str(&format!("        expand({expression}, {depth});\n"));
        } else {
            body.push_str(&format!("        send({expression});\n"));
        }
    }
    body
}

fn pattern_js(pattern: &ScriptPattern) -> String {
    format!("new RegExp({}, {})", js(&pattern.source), js(&pattern.flags))
}

/// A JavaScript string literal (JSON).
fn js(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

fn group_label(group: &str) -> String {
    if group.is_empty() {
        t(S::MudletLooseItems).to_string()
    } else {
        group.to_string()
    }
}

/// One comment line: line breaks and other control characters become spaces.
fn line(text: &mut String, prefix: &str, value: &str) {
    text.push_str(prefix);
    text.extend(value.chars().map(|c| {
        if c.is_control() || c == '\u{2028}' || c == '\u{2029}' {
            ' '
        } else {
            c
        }
    }));
    text.push('\n');
}

fn compare_ignoring_case(a: &str, b: &str) -> Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b))
}

/// A display name from untrusted text: control characters as spaces, trimmed, bounded, with a
/// fallback when nothing is left.
pub fn clean(value: &str, maximum: usize, fallback: &str) -> String {
    let cleaned: String = value.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut cleaned = cleaned.trim().to_string();
    if cleaned.chars().count() > maximum {
        cleaned = cleaned.chars().take(maximum).collect::<String>().trim_end().to_string();
    }
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}
