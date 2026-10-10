//! Brings a read Mudlet source into Wandur (the C# `MudletImporter`): picks or creates the
//! world, then writes the converted scripts into that world's script library. Importing the
//! same profile again replaces what the last import wrote, keeps the person's on and off
//! choices, and never overwrites a script they edited since.

use std::collections::HashSet;

use super::converter::{Converter, ImportPlan, clean};
use super::model::{ImportError, ItemKind, Source};
use super::summary::{Entry, Summary, reason};
use crate::charset::Charset;
use crate::db::scripts::{self, ImportInfo, LibraryEntry};
use crate::endpoint::Endpoint;
use crate::l10n::{S, t, tf};
use crate::settings::SavedWorld;

/// The world an import goes into.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// The world as it should be saved (a new one, or the saved one with a login filled in).
    pub world: SavedWorld,
    /// Its index among the saved worlds; `None` for a world the import adds.
    pub index: Option<usize>,
}

impl Target {
    pub fn created(&self) -> bool {
        self.index.is_none()
    }
}

/// The world for this source: the saved world with the same host and port, or a new one built
/// from the profile's connection details. A package names no world, so it goes into
/// `selected`. A stored password is never carried over.
pub fn resolve_world(source: &Source, worlds: &[SavedWorld], selected: Option<usize>) -> Result<Target, ImportError> {
    let host = normalize_host(&source.host);
    if host.is_empty() {
        let index = selected
            .filter(|&i| i < worlds.len())
            .ok_or_else(|| ImportError::new(S::MudletImportNoHost))?;
        return Ok(Target {
            world: worlds[index].clone(),
            index: Some(index),
        });
    }
    let port = source.port.filter(|&p| p >= 1).unwrap_or(23);
    if let Some(index) = worlds
        .iter()
        .position(|w| w.host.trim().eq_ignore_ascii_case(&host) && w.port == port)
    {
        let mut world = worlds[index].clone();
        let login = login(&source.login);
        // Not under a saved password: the username is part of its vault key, so filling it in
        // would leave the password unreachable.
        if world.username.is_empty() && world.password_id.is_none() && !login.is_empty() {
            world.username = login;
        }
        return Ok(Target {
            world,
            index: Some(index),
        });
    }
    let address = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    if address.parse::<Endpoint>().is_err() {
        return Err(ImportError(tf(S::MudletImportInvalidHost, &[&host])));
    }
    Ok(Target {
        world: SavedWorld {
            name: clean(&source.name, 100, &host),
            host: host.clone(),
            port,
            tls: source.tls == Some(true),
            username: login(&source.login),
            charset: if is_latin1(&source.encoding) {
                Charset::Latin1
            } else {
                Charset::Utf8
            },
            ..SavedWorld::default()
        },
        index: None,
    })
}

/// Convert the source for the world's library `existing` and return the library as it should
/// be saved, with what the import did. `world_key` seeds the stable script ids.
pub fn apply(
    source: &Source,
    world_key: &str,
    world_name: &str,
    created: bool,
    existing: &[LibraryEntry],
) -> (Vec<LibraryEntry>, Summary) {
    let source_name = clean(&source.name, 200, t(S::MudletImportUnnamed));
    let previous: Vec<&LibraryEntry> = existing
        .iter()
        .filter(|e| {
            e.import
                .as_ref()
                .is_some_and(|i| i.origin == ImportInfo::MUDLET && i.source == source_name)
        })
        .collect();
    let edited: HashSet<&str> = previous
        .iter()
        .filter(|e| !e.import.as_ref().is_some_and(|i| i.is_unchanged(&e.source)))
        .map(|e| e.id.as_str())
        .collect();
    let others = existing.len() - previous.len();
    let available = scripts::MAX_ENTRIES.saturating_sub(others + edited.len());
    let mut plan = Converter::new(world_key, available).convert(source);
    plan.summary.world_name = world_name.to_string();
    plan.summary.world_created = created;

    // A script the person edited after the last import is theirs now: neither replaced nor removed.
    let mut kept_edits = Vec::new();
    plan.scripts.retain(|script| {
        if edited.contains(script.id.as_str()) {
            kept_edits.push(Entry::new(None, script.name.clone(), reason::EDITED));
            false
        } else {
            true
        }
    });
    plan.summary.left_out.extend(kept_edits);
    // Earlier choices to switch an imported script on or off are kept.
    for script in plan.scripts.iter_mut() {
        if let Some(old) = previous.iter().find(|p| p.id == script.id)
            && !script.needs_conversion()
        {
            script.enabled = old.enabled;
        }
    }
    let previous_ids: HashSet<&str> = previous.iter().map(|e| e.id.as_str()).collect();
    let kept: Vec<LibraryEntry> = existing
        .iter()
        .filter(|e| !previous_ids.contains(e.id.as_str()) || edited.contains(e.id.as_str()))
        .cloned()
        .collect();
    fit_library_size(&mut plan, &kept);

    let wanted: HashSet<String> = plan.scripts.iter().map(|s| s.id.clone()).collect();
    let mut after: Vec<LibraryEntry> = existing
        .iter()
        .filter(|e| !previous_ids.contains(e.id.as_str()) || wanted.contains(&e.id) || edited.contains(e.id.as_str()))
        .cloned()
        .collect();
    for script in &plan.scripts {
        match after.iter_mut().find(|e| e.id == script.id) {
            Some(slot) => *slot = script.clone(),
            None => after.push(script.clone()),
        }
    }
    plan.summary.scripts_written = plan.scripts.len();
    (after, plan.summary)
}

/// The whole library must stay inside the size limit. Scripts that need conversion are the
/// largest and the least useful right away, so they are left out first, biggest first.
fn fit_library_size(plan: &mut ImportPlan, kept: &[LibraryEntry]) {
    let size = |plan: &ImportPlan| {
        let all: Vec<&LibraryEntry> = kept.iter().chain(plan.scripts.iter()).collect();
        serde_json::to_vec(&all).map_or(usize::MAX, |b| b.len())
    };
    while !plan.scripts.is_empty() && size(plan) > scripts::MAX_LIBRARY_BYTES {
        let drop = (0..plan.scripts.len())
            .max_by_key(|&i| (plan.scripts[i].needs_conversion(), plan.scripts[i].source.len()))
            .unwrap_or(0);
        let script = plan.scripts.remove(drop);
        for item in script.import.iter().flat_map(|i| i.items.iter()) {
            let path = [item.path.as_str(), item.name.as_str()]
                .into_iter()
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join(" / ");
            let kind = ItemKind::from_key(&item.kind);
            plan.summary
                .needs_conversion
                .retain(|e| !(e.path == path && e.kind == kind));
            if let Some(kind) = kind
                && item.reason.is_empty()
            {
                let counts = if item.active && script.enabled {
                    &mut plan.summary.working
                } else {
                    &mut plan.summary.off_in_mudlet
                };
                if let Some(n) = counts.get_mut(&kind).filter(|n| **n > 0) {
                    *n -= 1;
                }
            }
            plan.summary.left_out.push(Entry::new(kind, path, reason::NO_ROOM));
        }
    }
}

fn normalize_host(value: &str) -> String {
    let mut host = value.trim();
    if let Some(at) = host.find("://") {
        host = &host[at + 3..];
    }
    let host = host.trim_end_matches('/');
    if host.chars().any(char::is_control) || host.contains('/') {
        String::new()
    } else {
        host.to_string()
    }
}

fn login(value: &str) -> String {
    if !value.is_empty() && value.encode_utf16().count() <= 256 && !value.chars().any(char::is_control) {
        value.to_string()
    } else {
        String::new()
    }
}

fn is_latin1(encoding: &str) -> bool {
    let normalized: String = encoding.chars().filter(|c| !matches!(c, ' ' | '-' | '_')).collect();
    normalized.eq_ignore_ascii_case("ISO88591") || normalized.eq_ignore_ascii_case("LATIN1")
}
