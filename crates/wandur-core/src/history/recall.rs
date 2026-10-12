//! Command recall: the Up and Down history a new session starts with, read back from session
//! history.
//!
//! Only command lines the person typed come back, as typed (`/10 say 1;kill droid`, an alias's
//! name), from `history_commands`: the session records one wherever the input line keeps one
//! for Up and Down, never for private input or during a login, and never one with a remembered
//! secret in it. Commands nobody typed (map walks, triggers, timers, scripts, the agent, channel
//! replies) are only `sent` entries, which recall does not read; a database from before
//! `history_commands` existed restores nothing. The lines are kept with the input line's own
//! rules ([`push_command`]): not blank, not twice in a row, the newest `limit`.

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use super::recorder::REDACTED;

/// The history session ids a session tab recorded itself. The store reads it after it listed
/// the sessions to read, so a session that began recording meanwhile is still left out (its
/// id is added before its recorder starts).
pub type OwnSessions = Arc<Mutex<Vec<String>>>;

/// What to read back for a session that opens.
#[derive(Clone, Debug)]
pub struct RecallRequest {
    /// The session's canonical endpoint (`host:port`), as its history sessions are recorded.
    /// Every other address of the same saved world counts too.
    pub endpoint_key: String,
    /// The character, compared ignoring case (empty matches sessions recorded without one).
    pub character: String,
    /// Sessions not to read (the tab's own connections).
    pub own: OwnSessions,
    /// Entries before this are not read (the retention cutoff).
    pub since: Option<SystemTime>,
    /// Commands wanted at most.
    pub limit: usize,
}

/// Keep `line` for Up and Down as the input line does: not blank, not the same as the last one,
/// the oldest dropped past `limit`. Returns how many were dropped from the front (0 or 1).
pub fn push_command(history: &mut Vec<String>, line: String, limit: usize) -> usize {
    if line.trim().is_empty() || history.last() == Some(&line) {
        return 0;
    }
    history.push(line);
    let over = history.len().saturating_sub(limit);
    history.drain(..over);
    over
}

/// Whether a recorded command line may come back into the input line's history.
pub fn recallable(text: &str) -> bool {
    !text.trim().is_empty() && !text.contains(REDACTED)
}

/// Collects recorded commands read newest first into the list [`push_command`] would have built
/// from them oldest first (consecutive duplicates collapse the same way in either direction).
#[derive(Debug)]
pub struct Collector {
    newest_first: Vec<String>,
    limit: usize,
}

impl Collector {
    pub fn new(limit: usize) -> Self {
        Self {
            newest_first: Vec::new(),
            limit,
        }
    }

    /// Offer the next older line; returns true once the list is full.
    pub fn offer(&mut self, text: String) -> bool {
        if self.is_full() {
            return true;
        }
        if recallable(&text) && self.newest_first.last() != Some(&text) {
            self.newest_first.push(text);
        }
        self.is_full()
    }

    pub fn is_full(&self) -> bool {
        self.newest_first.len() >= self.limit
    }

    /// Oldest first, newest last.
    pub fn finish(mut self) -> Vec<String> {
        self.newest_first.reverse();
        self.newest_first
    }
}

/// Restored commands before the ones typed this run, as if they had all been typed in that
/// order. Returns the history and where the typed ones begin in it (the restored count).
pub fn merge(restored: Vec<String>, typed: Vec<String>, limit: usize) -> (Vec<String>, usize) {
    let mut out = Vec::with_capacity(limit.min(restored.len() + typed.len()));
    for line in restored {
        push_command(&mut out, line, limit);
    }
    // The last restored one and the first typed one are the same command: keep it once.
    if out.last().is_some_and(|last| typed.first() == Some(last)) {
        out.pop();
    }
    let room = limit.saturating_sub(typed.len());
    let over = out.len().saturating_sub(room);
    out.drain(..over);
    let offset = out.len();
    out.extend(typed);
    (out, offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn push_command_follows_the_input_lines_rules() {
        let mut history = Vec::new();
        for line in ["look", "look", "  ", "n", "look", "look"] {
            push_command(&mut history, line.into(), 3);
        }
        assert_eq!(history, strings(&["look", "n", "look"]));
        assert_eq!(push_command(&mut history, "s".into(), 3), 1);
        assert_eq!(history, strings(&["n", "look", "s"]));
    }

    #[test]
    fn the_collector_builds_what_typing_the_lines_would_have_built() {
        let typed = ["a", "a", "b", "", "b", "c", "a", "a", "[redacted] x", "a", "d"];
        for limit in 1..8 {
            let mut forward = Vec::new();
            for line in typed.iter().filter(|l| recallable(l)) {
                push_command(&mut forward, line.to_string(), limit);
            }
            let mut collector = Collector::new(limit);
            for line in typed.iter().rev() {
                if collector.offer(line.to_string()) {
                    break;
                }
            }
            assert_eq!(collector.finish(), forward, "limit {limit}");
        }
    }

    #[test]
    fn masked_and_blank_lines_are_not_recalled() {
        assert!(!recallable("say my password is [redacted]"));
        assert!(!recallable("   "));
        assert!(recallable("look"));
    }

    #[test]
    fn merge_puts_restored_commands_before_typed_ones() {
        let (history, offset) = merge(strings(&["a", "b", "c"]), strings(&["c", "d"]), 10);
        assert_eq!(history, strings(&["a", "b", "c", "d"]));
        assert_eq!(offset, 2);
        assert_eq!(history[offset], "c");

        // The limit drops the oldest restored commands first.
        let (history, offset) = merge(strings(&["a", "b", "c"]), strings(&["x", "y"]), 3);
        assert_eq!(history, strings(&["c", "x", "y"]));
        assert_eq!(offset, 1);

        let (history, offset) = merge(Vec::new(), strings(&["x"]), 3);
        assert_eq!((history, offset), (strings(&["x"]), 0));
    }
}
