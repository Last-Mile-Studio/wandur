//! A session's macros at run time: which saved macros run, when they may send, timers, the
//! session's Macros switch and the send rate limit. Pure logic with an injected clock; the app
//! feeds it lines, commands, keys and time, and sends what it returns.
//!
//! The C# rules it keeps:
//! - Macros run while the session is connected and the session's Macros switch is on; only the
//!   saved, enabled ones. The switch affects this session only and is not saved.
//! - Private input and an automatic login pause them: nothing matches, nothing sends, and lines
//!   received meanwhile are never seen.
//! - Triggers see complete public lines. An alias replaces a whole typed command; the first alias
//!   in library order wins. Every shortcut on a pressed function key runs.
//! - A timer waits a full interval after the macros start and does not catch up with a burst of
//!   missed intervals. Disconnecting or switching macros off stops timers; starting again waits
//!   a full interval again.
//! - At most 20 commands a second and 200 a minute leave a session through macros; a macro that
//!   goes over stops with "Script stopped because it sent commands too quickly."

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use super::definition::MacroDefinition;
use super::rules::RuleSet;
use crate::l10n::{S, t};

/// Commands a session may send through macros in one second, and in one minute.
pub const RATE_PER_SECOND: usize = 20;
pub const RATE_PER_MINUTE: usize = 200;

/// A saved macro as the runtime needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedMacro {
    pub id: String,
    pub name: String,
    /// Saved as enabled ("Enabled" in the world editor).
    pub enabled: bool,
    pub definition: MacroDefinition,
}

/// What the session is doing, as far as macros care.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gate {
    pub connected: bool,
    /// Private input (server echo off, a password prompt, or the manual switch).
    pub private: bool,
    /// An automatic login is under way.
    pub login: bool,
}

impl Gate {
    pub fn connected() -> Self {
        Self {
            connected: true,
            ..Self::default()
        }
    }

    fn paused(self) -> bool {
        self.private || self.login
    }
}

/// One macro's state, for the list in the world editor and the footer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacroStatus {
    Disabled,
    Waiting,
    Running,
    Paused,
    Failed,
}

impl MacroStatus {
    pub fn label(self) -> &'static str {
        t(match self {
            MacroStatus::Disabled => S::ScriptDisabled,
            MacroStatus::Waiting => S::ScriptWaiting,
            MacroStatus::Running => S::ScriptRunning,
            MacroStatus::Paused => S::ScriptPaused,
            MacroStatus::Failed => S::ScriptFailed,
        })
    }
}

/// The macros of one session.
pub struct MacroRuntime {
    library: Vec<SavedMacro>,
    rules: RuleSet,
    /// The session's Macros switch.
    switched_on: bool,
    /// Running since this instant (connected, switched on, something enabled).
    active: Option<Instant>,
    paused: bool,
    /// When each timer of [`RuleSet::timers`] is next due.
    due: Vec<Instant>,
    /// Macros stopped by an error this session, with the message.
    failed: HashMap<String, String>,
    sent: VecDeque<Instant>,
    /// The last error, for the session to show.
    pub error: Option<String>,
    scratch: Vec<u32>,
    /// Changes whenever the rules or their running state change; trigger results computed
    /// elsewhere (see [`super::worker`]) for another generation are dropped.
    generation: u64,
}

impl Default for MacroRuntime {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl MacroRuntime {
    pub fn new(library: Vec<SavedMacro>) -> Self {
        let mut runtime = Self {
            library: Vec::new(),
            rules: RuleSet::default(),
            switched_on: true,
            active: None,
            paused: false,
            due: Vec::new(),
            failed: HashMap::new(),
            sent: VecDeque::new(),
            error: None,
            scratch: Vec::new(),
            generation: 0,
        };
        runtime.set_library(library);
        runtime
    }

    /// The rules' generation (see the field).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn set_library(&mut self, library: Vec<SavedMacro>) {
        self.generation += 1;
        self.rules = RuleSet::build(
            library
                .iter()
                .filter(|m| m.enabled)
                .map(|m| (m.id.as_str(), &m.definition)),
        );
        self.library = library;
    }

    /// Adopt newly saved macros. Running timers start over; errors are forgotten.
    pub fn reload(&mut self, library: Vec<SavedMacro>, now: Instant, gate: Gate) {
        self.stop();
        self.failed.clear();
        self.error = None;
        self.set_library(library);
        self.update(now, gate);
    }

    /// The saved macros (enabled or not).
    pub fn library(&self) -> &[SavedMacro] {
        &self.library
    }

    /// Whether the world has any macros (the footer switch is enabled then).
    pub fn has_macros(&self) -> bool {
        !self.library.is_empty()
    }

    /// The session's Macros switch.
    pub fn switched_on(&self) -> bool {
        self.switched_on
    }

    /// Turn this session's macros on or off. Off stops timers; on starts them afresh.
    pub fn switch(&mut self, on: bool, now: Instant, gate: Gate) {
        if self.switched_on == on {
            return;
        }
        self.switched_on = on;
        self.stop();
        self.update(now, gate);
    }

    /// Whether macros are running (they may still be paused by private input).
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Whether the session should collect complete lines for triggers.
    pub fn wants_lines(&self) -> bool {
        self.active.is_some() && self.rules.has_triggers()
    }

    fn stop(&mut self) {
        if self.active.is_some() {
            self.generation += 1;
        }
        self.active = None;
        self.due.clear();
    }

    /// Follow the session's state: start when connected (timers wait a full interval from now),
    /// stop on disconnect, note a pause.
    pub fn update(&mut self, now: Instant, gate: Gate) {
        self.paused = gate.paused();
        let should_run = self.switched_on && gate.connected && !self.rules.is_empty();
        match (should_run, self.active.is_some()) {
            (true, false) => {
                self.generation += 1;
                self.active = Some(now);
                self.due = self.rules.timers().iter().map(|(_, every)| now + *every).collect();
            }
            (false, true) => self.stop(),
            _ => {}
        }
    }

    fn may_run(&mut self, now: Instant, gate: Gate) -> bool {
        self.update(now, gate);
        self.active.is_some() && !gate.paused()
    }

    /// Queue `rule`'s commands into `out` within the rate limit. Over the limit the macro stops.
    fn emit(&mut self, rule: u32, now: Instant, out: &mut Vec<String>) {
        let rule = self.rules.rule(rule);
        if self.failed.contains_key(&rule.id) {
            return;
        }
        for command in &rule.commands {
            while self
                .sent
                .front()
                .is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(60))
            {
                self.sent.pop_front();
            }
            let last_second = self
                .sent
                .iter()
                .rev()
                .take_while(|at| now.duration_since(**at) < Duration::from_secs(1))
                .count();
            if self.sent.len() >= RATE_PER_MINUTE || last_second >= RATE_PER_SECOND {
                let message = t(S::ScriptRateExceeded).to_string();
                self.failed.insert(rule.id.clone(), message.clone());
                self.error = Some(message);
                return;
            }
            self.sent.push_back(now);
            out.push(command.clone());
        }
    }

    /// A complete public line arrived. Commands of every matching trigger go into `out`.
    pub fn on_line(&mut self, line: &str, now: Instant, gate: Gate, out: &mut Vec<String>) {
        if !self.may_run(now, gate) || !self.rules.has_triggers() {
            return;
        }
        let mut hits = std::mem::take(&mut self.scratch);
        hits.clear();
        self.rules.match_line(line, &mut hits);
        for &rule in &hits {
            self.emit(rule, now, out);
        }
        self.scratch = hits;
    }

    /// Trigger rules that fired on lines matched elsewhere (a [`super::worker::TriggerWorker`]
    /// with the same macros). Results of another generation are dropped.
    pub fn on_hits(&mut self, generation: u64, hits: &[u32], now: Instant, gate: Gate, out: &mut Vec<String>) {
        if !self.may_run(now, gate) || generation != self.generation {
            return;
        }
        for &rule in hits {
            if (rule as usize) < self.rules.len() {
                self.emit(rule, now, out);
            }
        }
    }

    /// Stop every trigger of this session with `message` (the worker fell too far behind).
    pub fn stop_triggers(&mut self, message: &str) {
        for i in 0..self.rules.len() as u32 {
            let rule = self.rules.rule(i);
            if rule.kind == super::MacroKind::Trigger {
                self.failed.insert(rule.id.clone(), message.to_string());
            }
        }
        self.error = Some(message.to_string());
    }

    /// A typed command. `Some(commands)` when an alias replaces it (the commands may be empty if
    /// the macro was stopped by the rate limit); `None` sends the command as typed.
    pub fn on_command(&mut self, command: &str, now: Instant, gate: Gate) -> Option<Vec<String>> {
        if !self.may_run(now, gate) {
            return None;
        }
        let rule = self.rules.match_alias(command)?;
        if self.failed.contains_key(&self.rules.rule(rule).id) {
            return None;
        }
        let mut out = Vec::new();
        self.emit(rule, now, &mut out);
        Some(out)
    }

    /// A function key in the command input. `Some(commands)` when a shortcut took the key.
    pub fn on_key(&mut self, key: &str, now: Instant, gate: Gate) -> Option<Vec<String>> {
        if !self.may_run(now, gate) {
            return None;
        }
        let rules: Vec<u32> = self
            .rules
            .match_key(key)
            .filter(|&r| !self.failed.contains_key(&self.rules.rule(r).id))
            .collect();
        if rules.is_empty() {
            return None;
        }
        let mut out = Vec::new();
        for rule in rules {
            self.emit(rule, now, &mut out);
        }
        Some(out)
    }

    /// Run due timers. Each fires at most once per call and is next due an interval from now.
    pub fn tick(&mut self, now: Instant, gate: Gate, out: &mut Vec<String>) {
        if !self.may_run(now, gate) {
            return;
        }
        for i in 0..self.due.len() {
            if now >= self.due[i] {
                let (rule, every) = self.rules.timers()[i];
                self.due[i] = now + every;
                self.emit(rule, now, out);
            }
        }
    }

    /// When a timer is next due (a frame is needed then).
    pub fn deadline(&self) -> Option<Instant> {
        if self.active.is_none() || self.paused {
            return None;
        }
        self.due.iter().min().copied()
    }

    /// One macro's state.
    pub fn status(&self, id: &str) -> MacroStatus {
        match self.library.iter().find(|m| m.id == id) {
            None => MacroStatus::Disabled,
            Some(_) if self.failed.contains_key(id) => MacroStatus::Failed,
            Some(m) if !m.enabled => MacroStatus::Disabled,
            Some(_) if self.active.is_none() => MacroStatus::Waiting,
            Some(_) if self.paused => MacroStatus::Paused,
            Some(_) => MacroStatus::Running,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macros::definition::{MacroKind, MacroMatch};

    fn saved(id: &str, definition: MacroDefinition) -> SavedMacro {
        SavedMacro {
            id: id.into(),
            name: id.into(),
            enabled: true,
            definition,
        }
    }

    const ON: Gate = Gate {
        connected: true,
        private: false,
        login: false,
    };

    /// C# `TimerWaitsAndDoesNotCatchUpWithABurst`: due 10 s after start, fires once at 10 s, once
    /// at 90 s (not eight times), then not again until a full interval later.
    #[test]
    fn timer_waits_and_does_not_catch_up_with_a_burst() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut r = MacroRuntime::new(vec![saved(
            "t",
            MacroDefinition::new(MacroKind::Timer, "", "score").every(10),
        )]);
        let mut out = Vec::new();
        r.update(t0, ON);
        r.tick(t0, ON, &mut out);
        assert!(out.is_empty());
        r.tick(at(9_999), ON, &mut out);
        assert!(out.is_empty());
        assert_eq!(r.deadline(), Some(at(10_000)));
        r.tick(at(10_000), ON, &mut out);
        assert_eq!(out, ["score"]);
        out.clear();
        r.tick(at(90_000), ON, &mut out);
        assert_eq!(out, ["score"]);
        out.clear();
        r.tick(at(90_001), ON, &mut out);
        assert!(out.is_empty());
        r.tick(at(100_000), ON, &mut out);
        assert_eq!(out, ["score"]);
    }

    /// Timers stop on disconnect and on the session switch; starting again waits a full interval.
    #[test]
    fn timers_stop_on_disconnect_and_on_the_switch() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut r = MacroRuntime::new(vec![saved(
            "t",
            MacroDefinition::new(MacroKind::Timer, "", "score").every(5),
        )]);
        let mut out = Vec::new();
        r.update(t0, ON);
        r.update(at(3), Gate::default());
        assert!(!r.is_active());
        assert_eq!(r.deadline(), None);
        r.tick(at(6), Gate::default(), &mut out);
        assert!(out.is_empty(), "disconnected");
        r.tick(at(7), ON, &mut out);
        assert!(out.is_empty(), "reconnected at 7 s: due at 12 s");
        r.tick(at(12), ON, &mut out);
        assert_eq!(out.len(), 1);
        r.switch(false, at(13), ON);
        assert_eq!(r.status("t"), MacroStatus::Waiting);
        r.tick(at(30), ON, &mut out);
        assert_eq!(out.len(), 1, "switched off");
        r.switch(true, at(30), ON);
        r.tick(at(34), ON, &mut out);
        assert_eq!(out.len(), 1);
        r.tick(at(35), ON, &mut out);
        assert_eq!(out.len(), 2);
    }

    /// Private input and an automatic login pause macros: no line is matched, no alias or key
    /// replaces input, no timer sends.
    #[test]
    fn nothing_fires_on_private_or_login_lines() {
        let t0 = Instant::now();
        let mut r = MacroRuntime::new(vec![
            saved(
                "hunger",
                MacroDefinition::new(MacroKind::Trigger, "hungry", "eat bread"),
            ),
            saved("h", MacroDefinition::new(MacroKind::Alias, "h", "look")),
            saved("f2", MacroDefinition::new(MacroKind::Shortcut, "F2", "look")),
            saved("t", MacroDefinition::new(MacroKind::Timer, "", "score").every(1)),
        ]);
        let private = Gate { private: true, ..ON };
        let login = Gate { login: true, ..ON };
        let later = t0 + Duration::from_secs(5);
        let mut out = Vec::new();
        for gate in [private, login] {
            r.update(t0, gate);
            r.on_line("You are hungry.", t0, gate, &mut out);
            assert_eq!(r.on_command("h", t0, gate), None);
            assert_eq!(r.on_key("F2", t0, gate), None);
            r.tick(later, gate, &mut out);
            assert!(out.is_empty());
            assert_eq!(r.status("hunger"), MacroStatus::Paused);
        }
        r.on_line("You are hungry.", later, ON, &mut out);
        assert_eq!(out, ["eat bread"]);
        assert_eq!(r.status("hunger"), MacroStatus::Running);
    }

    #[test]
    fn only_saved_enabled_macros_run_and_the_first_alias_wins() {
        let now = Instant::now();
        let mut disabled = saved("off", MacroDefinition::new(MacroKind::Alias, "ford", "west"));
        disabled.enabled = false;
        let mut r = MacroRuntime::new(vec![
            disabled,
            saved("a", MacroDefinition::new(MacroKind::Alias, "ford", "east\neast\neast")),
            saved("b", MacroDefinition::new(MacroKind::Alias, "ford", "north")),
            saved(
                "t",
                MacroDefinition::new(MacroKind::Trigger, "ford", "swim").with_match(MacroMatch::Exact),
            ),
        ]);
        assert!(r.has_macros());
        assert_eq!(r.status("off"), MacroStatus::Disabled);
        assert_eq!(r.status("a"), MacroStatus::Waiting);
        assert_eq!(r.on_command("ford", now, Gate::default()), None, "not connected");
        assert_eq!(r.on_command("ford", now, ON).unwrap(), ["east", "east", "east"]);
        assert_eq!(r.on_command("ford now", now, ON), None);
        let mut out = Vec::new();
        r.on_line("ford", now, ON, &mut out);
        assert_eq!(out, ["swim"]);
    }

    #[test]
    fn a_macro_over_the_rate_limit_stops() {
        let t0 = Instant::now();
        let mut r = MacroRuntime::new(vec![
            saved(
                "spam",
                MacroDefinition::new(MacroKind::Shortcut, "F1", &vec!["look"; 15].join("\n")),
            ),
            saved("calm", MacroDefinition::new(MacroKind::Shortcut, "F2", "score")),
        ]);
        assert_eq!(r.on_key("F1", t0, ON).unwrap().len(), 15);
        // 5 more fit in this second; the sixth stops the macro.
        assert_eq!(r.on_key("F1", t0, ON).unwrap().len(), 5);
        assert_eq!(r.status("spam"), MacroStatus::Failed);
        assert_eq!(
            r.error.as_deref(),
            Some("Script stopped because it sent commands too quickly.")
        );
        assert_eq!(r.on_key("F1", t0 + Duration::from_secs(2), ON), None, "stopped");
        assert_eq!(r.on_key("F2", t0 + Duration::from_secs(2), ON).unwrap(), ["score"]);
        // A reload clears the failure.
        let library = r.library().to_vec();
        r.reload(library, t0 + Duration::from_secs(3), ON);
        assert_eq!(r.status("spam"), MacroStatus::Running);
    }
}
