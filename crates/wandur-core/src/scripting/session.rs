//! One session's scripts (the C# `WorldScriptLibrary` with its `SessionScripts` and
//! `SessionScriptWorker`, without the UI): which scripts run, their statuses and logs, which
//! events reach them, and what their results may do. Pure logic over the session's script
//! thread; the app feeds it the session's state, lines, events and commands, and carries out the
//! [`Effect`]s it returns.
//!
//! The C# rules it keeps:
//! - A script runs while the session is connected, its switch is on and input is public. Private
//!   input and an automatic login pause every script: no event goes in, and results computed
//!   meanwhile are dropped (a privacy change moves an epoch every queued request carries).
//! - Loads, stops and events share the thread's one queue, so an event sent while a script
//!   loads reaches it once it runs; the host's protocol cache (the seed) goes ahead of a load
//!   whenever it changed since the last seed.
//! - A typed command waits for the scripts with aliases: the first alias that matches takes it.
//!   A command whose script stopped or went private meanwhile is consumed, never sent as it was.
//! - At most 20 commands a second and 200 a minute leave a session through its scripts; a script
//!   that goes over stops with "Script stopped because it sent commands too quickly."
//! - At most 1,024 events wait for the thread (sent and not yet taken up by it); past that every
//!   script stops with the overflow message.
//! - A script error stops that script only. If the thread fails (a panic, or no answer within two
//!   seconds plus half a second for each further script), every script on it stops with an error
//!   and runs again on a new thread, at most three times in five minutes.
//! - A switch affects this session only; nothing here is saved.
//! - A Lua script runs only while Lua scripts are allowed (the preference); switching it on
//!   while they are not says so.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::time::{Duration, Instant};

use super::thread::{Reply, Request, ScriptThread, Wake};
use super::{ActionKind, EventKind, Runtime, ScriptEvent, ScriptResult, is_valid_msdp_name, limits};
use crate::l10n::{S, t, tf};
use crate::macros::Gate;

/// How often a running script's timers are checked.
pub const TICK: Duration = Duration::from_millis(250);
/// Thread failures allowed in [`RESTART_WINDOW`] before scripts stay stopped.
pub const MAX_RESTARTS: usize = 3;
pub const RESTART_WINDOW: Duration = Duration::from_secs(300);
/// Most of a script's output kept for its log.
pub const MAX_LOG: usize = 16_384;
/// The marker of the engine's pack send policy error, named in the UI language instead.
const PACK_SEND_POLICY: &str = "Pack send policy";

/// A saved script as the session runs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptDefinition {
    pub id: String,
    pub name: String,
    pub source: String,
    /// Starts with the session.
    pub enabled: bool,
    /// A pack script whose send policy has not been lifted.
    pub restricted_send: bool,
    /// Its language and layer.
    pub runtime: Runtime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Stopped,
    Loading,
    Running,
}

/// What the Scripts menu and the library show under a script's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptStatus {
    Disabled,
    Waiting,
    Busy,
    Paused,
    Running,
    Failed,
}

impl ScriptStatus {
    pub fn label(self) -> &'static str {
        t(match self {
            ScriptStatus::Disabled => S::ScriptDisabled,
            ScriptStatus::Waiting => S::ScriptWaiting,
            ScriptStatus::Busy => S::ScriptBusy,
            ScriptStatus::Paused => S::ScriptPaused,
            ScriptStatus::Running => S::ScriptRunning,
            ScriptStatus::Failed => S::ScriptFailed,
        })
    }
}

#[derive(Clone, Debug)]
pub struct ScriptEntry {
    pub id: String,
    pub name: String,
    pub source: String,
    /// The session's switch for the script (starts as saved).
    pub enabled: bool,
    pub restricted_send: bool,
    pub runtime: Runtime,
    pub state: RunState,
    /// Why the script stopped, in the UI language.
    pub error: Option<String>,
    /// The script's echoes and errors, newest last, at most [`MAX_LOG`] bytes.
    pub log: String,
    generation: u64,
    /// Started once since its switch or the library last changed (a failed script waits for
    /// the switch or a reload).
    attempted: bool,
    started: Option<Instant>,
    aliases: u32,
    tick_pending: bool,
    last_tick: u64,
}

impl ScriptEntry {
    fn new(definition: ScriptDefinition) -> Self {
        Self {
            id: definition.id,
            name: definition.name,
            source: definition.source,
            enabled: definition.enabled,
            restricted_send: definition.restricted_send,
            runtime: definition.runtime,
            state: RunState::Stopped,
            error: None,
            log: String::new(),
            generation: 0,
            attempted: false,
            started: None,
            aliases: 0,
            tick_pending: false,
            last_tick: 0,
        }
    }

    pub fn is_running(&self) -> bool {
        self.state == RunState::Running
    }

    fn append_log(&mut self, text: &str) {
        if !self.log.is_empty() {
            self.log.push('\n');
        }
        self.log.push_str(text);
        if self.log.len() > MAX_LOG {
            let mut cut = self.log.len() - MAX_LOG;
            while !self.log.is_char_boundary(cut) {
                cut += 1;
            }
            self.log.drain(..cut);
        }
    }
}

/// Something a script's result asks the session to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Send a command (it bypasses aliases and the command history).
    Send { script: String, command: String },
    /// Local text for the transcript.
    Echo { script: String, text: String },
    /// Ask the world to report an MSDP variable (`REPORT` and `SEND`).
    Report(String),
    /// A panel instruction (JSON), for the session's [`super::panels::PanelHost`].
    Panel { script: String, json: String },
}

/// A typed command the scripts have answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandOutcome {
    pub ticket: u64,
    /// An alias took it (or it can no longer be judged): do not send it as typed.
    pub consumed: bool,
}

enum Pending {
    Load {
        id: String,
        generation: u64,
        epoch: u64,
    },
    Dispatch {
        targets: Vec<(String, u64)>,
        epoch: u64,
        command: bool,
        tick: Option<String>,
    },
}

pub struct SessionScripts {
    pub entries: Vec<ScriptEntry>,
    name: String,
    wake: Wake,
    thread: Option<ScriptThread>,
    pending: HashMap<u64, Pending>,
    next_ticket: u64,
    /// Moves whenever privacy flips; results of requests from an older epoch are dropped.
    epoch: u64,
    blocked: bool,
    connected: bool,
    seed_sent: Option<String>,
    sent_at: VecDeque<Instant>,
    failures: VecDeque<Instant>,
    last_progress: Instant,
    /// MSDP variables scripts asked for, asked again when a private stretch ends.
    reported: BTreeSet<String>,
    /// Lua scripts may run (the preference; JavaScript is unaffected).
    lua_enabled: bool,
    /// An agent has control: every script is stopped and none starts by itself.
    suspended: bool,
    /// Script threads started (restarts included).
    pub starts: u32,
}

impl SessionScripts {
    /// `name` names the thread; `wake` wakes the UI when the thread has replies.
    pub fn new(name: &str, wake: Wake) -> Self {
        Self {
            entries: Vec::new(),
            name: name.to_string(),
            wake,
            thread: None,
            pending: HashMap::new(),
            next_ticket: 1,
            epoch: 0,
            blocked: false,
            connected: false,
            seed_sent: None,
            sent_at: VecDeque::new(),
            failures: VecDeque::new(),
            last_progress: Instant::now(),
            reported: BTreeSet::new(),
            lua_enabled: false,
            suspended: false,
            starts: 0,
        }
    }

    /// Adopt saved scripts (a new session, or Reload saved rules): every script stops and the
    /// enabled ones start again at the next [`update`](Self::update).
    pub fn set_library(&mut self, definitions: Vec<ScriptDefinition>) {
        self.shutdown();
        self.entries = definitions.into_iter().map(ScriptEntry::new).collect();
    }

    /// An agent takes control (Step and Play) or gives it back (the C# `SetSuspended`). Taking
    /// it stops every script. Giving it back starts none: starting a script again can replay its
    /// top-level sends, so scripts resume only when switched or reloaded.
    pub fn set_suspended(&mut self, suspended: bool) {
        if self.suspended == suspended {
            return;
        }
        self.suspended = suspended;
        if suspended {
            self.shutdown();
        }
        for entry in &mut self.entries {
            entry.attempted = true;
        }
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    pub fn lua_enabled(&self) -> bool {
        self.lua_enabled
    }

    /// Allow or stop Lua scripts (the C# `SetLuaEnabledAsync`): every Lua script stops, and
    /// when they are allowed the enabled ones start again at once (or at the next update).
    pub fn set_lua_enabled(&mut self, on: bool, now: Instant, seed: &dyn Fn() -> Option<String>) {
        if self.lua_enabled == on {
            return;
        }
        self.lua_enabled = on;
        let lua: Vec<usize> = (0..self.entries.len())
            .filter(|&i| self.entries[i].runtime.is_lua())
            .collect();
        for index in lua {
            self.stop_entry(index);
            let entry = &mut self.entries[index];
            entry.attempted = false;
            if on && entry.error.as_deref() == Some(t(S::LuaScriptsTurnedOff)) {
                entry.error = None;
            }
            if on && entry.enabled && self.connected && !self.blocked {
                self.activate(index, now, seed);
            }
        }
    }

    /// Whether this entry may start: enabled, and allowed in its language.
    fn may_run(&self, entry: &ScriptEntry) -> bool {
        entry.enabled && (self.lua_enabled || !entry.runtime.is_lua())
    }

    pub fn entry(&self, id: &str) -> Option<&ScriptEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn status(&self, entry: &ScriptEntry) -> ScriptStatus {
        if entry.error.is_some() {
            ScriptStatus::Failed
        } else if entry.state == RunState::Loading {
            ScriptStatus::Busy
        } else if entry.state == RunState::Running && self.blocked {
            ScriptStatus::Paused
        } else if entry.state == RunState::Running {
            ScriptStatus::Running
        } else if entry.enabled {
            ScriptStatus::Waiting
        } else {
            ScriptStatus::Disabled
        }
    }

    /// Whether any script runs or is starting.
    pub fn is_active(&self) -> bool {
        self.entries.iter().any(|e| e.state != RunState::Stopped)
    }

    /// Whether lines and events are wanted (a script runs or is starting, input is public).
    pub fn wants_events(&self) -> bool {
        self.connected && !self.blocked && self.is_active()
    }

    /// Follow the session: connected or not, private or not. Starts the enabled scripts that
    /// have not started (`seed` gives the host's protocol cache, asked only when one starts).
    /// Returns MSDP variables to ask for again after a private stretch.
    pub fn update(&mut self, gate: Gate, now: Instant, seed: &dyn Fn() -> Option<String>) -> Vec<Effect> {
        let mut effects = Vec::new();
        let blocked = gate.private || gate.login;
        if blocked != self.blocked {
            self.blocked = blocked;
            self.epoch += 1;
            if !blocked && gate.connected {
                // The answer that ended the private stretch was withheld: ask again.
                effects.extend(self.reported.iter().cloned().map(Effect::Report));
            }
        }
        if !gate.connected && self.connected {
            // Disconnecting stops every script; a new connection starts them again.
            self.shutdown();
            for entry in &mut self.entries {
                entry.attempted = false;
            }
            self.reported.clear();
        }
        self.connected = gate.connected;
        if self.connected && !self.blocked && !self.suspended {
            let start: Vec<usize> = (0..self.entries.len())
                .filter(|&i| {
                    let e = &self.entries[i];
                    self.may_run(e) && !e.attempted && e.state == RunState::Stopped
                })
                .collect();
            for index in start {
                self.activate(index, now, seed);
            }
        }
        self.watchdog(now);
        effects
    }

    /// The session's switch for one script: stop it, and start it again when on.
    pub fn set_enabled(&mut self, id: &str, on: bool, now: Instant, seed: &dyn Fn() -> Option<String>) {
        let Some(index) = self.entries.iter().position(|e| e.id == id) else {
            return;
        };
        self.entries[index].enabled = on;
        self.stop_entry(index);
        self.entries[index].attempted = false;
        let lua_off = self.entries[index].runtime.is_lua() && !self.lua_enabled;
        if lua_off {
            self.entries[index].error = on.then(|| t(S::LuaScriptsTurnedOff).to_string());
        } else if on && self.connected && !self.blocked {
            self.activate(index, now, seed);
        }
    }

    fn activate(&mut self, index: usize, now: Instant, seed: &dyn Fn() -> Option<String>) {
        if self.thread.is_none() {
            match ScriptThread::spawn(&self.name, std::sync::Arc::clone(&self.wake)) {
                Ok(thread) => {
                    self.thread = Some(thread);
                    self.starts += 1;
                    self.seed_sent = None;
                }
                Err(e) => {
                    let entry = &mut self.entries[index];
                    entry.attempted = true;
                    entry.error = Some(tf(S::ScriptFailed, &[&e]));
                    return;
                }
            }
        }
        let state = seed();
        if let Some(state) = state
            && self.seed_sent.as_ref() != Some(&state)
        {
            self.request(Request::Seed(state.clone()), now);
            self.seed_sent = Some(state);
        }
        let ticket = self.ticket();
        let epoch = self.epoch;
        let entry = &mut self.entries[index];
        entry.generation += 1;
        entry.state = RunState::Loading;
        entry.error = None;
        entry.log.clear();
        entry.attempted = true;
        entry.aliases = 0;
        entry.tick_pending = false;
        entry.last_tick = 0;
        let request = Request::Load {
            ticket,
            id: entry.id.clone(),
            source: entry.source.clone(),
            restricted_send: entry.restricted_send,
            runtime: entry.runtime,
        };
        self.pending.insert(
            ticket,
            Pending::Load {
                id: entry.id.clone(),
                generation: entry.generation,
                epoch,
            },
        );
        self.request(request, now);
    }

    fn ticket(&mut self) -> u64 {
        self.next_ticket += 1;
        self.next_ticket
    }

    fn request(&mut self, request: Request, now: Instant) {
        if self.pending.is_empty() {
            self.last_progress = now;
        }
        if let Some(thread) = &self.thread
            && !thread.send(request)
        {
            // The thread has gone; its failure reply is on its way or it already came.
        }
    }

    /// Stop one script: its pending results are dropped.
    fn stop_entry(&mut self, index: usize) {
        let entry = &mut self.entries[index];
        entry.generation += 1;
        let was = std::mem::replace(&mut entry.state, RunState::Stopped);
        entry.tick_pending = false;
        if was != RunState::Stopped
            && let Some(thread) = &self.thread
        {
            thread.send(Request::Stop(entry.id.clone()));
        }
    }

    fn fail(&mut self, index: usize, error: &str) {
        let error = if error.contains(PACK_SEND_POLICY) {
            t(S::ScriptPackSendRefused).to_string()
        } else {
            error.to_string()
        };
        self.stop_entry(index);
        let message = tf(S::ScriptFailed, &[&error]);
        let entry = &mut self.entries[index];
        entry.append_log(&message);
        entry.error = Some(message);
    }

    /// Stop every script and end the thread; the next script to start gets a new one.
    pub fn shutdown(&mut self) {
        for index in 0..self.entries.len() {
            let entry = &mut self.entries[index];
            entry.generation += 1;
            entry.state = RunState::Stopped;
            entry.tick_pending = false;
        }
        if let Some(thread) = self.thread.take() {
            // Never wait on the UI thread: the thread ends after its current call.
            thread.abandon();
        }
        self.pending.clear();
        self.seed_sent = None;
    }

    fn targets(&self, wants: impl Fn(&ScriptEntry) -> bool) -> Vec<(String, u64)> {
        self.entries
            .iter()
            .filter(|e| e.state != RunState::Stopped && wants(e))
            .map(|e| (e.id.clone(), e.generation))
            .collect()
    }

    /// Queue events for every running or starting script. False when nothing wanted them.
    fn dispatch(&mut self, events: Vec<ScriptEvent>, now: Instant) -> bool {
        if !self.wants_events() || events.is_empty() {
            return false;
        }
        let targets = self.targets(|_| true);
        self.queue(targets, events, false, None, now).is_some()
    }

    fn queue(
        &mut self,
        targets: Vec<(String, u64)>,
        events: Vec<ScriptEvent>,
        command: bool,
        tick: Option<String>,
        now: Instant,
    ) -> Option<u64> {
        if targets.is_empty() {
            return None;
        }
        let waiting = self.thread.as_ref().map_or(0, ScriptThread::waiting);
        if waiting + events.len() > limits::QUEUED_EVENTS_PER_SESSION {
            let message = t(S::ScriptQueueOverflow);
            for index in 0..self.entries.len() {
                if self.entries[index].state != RunState::Stopped {
                    self.fail(index, message);
                }
            }
            return None;
        }
        let ticket = self.ticket();
        self.pending.insert(
            ticket,
            Pending::Dispatch {
                targets: targets.clone(),
                epoch: self.epoch,
                command,
                tick,
            },
        );
        let ids = targets.into_iter().map(|(id, _)| id).collect();
        self.request(
            Request::Dispatch {
                ticket,
                targets: ids,
                events,
            },
            now,
        );
        Some(ticket)
    }

    /// Completed public lines (the caller keeps private and login lines out).
    pub fn feed_lines(&mut self, lines: &[String], now: Instant) {
        if !self.wants_events() || lines.is_empty() {
            return;
        }
        let events = lines
            .iter()
            .map(|line| ScriptEvent::new(EventKind::Line, line.as_str()))
            .collect();
        self.dispatch(events, now);
    }

    /// One event for every running script (a prompt, GMCP, MSDP).
    pub fn publish(&mut self, event: ScriptEvent, now: Instant) {
        self.dispatch(vec![event], now);
    }

    /// One event for one running script (a panel widget's callback). False when that script
    /// does not run or input is private.
    pub fn publish_to(&mut self, id: &str, event: ScriptEvent, now: Instant) -> bool {
        if !self.wants_events() {
            return false;
        }
        let targets = self.targets(|e| e.id == id && e.state == RunState::Running);
        self.queue(targets, vec![event], false, None, now).is_some()
    }

    /// Stop one script with an error the host found in its result (a rejected panel
    /// instruction).
    pub fn reject(&mut self, id: &str, message: &str) {
        if let Some(index) = self.index_of(id)
            && self.entries[index].state != RunState::Stopped
        {
            self.fail(index, message);
        }
    }

    /// A pack script's send policy changed (the person allowed or refused sending): a running
    /// script starts again under the new policy.
    pub fn set_restricted_send(&mut self, id: &str, restricted: bool, now: Instant, seed: &dyn Fn() -> Option<String>) {
        let Some(index) = self.index_of(id) else { return };
        if self.entries[index].restricted_send == restricted {
            return;
        }
        self.entries[index].restricted_send = restricted;
        let on = self.entries[index].enabled;
        self.set_enabled(id, on, now, seed);
    }

    /// A typed command, for the scripts with aliases (and those still starting, which may add
    /// some). Returns the ticket its [`CommandOutcome`] will carry, or `None` when no script
    /// wants it (send it as typed).
    pub fn command(&mut self, command: &str, now: Instant) -> Option<u64> {
        if !self.connected || self.blocked {
            return None;
        }
        let targets = self.targets(|e| e.state == RunState::Loading || e.aliases > 0);
        self.queue(
            targets,
            vec![ScriptEvent::new(EventKind::Command, command)],
            true,
            None,
            now,
        )
    }

    /// Run the timers of running scripts (each at most every [`TICK`]).
    pub fn tick(&mut self, now: Instant) {
        if !self.connected || self.blocked {
            return;
        }
        let due: Vec<(usize, u64)> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.state == RunState::Running && !e.tick_pending)
            .filter_map(|(i, e)| {
                let elapsed = now.saturating_duration_since(e.started?).as_millis() as u64;
                (elapsed.saturating_sub(e.last_tick) >= TICK.as_millis() as u64).then_some((i, elapsed))
            })
            .collect();
        for (index, elapsed) in due {
            let entry = &mut self.entries[index];
            entry.last_tick = elapsed;
            entry.tick_pending = true;
            let target = vec![(entry.id.clone(), entry.generation)];
            let id = entry.id.clone();
            self.queue(target, vec![ScriptEvent::tick(elapsed)], false, Some(id), now);
        }
    }

    /// When the session next needs a frame for scripts: a timer check or the thread's deadline.
    pub fn deadline(&self) -> Option<Instant> {
        let ticks = self
            .entries
            .iter()
            .filter(|e| e.state == RunState::Running && !e.tick_pending)
            .filter_map(|e| Some(e.started? + Duration::from_millis(e.last_tick) + TICK));
        let watchdog = (!self.pending.is_empty()).then(|| self.last_progress + self.thread_deadline());
        ticks.chain(watchdog).min()
    }

    fn thread_deadline(&self) -> Duration {
        let scripts = self.entries.iter().filter(|e| e.state != RunState::Stopped).count();
        Duration::from_secs(limits::WORKER_TIMEOUT_SECONDS)
            + Duration::from_millis(500) * scripts.saturating_sub(1) as u32
    }

    fn watchdog(&mut self, now: Instant) {
        if !self.pending.is_empty()
            && self.thread.is_some()
            && now.saturating_duration_since(self.last_progress) > self.thread_deadline()
        {
            self.thread_failed(now);
        }
    }

    /// Apply what the thread answered: results become effects, finished commands come back.
    pub fn poll(&mut self, now: Instant, effects: &mut Vec<Effect>, commands: &mut Vec<CommandOutcome>) {
        while let Some(reply) = self.thread.as_ref().and_then(ScriptThread::poll) {
            self.last_progress = now;
            match reply {
                Reply::Seeded { failed } => {
                    for (id, result) in failed {
                        if let Some(index) = self.index_of(&id)
                            && self.entries[index].state != RunState::Stopped
                        {
                            self.fail(index, result.error.as_deref().unwrap_or_default());
                        }
                    }
                }
                Reply::Loaded { ticket, result } => {
                    if let Some(Pending::Load { id, generation, epoch }) = self.pending.remove(&ticket) {
                        self.loaded(&id, generation, epoch, result, now, effects);
                    }
                }
                Reply::Dispatched { ticket, results } => {
                    if let Some(Pending::Dispatch {
                        targets,
                        epoch,
                        command,
                        tick,
                    }) = self.pending.remove(&ticket)
                    {
                        if let Some(id) = tick
                            && let Some(index) = self.index_of(&id)
                            && self.entries[index].generation == targets[0].1
                        {
                            self.entries[index].tick_pending = false;
                        }
                        let consumed = self.dispatched(&targets, epoch, results, now, effects);
                        if command {
                            commands.push(CommandOutcome { ticket, consumed });
                        }
                    }
                }
                Reply::Failed(_) => {
                    self.thread_failed(now);
                    break;
                }
            }
        }
        self.watchdog(now);
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }

    fn loaded(
        &mut self,
        id: &str,
        generation: u64,
        epoch: u64,
        result: ScriptResult,
        now: Instant,
        effects: &mut Vec<Effect>,
    ) {
        let Some(index) = self.index_of(id) else { return };
        if self.entries[index].generation != generation || self.entries[index].state != RunState::Loading {
            return;
        }
        if let Some(error) = &result.error {
            self.fail(index, error);
            return;
        }
        let entry = &mut self.entries[index];
        entry.state = RunState::Running;
        entry.started = Some(now);
        self.apply(index, generation, epoch, result, now, effects);
    }

    /// Apply one dispatch's results; returns whether a command was consumed.
    fn dispatched(
        &mut self,
        targets: &[(String, u64)],
        epoch: u64,
        results: Vec<(usize, usize, ScriptResult)>,
        now: Instant,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let mut consumed = false;
        let mut answered = vec![false; targets.len()];
        for (_, target, result) in results {
            let Some((id, generation)) = targets.get(target) else {
                continue;
            };
            answered[target] = true;
            let handled = result.handled;
            let Some(index) = self.index_of(id) else {
                consumed = true;
                continue;
            };
            let applied = self.apply(index, *generation, epoch, result, now, effects);
            consumed |= handled || !applied;
        }
        // A target that answered nothing still counts: one that can no longer be judged
        // consumes the command.
        for (target, (id, generation)) in targets.iter().enumerate() {
            if !answered[target] && !self.index_of(id).is_some_and(|i| self.valid(i, *generation, epoch)) {
                consumed = true;
            }
        }
        consumed
    }

    fn valid(&self, index: usize, generation: u64, epoch: u64) -> bool {
        let entry = &self.entries[index];
        entry.generation == generation
            && entry.state == RunState::Running
            && epoch == self.epoch
            && self.connected
            && !self.blocked
    }

    /// Apply one script's result. False when it could not be applied (stale, private, failed).
    fn apply(
        &mut self,
        index: usize,
        generation: u64,
        epoch: u64,
        result: ScriptResult,
        now: Instant,
        effects: &mut Vec<Effect>,
    ) -> bool {
        if self.entries[index].generation != generation {
            return false;
        }
        if let Some(aliases) = result.aliases {
            self.entries[index].aliases = aliases;
        }
        if let Some(error) = &result.error {
            // An engine error is shown even when privacy changed meanwhile.
            self.fail(index, error);
            return false;
        }
        if !self.valid(index, generation, epoch) {
            return false;
        }
        let id = self.entries[index].id.clone();
        for action in result.actions {
            match action.kind {
                ActionKind::Send => {
                    while self
                        .sent_at
                        .front()
                        .is_some_and(|&at| now.saturating_duration_since(at) >= Duration::from_secs(60))
                    {
                        self.sent_at.pop_front();
                    }
                    let last_second = self
                        .sent_at
                        .iter()
                        .filter(|&&at| now.saturating_duration_since(at) < Duration::from_secs(1))
                        .count();
                    if self.sent_at.len() >= limits::SENDS_PER_MINUTE || last_second >= limits::SENDS_PER_SECOND {
                        self.fail(index, t(S::ScriptRateExceeded));
                        return false;
                    }
                    let text = &action.text;
                    if text.trim().is_empty()
                        || super::js_length(text) > limits::SEND_CHARACTERS
                        || text.chars().any(char::is_control)
                    {
                        self.fail(index, t(S::ScriptWorkerFailed));
                        return false;
                    }
                    self.sent_at.push_back(now);
                    effects.push(Effect::Send {
                        script: id.clone(),
                        command: action.text,
                    });
                }
                ActionKind::Echo => {
                    self.entries[index].append_log(&action.text);
                    effects.push(Effect::Echo {
                        script: id.clone(),
                        text: action.text,
                    });
                }
                ActionKind::Panel => effects.push(Effect::Panel {
                    script: id.clone(),
                    json: action.text,
                }),
                ActionKind::Report => {
                    if is_valid_msdp_name(&action.text) {
                        self.reported.insert(action.text.clone());
                        effects.push(Effect::Report(action.text));
                    }
                }
            }
        }
        true
    }

    /// The thread failed: its scripts stop with an error and, unless it has failed three times
    /// in five minutes, start again on a new thread at the next update.
    fn thread_failed(&mut self, now: Instant) {
        if let Some(thread) = self.thread.take() {
            thread.abandon();
        }
        self.pending.clear();
        self.seed_sent = None;
        while self
            .failures
            .front()
            .is_some_and(|&at| now.saturating_duration_since(at) >= RESTART_WINDOW)
        {
            self.failures.pop_front();
        }
        self.failures.push_back(now);
        let restart = self.failures.len() <= MAX_RESTARTS;
        let message = t(if restart {
            S::ScriptWorkerFailed
        } else {
            S::ScriptWorkerRestartLimit
        });
        for index in 0..self.entries.len() {
            if self.entries[index].state != RunState::Stopped {
                self.fail(index, message);
                if restart {
                    self.entries[index].attempted = false;
                }
            }
        }
    }

    /// Tests: make the thread fail as a bug in it would.
    #[doc(hidden)]
    pub fn crash_thread(&self) {
        if let Some(thread) = &self.thread {
            thread.send(Request::Panic);
        }
    }

    /// Tests: whether a thread is running.
    #[doc(hidden)]
    pub fn has_thread(&self) -> bool {
        self.thread.is_some()
    }
}

impl Drop for SessionScripts {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(all(test, feature = "scripting"))]
mod tests;
