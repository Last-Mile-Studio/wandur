//! The text login handshake: a bounded, one-shot exchange that sends the saved username at the
//! first username prompt and the saved password at the next password prompt, then stops. It
//! never holds the password itself and never retries: a second username prompt (a rejected
//! name) ends it, and so does the two-minute limit.
//!
//! Prompts are whole-line, case-insensitive regular expressions. The `regex` crate matches in
//! linear time, so a pattern cannot hang the client (the C# client uses .NET's non-backtracking
//! engine for the same reason); patterns with look-around or back references are refused.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

pub use regex::Regex;
use regex::RegexBuilder;

use crate::l10n::{S, t};

/// The C# client's default username prompt (`AutoLoginSequence.DefaultUsernamePrompt`).
pub const DEFAULT_USERNAME_PROMPT: &str = r"^\s*(?:(?:(?:please\s+)?(?:\(e\)|e)nter\s+(?:your\s+)?|your\s+)?(?:user\s*name|login|name|account(?:\s+name)?|character(?:\s+(?:or\s+account\s+)?name)?)[^:\r\n]{0,180}[:>]|by what name[^\r\n]{0,180}\?)\s*$";
/// The C# client's default password prompt (`AutoLoginSequence.DefaultPasswordPrompt`).
pub const DEFAULT_PASSWORD_PROMPT: &str =
    r"^\s*(?:(?:please\s+)?enter\s+(?:your\s+)?|your\s+)?(?:(?:p|\(p\))assword|passphrase|passcode)\s*[:>]\s*$";

/// How long the text handshake may take after the connection opens.
pub const LOGIN_WINDOW: Duration = Duration::from_secs(120);
/// Longest pattern, and longest prompt line looked at.
pub const MAX_PATTERN: usize = 512;
/// Compiled patterns kept (an editor can produce many one-off patterns).
const CACHE_LIMIT: usize = 256;
/// Compiled size limit per pattern, so a pathological repetition is refused rather than built.
const SIZE_LIMIT: usize = 4 << 20;

/// What to send for the prompt just seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginStep {
    None,
    Username,
    Password,
}

fn cache() -> &'static Mutex<HashMap<String, Regex>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Regex>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Compile a prompt pattern (case-insensitive). The message says what is wrong with it.
/// Accepted patterns are kept, so validating every saved world stays cheap; a refused one is
/// compiled again (and refused again) on every call.
pub fn compile(pattern: &str) -> Result<Regex, String> {
    if pattern.trim().is_empty() || pattern.chars().count() > MAX_PATTERN {
        return Err(t(S::LoginPromptPatternsMustBe1512Characters).into());
    }
    if let Some(regex) = cache().lock().unwrap_or_else(PoisonError::into_inner).get(pattern) {
        return Ok(regex.clone());
    }
    let regex = RegexBuilder::new(pattern)
        .case_insensitive(true)
        .size_limit(SIZE_LIMIT)
        .build()
        .map_err(|_| t(S::InvalidLoginPromptPatternUseASimpleRegularExpression).to_string())?;
    if regex.is_match("") {
        return Err(t(S::LoginPromptPatternsMustNotMatchEmptyText).into());
    }
    let mut cache = cache().lock().unwrap_or_else(PoisonError::into_inner);
    if cache.len() < CACHE_LIMIT {
        cache.insert(pattern.to_string(), regex.clone());
    }
    Ok(regex)
}

/// One connection's text login handshake.
#[derive(Clone, Debug)]
pub struct AutoLoginSequence {
    username: Regex,
    password: Regex,
    started: Instant,
    /// 0: waiting for the username prompt, 1: username sent, 2: finished.
    stage: u8,
}

impl AutoLoginSequence {
    /// A handshake for these prompt patterns, started at `started`.
    pub fn new(username_prompt: &str, password_prompt: &str, started: Instant) -> Result<Self, String> {
        Ok(Self {
            username: compile(username_prompt)?,
            password: compile(password_prompt)?,
            started,
            stage: 0,
        })
    }

    /// With the default patterns.
    pub fn with_defaults(started: Instant) -> Self {
        Self::new(DEFAULT_USERNAME_PROMPT, DEFAULT_PASSWORD_PROMPT, started)
            .expect("the default prompt patterns compile")
    }

    /// The username was sent (a password may follow).
    pub fn started(&self) -> bool {
        self.stage != 0
    }

    /// Nothing more will be sent.
    pub fn finished(&self) -> bool {
        self.stage == 2
    }

    /// The two minutes are over.
    pub fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) > LOGIN_WINDOW
    }

    /// When the handshake runs out, for a timer.
    pub fn deadline(&self) -> Instant {
        self.started + LOGIN_WINDOW
    }

    /// What to send for `prompt` (the current prompt line, ANSI already removed).
    pub fn next(&mut self, prompt: &str, now: Instant) -> LoginStep {
        if self.expired(now) {
            self.stage = 2;
        }
        if self.stage == 2 || prompt.chars().count() > MAX_PATTERN {
            return LoginStep::None;
        }
        if self.stage == 0 && self.username.is_match(prompt) {
            self.stage = 1;
            return LoginStep::Username;
        }
        if self.stage == 1 && self.password.is_match(prompt) {
            self.stage = 2;
            return LoginStep::Password;
        }
        // A second username prompt means the name was refused: stop rather than guess.
        if self.stage == 1 && self.username.is_match(prompt) {
            self.stage = 2;
        }
        LoginStep::None
    }
}

/// The server text since the last input, reduced to what prompt matching needs: the last line
/// with visible text (the line in progress counts). ANSI escape sequences and control characters
/// are dropped as they arrive, across packet boundaries, so a coloured prompt split over several
/// reads still reads `Password: `. Bounded: a line keeps at most [`MAX_PATTERN`] + 1 characters
/// (a longer one can never match).
#[derive(Clone, Debug, Default)]
pub struct PromptLine {
    /// The line in progress.
    current: String,
    current_chars: usize,
    /// The last complete line with visible text.
    last: String,
    escape: Escape,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Escape {
    #[default]
    None,
    /// After ESC.
    Start,
    /// Inside `ESC [ ...` until a final byte.
    Csi,
    /// Inside `ESC ] ...` until BEL or ESC \.
    Osc,
    /// ESC seen inside an OSC string.
    OscEsc,
}

impl PromptLine {
    /// Forget everything (input was sent, or a new connection).
    pub fn clear(&mut self) {
        self.current.clear();
        self.current_chars = 0;
        self.last.clear();
        self.escape = Escape::None;
    }

    /// Add server text. Only the end of it can matter: the line in progress and the last
    /// complete line with visible text before it. Lines before that are skipped with a byte
    /// search, so a flood costs about one scan of its newlines, not a walk over every character.
    pub fn push(&mut self, text: &str) {
        let Some(last_newline) = text.rfind('\n') else {
            self.push_chars(text);
            return;
        };
        let complete = &text[..last_newline];
        // Walk back over the complete lines to the last one with visible text. The first one
        // continues the line in progress (and its escape state).
        let mut end = complete.len();
        loop {
            match complete[..end].rfind('\n') {
                Some(newline) => {
                    let line = &complete[newline + 1..end];
                    let (current, chars, escape) = (std::mem::take(&mut self.current), self.current_chars, self.escape);
                    self.current_chars = 0;
                    self.escape = Escape::None;
                    self.push_chars(line);
                    if !self.current.trim().is_empty() {
                        self.end_line();
                        break;
                    }
                    // Blank: put the line in progress back and look further up.
                    self.current = current;
                    self.current_chars = chars;
                    self.escape = escape;
                    end = newline;
                }
                None => {
                    self.push_chars(&complete[..end]);
                    self.end_line();
                    break;
                }
            }
        }
        // A sequence never spans the end of a line in practice; start the new line clean.
        self.current.clear();
        self.current_chars = 0;
        self.escape = Escape::None;
        self.push_chars(&text[last_newline + 1..]);
    }

    /// The line in progress ends: it becomes the last line if it has visible text.
    fn end_line(&mut self) {
        if !self.current.trim().is_empty() {
            std::mem::swap(&mut self.last, &mut self.current);
        }
        self.current.clear();
        self.current_chars = 0;
    }

    /// Characters of one line (no newline), escapes and control characters dropped.
    fn push_chars(&mut self, text: &str) {
        for c in text.chars() {
            match self.escape {
                Escape::Start => {
                    self.escape = match c {
                        '[' => Escape::Csi,
                        ']' => Escape::Osc,
                        _ => Escape::None,
                    };
                    continue;
                }
                Escape::Csi => {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        self.escape = Escape::None;
                    }
                    continue;
                }
                Escape::Osc => {
                    match c {
                        '\u{7}' => self.escape = Escape::None,
                        '\u{1b}' => self.escape = Escape::OscEsc,
                        _ => {}
                    }
                    continue;
                }
                Escape::OscEsc => {
                    self.escape = if c == '\\' { Escape::None } else { Escape::Osc };
                    continue;
                }
                Escape::None => {}
            }
            match c {
                '\u{1b}' => self.escape = Escape::Start,
                c if c.is_control() => {}
                c => {
                    if self.current_chars > MAX_PATTERN {
                        // Longer than any prompt that can match: the rest of the line is not
                        // needed (its escape state neither; the next line starts clean).
                        return;
                    }
                    self.current.push(c);
                    self.current_chars += 1;
                }
            }
        }
    }

    /// The last line with visible text, or "" when there is none since the last clear.
    pub fn current(&self) -> &str {
        if self.current.trim().is_empty() {
            &self.last
        } else {
            &self.current
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> AutoLoginSequence {
        AutoLoginSequence::with_defaults(Instant::now())
    }

    fn next(login: &mut AutoLoginSequence, prompt: &str) -> LoginStep {
        login.next(prompt, Instant::now())
    }

    /// AutoLoginTests.SendsEachCredentialOnceAndOnlyInOrder.
    #[test]
    fn sends_each_credential_once_and_only_in_order() {
        for prompt in [
            "Name: ",
            "Username: ",
            "Account name: ",
            "(E)nter your character or account name, or type NEW: ",
            "By what name do you wish to be known? ",
        ] {
            let mut login = defaults();
            assert_eq!(next(&mut login, "Password: "), LoginStep::None, "{prompt}");
            assert_eq!(next(&mut login, prompt), LoginStep::Username, "{prompt}");
            assert_eq!(next(&mut login, "Passw"), LoginStep::None);
            assert_eq!(next(&mut login, "Password: "), LoginStep::Password);
            assert_eq!(next(&mut login, "Password: "), LoginStep::None);
            assert_eq!(next(&mut login, prompt), LoginStep::None);
        }
    }

    /// AutoLoginTests.RecognizesParenthesizedPasswordShortcut.
    #[test]
    fn recognizes_parenthesized_password_shortcut() {
        for prompt in ["(P)assword: ", "(p)assword:\r\n", "Enter your (P)assword: "] {
            let mut login = defaults();
            assert_eq!(
                next(&mut login, "(E)nter your character or account name, or type NEW: "),
                LoginStep::Username
            );
            assert_eq!(next(&mut login, prompt), LoginStep::Password, "{prompt:?}");
            assert_eq!(next(&mut login, prompt), LoginStep::None);
        }
    }

    /// AutoLoginTests.RetryPromptStopsAutomaticLogin.
    #[test]
    fn retry_prompt_stops_automatic_login() {
        let mut login = defaults();
        assert_eq!(next(&mut login, "Name: "), LoginStep::Username);
        assert_eq!(next(&mut login, "Unknown player. Name: "), LoginStep::None);
        assert_eq!(next(&mut login, "Name: "), LoginStep::None);
        assert!(login.finished(), "a second username prompt ends it");
        assert_eq!(next(&mut login, "Password: "), LoginStep::None);
    }

    /// AutoLoginTests.CustomPromptsAndExpiryAreRespected.
    #[test]
    fn custom_prompts_and_expiry_are_respected() {
        let now = Instant::now();
        let mut login = AutoLoginSequence::new(r"^Who enters\?$", r"^Secret\?$", now).unwrap();
        assert_eq!(login.next("Name: ", now), LoginStep::None);
        assert_eq!(login.next("Who enters?", now), LoginStep::Username);
        assert_eq!(login.next("Secret?", now), LoginStep::Password);
        let mut expired = AutoLoginSequence::new(r"^Who enters\?$", r"^Secret\?$", now).unwrap();
        let later = now + Duration::from_secs(180);
        assert!(expired.expired(later));
        assert_eq!(expired.next("Who enters?", later), LoginStep::None);
        assert!(expired.finished());
    }

    #[test]
    fn bad_patterns_are_refused_with_a_reason() {
        crate::l10n::override_thread(Some(crate::l10n::Language::En));
        assert!(compile("").unwrap_err().contains("1–512"));
        assert!(compile(&"a".repeat(513)).is_err());
        assert!(compile(".*").unwrap_err().contains("empty text"));
        assert!(compile(r"(?=x)name").unwrap_err().contains("lookarounds"));
        assert!(compile(r"(a)\1").is_err());
        assert!(compile(DEFAULT_USERNAME_PROMPT).is_ok());
        assert!(compile("^name:$").unwrap().is_match("NAME:"), "case-insensitive");
        crate::l10n::override_thread(None);
    }

    #[test]
    fn long_prompts_are_not_matched() {
        let mut login = defaults();
        let long = format!("Name{}:", " ".repeat(600));
        assert_eq!(next(&mut login, &long), LoginStep::None);
    }

    /// The prompt line follows split, coloured packets and trailing newlines (LoginTests'
    /// fragmented prompts), and keeps the last line with text.
    #[test]
    fn the_prompt_line_drops_escapes_across_packets() {
        let mut line = PromptLine::default();
        line.push("\x1b[1;36mUserna");
        assert_eq!(line.current(), "Userna");
        line.push("me: \x1b[0m");
        assert_eq!(line.current(), "Username: ");
        line.clear();
        line.push("\r\n\x1b[0m ");
        assert_eq!(line.current(), "");
        line.push("\x1b[1;30m(P)");
        line.push("assword: \x1b");
        line.push("[0m");
        assert_eq!(line.current(), " (P)assword: ");
        line.push("\r\n");
        assert_eq!(
            line.current(),
            " (P)assword: ",
            "a newline keeps the last line with text"
        );
        line.push("Wrong password.\r\nPassword: ");
        assert_eq!(line.current(), "Password: ");
        line.push("\x1b]0;title\x07Name:");
        assert_eq!(line.current(), "Password: Name:");
        line.clear();
        line.push(&"x".repeat(5000));
        assert_eq!(line.current().len(), MAX_PATTERN + 1);
    }

    /// One batch with many lines: the last line with text wins, blank lines after it are
    /// skipped, and the line in progress continues across batches.
    #[test]
    fn the_prompt_line_skips_to_the_end_of_a_batch() {
        let mut line = PromptLine::default();
        line.push("one\r\ntwo\r\n\x1b[32mthree\x1b[0m\r\n\r\n   \r\n");
        assert_eq!(line.current(), "three");
        line.push("Pass");
        line.push("word: ");
        assert_eq!(line.current(), "Password: ");
        line.push("\r\n\r\n");
        assert_eq!(line.current(), "Password: ", "blank lines keep the last line with text");
        line.push("Na");
        line.push("me\r\n\r\n");
        assert_eq!(line.current(), "Name");
        let flood = "a line of flood text\r\n".repeat(10_000);
        line.push(&flood);
        assert_eq!(line.current(), "a line of flood text");
    }
}
