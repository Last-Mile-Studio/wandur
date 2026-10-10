//! Channel traffic, mirrored into the Channels panel: the transcript keeps every line as the
//! world sent it, and this keeps a copy, per session, as an All tab plus one tab per channel,
//! each bounded at 500 messages (the C# panel's limit), with unread counts and the command that
//! replies on a channel.
//!
//! Two kinds of channel (as C#):
//! - protocol channels: a world that speaks GMCP names its own, `Comm.Channel.Text`
//!   (`{"channel", "talker", "text"}`, IRE and most GMCP worlds) or Aardwolf's `comm.channel`
//!   (`{"chan", "player", "msg"}`); the printed copy that follows within a second is dropped;
//! - text channels: lines the world prints, recognized by [`rules`] (the world's own rules
//!   first, then the shipped [`families`] rule set for its codebase), taught per world from the
//!   transcript with the [`proposer`]'s guess. [`classifier`] runs them over received text.
//!
//! [`SessionChannels`] ties these together for one session.

pub mod classifier;
pub mod families;
pub mod proposer;
pub mod rules;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::Value;

pub use classifier::{ChannelLine, Classifier, Incoming};
pub use rules::{ChannelRule, RuleSet};

use crate::protocol::GmcpMessage;

/// Messages kept per tab.
pub const MAX_MESSAGES: usize = 500;
/// Longest channel name, speaker or text kept (the C# decoder's 2048 for text).
const MAX_TEXT: usize = 2048;

#[derive(Clone, Debug, PartialEq)]
pub struct ChannelMessage {
    /// Increases by one per message in a session; the panel uses it to cache layouts.
    pub seq: u64,
    pub channel: String,
    pub speaker: String,
    /// The text as sent (it may carry ANSI colour codes).
    pub text: String,
    /// The text without the channel tag and speaker the world printed in front of it (they are
    /// shown on their own), colour codes kept. The same as `text` when no such prefix was found.
    pub body: String,
    pub private: bool,
    /// When it arrived, seconds since 1970.
    pub at: u64,
    /// The command that speaks on its channel (`{speaker}` for the person answered).
    pub reply_command: Option<String>,
}

/// Decode a channel message from a GMCP message, if it is one.
pub fn decode(message: &GmcpMessage) -> Option<(String, String, String)> {
    let data = message.data.as_ref()?.as_object()?;
    let field = |names: &[&str]| -> String {
        for (k, v) in data {
            if names.iter().any(|n| k.eq_ignore_ascii_case(n))
                && let Value::String(s) = v
            {
                return s.chars().take(MAX_TEXT).collect();
            }
        }
        String::new()
    };
    let (channel, talker, text) = if message.is("Comm.Channel.Text") {
        (field(&["channel"]), field(&["talker"]), field(&["text"]))
    } else if message.is("comm.channel") {
        (
            field(&["chan", "channel"]),
            field(&["player", "talker"]),
            field(&["msg", "text"]),
        )
    } else {
        return None;
    };
    let channel = channel.trim().to_string();
    (!channel.is_empty() && !text.trim().is_empty()).then_some((channel, talker.trim().to_string(), text))
}

/// Remove ANSI escape sequences (CSI and OSC) and other control characters except tabs.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if !c.is_control() || c == '\t' {
            out.push(c);
        }
    }
    out
}

/// The text a world printed for a channel message, without the prefix that repeats what the panel
/// already shows: an optional channel tag (`[gossip]`, `(ooc)`, `<newbie>`) followed by the speaker
/// and a colon (`Ann: hi`) or one verb (`Ann gossips: 'hi'`, `Bob tells you, 'hi'`). Colour codes
/// before the cut are kept so the rest keeps its colour. Without such a prefix the text is returned
/// as it is (a tag alone is kept: it may be all that names the channel).
pub fn trim_prefix(text: &str, speaker: &str) -> String {
    if speaker.is_empty() {
        return text.to_string();
    }
    // Printable characters with their byte offsets in `text`.
    let mut plain: Vec<(usize, char)> = Vec::with_capacity(text.len());
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < text.len() {
        if bytes[i] == 0x1b {
            i += escape_len(&text[i..]);
            continue;
        }
        let c = text[i..].chars().next().unwrap_or(' ');
        if !c.is_control() || c == '\t' {
            plain.push((i, c));
        }
        i += c.len_utf8();
    }
    let chars: Vec<char> = plain.iter().map(|(_, c)| *c).collect();
    let mut at = 0;
    let skip_spaces = |at: &mut usize| {
        while *at < chars.len() && chars[*at].is_whitespace() {
            *at += 1;
        }
    };
    skip_spaces(&mut at);
    if let Some(close) = chars.get(at).and_then(|c| match c {
        '[' => Some(']'),
        '(' => Some(')'),
        '<' => Some('>'),
        '{' => Some('}'),
        _ => None,
    }) && let Some(end) = chars[at + 1..].iter().take(40).position(|&c| c == close)
    {
        at += end + 2;
        skip_spaces(&mut at);
    }
    let name: Vec<char> = speaker.chars().collect();
    let matches_name = chars.len() >= at + name.len()
        && chars[at..at + name.len()]
            .iter()
            .zip(&name)
            .all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()));
    if !matches_name {
        return text.to_string();
    }
    at += name.len();
    let cut = match chars.get(at) {
        Some(':') | Some(',') => Some(at + 1),
        Some(' ') => {
            // One verb, perhaps "you", then a colon or comma.
            let mut j = at + 1;
            let mut words = 0;
            loop {
                let start = j;
                while j < chars.len() && chars[j].is_alphabetic() {
                    j += 1;
                }
                if j == start || j - start > 14 {
                    break None;
                }
                words += 1;
                match chars.get(j) {
                    Some(':') | Some(',') => break Some(j + 1),
                    Some(' ') if words < 2 => j += 1,
                    _ => break None,
                }
            }
        }
        _ => None,
    };
    let Some(mut cut) = cut else {
        return text.to_string();
    };
    while cut < chars.len() && chars[cut].is_whitespace() {
        cut += 1;
    }
    let from = plain.get(cut).map_or(text.len(), |(b, _)| *b);
    // Keep the escape sequences that came before the cut, then the rest.
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < from {
        if bytes[i] == 0x1b {
            let n = escape_len(&text[i..]);
            out.push_str(&text[i..i + n]);
            i += n;
        } else {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out.push_str(&text[from..]);
    out
}

/// Length in bytes of the escape sequence at the start of `s` (which starts with ESC).
fn escape_len(s: &str) -> usize {
    let b = s.as_bytes();
    match b.get(1) {
        Some(b'[') => b[2..]
            .iter()
            .position(|c| (0x40..=0x7e).contains(c))
            .map_or(s.len(), |p| p + 3),
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
            s.len()
        }
        Some(c) if c.is_ascii() => 2,
        _ => 1,
    }
}

/// Tells, pages and whispers are private conversations.
pub fn is_private(channel: &str) -> bool {
    let c = channel.to_ascii_lowercase();
    ["tell", "page", "whisper"].iter().any(|n| c.contains(n))
}

/// Worlds name the command after the channel: `gossip`, or `tell {speaker}` for a private one.
pub fn default_reply(channel: &str, private: bool) -> Option<String> {
    let name: String = channel
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    if name.is_empty() {
        None
    } else if private {
        Some(format!("{name} {{speaker}}"))
    } else {
        Some(name)
    }
}

#[derive(Debug, Default)]
pub struct ChannelTab {
    /// Empty for All.
    pub channel: String,
    pub private: bool,
    pub is_all: bool,
    pub messages: VecDeque<Arc<ChannelMessage>>,
    pub unread: usize,
    pub reply_command: Option<String>,
    pub last_speaker: Option<String>,
}

impl ChannelTab {
    pub fn title(&self) -> &str {
        if self.is_all {
            crate::l10n::t(crate::l10n::S::ChannelsAll)
        } else {
            &self.channel
        }
    }

    /// The command that says `text` on this channel, with the person being answered filled in.
    pub fn reply(&self, text: &str) -> Option<String> {
        if self.is_all {
            // All shows every channel at once: there is no one channel to answer on.
            return None;
        }
        let mut command = self.reply_command.clone()?;
        if command.to_ascii_lowercase().contains("{speaker}") {
            let speaker = self.last_speaker.as_deref()?;
            command = command.replace("{speaker}", speaker);
        }
        Some(format!("{} {text}", command.trim()))
    }

    fn push(&mut self, message: Arc<ChannelMessage>, continuation: bool) {
        if let Some(reply) = message.reply_command.as_ref().filter(|r| !r.is_empty()) {
            self.reply_command = Some(reply.clone());
        }
        if !message.speaker.is_empty() {
            self.last_speaker = Some(message.speaker.clone());
        }
        if continuation && let Some(last) = self.messages.back_mut() {
            *last = message;
            return;
        }
        self.messages.push_back(message);
        while self.messages.len() > MAX_MESSAGES {
            self.messages.pop_front();
        }
    }
}

/// One session's channels.
#[derive(Debug)]
pub struct ChannelLog {
    tabs: Vec<ChannelTab>,
    selected: usize,
    next_seq: u64,
    /// Changes with every message or selection change.
    pub revision: u64,
}

impl Default for ChannelLog {
    fn default() -> Self {
        Self {
            tabs: vec![ChannelTab {
                is_all: true,
                ..ChannelTab::default()
            }],
            selected: 0,
            next_seq: 0,
            revision: 0,
        }
    }
}

impl ChannelLog {
    pub fn tabs(&self) -> &[ChannelTab] {
        &self.tabs
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn selected_tab(&self) -> &ChannelTab {
        &self.tabs[self.selected]
    }

    pub fn select(&mut self, index: usize) {
        if index < self.tabs.len() && index != self.selected {
            self.selected = index;
            self.tabs[index].unread = 0;
            self.revision += 1;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tabs[0].messages.is_empty()
    }

    /// Unread messages on tabs other than the selected one (for an activity marker).
    pub fn unread_total(&self) -> usize {
        self.tabs.iter().skip(1).map(|t| t.unread).sum()
    }

    /// Add one message (a convenience for tests and benches): the channel's tab is private and
    /// replies by the name rules, the world's printed prefix is trimmed from the shown text.
    pub fn add(&mut self, channel: &str, speaker: &str, text: &str, at: u64) -> Arc<ChannelMessage> {
        let private = is_private(channel);
        self.push(
            Incoming {
                channel: channel.into(),
                speaker: speaker.into(),
                text: text.into(),
                body: trim_prefix(text, speaker),
                raw_line: strip_ansi(text),
                private,
                reply_command: default_reply(channel, private),
                at: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(at),
            },
            false,
        )
    }

    /// Store one message: it lands on its channel's tab (created on first use) and on All. A
    /// continuation replaces the last message of both instead and is not counted as unread.
    pub fn push(&mut self, incoming: Incoming, continuation: bool) -> Arc<ChannelMessage> {
        self.next_seq += 1;
        let at = incoming
            .at
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let channel = incoming.channel;
        let message = Arc::new(ChannelMessage {
            seq: self.next_seq,
            speaker: incoming.speaker,
            text: incoming.text,
            body: incoming.body,
            private: incoming.private,
            at,
            reply_command: incoming.reply_command,
            channel,
        });
        let index = match self
            .tabs
            .iter()
            .position(|t| !t.is_all && t.channel.eq_ignore_ascii_case(&message.channel))
        {
            Some(i) => i,
            None => self.insert(ChannelTab {
                channel: message.channel.clone(),
                private: message.private,
                reply_command: message.reply_command.clone(),
                ..ChannelTab::default()
            }),
        };
        self.tabs[0].push(Arc::clone(&message), continuation);
        self.tabs[index].push(Arc::clone(&message), continuation);
        if !continuation {
            for i in [0, index] {
                if i != self.selected {
                    self.tabs[i].unread += 1;
                }
            }
        }
        self.revision += 1;
        message
    }

    /// Private conversations come first after All: they wait on an answer, a channel does not.
    /// The shown tab stays shown.
    fn insert(&mut self, tab: ChannelTab) -> usize {
        let index = if tab.private {
            self.tabs.iter().take_while(|t| t.is_all || t.private).count()
        } else {
            self.tabs.len()
        };
        self.tabs.insert(index, tab);
        if index <= self.selected && self.selected != 0 {
            self.selected += 1;
        }
        index
    }
}

/// One session's channels: the panel's log, the classifier over received text, and the rules
/// in force (the world's own, then its codebase family's).
pub struct SessionChannels {
    pub log: ChannelLog,
    classifier: Classifier,
    world_rules: Vec<ChannelRule>,
    /// The codebase the saved world names (from the world, else its directory listing).
    codebase: String,
    /// The codebase the server reported over MSSP; picks the family only while `codebase` is
    /// empty, and is never saved.
    server_codebase: Option<String>,
    found: Vec<ChannelLine>,
}

impl Default for SessionChannels {
    fn default() -> Self {
        Self::new(Vec::new(), "")
    }
}

impl std::fmt::Debug for SessionChannels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionChannels")
            .field("rules", &self.classifier.rules().len())
            .field("codebase", &self.codebase())
            .finish_non_exhaustive()
    }
}

impl SessionChannels {
    pub fn new(world_rules: Vec<ChannelRule>, codebase: &str) -> Self {
        let rules = families::for_world(&world_rules, non_empty(codebase));
        Self {
            log: ChannelLog::default(),
            classifier: Classifier::new(rules),
            world_rules,
            codebase: codebase.trim().to_string(),
            server_codebase: None,
            found: Vec::new(),
        }
    }

    /// The rules in force.
    pub fn rules(&self) -> &Arc<RuleSet> {
        self.classifier.rules()
    }

    /// The world's own rules.
    pub fn world_rules(&self) -> &[ChannelRule] {
        &self.world_rules
    }

    /// The codebase that picks the family: the world's, else the one the server reported.
    pub fn codebase(&self) -> Option<&str> {
        non_empty(&self.codebase).or(self.server_codebase.as_deref())
    }

    /// The saved world changed (taught a rule, edited in the world editor): when its rules or
    /// codebase differ, they apply from the next line, keeping the line in flight.
    pub fn configure(&mut self, world_rules: &[ChannelRule], codebase: &str) {
        let codebase = codebase.trim();
        if self.world_rules == world_rules && self.codebase == codebase {
            return;
        }
        self.world_rules = world_rules.to_vec();
        self.codebase = codebase.to_string();
        self.rebuild();
    }

    /// The server reported its codebase (MSSP CODEBASE): while the world names none, the family
    /// follows it for this session. The world itself is not changed.
    pub fn use_server_codebase(&mut self, codebase: &str) {
        let codebase = codebase.trim();
        if codebase.is_empty() || codebase.chars().count() > 200 || self.server_codebase.as_deref() == Some(codebase) {
            return;
        }
        self.server_codebase = Some(codebase.to_string());
        if self.codebase.is_empty() {
            self.rebuild();
        }
    }

    fn rebuild(&mut self) {
        let rules = families::for_world(&self.world_rules, self.codebase());
        self.classifier.set_rules(rules);
    }

    /// Public server text as applied to the transcript: recognized lines are copied to the
    /// panel. Returns how many messages arrived or grew.
    pub fn receive_text(&mut self, text: &str, at: SystemTime) -> usize {
        if text.is_empty() {
            return 0;
        }
        let mut found = std::mem::take(&mut self.found);
        self.classifier.feed(text, at, &mut found);
        let n = found.len();
        for line in found.drain(..) {
            self.log.push(line.message, line.continuation);
        }
        self.found = found;
        n
    }

    /// A privacy interval or a cleared transcript: the line in flight is dropped.
    pub fn reset(&mut self) {
        self.classifier.reset();
    }

    /// A GMCP message: when it is a channel message, it is routed by its own channel and
    /// speaker, its printed copy is expected (and dropped), and `true` is returned.
    pub fn receive_gmcp(&mut self, message: &GmcpMessage, at: SystemTime) -> bool {
        let Some((channel, talker, text)) = decode(message) else {
            return false;
        };
        let plain = strip_ansi(&text);
        self.classifier.expect_printed_copy(&plain, at);
        let rules = Arc::clone(self.classifier.rules());
        // The package's text is the printed line: a matching rule cuts out the prefix the panel
        // shows on its own; without one, the M4 trim of a tag and the speaker does.
        let matched = rules.match_plain(&plain);
        let rule = rules.for_channel(&channel);
        let speaker = if talker.is_empty() {
            matched.as_ref().map(|m| m.speaker.clone()).unwrap_or_default()
        } else {
            talker
        };
        let (shown, body) = match matched.as_ref().filter(|m| !m.text.is_empty()) {
            Some(m) => (m.text.clone(), rules::styled_slice(&text, m.text_range.clone())),
            None => (text.clone(), trim_prefix(&text, &speaker)),
        };
        let private = rule
            .map(|r| r.private)
            .or(matched.as_ref().map(|m| m.private))
            .unwrap_or_else(|| is_private(&channel));
        let reply_command = rule
            .and_then(|r| r.reply_command.clone())
            .or_else(|| matched.as_ref().and_then(|m| m.reply_command.clone()))
            .or_else(|| default_reply(&channel, private));
        self.log.push(
            Incoming {
                channel,
                speaker,
                text: shown,
                body,
                raw_line: plain,
                private,
                reply_command,
                at,
            },
            false,
        );
        true
    }
}

fn non_empty(s: &str) -> Option<&str> {
    let s = s.trim();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod log_tests {
    use super::*;
    use crate::protocol::parse_gmcp;

    #[test]
    fn decodes_both_package_shapes() {
        let m =
            parse_gmcp(br#"Comm.Channel.Text {"channel":"gossip","talker":"Ann","text":"\u001b[33mhi all\u001b[0m"}"#)
                .unwrap();
        let (c, t, x) = decode(&m).unwrap();
        assert_eq!((c.as_str(), t.as_str()), ("gossip", "Ann"));
        assert_eq!(strip_ansi(&x), "hi all");
        let m = parse_gmcp(br#"comm.channel {"chan":"tell","player":"Bob","msg":"psst"}"#).unwrap();
        assert_eq!(decode(&m).unwrap().0, "tell");
        assert!(decode(&parse_gmcp(b"Comm.Channel.Text \"text\"").unwrap()).is_none());
        assert!(decode(&parse_gmcp(br#"Comm.Channel.Text {"channel":"x","text":"  "}"#).unwrap()).is_none());
        assert!(decode(&parse_gmcp(br#"Room.Info {"num":1}"#).unwrap()).is_none());
    }

    #[test]
    fn messages_land_on_their_tab_and_on_all_with_unread_counts() {
        let mut log = ChannelLog::default();
        log.add("gossip", "Ann", "one", 1);
        log.add("Gossip", "Bob", "two", 2);
        log.add("newbie", "", "three", 3);
        assert_eq!(log.tabs().len(), 3);
        assert_eq!(log.tabs()[0].messages.len(), 3);
        assert_eq!(log.tabs()[1].messages.len(), 2, "channel names compare without case");
        assert_eq!(log.tabs()[0].unread, 0, "All is shown");
        assert_eq!(log.tabs()[1].unread, 2);
        log.select(1);
        assert_eq!(log.tabs()[1].unread, 0);
        log.add("gossip", "Cy", "four", 4);
        assert_eq!(log.tabs()[0].unread, 1);
        assert_eq!(log.unread_total(), 1, "newbie has one unread");
        let seqs: Vec<u64> = log.tabs()[0].messages.iter().map(|m| m.seq).collect();
        assert_eq!(seqs, [1, 2, 3, 4]);
    }

    #[test]
    fn private_tabs_go_first_without_moving_the_shown_tab() {
        let mut log = ChannelLog::default();
        log.add("gossip", "Ann", "hi", 1);
        log.select(1);
        log.add("tell", "Bob", "psst", 2);
        assert_eq!(log.tabs()[1].channel, "tell");
        assert_eq!(log.selected_tab().channel, "gossip", "the reader stays on gossip");
        assert_eq!(log.tabs()[1].reply("ok").as_deref(), Some("tell Bob ok"));
        assert_eq!(log.selected_tab().reply("hello").as_deref(), Some("gossip hello"));
        assert_eq!(log.tabs()[0].reply("x"), None, "All has no single channel");
    }

    #[test]
    fn tabs_are_bounded() {
        let mut log = ChannelLog::default();
        for i in 0..(MAX_MESSAGES + 50) {
            log.add(if i % 2 == 0 { "a" } else { "b" }, "", &format!("m{i}"), 0);
        }
        assert_eq!(log.tabs()[0].messages.len(), MAX_MESSAGES);
        assert_eq!(log.tabs()[0].messages.front().unwrap().text, "m50");
        assert_eq!(log.tabs()[1].messages.len(), (MAX_MESSAGES + 50) / 2);
    }

    #[test]
    fn strip_handles_osc_and_controls() {
        assert_eq!(strip_ansi("a\u{1b}]0;title\u{7}b\u{1b}[1;31mc\u{0}d"), "abcd");
    }

    #[test]
    fn the_printed_prefix_is_trimmed_and_colours_kept() {
        assert_eq!(trim_prefix("[gossip] Ann: hello there", "Ann"), "hello there");
        assert_eq!(trim_prefix("Ann gossips: 'hi all'", "Ann"), "'hi all'");
        assert_eq!(trim_prefix("Bob tells you, 'psst'", "bob"), "'psst'");
        assert_eq!(
            trim_prefix("\u{1b}[35m[ooc]\u{1b}[0m Dee: \u{1b}[33mwave\u{1b}[0m", "Dee"),
            "\u{1b}[35m\u{1b}[0m\u{1b}[33mwave\u{1b}[0m"
        );
        // Nothing to trim: the text stays as it was.
        assert_eq!(
            trim_prefix("[gossip] someone else said it", "Ann"),
            "[gossip] someone else said it"
        );
        assert_eq!(
            trim_prefix("Ann is a long sentence here", "Ann"),
            "Ann is a long sentence here"
        );
        assert_eq!(trim_prefix("no speaker", ""), "no speaker");
        let mut log = ChannelLog::default();
        let m = log.add("gossip", "Ann", "[gossip] Ann: hi", 0);
        assert_eq!(m.body, "hi");
        assert_eq!(m.text, "[gossip] Ann: hi", "the original stays for the transcript copy");
    }
}
