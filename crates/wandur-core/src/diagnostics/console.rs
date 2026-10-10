//! The Diagnostics console (the C# `ConsoleLog`): a bounded, session-local ring of the text
//! stream the transcript received, in arrival order, plus the commands sent back. Nothing is
//! written to disk. Text received or sent while input was private is replaced by one `[private]`
//! marker per private stretch, and remembered secrets are masked as protocol diagnostics mask
//! them.

use std::collections::VecDeque;
use std::time::SystemTime;

use crate::protocol::format::Scrubber;

pub const MAX_ENTRIES: usize = 2000;
/// Characters kept (UTF-16 units, as the C# limit counts them).
pub const MAX_CHARS: usize = 524_288;
pub const PRIVATE_MARKER: &str = "[private]";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleKind {
    Received,
    Sent,
    Script,
    Private,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConsoleEntry {
    pub sequence: u64,
    pub at: SystemTime,
    pub kind: ConsoleKind,
    pub text: String,
    /// `text` in UTF-16 units.
    chars: usize,
}

impl ConsoleEntry {
    pub fn new(sequence: u64, at: SystemTime, kind: ConsoleKind, text: &str) -> Self {
        Self {
            sequence,
            at,
            kind,
            text: text.to_string(),
            chars: text.encode_utf16().count(),
        }
    }

    pub fn marker(&self) -> &'static str {
        match self.kind {
            ConsoleKind::Received => "<<",
            ConsoleKind::Sent => ">>",
            ConsoleKind::Script => "[script]",
            ConsoleKind::Private => PRIVATE_MARKER,
        }
    }

    /// The entry as display lines: local time (`HH:mm:ss.fff`, from `utc_offset` seconds), the
    /// direction marker, then the text with control characters made visible and continuation
    /// lines indented under it.
    pub fn render(&self, utc_offset: i64) -> String {
        let prefix = format!("{} {}", clock(self.at, utc_offset), self.marker());
        if self.kind == ConsoleKind::Private {
            prefix
        } else {
            let indent = prefix.chars().count() + 1;
            format!("{prefix} {}", visible(&self.text, indent))
        }
    }
}

/// `HH:mm:ss.fff` of a time at a UTC offset.
pub fn clock(at: SystemTime, utc_offset: i64) -> String {
    let since = at.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let secs = (since.as_secs() as i64 + utc_offset).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60,
        since.subsec_millis()
    )
}

/// Control characters made visible: ESC as ␛, CR as ␍, LF as ␊ and a real line break (the next
/// line indented by `indent`), DEL as ^?, other C0 controls as ^X. A chunk that ends with a line
/// feed leaves no empty continuation line.
pub fn visible(text: &str, indent: usize) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        match c {
            '\u{1b}' => out.push('␛'),
            '\r' => out.push('␍'),
            '\n' => {
                out.push('␊');
                out.push('\n');
                out.extend(std::iter::repeat_n(' ', indent));
            }
            '\u{7f}' => out.push_str("^?"),
            c if (c as u32) < 0x20 => {
                out.push('^');
                out.push(char::from(c as u8 + 64));
            }
            c => out.push(c),
        }
    }
    if text.ends_with('\n') {
        out.truncate(out.len() - indent - 1);
    }
    out
}

/// Remembered secrets replaced by `[redacted]` (the protocol diagnostics' rule).
pub fn mask(text: &str, secrets: &[String]) -> String {
    let mut scrubber = Scrubber::new(secrets);
    if !scrubber.finds(text) {
        return text.to_string();
    }
    scrubber.scrub(text)
}

#[derive(Debug, Default)]
pub struct ConsoleLog {
    entries: VecDeque<ConsoleEntry>,
    sequence: u64,
    chars: usize,
    /// Bumped on every change (append or clear), so a view can tell it is stale.
    revision: u64,
}

impl ConsoleLog {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Characters held (UTF-16 units).
    pub fn chars(&self) -> usize {
        self.chars
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn entries(&self) -> impl Iterator<Item = &ConsoleEntry> {
        self.entries.iter()
    }

    pub fn append(&mut self, kind: ConsoleKind, text: &str, hidden: bool, secrets: &[String]) {
        self.append_at(kind, text, hidden, secrets, SystemTime::now());
    }

    pub fn append_at(&mut self, kind: ConsoleKind, text: &str, hidden: bool, secrets: &[String], at: SystemTime) {
        if !hidden && text.is_empty() {
            return;
        }
        let entry = if hidden {
            // A private stretch reads as one marker, however many chunks it had.
            if self.entries.back().is_some_and(|e| e.kind == ConsoleKind::Private) {
                return;
            }
            self.sequence += 1;
            ConsoleEntry::new(self.sequence, at, ConsoleKind::Private, "")
        } else {
            self.sequence += 1;
            let text = if secrets.is_empty() {
                text.to_string()
            } else {
                mask(text, secrets)
            };
            ConsoleEntry::new(self.sequence, at, kind, &text)
        };
        self.chars += entry.chars;
        self.entries.push_back(entry);
        while self.entries.len() > 1 && (self.entries.len() > MAX_ENTRIES || self.chars > MAX_CHARS) {
            if let Some(old) = self.entries.pop_front() {
                self.chars -= old.chars;
            }
        }
        self.revision += 1;
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.chars = 0;
        self.revision += 1;
    }

    /// Every entry rendered, one after another.
    pub fn render(&self, utc_offset: i64) -> String {
        self.entries
            .iter()
            .map(|e| e.render(utc_offset))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_the_latest_two_thousand_entries() {
        let mut log = ConsoleLog::default();
        for i in 1..=2100 {
            log.append(ConsoleKind::Received, &format!("line {i}\r\n"), false, &[]);
        }
        assert_eq!(log.len(), MAX_ENTRIES);
        let first = log.entries().next().unwrap();
        assert_eq!(first.sequence, 101);
        assert_eq!(first.text, "line 101\r\n");
        assert_eq!(log.entries().last().unwrap().sequence, 2100);
    }

    #[test]
    fn ring_keeps_at_most_half_a_mebibyte_of_text_but_never_drops_the_newest_entry() {
        let mut log = ConsoleLog::default();
        for i in 0..6u8 {
            log.append(
                ConsoleKind::Received,
                &char::from(b'a' + i).to_string().repeat(100_000),
                false,
                &[],
            );
        }
        assert_eq!(log.len(), 5);
        assert!(log.chars() <= MAX_CHARS);
        assert!(log.entries().next().unwrap().text.starts_with('b'));
        log.append(ConsoleKind::Received, &"z".repeat(MAX_CHARS + 1), false, &[]);
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn control_characters_are_made_visible() {
        assert_eq!(visible("\u{1b}[32mgreen\u{1b}[0m\r\n", 0), "␛[32mgreen␛[0m␍␊");
        assert_eq!(visible("a\nb", 4), "a␊\n    b");
        assert_eq!(visible("\u{7}\t\u{7f}\0", 0), "^G^I^?^@");
        assert_eq!(visible("plain text é", 0), "plain text é");
    }

    #[test]
    fn render_shows_timestamp_marker_and_aligned_continuation_lines() {
        let at = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_789_000_000_123);
        let sent = ConsoleEntry::new(1, at, ConsoleKind::Sent, "look").render(0);
        assert_eq!(sent.len(), "HH:mm:ss.fff >> look".len());
        assert!(sent.ends_with(".123 >> look"), "{sent}");
        let received = ConsoleEntry::new(2, at, ConsoleKind::Received, "one\r\ntwo\r\n").render(0);
        let lines: Vec<&str> = received.split('\n').collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(" << one␍␊"));
        assert_eq!(lines[1], format!("{}two␍␊", " ".repeat("HH:mm:ss.fff << ".len())));
        assert!(
            ConsoleEntry::new(3, at, ConsoleKind::Script, "hello")
                .render(0)
                .ends_with(" [script] hello")
        );
        assert!(
            ConsoleEntry::new(4, at, ConsoleKind::Private, "")
                .render(0)
                .ends_with(" [private]")
        );
        // The local offset moves the clock.
        assert_eq!(&clock(at, 3600)[2..], &clock(at, 0)[2..]);
        assert_ne!(&clock(at, 3600)[..2], &clock(at, 0)[..2]);
    }

    #[test]
    fn hidden_chunks_collapse_into_one_private_marker() {
        let mut log = ConsoleLog::default();
        for _ in 0..5 {
            log.append(ConsoleKind::Received, "Password: hunter2\r\n", true, &[]);
        }
        log.append(ConsoleKind::Sent, "hunter2", true, &[]);
        assert_eq!(log.len(), 1);
        let only = log.entries().next().unwrap();
        assert_eq!(only.kind, ConsoleKind::Private);
        assert_eq!(only.text, "");
        assert!(only.render(0).ends_with(PRIVATE_MARKER));
        log.append(ConsoleKind::Received, "Welcome back.\r\n", false, &[]);
        log.append(ConsoleKind::Received, "secret again", true, &[]);
        log.append(ConsoleKind::Script, "still secret", true, &[]);
        let kinds: Vec<ConsoleKind> = log.entries().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [ConsoleKind::Private, ConsoleKind::Received, ConsoleKind::Private]
        );
        let all = log.render(0);
        assert!(!all.contains("hunter2") && !all.contains("secret"));
    }

    #[test]
    fn remembered_secrets_are_masked_the_way_protocol_diagnostics_mask_them() {
        let secrets = vec!["hunter2".to_string(), "hunter".to_string()];
        let mut log = ConsoleLog::default();
        log.append(
            ConsoleKind::Received,
            "Your password hunter2 is weak.\r\n",
            false,
            &secrets,
        );
        let protocol = crate::protocol::format::format(201, br#"Char.Info {"note":"hunter2"}"#, false, &secrets);
        assert!(protocol.body.contains(crate::protocol::format::REDACTED));
        assert_eq!(
            log.entries().next().unwrap().text,
            "Your password [redacted] is weak.\r\n"
        );
        assert_eq!(
            mask("say \"hi\" \\ é \u{1b}[1mhunter2", &secrets),
            "say \"hi\" \\ é \u{1b}[1m[redacted]"
        );
        assert_eq!(mask("nothing to hide", &secrets), "nothing to hide");
        assert_eq!(mask("nothing to hide", &[]), "nothing to hide");
    }

    #[test]
    fn clear_empties_the_ring_and_every_change_moves_the_revision() {
        let mut log = ConsoleLog::default();
        log.append(ConsoleKind::Received, "a", false, &[]);
        log.append(ConsoleKind::Received, "", false, &[]);
        assert_eq!(log.revision(), 1);
        log.clear();
        assert_eq!((log.len(), log.chars(), log.render(0).as_str()), (0, 0, ""));
        assert_eq!(log.revision(), 2);
        log.append(ConsoleKind::Received, "b", true, &[]);
        assert_eq!(log.entries().next().unwrap().kind, ConsoleKind::Private);
    }
}
