//! Recognizes channel traffic in received text (the C# `ChannelClassifier`). It is a mirror,
//! not a filter: the transcript keeps every line, and this only says what to copy into the
//! Channels panel.
//!
//! It reads the server text as applied, colour codes included, cut into lines at LF, rather
//! than the terminal grid's line events: reading lines back from the grid costs about 0.2 ms a
//! frame at 1 MB/s (measured for completion in t05), more than the whole classification. A
//! line is stripped of its escape sequences into a reused buffer and matched against every
//! rule in one automaton pass; only a line that matches pays for captures and the coloured
//! copy of its text.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::rules::{RuleSet, strip, strip_into, styled_slice};

/// A world that sends a structured message and then prints it too must not be shown twice.
const DUPLICATE_WINDOW: Duration = Duration::from_secs(1);
/// A line nobody terminates must not grow without bound (the C# limit, in bytes here).
const MAX_PENDING: usize = 16_384;
/// More messages than this in one call is a flood: the classifier starts over (C#).
const MAX_RESULTS: usize = 128;

/// A recognized channel message, before the panel numbers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incoming {
    pub channel: String,
    pub speaker: String,
    /// The text, plain and trimmed.
    pub text: String,
    /// The text with the world's colour codes.
    pub body: String,
    /// The whole plain line (lines, for a message continued over several).
    pub raw_line: String,
    pub private: bool,
    pub reply_command: Option<String>,
    pub at: SystemTime,
}

/// One received line that was channel traffic. A continuation extends the message before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelLine {
    pub message: Incoming,
    pub continuation: bool,
}

pub struct Classifier {
    rules: Arc<RuleSet>,
    /// The raw text of the line not yet ended.
    pending: String,
    /// The plain text of the line being classified (reused).
    plain: String,
    last: Option<Incoming>,
    /// The printed copy a structured message announced, and when.
    structured: Option<(String, SystemTime)>,
}

impl Classifier {
    pub fn new(rules: Arc<RuleSet>) -> Self {
        Self {
            rules,
            pending: String::new(),
            plain: String::new(),
            last: None,
            structured: None,
        }
    }

    pub fn rules(&self) -> &Arc<RuleSet> {
        &self.rules
    }

    /// Replace the rules mid stream, keeping the line in flight (a rule taught mid session).
    pub fn set_rules(&mut self, rules: Arc<RuleSet>) {
        self.rules = rules;
    }

    /// Complete server lines only: text that ends mid line waits for the rest.
    pub fn feed(&mut self, text: &str, at: SystemTime, out: &mut Vec<ChannelLine>) {
        let start_len = out.len();
        let mut rest = text;
        while let Some(i) = rest.find('\n') {
            let mut line = std::mem::take(&mut self.pending);
            let found = if line.is_empty() {
                self.classify_raw(&rest[..i], at)
            } else {
                line.push_str(&rest[..i]);
                self.classify_raw(&line, at)
            };
            // Keep the buffer's capacity for the next partial line.
            line.clear();
            self.pending = line;
            if let Some(found) = found {
                out.push(found);
            }
            rest = &rest[i + 1..];
            if out.len() - start_len > MAX_RESULTS {
                self.reset();
                return;
            }
        }
        self.pending.push_str(rest);
        if self.pending.len() > MAX_PENDING {
            self.pending.clear();
        }
    }

    fn classify_raw(&mut self, raw: &str, at: SystemTime) -> Option<ChannelLine> {
        let mut plain = std::mem::take(&mut self.plain);
        strip_into(raw, &mut plain);
        let found = self.classify(raw, &plain, at);
        self.plain = plain;
        found
    }

    /// Classify one complete line, joining it to the message above when it is a wrapped tail.
    fn classify(&mut self, raw: &str, plain: &str, at: SystemTime) -> Option<ChannelLine> {
        if let Some((duplicate, when)) = &self.structured
            && at.duration_since(*when).unwrap_or_default() <= DUPLICATE_WINDOW
        {
            let same = duplicate == plain;
            self.structured = None;
            if same {
                return None;
            }
        }
        if let Some(m) = self.rules.match_plain(plain) {
            let message = Incoming {
                body: styled_slice(raw, m.text_range.clone()),
                channel: m.channel,
                speaker: m.speaker,
                text: m.text,
                raw_line: plain.to_string(),
                private: m.private,
                reply_command: m.reply_command,
                at,
            };
            self.last = Some(message.clone());
            return Some(ChannelLine {
                message,
                continuation: false,
            });
        }
        // A wrapped channel line is indented; anything flush with the margin has left the
        // conversation.
        if let Some(last) = &mut self.last
            && plain.starts_with(char::is_whitespace)
        {
            let tail = plain.trim();
            let start = plain.len() - plain.trim_start().len();
            last.text.push(' ');
            last.text.push_str(tail);
            last.raw_line.push('\n');
            last.raw_line.push_str(plain);
            last.body.push(' ');
            last.body.push_str(&styled_slice(raw, start..start + tail.len()));
            return Some(ChannelLine {
                message: last.clone(),
                continuation: true,
            });
        }
        self.last = None;
        None
    }

    /// A structured message the world also prints: the printed copy arrives within a moment
    /// with the same text, so it is dropped rather than mirrored twice.
    pub fn expect_printed_copy(&mut self, text: &str, at: SystemTime) {
        self.structured = Some((strip(text), at));
        self.last = None;
    }

    /// A privacy interval or a cleared transcript ends whatever line was in flight.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.last = None;
        self.structured = None;
    }
}
