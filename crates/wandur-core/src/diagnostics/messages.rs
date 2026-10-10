//! The Diagnostics Messages tab without its drawing (the C# `ProtocolDiagnosticsViewModel`):
//! a bounded, session-local history of received GMCP, MSDP and MSSP messages, the kinds seen
//! (a chip each, with a count) and the filter over them, the Observed fields inventory, and the
//! Server details text from MSSP. Payloads stay in memory; nothing is written to disk.

use std::collections::VecDeque;
use std::time::SystemTime;

use crate::l10n::{S, t, tf};
use crate::protocol::format::{self, Content, Scrubber, protocol_name};
use crate::protocol::schema::SchemaInventory;
use crate::protocol::{MsspTable, OPT_GMCP, OPT_MSDP};

pub const MAX_ENTRIES: usize = 200;
pub const MAX_CHARS: usize = 1_048_576;
/// Chips shown before "n more" takes over (a large MSDP world reports about a hundred).
pub const VISIBLE_KIND_CAP: usize = 24;

/// One received message.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Stable for the session (entries are evicted from the front).
    pub id: u64,
    pub received_at: SystemTime,
    /// GMCP, MSDP, MSSP or another option's name.
    pub protocol: String,
    /// `None`: hidden during private input or login.
    pub content: Option<Content>,
    /// The kinds it belongs to: the GMCP package, every MSDP variable, or the option's name.
    pub kinds: Vec<String>,
}

impl Entry {
    /// The heading in the list: the package or variables, else the protocol.
    pub fn title(&self) -> &str {
        self.content
            .as_ref()
            .map_or(self.protocol.as_str(), |c| c.name.as_str())
    }

    fn chars(&self) -> usize {
        self.content.as_ref().map_or(0, |c| c.body.encode_utf16().count())
    }
}

/// One kind seen this session: a chip with its count, on or off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kind {
    pub protocol: String,
    pub name: String,
    pub count: usize,
    pub active: bool,
}

impl Kind {
    /// `Char.Vitals (2)`.
    pub fn label(&self) -> String {
        format!("{} ({})", self.name, self.count)
    }
}

#[derive(Debug)]
pub struct Messages {
    entries: VecDeque<Entry>,
    /// Ids of the entries that pass the chips and the filter, in order.
    visible: Vec<u64>,
    kinds: Vec<Kind>,
    selected: Option<u64>,
    follow: bool,
    filter: String,
    kinds_expanded: bool,
    chars: usize,
    next_id: u64,
    schema: SchemaInventory,
    server: Option<MsspTable>,
    server_details: Option<String>,
    revision: u64,
}

impl Default for Messages {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            visible: Vec::new(),
            kinds: Vec::new(),
            selected: None,
            follow: true,
            filter: String::new(),
            kinds_expanded: false,
            chars: 0,
            next_id: 0,
            schema: SchemaInventory::default(),
            server: None,
            server_details: None,
            revision: 0,
        }
    }
}

/// The kinds a message belongs to (the C# `DeriveKinds`).
pub fn derive_kinds(option: u8, protocol: &str, content: Option<&Content>) -> Vec<String> {
    let Some(content) = content else {
        return Vec::new();
    };
    if option == OPT_GMCP {
        return if content.name.is_empty() {
            Vec::new()
        } else {
            vec![content.name.clone()]
        };
    }
    if option != OPT_MSDP {
        return vec![protocol.to_string()];
    }
    if content.malformed || content.body.is_empty() {
        return vec![content.name.clone()];
    }
    if let Some(serde_json::Value::Object(map)) = format::parse_json(&content.body) {
        let names: Vec<String> = map.keys().filter(|k| !k.is_empty()).cloned().collect();
        if !names.is_empty() {
            return names;
        }
    }
    content
        .name
        .split(", ")
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(String::from)
        .collect()
}

fn contains_ignore_case(hay: &str, needle: &str) -> bool {
    hay.to_lowercase().contains(&needle.to_lowercase())
}

impl Messages {
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entry(&self, id: u64) -> Option<&Entry> {
        let first = self.entries.front()?.id;
        self.entries.get(id.checked_sub(first)? as usize)
    }

    /// The entries the list shows, in order.
    pub fn visible(&self) -> impl Iterator<Item = &Entry> {
        self.visible.iter().filter_map(|id| self.entry(*id))
    }

    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    pub fn visible_ids(&self) -> &[u64] {
        &self.visible
    }

    /// Every kind seen since the last clear, by protocol then name.
    pub fn kinds(&self) -> &[Kind] {
        &self.kinds
    }

    /// The kinds the chip row shows: all when expanded, else the first [`VISIBLE_KIND_CAP`].
    pub fn chip_kinds(&self) -> &[Kind] {
        if self.kinds_expanded {
            &self.kinds
        } else {
            &self.kinds[..self.kinds.len().min(VISIBLE_KIND_CAP)]
        }
    }

    pub fn has_more_kinds(&self) -> bool {
        self.kinds.len() > VISIBLE_KIND_CAP
    }

    pub fn hidden_kind_count(&self) -> usize {
        self.kinds.len().saturating_sub(VISIBLE_KIND_CAP)
    }

    pub fn more_kinds_label(&self) -> String {
        tf(S::DiagnosticsMoreKinds, &[&self.hidden_kind_count()])
    }

    pub fn kinds_expanded(&self) -> bool {
        self.kinds_expanded
    }

    pub fn set_kinds_expanded(&mut self, on: bool) {
        self.kinds_expanded = on;
        self.revision += 1;
    }

    pub fn has_active_kinds(&self) -> bool {
        self.kinds.iter().any(|k| k.active)
    }

    pub fn is_filtering(&self) -> bool {
        self.has_active_kinds() || !self.filter.is_empty()
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn follow(&self) -> bool {
        self.follow
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.selected.and_then(|id| self.entry(id))
    }

    pub fn selected_id(&self) -> Option<u64> {
        self.selected
    }

    /// Bumped on every change, so a view can skip work.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// "14 messages · latest 200 retained", or "1 of 5" while filtering.
    pub fn count_label(&self) -> String {
        if self.is_filtering() {
            tf(S::DiagnosticsShownOfTotal, &[&self.visible.len(), &self.entries.len()])
        } else {
            tf(S::DiagnosticsCount, &[&self.entries.len(), &MAX_ENTRIES])
        }
    }

    /// What the detail pane shows for the selection.
    pub fn detail(&self) -> String {
        let Some(entry) = self.selected() else {
            return t(S::DiagnosticsSelectMessage).into();
        };
        let Some(content) = &entry.content else {
            return t(S::DiagnosticsPrivate).into();
        };
        let mut parts: Vec<&str> = Vec::new();
        if content.redacted {
            parts.push(t(S::DiagnosticsRedacted));
        }
        if content.malformed && !content.redacted {
            parts.push(t(S::DiagnosticsMalformed));
        }
        if content.truncated {
            parts.push(t(S::DiagnosticsTruncated));
        }
        if !content.body.is_empty() {
            parts.push(&content.body);
        }
        parts.join("\n\n")
    }

    /// The Observed fields JSON.
    pub fn schema_detail(&mut self) -> String {
        self.schema.snapshot()
    }

    pub fn schema(&self) -> &SchemaInventory {
        &self.schema
    }

    /// Format and keep a payload (`None`: hidden). Used by tests and anything without secrets.
    pub fn append(&mut self, at: SystemTime, option: u8, payload: Option<&[u8]>) {
        let content = payload.map(|p| format::format(option, p, false, &[]));
        self.append_content(at, option, content);
    }

    /// Keep a message that is already formatted (and redacted).
    pub fn append_content(&mut self, at: SystemTime, option: u8, content: Option<Content>) {
        let protocol = protocol_name(option);
        let kinds = derive_kinds(option, &protocol, content.as_ref());
        if let Some(observed) = &content {
            self.schema.observe(option, observed);
        }
        let id = self.next_id;
        self.next_id += 1;
        let entry = Entry {
            id,
            received_at: at,
            protocol: protocol.clone(),
            content,
            kinds,
        };
        self.chars += entry.chars();
        for kind in &entry.kinds {
            self.count(&protocol, kind);
        }
        let matches = self.matches(&entry);
        self.entries.push_back(entry);
        while self.entries.len() > MAX_ENTRIES || self.chars > MAX_CHARS {
            let Some(removed) = self.entries.pop_front() else { break };
            self.chars -= removed.chars();
            if self.visible.first() == Some(&removed.id) {
                self.visible.remove(0);
            }
            if self.selected == Some(removed.id) {
                self.selected = None;
            }
        }
        if matches && self.entry(id).is_some() {
            self.visible.push(id);
            if self.follow {
                self.selected = Some(id);
            }
        }
        self.revision += 1;
    }

    fn count(&mut self, protocol: &str, name: &str) {
        let mut index = 0;
        while index < self.kinds.len() {
            let existing = &self.kinds[index];
            let order = existing
                .protocol
                .as_str()
                .cmp(protocol)
                .then_with(|| existing.name.to_lowercase().cmp(&name.to_lowercase()));
            match order {
                std::cmp::Ordering::Equal => {
                    self.kinds[index].count += 1;
                    return;
                }
                std::cmp::Ordering::Greater => break,
                std::cmp::Ordering::Less => index += 1,
            }
        }
        self.kinds.insert(
            index,
            Kind {
                protocol: protocol.into(),
                name: name.into(),
                count: 1,
                active: false,
            },
        );
    }

    fn matches(&self, entry: &Entry) -> bool {
        if self.has_active_kinds()
            && !entry
                .kinds
                .iter()
                .any(|name| self.kinds.iter().any(|k| k.active && k.name == *name))
        {
            return false;
        }
        if self.filter.is_empty() {
            return true;
        }
        entry.kinds.iter().any(|n| contains_ignore_case(n, &self.filter))
            || entry.content.as_ref().is_some_and(|c| {
                contains_ignore_case(&c.name, &self.filter) || contains_ignore_case(&c.body, &self.filter)
            })
    }

    /// One pass over the retained entries; the selection stays while it still shows, and Follow
    /// moves it to the last shown.
    fn refilter(&mut self) {
        let previous = self.selected;
        self.visible = self.entries.iter().filter(|e| self.matches(e)).map(|e| e.id).collect();
        self.selected = if self.follow {
            self.visible.last().copied()
        } else {
            previous.filter(|id| self.visible.contains(id))
        };
        self.revision += 1;
    }

    /// Clear: the history, the kinds and the inventory (the filter text stays).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.visible.clear();
        self.chars = 0;
        self.selected = None;
        self.schema.clear();
        self.kinds.clear();
        self.kinds_expanded = false;
        self.revision += 1;
    }

    /// The All chip: every kind off.
    pub fn clear_kinds(&mut self) {
        for kind in &mut self.kinds {
            kind.active = false;
        }
        self.refilter();
    }

    /// Escape in the filter box.
    pub fn clear_filter(&mut self) {
        self.set_filter("");
    }

    pub fn set_filter(&mut self, text: &str) {
        if self.filter != text {
            self.filter = text.to_string();
            self.refilter();
        }
    }

    /// Turn a kind's chip on or off; nothing for a kind not seen yet.
    pub fn toggle_kind(&mut self, name: &str) {
        if let Some(kind) = self.kinds.iter_mut().find(|k| k.name == name) {
            kind.active = !kind.active;
            self.refilter();
        }
    }

    pub fn set_follow(&mut self, on: bool) {
        self.follow = on;
        if on {
            self.selected = self.visible.last().copied();
        }
        self.revision += 1;
    }

    /// Select an entry (`None` clears the selection).
    pub fn select(&mut self, id: Option<u64>) {
        self.selected = id;
        self.revision += 1;
    }

    /// The person clicked an entry: anything but the newest one pauses following.
    pub fn select_by_person(&mut self, id: u64) {
        if self.visible.last() != Some(&id) {
            self.follow = false;
        }
        self.select(Some(id));
    }

    /// The server's MSSP table for this session, if it sent one.
    pub fn server(&self) -> Option<&MsspTable> {
        self.server.as_ref()
    }

    pub fn has_server_details(&self) -> bool {
        self.server_details.is_some()
    }

    /// The Server details text, or the note that the world has not described itself.
    pub fn server_details(&self) -> &str {
        self.server_details
            .as_deref()
            .unwrap_or_else(|| t(S::DiagnosticsServerDetailsNone))
    }

    pub fn set_server_details(&mut self, table: MsspTable, details: String) {
        self.server = Some(table);
        self.server_details = Some(details);
        self.revision += 1;
    }

    /// A new session starts with no details; Clear leaves them.
    pub fn reset_server_details(&mut self) {
        self.server = None;
        self.server_details = None;
        self.revision += 1;
    }
}

static ESCAPES: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new("\u{1b}\\[[0-9;?]*[ -/]*[@-~]|\u{1b}\\][^\u{7}\u{1b}]*(?:\u{7}|\u{1b}\\\\)?|\u{1b}.?")
        .expect("the escape pattern compiles")
});

fn plain(text: &str) -> String {
    ESCAPES
        .replace_all(text, "")
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

/// An MSSP table as a Diagnostics entry and as the Server details text (`NAME: value` lines,
/// several values joined by commas). Remembered secrets are masked in every name and value
/// first; the details drop escape sequences and control characters; both are capped.
pub fn format_server_details(table: &MsspTable, secrets: &[String]) -> (Content, String) {
    let mut scrubber = Scrubber::new(secrets);
    let scrubbed: Vec<(String, Vec<String>)> = table
        .entries
        .iter()
        .map(|(name, values)| (scrubber.scrub(name), values.iter().map(|v| scrubber.scrub(v)).collect()))
        .collect();
    let mut json = serde_json::Map::new();
    for (name, values) in &scrubbed {
        if !json.contains_key(name) {
            let value = if values.len() == 1 {
                serde_json::Value::String(values[0].clone())
            } else {
                serde_json::json!(values)
            };
            json.insert(name.clone(), value);
        }
    }
    let mut body = format::pretty(&serde_json::Value::Object(json));
    let truncated = body.chars().count() > format::MAX_BODY_CHARS;
    if truncated {
        body = body.chars().take(format::MAX_BODY_CHARS).collect();
    }
    let mut details = scrubbed
        .iter()
        .map(|(name, values)| {
            format!(
                "{}: {}",
                plain(name),
                values.iter().map(|v| plain(v)).collect::<Vec<_>>().join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if details.chars().count() > format::MAX_BODY_CHARS {
        details = details.chars().take(format::MAX_BODY_CHARS).collect();
    }
    let mut content = Content::new("MSSP", &body, false, truncated);
    content.redacted = scrubber.redacted;
    (content, details)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::OPT_MSSP;

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_789_000_000)
    }

    fn msdp(fields: &[(&str, &str)]) -> Vec<u8> {
        fields
            .iter()
            .flat_map(|(n, v)| format!("\x01{n}\x02{v}").into_bytes())
            .collect()
    }

    const MSSP: &[u8] = b"\x01NAME\x02Fixture World\x01PLAYERS\x023";

    fn seeded() -> Messages {
        let mut m = Messages::default();
        m.append(now(), OPT_GMCP, Some(br#"Char.Vitals {"hp":42,"maxhp":100}"#));
        m.append(
            now(),
            OPT_MSDP,
            Some(&msdp(&[("OPPONENTHEALTH", "80"), ("HEALTH", "42")])),
        );
        m.append(now(), OPT_MSSP, Some(MSSP));
        m.append(now(), OPT_GMCP, Some(br#"Char.Vitals {"hp":40,"maxhp":100}"#));
        m.append(now(), OPT_GMCP, Some(br#"Room.Info {"name":"The Cockpit"}"#));
        m
    }

    fn ids(m: &Messages) -> Vec<u64> {
        m.visible_ids().to_vec()
    }

    #[test]
    fn kinds_come_from_the_gmcp_package_every_msdp_variable_and_the_option_name_with_counts() {
        let mut m = seeded();
        let names: Vec<&str> = m.kinds().iter().map(|k| k.name.as_str()).collect();
        assert_eq!(names, ["Char.Vitals", "Room.Info", "HEALTH", "OPPONENTHEALTH", "MSSP"]);
        let protocols: Vec<&str> = m.kinds().iter().map(|k| k.protocol.as_str()).collect();
        assert_eq!(protocols, ["GMCP", "GMCP", "MSDP", "MSDP", "MSSP"]);
        let counts: Vec<usize> = m.kinds().iter().map(|k| k.count).collect();
        assert_eq!(counts, [2, 1, 1, 1, 1]);
        assert_eq!(m.kinds()[0].label(), "Char.Vitals (2)");
        assert_eq!(m.entry(1).unwrap().kinds, ["OPPONENTHEALTH", "HEALTH"]);
        assert_eq!(m.entry(2).unwrap().protocol, "MSSP");
        assert_eq!(m.entry(2).unwrap().kinds, ["MSSP"]);
        m.append(now(), OPT_GMCP, None);
        assert!(m.entries().last().unwrap().kinds.is_empty());
        assert_eq!(m.kinds().len(), 5);
        assert_eq!(m.visible_len(), m.len());
        assert!(!m.is_filtering());
    }

    #[test]
    fn msdp_kinds_survive_a_redacted_variable_and_a_malformed_payload() {
        let mut m = Messages::default();
        m.append(
            now(),
            OPT_MSDP,
            Some(&msdp(&[("HEALTH", "42"), ("PASSWORD", "hidden-secret")])),
        );
        assert_eq!(m.entry(0).unwrap().kinds, ["HEALTH", "PASSWORD"]);
        assert!(
            !m.entry(0)
                .unwrap()
                .content
                .as_ref()
                .unwrap()
                .body
                .contains("hidden-secret")
        );
        m.append(now(), OPT_MSDP, Some(b"not-msdp-data"));
        assert_eq!(m.entry(1).unwrap().kinds, ["MSDP"]);
        let thirty: Vec<(String, &str)> = (0..30).map(|i| (format!("VARIABLE_{i:02}"), "1")).collect();
        let thirty: Vec<(&str, &str)> = thirty.iter().map(|(n, v)| (n.as_str(), *v)).collect();
        m.append(now(), OPT_MSDP, Some(&msdp(&thirty)));
        assert_eq!(m.entry(2).unwrap().kinds.len(), 30);
        assert_eq!(m.kinds().len(), 33);
        assert!(m.has_more_kinds());
        assert_eq!(m.hidden_kind_count(), 9);
        assert_eq!(m.more_kinds_label(), "9 more");
        assert_eq!(m.chip_kinds().len(), VISIBLE_KIND_CAP);
        m.set_kinds_expanded(true);
        assert_eq!(m.chip_kinds().len(), 33);
    }

    #[test]
    fn toggling_chips_filters_the_visible_list_and_several_can_be_active() {
        let mut m = seeded();
        m.toggle_kind("OPPONENTHEALTH");
        assert!(m.is_filtering());
        assert_eq!(ids(&m), [1]);
        assert_eq!(m.count_label(), "1 of 5");
        m.toggle_kind("Char.Vitals");
        assert_eq!(ids(&m), [0, 1, 3]);
        assert_eq!(m.count_label(), "3 of 5");
        m.toggle_kind("OPPONENTHEALTH");
        assert_eq!(ids(&m), [0, 3]);
        assert_eq!(m.len(), 5);
        m.clear_kinds();
        assert_eq!(ids(&m), [0, 1, 2, 3, 4]);
        assert!(!m.is_filtering());
        assert!(m.count_label().contains("5 messages"));
    }

    #[test]
    fn text_filters_on_kind_names_and_bodies_case_insensitively_and_combines_with_chips() {
        let mut m = seeded();
        m.set_filter("vitals");
        assert_eq!(ids(&m), [0, 3]);
        m.set_filter("cockpit");
        assert_eq!(ids(&m), [4]);
        m.set_filter("health");
        assert_eq!(ids(&m), [1]);
        m.set_filter("42");
        assert_eq!(ids(&m), [0, 1]);
        m.toggle_kind("HEALTH");
        assert_eq!(ids(&m), [1]);
        assert_eq!(m.count_label(), "1 of 5");
        m.set_filter("no-such-text");
        assert!(ids(&m).is_empty());
        assert!(!m.is_empty());
        m.clear_filter();
        assert_eq!(m.filter(), "");
        assert_eq!(ids(&m), [1]);
    }

    #[test]
    fn follow_latest_tracks_the_filtered_list_and_new_entries_join_it_when_they_match() {
        let mut m = seeded();
        m.toggle_kind("Char.Vitals");
        assert!(m.follow());
        assert_eq!(m.selected_id(), Some(3));
        m.append(now(), OPT_MSDP, Some(&msdp(&[("HEALTH", "41")])));
        assert_eq!(m.selected_id(), Some(3));
        assert_eq!(m.visible_len(), 2);
        m.append(now(), OPT_GMCP, Some(br#"Char.Vitals {"hp":39}"#));
        assert_eq!(m.visible_len(), 3);
        assert_eq!(m.selected_id(), Some(6));
        assert_eq!(m.kinds().iter().find(|k| k.name == "Char.Vitals").unwrap().count, 3);
        m.set_filter("39");
        assert_eq!(ids(&m), [6]);
        assert_eq!(m.selected_id(), Some(6));
    }

    #[test]
    fn a_selected_entry_stays_selected_while_it_matches_and_clears_when_it_does_not() {
        let mut m = seeded();
        m.set_follow(false);
        m.select(Some(1));
        m.toggle_kind("HEALTH");
        assert_eq!(m.selected_id(), Some(1));
        m.set_filter("cockpit");
        assert_eq!(m.selected_id(), None);
        m.set_filter("");
        assert_eq!(m.selected_id(), None);
        assert!(!m.follow());
        m.select(Some(1));
        m.set_follow(true);
        assert_eq!(m.selected_id(), m.visible_ids().last().copied());
        // A click on the newest entry keeps following; on an older one it pauses.
        m.clear_kinds();
        m.select_by_person(4);
        assert!(m.follow());
        m.select_by_person(1);
        assert!(!m.follow());
    }

    #[test]
    fn clearing_the_history_resets_kinds_selection_and_text_and_eviction_keeps_the_view_in_step() {
        let mut m = seeded();
        m.toggle_kind("Char.Vitals");
        m.set_filter("hp");
        m.set_kinds_expanded(true);
        m.clear();
        assert!(m.kinds().is_empty() && m.chip_kinds().is_empty() && m.visible_len() == 0);
        assert!(!m.has_active_kinds() && !m.kinds_expanded());
        assert_eq!(m.filter(), "hp");
        m.append(now(), OPT_GMCP, Some(br#"Char.Vitals {"hp":1}"#));
        assert_eq!(m.visible_len(), 1);
        assert!(!m.kinds()[0].active);
        m.set_filter("");
        for i in 0..MAX_ENTRIES + 50 {
            let name = if i % 2 == 0 { "HEALTH" } else { "MANA" };
            m.append(now(), OPT_MSDP, Some(&msdp(&[(name, &i.to_string())])));
        }
        m.toggle_kind("HEALTH");
        assert_eq!(m.len(), MAX_ENTRIES);
        let health: Vec<u64> = m
            .entries()
            .filter(|e| e.kinds.iter().any(|k| k == "HEALTH"))
            .map(|e| e.id)
            .collect();
        assert_eq!(ids(&m), health);
        m.append(now(), OPT_MSDP, Some(&msdp(&[("HEALTH", "x")])));
        let health: Vec<u64> = m
            .entries()
            .filter(|e| e.kinds.iter().any(|k| k == "HEALTH"))
            .map(|e| e.id)
            .collect();
        assert_eq!(ids(&m), health);
    }

    #[test]
    fn filtering_two_thousand_entries_over_a_hundred_kinds_is_quick() {
        let mut m = Messages::default();
        let started = std::time::Instant::now();
        for i in 0..2000 {
            let a = format!("VARIABLE_{:03}", i % 100);
            let b = format!("OTHER_{:03}", (i * 7) % 100);
            m.append(now(), OPT_MSDP, Some(&msdp(&[(&a, &i.to_string()), (&b, "value")])));
        }
        assert_eq!(m.kinds().len(), 200);
        for i in 0..20 {
            m.set_filter(&format!("VARIABLE_0{}", i % 10));
            m.toggle_kind(&format!("OTHER_{i:03}"));
        }
        m.clear_kinds();
        m.set_filter("");
        assert_eq!(m.visible_len(), MAX_ENTRIES);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn the_detail_pane_explains_hidden_redacted_and_malformed_messages() {
        let mut m = Messages::default();
        assert_eq!(m.detail(), t(S::DiagnosticsSelectMessage));
        m.append(now(), OPT_GMCP, None);
        assert_eq!(m.detail(), t(S::DiagnosticsPrivate));
        m.append(now(), OPT_GMCP, Some(br#"Char.Vitals {"hp":42}"#));
        assert_eq!(m.detail(), "{\n  \"hp\": 42\n}");
        m.append(now(), OPT_GMCP, Some(b"Room.Info {broken"));
        assert!(m.detail().starts_with(t(S::DiagnosticsMalformed)));
        m.append(now(), OPT_GMCP, Some(br#"Auth.Info {"password":"x"}"#));
        assert!(m.detail().starts_with(t(S::DiagnosticsRedacted)));
    }

    fn table(pairs: &[(&str, &str)]) -> MsspTable {
        let payload: Vec<u8> = pairs
            .iter()
            .flat_map(|(n, v)| [&[1u8][..], n.as_bytes(), &[2u8], v.as_bytes()].concat())
            .collect();
        crate::protocol::parse_mssp(&payload)
    }

    #[test]
    fn server_details_list_every_variable_and_mask_remembered_secrets() {
        let t = table(&[
            ("NAME", "Fixture World"),
            ("CODEBASE", "SmaugFUSS 1.9"),
            ("PLAYERS", "12"),
        ]);
        let (content, details) = format_server_details(&t, &[]);
        assert_eq!(details, "NAME: Fixture World\nCODEBASE: SmaugFUSS 1.9\nPLAYERS: 12");
        assert!(content.body.contains("\"CODEBASE\": \"SmaugFUSS 1.9\""));
        let t = table(&[
            ("NAME", "Fixture p&ss world"),
            ("CAF\u{c9}", "caf\u{e9}-secret"),
            ("p&ss", "value"),
        ]);
        let (content, details) = format_server_details(&t, &["p&ss".to_string(), "caf\u{e9}-secret".to_string()]);
        assert!(content.redacted);
        for text in [&content.body, &details] {
            assert!(!text.contains("p&ss") && !text.contains("caf\u{e9}-secret"));
            assert!(text.contains(format::REDACTED));
        }
        let mut m = Messages::default();
        assert!(!m.has_server_details());
        assert_eq!(m.server_details(), crate::l10n::t(S::DiagnosticsServerDetailsNone));
        m.set_server_details(t, details);
        assert!(m.has_server_details());
        m.clear();
        assert!(m.has_server_details());
        m.reset_server_details();
        assert!(!m.has_server_details());
    }

    #[test]
    fn details_drop_ansi_and_control_characters_and_the_body_is_capped() {
        let t = table(&[
            ("NAME", "\u{1b}[1;31mRed\u{1b}[0m World\u{7}"),
            ("CODEBASE", "Smaug\r\nFUSS"),
            ("DESC", &"x".repeat(40_000)),
        ]);
        let (content, details) = format_server_details(&t, &[]);
        assert!(details.starts_with("NAME: Red World\nCODEBASE: SmaugFUSS\nDESC: x"));
        assert!(!details.contains('\u{1b}'));
        assert!(content.truncated);
        assert!(content.body.chars().count() <= format::MAX_BODY_CHARS);
        assert!(details.chars().count() <= format::MAX_BODY_CHARS);
    }
}
