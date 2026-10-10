//! Prompt detection. Pure: the caller tells it what happened and when, and reads the prompt text
//! from its terminal model (the current line) when told to.
//!
//! Two sources:
//! - **Marks**: the server ends a prompt with IAC GA or IAC EOR. Exact; once a session has sent a
//!   mark, the heuristic is switched off for it.
//! - **Heuristic**: text that stays unterminated (no line feed after it) for a quiet period is
//!   taken as a prompt, once. More output on the same line restarts the wait.

use std::time::{Duration, Instant};

/// How long an unterminated line must stay quiet before it counts as a prompt.
pub const DEFAULT_QUIET: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptSource {
    /// IAC GA or IAC EOR.
    Mark,
    /// An unterminated line that went quiet.
    Quiet,
}

/// A detected prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub text: String,
    pub source: PromptSource,
    /// Whether it looks like a password prompt (input should be masked).
    pub password: bool,
}

impl Prompt {
    pub fn new(text: String, source: PromptSource) -> Self {
        let password = looks_like_password_prompt(&text);
        Self { text, source, password }
    }
}

#[derive(Debug)]
pub struct PromptTracker {
    quiet: Duration,
    marks_seen: bool,
    /// Unterminated text arrived at this time and has not been reported.
    pending_since: Option<Instant>,
}

impl Default for PromptTracker {
    fn default() -> Self {
        Self::new(DEFAULT_QUIET)
    }
}

impl PromptTracker {
    pub fn new(quiet: Duration) -> Self {
        Self {
            quiet,
            marks_seen: false,
            pending_since: None,
        }
    }

    /// The server sent GA or EOR: the current line is a prompt. Returns the source to report.
    pub fn on_mark(&mut self) -> PromptSource {
        self.marks_seen = true;
        self.pending_since = None;
        PromptSource::Mark
    }

    /// Output was applied. `unterminated` says whether the terminal's cursor is now after text on
    /// a line that has not ended.
    pub fn on_output(&mut self, now: Instant, unterminated: bool) {
        self.pending_since = (unterminated && !self.marks_seen).then_some(now);
    }

    /// Whether the quiet period has passed: if so, the caller reads the current line and reports
    /// it as a prompt; this returns true once per unterminated line.
    pub fn poll(&mut self, now: Instant) -> bool {
        match self.pending_since {
            Some(since) if now.duration_since(since) >= self.quiet => {
                self.pending_since = None;
                true
            }
            _ => false,
        }
    }

    /// When [`Self::poll`] should next be called, if a prompt may be pending.
    pub fn deadline(&self) -> Option<Instant> {
        self.pending_since.map(|since| since + self.quiet)
    }

    /// Forget the session's history (a new connection).
    pub fn reset(&mut self) {
        self.marks_seen = false;
        self.pending_since = None;
    }

    pub fn marks_seen(&self) -> bool {
        self.marks_seen
    }
}

/// Whether a prompt asks for a password ("Password:", "Enter your passphrase", "Mot de passe").
pub fn looks_like_password_prompt(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    ["password", "passphrase", "passwort", "mot de passe", "contraseña"]
        .iter()
        .any(|w| lower.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn quiet_unterminated_line_is_a_prompt_once() {
        let t0 = Instant::now();
        let mut p = PromptTracker::new(ms(100));
        p.on_output(t0, true);
        assert!(!p.poll(t0 + ms(50)));
        assert_eq!(p.deadline(), Some(t0 + ms(100)));
        assert!(p.poll(t0 + ms(100)));
        assert!(!p.poll(t0 + ms(500)), "only once");
        assert_eq!(p.deadline(), None);
    }

    #[test]
    fn more_output_restarts_the_wait_and_newlines_cancel_it() {
        let t0 = Instant::now();
        let mut p = PromptTracker::new(ms(100));
        p.on_output(t0, true);
        p.on_output(t0 + ms(80), true);
        assert!(!p.poll(t0 + ms(120)));
        assert!(p.poll(t0 + ms(180)));
        p.on_output(t0 + ms(200), true);
        p.on_output(t0 + ms(210), false);
        assert!(!p.poll(t0 + ms(1000)));
    }

    #[test]
    fn marks_switch_the_heuristic_off() {
        let t0 = Instant::now();
        let mut p = PromptTracker::new(ms(100));
        assert_eq!(p.on_mark(), PromptSource::Mark);
        p.on_output(t0, true);
        assert!(!p.poll(t0 + ms(1000)));
        p.reset();
        p.on_output(t0, true);
        assert!(p.poll(t0 + ms(100)));
    }

    #[test]
    fn password_prompts() {
        assert!(Prompt::new("Password:".into(), PromptSource::Quiet).password);
        assert!(looks_like_password_prompt("Enter your PASSPHRASE "));
        assert!(!looks_like_password_prompt("<100hp 20mv>"));
        assert!(!looks_like_password_prompt("What is your name?"));
    }
}
