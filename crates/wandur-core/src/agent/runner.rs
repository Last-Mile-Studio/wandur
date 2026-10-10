//! The decision loop of one connection (the C# `AgentRunner`), as a state machine the session
//! drives from the UI thread: [`AgentRunner::start`], then [`AgentRunner::poll`] each frame (and
//! at [`AgentRunner::deadline`]). The credential read and every model call run on a thread of
//! their own and report back over a channel, so the UI never waits on the model; a cancelled
//! call's answer is dropped, and an HTTP call stops within a fraction of a second, ending its
//! thread (see `agent_http`).
//!
//! The rules are the C# ones:
//! - a run starts only while the session can act (connected, logged in, input public);
//! - Preview asks once and shows the decision without sending it or keeping its memory; Step
//!   sends at most one command and pauses once fresh output arrived; Run repeats within the
//!   decision count and the run time;
//! - a decision is discarded when the observation changed while the model thought;
//! - only a catalog command is ever sent, through the session, which checks the observation
//!   again; `wait` sends nothing and `done` ends the run;
//! - after a command the runner waits for fresh public output (up to the response timeout),
//!   then half a second more for the rest of it, then the action interval;
//! - cancelling (Stop, a manual command, walking, private input, a disconnect, a goal change)
//!   ends the run at once.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use super::decision_codec::{AgentDecision, AgentRequest};
use super::profile::{self, AgentCommand, AgentProfile, DONE, MAX_GOAL_TEXT, MAX_MEMORY, MAX_REASON, WAIT};
use super::providers::{AgentModelProvider, ProviderRegistry};
use super::{AgentError, CancelToken, len16};
use crate::l10n::{S, t};

/// Recent decisions kept.
pub const ACTIVITY_LIMIT: usize = 20;
/// How long fresh output may keep arriving before the next decision (a reply split across
/// packets settles).
pub const SETTLE: Duration = Duration::from_millis(500);
/// Extra time a model call may take past its response timeout before the runner stops waiting
/// for a provider that never answers.
const CALL_GRACE: Duration = Duration::from_secs(2);

/// What a press of Preview, Step or Play asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentRunMode {
    Preview,
    Step,
    Run,
}

/// What the session shows the agent: a revision that moves with every change of public output,
/// a generation that moves when the context is reset (private input, a disconnect), the text,
/// and whether a command may be sent now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentObservation {
    pub revision: u64,
    pub generation: u64,
    pub text: String,
    pub can_act: bool,
}

impl AgentObservation {
    /// Whether `now` is still the world `self` was taken from.
    pub fn same_as(&self, now: &AgentObservation) -> bool {
        now.can_act && self.revision == now.revision && self.generation == now.generation
    }
}

/// The session as the runner sees it.
pub trait AgentGateway {
    fn observe(&mut self) -> AgentObservation;
    /// The agent takes control (no other command source sends; scripts stop; a walk stops) or
    /// gives it back.
    fn set_agent_control(&mut self, enabled: bool);
    /// Send an allowed command if the world is still the one `expected` saw.
    fn send(&mut self, command: &str, expected: &AgentObservation) -> bool;
}

/// The runner's status (the C# status keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentStatus {
    Stopped,
    Thinking,
    Waiting,
    Paused,
    Completed,
    Failed,
    NoOutput,
    Stale,
    Manual,
    PreviewReady,
    RunLimit,
    Unavailable,
    InvalidResponse,
    RequestTimedOut,
    ConnectionFailed,
    ConfigurationRequired,
}

impl AgentStatus {
    pub fn key(self) -> S {
        match self {
            AgentStatus::Stopped => S::AgentStopped,
            AgentStatus::Thinking => S::AgentThinking,
            AgentStatus::Waiting => S::AgentWaiting,
            AgentStatus::Paused => S::AgentPaused,
            AgentStatus::Completed => S::AgentCompleted,
            AgentStatus::Failed => S::AgentFailed,
            AgentStatus::NoOutput => S::AgentNoOutput,
            AgentStatus::Stale => S::AgentStale,
            AgentStatus::Manual => S::AgentManual,
            AgentStatus::PreviewReady => S::AgentPreviewReady,
            AgentStatus::RunLimit => S::AgentRunLimit,
            AgentStatus::Unavailable => S::AgentUnavailable,
            AgentStatus::InvalidResponse => S::AgentInvalidResponse,
            AgentStatus::RequestTimedOut => S::AgentRequestTimedOut,
            AgentStatus::ConnectionFailed => S::AgentConnectionFailed,
            AgentStatus::ConfigurationRequired => S::AgentConfigurationRequired,
        }
    }

    pub fn label(self) -> &'static str {
        t(self.key())
    }

    /// The status a failed call leaves (a cancelled one: see [`AgentRunner::poll`]).
    fn of(error: &AgentError) -> AgentStatus {
        match error {
            AgentError::InvalidResponse => AgentStatus::InvalidResponse,
            AgentError::Timeout => AgentStatus::RequestTimedOut,
            AgentError::Http(_) => AgentStatus::ConnectionFailed,
            AgentError::Invalid(_) => AgentStatus::ConfigurationRequired,
            AgentError::Cancelled | AgentError::Storage(_) => AgentStatus::Failed,
        }
    }
}

/// Reads the profile's API key (from the vault); runs on a thread of its own.
pub type CredentialReader = Arc<dyn Fn(&AgentProfile) -> Result<Option<String>, AgentError> + Send + Sync>;
/// Wakes the UI when a thread has an answer.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

enum Phase {
    /// Reading the API key.
    Credential(Receiver<Result<Option<String>, AgentError>>),
    /// A model call is out.
    Thinking {
        observation: AgentObservation,
        reply: Receiver<Result<AgentDecision, AgentError>>,
        started: Instant,
    },
    /// A command went out (or `wait`): waiting for fresh output until `until`, then for it to
    /// settle until `settle`.
    Waiting {
        before: AgentObservation,
        until: Instant,
        settle: Option<Instant>,
    },
    /// The action interval before the next decision.
    Interval(Instant),
}

/// What a poll found to do.
enum Next {
    Nothing,
    Changed,
    Finish(AgentStatus),
    Decide,
    Decided(Result<AgentDecision, AgentError>, AgentObservation),
}

struct Run {
    profile: AgentProfile,
    goal: String,
    mode: AgentRunMode,
    catalog: Vec<AgentCommand>,
    provider: Arc<dyn AgentModelProvider>,
    key: Option<String>,
    decisions: i32,
    cancel: CancelToken,
    /// The run's time limit.
    ends: Instant,
    phase: Phase,
}

/// One connection's agent: status, working memory, recent decisions and the run in progress.
pub struct AgentRunner {
    providers: ProviderRegistry,
    credentials: CredentialReader,
    wake: Option<Wake>,
    run: Option<Run>,
    status: AgentStatus,
    memory: String,
    activity: VecDeque<String>,
    /// Moves whenever something the UI shows changed.
    pub changes: u64,
    /// Model calls started (tests and the probe).
    pub calls: u64,
}

impl Default for AgentRunner {
    fn default() -> Self {
        Self::new(ProviderRegistry::default(), Arc::new(|_| Ok(None)), None)
    }
}

impl std::fmt::Debug for AgentRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRunner")
            .field("status", &self.status)
            .field("busy", &self.is_busy())
            .finish_non_exhaustive()
    }
}

impl AgentRunner {
    pub fn new(providers: ProviderRegistry, credentials: CredentialReader, wake: Option<Wake>) -> Self {
        Self {
            providers,
            credentials,
            wake,
            run: None,
            status: AgentStatus::Stopped,
            memory: String::new(),
            activity: VecDeque::new(),
            changes: 0,
            calls: 0,
        }
    }

    pub fn is_busy(&self) -> bool {
        self.run.is_some()
    }

    pub fn status(&self) -> AgentStatus {
        self.status
    }

    pub fn memory(&self) -> &str {
        &self.memory
    }

    /// The recent decisions, oldest first (`action: reason`).
    pub fn activity(&self) -> impl Iterator<Item = &str> {
        self.activity.iter().map(String::as_str)
    }

    /// The recent decisions as one text, a blank line between them (the C# `Activity`).
    pub fn activity_text(&self) -> String {
        self.activity
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Set the status from outside a run (the session could not read the profile).
    pub fn note_status(&mut self, status: AgentStatus) {
        self.set_status(status);
    }

    /// Show a decision in Recent decisions without running (scenes).
    pub fn note_activity(&mut self, entry: &str, status: AgentStatus) {
        self.push_activity(entry.to_string());
        self.set_status(status);
    }

    fn set_status(&mut self, status: AgentStatus) {
        self.status = status;
        self.changes += 1;
    }

    fn push_activity(&mut self, entry: String) {
        self.activity.push_back(entry);
        while self.activity.len() > ACTIVITY_LIMIT {
            self.activity.pop_front();
        }
        self.changes += 1;
    }

    /// Cancel what is in flight; the next [`poll`](Self::poll) ends the run (Paused when it
    /// was thinking or waiting). Cheap enough to call from any event.
    pub fn cancel_pending(&mut self) {
        if let Some(run) = &self.run {
            run.cancel.cancel();
        }
    }

    /// End the run now with `status` (Stop: [`AgentStatus::Stopped`]).
    pub fn stop(&mut self, status: AgentStatus, gateway: &mut dyn AgentGateway) {
        if let Some(run) = self.run.take() {
            run.cancel.cancel();
        }
        gateway.set_agent_control(false);
        self.set_status(status);
    }

    /// Stop, and forget the working memory and the recent decisions.
    pub fn clear_memory(&mut self, gateway: &mut dyn AgentGateway) {
        self.stop(AgentStatus::Stopped, gateway);
        self.memory.clear();
        self.activity.clear();
        self.changes += 1;
    }

    /// Start a run for `goal` (the selected goal's instructions). Does nothing while one runs.
    pub fn start(
        &mut self,
        profile: &AgentProfile,
        goal: &str,
        mode: AgentRunMode,
        gateway: &mut dyn AgentGateway,
        now: Instant,
    ) {
        if self.is_busy() {
            return;
        }
        if !gateway.observe().can_act {
            self.set_status(AgentStatus::Unavailable);
            return;
        }
        let prepared = (|| {
            profile::validate(profile, true)?;
            if goal.trim().is_empty() || len16(goal) > MAX_GOAL_TEXT {
                return Err(AgentError::Invalid("Invalid goal."));
            }
            let catalog = profile::parse_catalog(&profile.commands)?;
            let provider = self.providers.resolve(&profile.provider)?;
            Ok((catalog, provider))
        })();
        let (catalog, provider) = match prepared {
            Ok(p) => p,
            Err(e) => {
                gateway.set_agent_control(false);
                self.set_status(AgentStatus::of(&e));
                return;
            }
        };
        if mode != AgentRunMode::Preview {
            gateway.set_agent_control(true);
        }
        let cancel = CancelToken::new();
        let (tx, rx) = mpsc::channel();
        let read = Arc::clone(&self.credentials);
        let wake = self.wake.clone();
        let for_thread = profile.clone();
        let spawned = std::thread::Builder::new().name("wandur-agent".into()).spawn(move || {
            let _ = tx.send(read(&for_thread));
            if let Some(wake) = wake {
                wake();
            }
        });
        if spawned.is_err() {
            gateway.set_agent_control(false);
            self.set_status(AgentStatus::Failed);
            return;
        }
        let run_time = Duration::from_secs(profile.max_run_seconds.max(1) as u64);
        self.run = Some(Run {
            profile: profile.clone(),
            goal: goal.to_string(),
            mode,
            catalog,
            provider,
            key: None,
            decisions: 0,
            cancel,
            ends: now + run_time,
            phase: Phase::Credential(rx),
        });
        self.set_status(AgentStatus::Thinking);
    }

    /// When the runner next needs a poll without any other event.
    pub fn deadline(&self) -> Option<Instant> {
        let run = self.run.as_ref()?;
        let phase = match &run.phase {
            Phase::Credential(_) => None,
            Phase::Thinking { started, .. } => Some(*started + call_timeout(&run.profile)),
            Phase::Waiting { until, settle, .. } => Some(settle.unwrap_or(*until)),
            Phase::Interval(until) => Some(*until),
        };
        Some(phase.map_or(run.ends, |p| p.min(run.ends)))
    }

    /// Move the run along. Returns whether anything changed.
    pub fn poll(&mut self, gateway: &mut dyn AgentGateway, now: Instant) -> bool {
        let Some(run) = self.run.as_mut() else {
            return false;
        };
        if run.cancel.is_cancelled() || now >= run.ends {
            self.cancelled(gateway);
            return true;
        }
        let next = match &mut run.phase {
            Phase::Credential(rx) => match rx.try_recv() {
                Err(TryRecvError::Empty) => Next::Nothing,
                Ok(Ok(key)) => {
                    run.key = key;
                    Next::Decide
                }
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => Next::Finish(AgentStatus::Failed),
            },
            Phase::Thinking {
                reply,
                started,
                observation,
            } => match reply.try_recv() {
                Err(TryRecvError::Empty) if now >= *started + call_timeout(&run.profile) => {
                    Next::Finish(AgentStatus::RequestTimedOut)
                }
                Err(TryRecvError::Empty) => Next::Nothing,
                Ok(answer) => Next::Decided(answer, std::mem::take(observation)),
                Err(TryRecvError::Disconnected) => Next::Finish(AgentStatus::Failed),
            },
            Phase::Waiting { before, until, settle } => match settle {
                Some(settle) if now < *settle => Next::Nothing,
                Some(_) if run.mode == AgentRunMode::Step => Next::Finish(AgentStatus::Paused),
                Some(_) => {
                    let interval = Duration::from_secs(run.profile.action_interval_seconds.max(1) as u64);
                    run.phase = Phase::Interval(now + interval);
                    Next::Changed
                }
                None => {
                    let seen = gateway.observe();
                    if !seen.can_act || seen.generation != before.generation {
                        Next::Finish(AgentStatus::NoOutput)
                    } else if seen.revision != before.revision {
                        *settle = Some(now + SETTLE);
                        Next::Changed
                    } else if now >= *until {
                        Next::Finish(AgentStatus::NoOutput)
                    } else {
                        Next::Nothing
                    }
                }
            },
            Phase::Interval(until) if now < *until => Next::Nothing,
            Phase::Interval(_) => Next::Decide,
        };
        match next {
            Next::Nothing => false,
            Next::Changed => true,
            Next::Finish(status) => {
                self.finish(gateway, status);
                true
            }
            Next::Decide => {
                self.next_decision(gateway, now);
                true
            }
            Next::Decided(answer, observation) => {
                self.decided(answer, observation, gateway, now);
                true
            }
        }
    }

    /// The run was cancelled: Paused if it was thinking or waiting, else the status stays.
    fn cancelled(&mut self, gateway: &mut dyn AgentGateway) {
        if let Some(run) = self.run.take() {
            run.cancel.cancel();
        }
        gateway.set_agent_control(false);
        if matches!(self.status, AgentStatus::Thinking | AgentStatus::Waiting) {
            self.set_status(AgentStatus::Paused);
        } else {
            self.changes += 1;
        }
    }

    fn finish(&mut self, gateway: &mut dyn AgentGateway, status: AgentStatus) {
        if let Some(run) = self.run.take() {
            run.cancel.cancel();
        }
        gateway.set_agent_control(false);
        self.set_status(status);
    }

    /// Ask the model about the world as it is now.
    fn next_decision(&mut self, gateway: &mut dyn AgentGateway, now: Instant) {
        let Some(run) = self.run.as_mut() else { return };
        if run.decisions >= run.profile.max_decisions {
            self.finish(gateway, AgentStatus::RunLimit);
            return;
        }
        run.decisions += 1;
        let observation = gateway.observe();
        if !observation.can_act {
            self.finish(gateway, AgentStatus::Unavailable);
            return;
        }
        let request = AgentRequest {
            profile: run.profile.clone(),
            goal: run.goal.clone(),
            memory: self.memory.clone(),
            observation: observation.text.clone(),
        };
        let (tx, rx) = mpsc::channel();
        let provider = Arc::clone(&run.provider);
        let key = run.key.clone();
        let cancel = run.cancel.clone();
        let wake = self.wake.clone();
        let spawned = std::thread::Builder::new().name("wandur-agent".into()).spawn(move || {
            let answer = provider.decide(&request, key.as_deref(), &cancel);
            if tx.send(answer).is_ok()
                && let Some(wake) = wake
            {
                wake();
            }
        });
        if spawned.is_err() {
            self.finish(gateway, AgentStatus::Failed);
            return;
        }
        run.phase = Phase::Thinking {
            observation,
            reply: rx,
            started: now,
        };
        self.calls += 1;
        self.set_status(AgentStatus::Thinking);
    }

    fn decided(
        &mut self,
        answer: Result<AgentDecision, AgentError>,
        observation: AgentObservation,
        gateway: &mut dyn AgentGateway,
        now: Instant,
    ) {
        let decision = match answer {
            Ok(decision) => decision,
            Err(AgentError::Cancelled) => {
                self.cancelled(gateway);
                return;
            }
            Err(e) => {
                self.finish(gateway, AgentStatus::of(&e));
                return;
            }
        };
        if !observation.same_as(&gateway.observe()) {
            self.finish(gateway, AgentStatus::Stale);
            return;
        }
        let Some(run) = self.run.as_ref() else { return };
        if len16(&decision.memory) > MAX_MEMORY || len16(&decision.reason) > MAX_REASON {
            self.finish(gateway, AgentStatus::Failed);
            return;
        }
        let command = run
            .catalog
            .iter()
            .find(|c| c.id == decision.action)
            .map(|c| c.command.clone());
        if command.is_none() && decision.action != WAIT && decision.action != DONE {
            self.finish(gateway, AgentStatus::Failed);
            return;
        }
        let mode = run.mode;
        let timeout = Duration::from_secs(run.profile.response_timeout_seconds.max(1) as u64);
        self.push_activity([decision.action.as_str(), ": ", decision.reason.as_str()].concat());
        if mode == AgentRunMode::Preview {
            self.finish(gateway, AgentStatus::PreviewReady);
            return;
        }
        if decision.action == DONE {
            self.memory = decision.memory;
            self.finish(gateway, AgentStatus::Completed);
            return;
        }
        if let Some(command) = command
            && !gateway.send(&command, &observation)
        {
            self.finish(gateway, AgentStatus::Stale);
            return;
        }
        if self.run.as_ref().is_none_or(|r| r.cancel.is_cancelled()) {
            self.cancelled(gateway);
            return;
        }
        self.memory = decision.memory;
        if let Some(run) = self.run.as_mut() {
            run.phase = Phase::Waiting {
                before: observation,
                until: now + timeout,
                settle: None,
            };
        }
        self.set_status(AgentStatus::Waiting);
    }
}

fn call_timeout(profile: &AgentProfile) -> Duration {
    Duration::from_secs(profile.response_timeout_seconds.max(1) as u64) + CALL_GRACE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::providers::AgentModelProvider;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// The fake world of the C# `AgentRunnerTests`.
    #[derive(Default)]
    struct World {
        revision: Arc<AtomicU64>,
        generation: u64,
        owned: bool,
        commands: Vec<String>,
        can_act: bool,
    }

    impl World {
        fn new() -> Self {
            Self {
                can_act: true,
                ..Self::default()
            }
        }
    }

    impl AgentGateway for World {
        fn observe(&mut self) -> AgentObservation {
            AgentObservation {
                revision: self.revision.load(Ordering::SeqCst),
                generation: self.generation,
                text: "A quiet room".into(),
                can_act: self.can_act,
            }
        }
        fn set_agent_control(&mut self, enabled: bool) {
            self.owned = enabled;
        }
        fn send(&mut self, command: &str, _: &AgentObservation) -> bool {
            self.commands.push(command.into());
            self.revision.fetch_add(1, Ordering::SeqCst);
            true
        }
    }

    type Pending = Arc<Mutex<Option<mpsc::Receiver<AgentDecision>>>>;

    struct Provider {
        action: String,
        on_decision: Option<Arc<AtomicU64>>,
        pending: Pending,
        requests: Arc<Mutex<Vec<AgentRequest>>>,
    }

    impl Provider {
        fn new() -> Self {
            Self {
                action: "look".into(),
                on_decision: None,
                pending: Arc::default(),
                requests: Arc::default(),
            }
        }
    }

    impl AgentModelProvider for Provider {
        fn key(&self) -> &str {
            profile::OPENAI_COMPATIBLE
        }
        fn decide(
            &self,
            request: &AgentRequest,
            _: Option<&str>,
            _: &CancelToken,
        ) -> Result<AgentDecision, AgentError> {
            self.requests.lock().unwrap().push(request.clone());
            if let Some(revision) = &self.on_decision {
                revision.fetch_add(1, Ordering::SeqCst);
            }
            // A provider that ignores cancellation: it answers whenever it is told to.
            let pending = self.pending.lock().unwrap().take();
            if let Some(rx) = pending {
                return rx.recv().map_err(|_| AgentError::Cancelled);
            }
            Ok(AgentDecision::new(&self.action, "Observe room", "Room visited"))
        }
        fn list_models(&self, _: &AgentProfile, _: Option<&str>, _: &CancelToken) -> Result<Vec<String>, AgentError> {
            Ok(Vec::new())
        }
    }

    fn create(provider: Provider) -> AgentRunner {
        AgentRunner::new(
            ProviderRegistry::new(vec![Arc::new(provider)]),
            Arc::new(|_| Ok(None)),
            None,
        )
    }

    fn profile() -> AgentProfile {
        AgentProfile {
            model: "test".into(),
            ..AgentProfile::default()
        }
    }

    /// Poll on a fast clock (100 ms a turn) until the run ends.
    fn run_out(runner: &mut AgentRunner, world: &mut World, start: Instant) -> Instant {
        let mut now = start;
        let wall = Instant::now();
        while runner.is_busy() {
            runner.poll(world, now);
            now += Duration::from_millis(100);
            std::thread::sleep(Duration::from_millis(1));
            assert!(wall.elapsed() < Duration::from_secs(10), "the run never ended");
        }
        now
    }

    #[test]
    fn preview_never_sends_or_commits_memory() {
        let (mut world, mut runner) = (World::new(), create(Provider::new()));
        let now = Instant::now();
        runner.start(&profile(), "Explore", AgentRunMode::Preview, &mut world, now);
        assert!(!world.owned, "Preview does not take control");
        run_out(&mut runner, &mut world, now);
        assert!(world.commands.is_empty());
        assert_eq!(runner.memory(), "");
        assert_eq!(runner.status(), AgentStatus::PreviewReady);
        assert!(!world.owned);
        assert_eq!(runner.activity_text(), "look: Observe room");
    }

    #[test]
    fn step_sends_a_catalog_command_and_then_pauses() {
        let (mut world, mut runner) = (World::new(), create(Provider::new()));
        let now = Instant::now();
        runner.start(&profile(), "Explore", AgentRunMode::Step, &mut world, now);
        assert!(world.owned, "Step takes control");
        run_out(&mut runner, &mut world, now);
        assert_eq!(world.commands, ["look"]);
        assert_eq!(runner.memory(), "Room visited");
        assert!(!world.owned);
        assert_eq!(runner.status(), AgentStatus::Paused);
    }

    #[test]
    fn a_changed_world_discards_the_decision() {
        let mut world = World::new();
        let mut provider = Provider::new();
        provider.on_decision = Some(Arc::clone(&world.revision));
        let mut runner = create(provider);
        let now = Instant::now();
        runner.start(&profile(), "Explore", AgentRunMode::Step, &mut world, now);
        run_out(&mut runner, &mut world, now);
        assert!(world.commands.is_empty());
        assert_eq!(runner.status(), AgentStatus::Stale);
    }

    #[test]
    fn stop_cancels_a_pending_decision_even_if_the_provider_ignores_cancellation() {
        let mut world = World::new();
        let provider = Provider::new();
        let (answer, rx) = mpsc::channel();
        *provider.pending.lock().unwrap() = Some(rx);
        let mut runner = create(provider);
        let now = Instant::now();
        runner.start(&profile(), "Explore", AgentRunMode::Run, &mut world, now);
        // Wait until the call is out.
        let wall = Instant::now();
        while runner.calls == 0 {
            runner.poll(&mut world, now);
            std::thread::sleep(Duration::from_millis(1));
            assert!(wall.elapsed() < Duration::from_secs(5));
        }
        runner.stop(AgentStatus::Stopped, &mut world);
        answer.send(AgentDecision::new("look", "observe", "memory")).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        runner.poll(&mut world, now);
        assert!(world.commands.is_empty());
        assert!(!world.owned);
        assert!(!runner.is_busy());
        assert_eq!(runner.status(), AgentStatus::Stopped);
        assert_eq!(runner.memory(), "");
    }

    #[test]
    fn an_unknown_action_fails_closed() {
        let mut world = World::new();
        let mut provider = Provider::new();
        provider.action = "kill".into();
        let mut runner = create(provider);
        let now = Instant::now();
        runner.start(&profile(), "Explore", AgentRunMode::Step, &mut world, now);
        run_out(&mut runner, &mut world, now);
        assert!(world.commands.is_empty());
        assert_eq!(runner.status(), AgentStatus::Failed);
    }

    #[test]
    fn stop_releases_a_blocked_credential_lookup() {
        let mut world = World::new();
        let (release, blocked) = mpsc::channel::<()>();
        let blocked = Arc::new(Mutex::new(blocked));
        let reader: CredentialReader = Arc::new(move |_| {
            let _ = blocked.lock().unwrap().recv();
            Ok(Some("unused-secret".into()))
        });
        let mut runner = AgentRunner::new(ProviderRegistry::new(vec![Arc::new(Provider::new())]), reader, None);
        let now = Instant::now();
        let started = Instant::now();
        runner.start(&profile(), "Explore", AgentRunMode::Run, &mut world, now);
        assert!(runner.is_busy());
        runner.stop(AgentStatus::Stopped, &mut world);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!runner.is_busy());
        assert!(world.commands.is_empty());
        release.send(()).unwrap();
    }

    #[test]
    fn a_session_that_cannot_act_does_not_start_and_bad_settings_say_so() {
        let mut world = World::new();
        world.can_act = false;
        let mut runner = create(Provider::new());
        runner.start(&profile(), "Explore", AgentRunMode::Run, &mut world, Instant::now());
        assert!(!runner.is_busy());
        assert_eq!(runner.status(), AgentStatus::Unavailable);
        world.can_act = true;
        runner.start(
            &AgentProfile::default(),
            "Explore",
            AgentRunMode::Run,
            &mut world,
            Instant::now(),
        );
        assert_eq!(runner.status(), AgentStatus::ConfigurationRequired, "no model");
        runner.start(&profile(), "  ", AgentRunMode::Run, &mut world, Instant::now());
        assert_eq!(runner.status(), AgentStatus::ConfigurationRequired, "no goal");
        let other = AgentProfile {
            provider: "anthropic".into(),
            ..profile()
        };
        runner.start(&other, "Explore", AgentRunMode::Run, &mut world, Instant::now());
        assert_eq!(runner.status(), AgentStatus::ConfigurationRequired, "unknown provider");
        assert!(!world.owned);
    }

    #[test]
    fn run_repeats_until_the_decision_limit_and_waits_for_output_each_time() {
        let mut world = World::new();
        let mut runner = create(Provider::new());
        let p = AgentProfile {
            max_decisions: 3,
            ..profile()
        };
        let now = Instant::now();
        runner.start(&p, "Explore", AgentRunMode::Run, &mut world, now);
        run_out(&mut runner, &mut world, now);
        assert_eq!(world.commands, ["look", "look", "look"]);
        assert_eq!(runner.status(), AgentStatus::RunLimit);
        assert_eq!(runner.activity().count(), 3);
    }

    #[test]
    fn no_fresh_output_pauses_without_retrying() {
        struct Quiet(World);
        impl AgentGateway for Quiet {
            fn observe(&mut self) -> AgentObservation {
                self.0.observe()
            }
            fn set_agent_control(&mut self, enabled: bool) {
                self.0.owned = enabled;
            }
            fn send(&mut self, command: &str, _: &AgentObservation) -> bool {
                self.0.commands.push(command.into());
                true
            }
        }
        let mut world = Quiet(World::new());
        let mut runner = create(Provider::new());
        let p = AgentProfile {
            response_timeout_seconds: 2,
            ..profile()
        };
        let mut now = Instant::now();
        runner.start(&p, "Explore", AgentRunMode::Run, &mut world, now);
        let wall = Instant::now();
        while runner.is_busy() {
            runner.poll(&mut world, now);
            now += Duration::from_millis(100);
            std::thread::sleep(Duration::from_millis(1));
            assert!(wall.elapsed() < Duration::from_secs(10));
        }
        assert_eq!(world.0.commands, ["look"]);
        assert_eq!(runner.status(), AgentStatus::NoOutput);
    }

    #[test]
    fn cancelling_while_thinking_pauses_and_the_run_time_limit_ends_a_run() {
        let mut world = World::new();
        let provider = Provider::new();
        let (answer, rx) = mpsc::channel();
        *provider.pending.lock().unwrap() = Some(rx);
        let mut runner = create(provider);
        let now = Instant::now();
        runner.start(&profile(), "Explore", AgentRunMode::Run, &mut world, now);
        runner.cancel_pending();
        runner.poll(&mut world, now);
        assert!(!runner.is_busy());
        assert_eq!(runner.status(), AgentStatus::Paused);
        drop(answer);
        let p = AgentProfile {
            max_run_seconds: 1,
            ..profile()
        };
        let mut runner = create(Provider::new());
        runner.start(&p, "Explore", AgentRunMode::Run, &mut world, now);
        runner.poll(&mut world, now + Duration::from_secs(2));
        assert!(!runner.is_busy());
        assert!(!world.owned);
    }

    #[test]
    fn a_provider_that_never_answers_times_out_without_blocking() {
        let mut world = World::new();
        let provider = Provider::new();
        let (_answer, rx) = mpsc::channel();
        *provider.pending.lock().unwrap() = Some(rx);
        let mut runner = create(provider);
        let p = AgentProfile {
            response_timeout_seconds: 1,
            ..profile()
        };
        let now = Instant::now();
        runner.start(&p, "Explore", AgentRunMode::Run, &mut world, now);
        let wall = Instant::now();
        while runner.calls == 0 {
            runner.poll(&mut world, now);
            assert!(wall.elapsed() < Duration::from_secs(5));
        }
        let polled = Instant::now();
        assert!(!runner.poll(&mut world, now + Duration::from_millis(500)));
        assert!(
            polled.elapsed() < Duration::from_millis(50),
            "poll never waits on the model"
        );
        assert!(runner.deadline().unwrap() <= now + Duration::from_secs(4));
        runner.poll(&mut world, now + Duration::from_secs(4));
        assert_eq!(runner.status(), AgentStatus::RequestTimedOut);
        assert!(!runner.is_busy());
    }

    #[test]
    fn clearing_memory_stops_and_forgets() {
        let (mut world, mut runner) = (World::new(), create(Provider::new()));
        let now = Instant::now();
        runner.start(&profile(), "Explore", AgentRunMode::Step, &mut world, now);
        run_out(&mut runner, &mut world, now);
        assert_eq!(runner.memory(), "Room visited");
        runner.clear_memory(&mut world);
        assert_eq!(runner.memory(), "");
        assert_eq!(runner.activity().count(), 0);
        assert_eq!(runner.status(), AgentStatus::Stopped);
        // Activity keeps the newest twenty.
        for i in 0..25 {
            runner.note_activity(&format!("look: {i}"), AgentStatus::Paused);
        }
        assert_eq!(runner.activity().count(), ACTIVITY_LIMIT);
        assert_eq!(runner.activity().next(), Some("look: 5"));
    }

    #[test]
    fn the_request_carries_goal_memory_and_observation() {
        let provider = Provider::new();
        let requests = Arc::clone(&provider.requests);
        let (mut world, mut runner) = (World::new(), create(provider));
        let now = Instant::now();
        runner.start(&profile(), "Explore", AgentRunMode::Step, &mut world, now);
        run_out(&mut runner, &mut world, now);
        runner.start(&profile(), "Explore", AgentRunMode::Preview, &mut world, now);
        run_out(&mut runner, &mut world, now);
        let requests = requests.lock().unwrap();
        assert_eq!(requests[0].memory, "");
        assert_eq!(requests[1].memory, "Room visited");
        assert_eq!(requests[1].goal, "Explore");
        assert_eq!(requests[1].observation, "A quiet room");
    }
}
