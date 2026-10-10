//! State of one session as the app sees it: the connection (with its reconnect policy), the
//! terminal grid, the input line, command history, prompts, protocol data and activity counters.
//! No drawing here; see `terminal_view`.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

use wandur_core::channels::{ChannelRule, SessionChannels};
use wandur_core::completion::Vocabulary;
use wandur_core::connection::{ConnectionState, Notice};
use wandur_core::demo::DemoWorld;
use wandur_core::diagnostics::{Gate as ProtocolGate, SessionProtocol};
use wandur_core::history::{Clock, HistoryRecorder, HistorySession, HistoryStore};
use wandur_core::l10n::{S, t, tf};
use wandur_core::login::gmcp::{self as gmcp_login, GmcpLogin};
use wandur_core::login::{AutoLoginSequence, LoginStep, PasswordVault, PromptLine, VaultError};
use wandur_core::macros::worker::{Refused, TriggerWorker};
use wandur_core::macros::{Gate, MacroRuntime, SavedMacro};
use wandur_core::map::store::{LoadResult, MapWorker, MapWorld};
use wandur_core::map::{MapRoute, MapSession, OptionState, WalkGate, WalkStatus};
use wandur_core::prompt::{Prompt, PromptSource, PromptTracker};
use wandur_core::protocol::MsspTable;
use wandur_core::protocol::mapping::WorldMapping;
use wandur_core::scripting::panels::{PanelAction, PanelEvent, PanelHost};
use wandur_core::scripting::session::{CommandOutcome, Effect, ScriptDefinition, SessionScripts};
use wandur_core::scripting::{EventKind, ScriptEvent};
use wandur_core::session::{Drained, SessionConfig, SessionEvent, Waker};
use wandur_core::{Connection, Endpoint};
use wandur_term::{TermSize, Terminal};

/// Palette entry for locally echoed commands and client notices (grey, as the C# client does).
pub const LOCAL_ECHO_COLOR: u8 = 8;

/// Commands kept per session for Up and Down.
const HISTORY_LIMIT: usize = 500;

/// How a new session is set up.
#[derive(Clone, Debug)]
pub struct TabOptions {
    pub name: String,
    /// The saved world this session was opened from.
    pub world: Option<usize>,
    pub scrollback: usize,
    pub echo_commands: bool,
    pub auto_reconnect: bool,
    pub charset: wandur_core::Charset,
    pub prompt_quiet: std::time::Duration,
    /// The character shown with the world's name ("World · Character"): the saved username
    /// until the world names one.
    pub character: String,
    /// The password prompt pattern (it makes input private when it matches the prompt line).
    pub password_prompt: String,
    /// Log in automatically with the world's saved password.
    pub login: Option<LoginConfig>,
    /// Learn words for inline completion (Settings > Input > Suggest completions).
    pub learn_words: bool,
    /// Lua scripts may run (Settings > Input > Allow Lua scripts).
    pub lua_scripts: bool,
    /// The world's protocol mapping (vitals, the opponent card, the reported character), when
    /// there is a valid one for this exact address.
    pub mapping: Option<WorldMapping>,
    /// Where session history goes and how (none: not recorded).
    pub history: Option<HistoryConfig>,
    /// The saved world's channel rules (they run before the codebase family's).
    pub channel_rules: Vec<ChannelRule>,
    /// The world's codebase (picks the channel rule family); empty when unknown.
    pub codebase: String,
    /// Where the session's map is saved (none: kept in memory only).
    pub maps: Option<MapsConfig>,
    /// Room terrain inference: the classifier service, whether it is on, its threshold (none:
    /// this build or run has no classifier).
    pub inference: Option<(Arc<wandur_core::classify::RoomClassificationService>, bool, f64)>,
    /// The world's own theme (from its listing or its saved world), applied while this session
    /// is the active one.
    pub world_theme: Option<wandur_core::directory::WorldTheme>,
}

/// The map store a session loads its world's map from and saves it to.
#[derive(Clone)]
pub struct MapsConfig {
    pub worker: Arc<MapWorker>,
    pub world: MapWorld,
}

impl std::fmt::Debug for MapsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapsConfig")
            .field("world", &self.world)
            .finish_non_exhaustive()
    }
}

/// Who sends a command (walking stops for anything it did not send itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    /// The person, a macro or a script.
    Person,
    /// Automatic login's credentials.
    Login,
    /// A step of a map walk.
    Walk,
    /// The agent's allowed command.
    #[cfg_attr(not(feature = "agent"), allow(dead_code))]
    Agent,
}

/// Session history for a tab: the store, the saved choices and the clock.
#[derive(Clone)]
pub struct HistoryConfig {
    pub store: Arc<dyn HistoryStore>,
    /// Settings > General > Save session history on this device.
    pub enabled: bool,
    /// Days kept (0: forever).
    pub retention_days: u32,
    /// Show the "Session history is saved on this device" reminder when recording starts.
    pub show_notice: bool,
    pub clock: Clock,
}

impl std::fmt::Debug for HistoryConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryConfig")
            .field("enabled", &self.enabled)
            .field("retention_days", &self.retention_days)
            .field("show_notice", &self.show_notice)
            .finish_non_exhaustive()
    }
}

/// The notice strip under the toolbar while this session is shown (the C# controller's notice).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StripNotice {
    /// "Session history is saved on this device", with Don't show again.
    HistoryRecording,
    /// Any other message (recording paused, a preference not saved).
    Text(String),
}

/// What auto-login needs: the username, where the password is, the prompt patterns. The
/// password is read from the vault (off the UI thread) when the connection opens.
#[derive(Clone)]
pub struct LoginConfig {
    pub username: String,
    /// The vault key of the saved password.
    pub key: String,
    pub username_prompt: String,
    pub password_prompt: String,
    pub vault: Arc<dyn PasswordVault>,
}

impl std::fmt::Debug for LoginConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginConfig")
            .field("username", &self.username)
            .field("username_prompt", &self.username_prompt)
            .field("password_prompt", &self.password_prompt)
            .finish_non_exhaustive()
    }
}

/// One connection's automatic login: the password read from the vault and the text handshake,
/// or a GMCP `Char.Login` exchange waiting for its result.
struct LoginAttempt {
    username: String,
    password: String,
    sequence: AutoLoginSequence,
    /// Credentials went over GMCP; waiting for `Char.Login.Result` until this time.
    protocol_deadline: Option<Instant>,
}

impl Default for TabOptions {
    fn default() -> Self {
        Self {
            name: String::new(),
            world: None,
            scrollback: wandur_term::DEFAULT_SCROLLBACK,
            echo_commands: true,
            auto_reconnect: false,
            charset: wandur_core::Charset::Utf8,
            prompt_quiet: wandur_core::prompt::DEFAULT_QUIET,
            character: String::new(),
            password_prompt: wandur_core::login::DEFAULT_PASSWORD_PROMPT.into(),
            login: None,
            learn_words: true,
            lua_scripts: false,
            mapping: None,
            history: None,
            channel_rules: Vec::new(),
            codebase: String::new(),
            maps: None,
            inference: None,
            world_theme: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Connected {
        peer: String,
        secure: bool,
    },
    Closed {
        reason: String,
    },
    Waiting {
        until: Instant,
        attempt: u32,
    },
    /// The offline demo is running (no server).
    Demo,
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Connecting => t(S::Connecting).into(),
            Status::Connected { secure: true, .. } => t(S::ConnectedTLS).into(),
            Status::Connected { .. } => t(S::ConnectedTelnet).into(),
            Status::Closed { reason } => reason.clone(),
            Status::Waiting { until, attempt } => {
                let secs = until.saturating_duration_since(Instant::now()).as_secs_f32().ceil();
                tf(S::ReconnectingInSeconds, &[&format!("{secs:.0}"), attempt])
            }
            Status::Demo => wandur_core::demo::STATUS.into(),
        }
    }

    fn of(state: &ConnectionState) -> Status {
        match state {
            ConnectionState::Connecting => Status::Connecting,
            ConnectionState::Connected { peer, secure } => Status::Connected {
                peer: peer.clone(),
                secure: *secure,
            },
            ConnectionState::Closed { reason } => Status::Closed { reason: reason.clone() },
            ConnectionState::Waiting { until, attempt, .. } => Status::Waiting {
                until: *until,
                attempt: *attempt,
            },
        }
    }
}

pub type SessionId = u64;

/// "World · Character", or the world's name alone.
fn label(name: &str, character: &str) -> String {
    if character.is_empty() {
        name.to_string()
    } else {
        format!("{name} · {character}")
    }
}

/// Where a session's text comes from: a server, or the offline demo world on this machine.
/// One per session, so the size difference between the variants does not matter.
#[allow(clippy::large_enum_variant)]
pub enum Link {
    Net(Connection),
    Demo { world: DemoWorld, running: bool },
}

pub struct SessionTab {
    pub id: SessionId,
    /// The world's name (the demo: the room's).
    name: String,
    /// The character playing, or empty (see [`TabOptions::character`]).
    character: String,
    /// "World · Character", or the world's name alone.
    label: String,
    /// A name the person gave this session (Rename), shown instead of `name`.
    pub custom_name: Option<String>,
    /// The saved world it was opened from, if any (an index into the saved worlds).
    pub world: Option<usize>,
    /// The world's own theme, applied while this session is the active one.
    pub world_theme: Option<wandur_core::directory::WorldTheme>,
    /// Channel traffic for the Channels panel: GMCP channels and lines the rules recognize.
    pub channels: SessionChannels,
    /// The session's map: rooms tracked from GMCP, MSDP and the text, verified walking.
    pub map: MapSession,
    /// Room terrain inference for this map, when the app has a classifier.
    pub inference: Option<crate::map_inference::SessionInference>,
    /// Where the map is saved, and the load and saves in flight.
    maps: Option<MapsConfig>,
    map_load: Option<Receiver<LoadResult>>,
    map_saves: Vec<Receiver<Result<(), String>>>,
    pub endpoint: Endpoint,
    link: Link,
    pub terminal: Terminal,
    drained: Drained,
    notices: Vec<Notice>,
    pub status: Status,
    /// The server echoes, so the input is private (masked, not echoed, not kept in history).
    pub server_echo: bool,
    /// The last prompt looked like a password prompt: mask the next command.
    pub password_prompt: bool,
    /// The prompt line matches the world's password prompt pattern.
    pub prompt_private: bool,
    /// The person turned Private input on (footer padlock, Session menu); off on disconnect.
    pub manual_private: bool,
    /// Server text since the last input, for login and password prompt matching.
    prompt_line: PromptLine,
    password_pattern: wandur_core::login::sequence::Regex,
    login_config: Option<LoginConfig>,
    /// The vault read for this connection's auto-login, while it runs.
    login_read: Option<Receiver<Result<Option<String>, VaultError>>>,
    login: Option<LoginAttempt>,
    /// A GMCP login offer was answered (or a result came): never a second automatic attempt.
    gmcp_login_handled: bool,
    /// A GMCP offer that arrived while the password was still being read.
    pending_offer: Option<bool>,
    /// Credentials auto-login sent (tests).
    pub login_sends: u64,
    pub echo_commands: bool,
    pub input: String,
    history: Vec<String>,
    history_pos: Option<usize>,
    draft: String,
    prompts: PromptTracker,
    pub last_prompt: Option<Prompt>,
    pub prompt_count: u64,
    pub gmcp_enabled: bool,
    pub gmcp_messages: u64,
    pub mssp: Option<MsspTable>,
    /// Lines completed when the tab was last on screen, for the activity marker.
    seen_lines: u64,
    seen_revision: u64,
    /// Characters of server text applied since the session was opened.
    pub chars_received: u64,
    /// Frame number when the tab was last drawn.
    pub last_shown_frame: u64,
    /// Ask the view to focus the input line on its next draw.
    pub focus_input: bool,
    /// Ask the view to put the caret at the end of the input line (it was set from outside).
    pub caret_to_end: bool,
    /// The demo world's answer to the command being sent, and the room it leaves the player in.
    pending_demo: Option<(String, &'static str)>,
    /// The world id the session's macros are saved under, when it has one.
    pub world_id: Option<String>,
    /// The world's macros, as this session runs them.
    pub macros: MacroRuntime,
    /// Matches trigger lines on its own thread (started when the world has triggers).
    trigger_worker: Option<TriggerWorker>,
    /// Wakes the UI (from the trigger worker).
    waker: Option<Waker>,
    lines: Vec<String>,
    macro_out: Vec<String>,
    /// Commands macros sent (tests and the probe).
    pub macro_commands_sent: u64,
    /// The world's scripts, as this session runs them (one script thread, started when the
    /// first script runs).
    pub scripts: SessionScripts,
    /// Typed commands waiting for the scripts' aliases, by ticket.
    pending_commands: std::collections::VecDeque<(u64, String)>,
    script_effects: Vec<Effect>,
    script_outcomes: Vec<CommandOutcome>,
    /// Commands scripts sent (tests and the probe).
    pub script_commands_sent: u64,
    /// The panels the session's scripts declare (the session rail and the vitals strip draw
    /// them).
    pub panels: PanelHost,
    /// Server text applied so far (pieces), so a prompt mark with nothing new raises no event.
    text_pieces: u64,
    prompt_published_at: Option<u64>,
    /// Disconnect was chosen; the link may still report connected until it has closed.
    disconnecting: bool,
    /// Words seen in public lines and sent commands, for inline completion (learned on their
    /// own thread).
    pub completions: Vocabulary,
    learn_words: bool,
    /// Diagnostics, the console, the latest-value cache and the mapped game state.
    pub protocol: SessionProtocol,
    /// Session history: where it goes, and this connection's recorder while it records.
    history_config: Option<HistoryConfig>,
    recorder: Option<HistoryRecorder>,
    /// Recorders of ended connections, still writing their last batch (the app waits at exit).
    recorder_drains: Vec<std::thread::JoinHandle<()>>,
    /// The notice strip's message for this session.
    pub strip: Option<StripNotice>,
    /// The map address the server last named with GMCP `Client.Map`, for the app to take.
    pub client_map: Option<String>,
    /// The game's official map offer ([`crate::official_map`]), once the server named one.
    pub official_map: Option<crate::official_map::OfficialMap>,
    /// The local model agent (goals, runner, the public observation).
    pub agent: crate::agent_session::AgentSession,
    /// Commands the agent sent (tests and the probe).
    pub agent_commands_sent: u64,
}

impl SessionTab {
    pub fn open(id: SessionId, endpoint: Endpoint, options: &TabOptions, waker: Waker) -> Self {
        let mut config = SessionConfig::new(endpoint.clone()).with_charset(options.charset);
        config.window = (100, 40);
        let mut conn = Connection::open(config, Arc::clone(&waker));
        conn.auto_reconnect = options.auto_reconnect;
        let mut tab = Self::with_link(id, endpoint, options, Link::Net(conn));
        tab.scripts = SessionScripts::new(&format!("wandur-scripts {id}"), Arc::clone(&waker));
        tab.waker = Some(waker);
        tab.load_map();
        tab
    }

    /// Read the world's saved map off the UI thread; rooms seen meanwhile wait for it.
    fn load_map(&mut self) {
        let Some(maps) = &self.maps else {
            return;
        };
        self.map.begin_load();
        let waker = self.waker.clone();
        self.map_load = Some(maps.worker.load(
            maps.world.clone(),
            Box::new(move || {
                if let Some(w) = &waker {
                    w();
                }
            }),
        ));
    }

    /// The world the session's map is saved under (none: kept in memory only).
    pub fn map_world(&self) -> Option<&MapWorld> {
        self.maps.as_ref().map(|m| &m.world)
    }

    /// Save the map if it changed (every two seconds at most unless `force`), off the UI thread.
    pub fn save_map(&mut self, force: bool) {
        let Some(maps) = &self.maps else {
            return;
        };
        if let Some(snapshot) = self.map.take_save(Instant::now(), force) {
            self.map_saves.push(maps.worker.save(maps.world.clone(), snapshot));
        }
    }

    /// What walking needs to know of the session.
    pub fn walk_gate(&self) -> WalkGate {
        WalkGate {
            connected: self.link_connected() && !self.disconnecting,
            private: self.private_input(),
            login: self.login_running(),
            remote_echo: self.server_echo,
        }
    }

    /// Walk a planned route (a double click on the map, or Walk route): the first step goes
    /// out now, each next one once the server reports the expected room.
    pub fn walk_route(&mut self, route: &MapRoute) {
        // Walking takes over from the agent (C# `StartMapWalkAsync`).
        self.agent_stop_for_walk();
        let now = Instant::now();
        let gate = self.walk_gate();
        if let Some(command) = self.map.start_walk(route, gate, now) {
            self.send_walk_step(command);
        }
    }

    /// The Stop button.
    pub fn stop_walk(&mut self) {
        self.map.stop_walk(WalkStatus::Stopped);
    }

    fn send_walk_step(&mut self, command: String) {
        if self.send_line_from(&command, Origin::Walk) {
            if self.echo_commands {
                self.terminal.feed_local(&format!("{command}\n"), LOCAL_ECHO_COLOR);
            }
            self.flush_demo();
            self.terminal.scroll_to_bottom();
        } else {
            self.map.stop_walk(WalkStatus::SendFailed);
        }
    }

    /// The map's part of a pump: its load, its saves, the next walking step.
    fn follow_map(&mut self, now: Instant) {
        if let Some(load) = &self.map_load {
            match load.try_recv() {
                Ok(result) => {
                    self.map_load = None;
                    let gate = self.walk_gate();
                    match result {
                        Ok(map) => self.map.finish_load(map, gate, now),
                        Err(_) => {
                            self.map.finish_load(None, gate, now);
                            self.strip = Some(StripNotice::Text(t(S::MapCacheFailed).into()));
                        }
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.map_load = None;
                    let gate = self.walk_gate();
                    self.map.finish_load(None, gate, now);
                }
            }
        }
        let mut failed = false;
        self.map_saves.retain(|save| match save.try_recv() {
            Ok(result) => {
                failed |= result.is_err();
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => false,
        });
        if failed {
            self.strip = Some(StripNotice::Text(t(S::MapCacheFailed).into()));
        }
        let gate = self.walk_gate();
        if let Some(command) = self.map.advance_walk(gate, now) {
            self.send_walk_step(command);
        }
        if let Some(inference) = &mut self.inference {
            inference.follow(&mut self.map, self.waker.as_ref());
        }
        self.save_map(false);
    }

    /// The offline demo world (File > Open Offline Demo): no server, the C# rooms and commands.
    pub fn demo(id: SessionId, options: &TabOptions) -> Self {
        let mut world = DemoWorld::new();
        let opening = world.start();
        let endpoint = Endpoint::new("offline-demo", 0);
        let mut tab = Self::with_link(
            id,
            endpoint,
            options,
            Link::Demo {
                world: DemoWorld::new(),
                running: true,
            },
        );
        tab.set_name(world.room_name());
        if let Link::Demo { world: w, .. } = &mut tab.link {
            *w = world;
        }
        tab.terminal.feed(opening.as_bytes());
        let walk = tab.walk_gate();
        tab.map.track_output(&opening, walk, Instant::now());
        tab.status = Status::Demo;
        tab.begin_history();
        tab.record_received(&opening);
        tab.observe_words(&opening);
        tab.completions.flush();
        tab
    }

    fn with_link(id: SessionId, endpoint: Endpoint, options: &TabOptions, link: Link) -> Self {
        let size = TermSize::new(100, 40);
        let name = if options.name.is_empty() {
            endpoint.to_string()
        } else {
            options.name.clone()
        };
        let character = options.character.trim().chars().take(100).collect::<String>();
        let password_pattern = wandur_core::login::sequence::compile(&options.password_prompt)
            .or_else(|_| wandur_core::login::sequence::compile(wandur_core::login::DEFAULT_PASSWORD_PROMPT))
            .expect("the default password prompt compiles");
        let mut tab = Self {
            id,
            label: label(&name, &character),
            name,
            character,
            custom_name: None,
            world: options.world,
            world_theme: options.world_theme.clone(),
            channels: SessionChannels::new(options.channel_rules.clone(), &options.codebase),
            map: MapSession::new(),
            maps: options.maps.clone(),
            inference: options.inference.as_ref().map(|(service, on, threshold)| {
                crate::map_inference::SessionInference::new(Arc::clone(service), *on, *threshold)
            }),
            map_load: None,
            map_saves: Vec::new(),
            endpoint,
            link,
            terminal: Terminal::new(size, options.scrollback),
            drained: Drained::default(),
            notices: Vec::new(),
            status: Status::Connecting,
            server_echo: false,
            password_prompt: false,
            prompt_private: false,
            manual_private: false,
            prompt_line: PromptLine::default(),
            password_pattern,
            login_config: options.login.clone(),
            login_read: None,
            login: None,
            gmcp_login_handled: false,
            pending_offer: None,
            login_sends: 0,
            echo_commands: options.echo_commands,
            input: String::new(),
            history: Vec::new(),
            history_pos: None,
            draft: String::new(),
            prompts: PromptTracker::new(options.prompt_quiet),
            last_prompt: None,
            prompt_count: 0,
            gmcp_enabled: false,
            gmcp_messages: 0,
            mssp: None,
            seen_lines: 0,
            seen_revision: 0,
            chars_received: 0,
            last_shown_frame: 0,
            focus_input: true,
            caret_to_end: false,
            pending_demo: None,
            world_id: None,
            macros: MacroRuntime::default(),
            trigger_worker: None,
            waker: None,
            lines: Vec::new(),
            macro_out: Vec::new(),
            macro_commands_sent: 0,
            scripts: {
                let mut scripts = SessionScripts::new(&format!("wandur-scripts {id}"), Arc::new(|| {}));
                scripts.set_lua_enabled(options.lua_scripts, Instant::now(), &|| None);
                scripts
            },
            pending_commands: std::collections::VecDeque::new(),
            script_effects: Vec::new(),
            script_outcomes: Vec::new(),
            script_commands_sent: 0,
            panels: PanelHost::default(),
            text_pieces: 0,
            prompt_published_at: None,
            disconnecting: false,
            completions: Vocabulary::spawn(&format!("wandur-words {id}")),
            learn_words: options.learn_words,
            protocol: SessionProtocol::new(options.mapping.clone()),
            history_config: options.history.clone(),
            recorder: None,
            recorder_drains: Vec::new(),
            strip: None,
            client_map: None,
            official_map: None,
            agent: Default::default(),
            agent_commands_sent: 0,
        };
        tab.follow_lines();
        tab
    }

    /// What the session is doing, as far as macros care. Read from the link, so it is current
    /// in the middle of applying output.
    pub fn macro_gate(&self) -> Gate {
        Gate {
            connected: !self.disconnecting && self.link_connected(),
            private: self.private_input(),
            login: self.login_running(),
        }
    }

    /// Adopt the world's saved macros (a new session, or the world editor saved).
    pub fn set_macros(&mut self, world_id: Option<String>, library: Vec<SavedMacro>, now: Instant) {
        self.world_id = world_id;
        let gate = self.macro_gate();
        let has_triggers = library
            .iter()
            .any(|m| m.enabled && m.definition.kind == wandur_core::macros::MacroKind::Trigger);
        if has_triggers && self.trigger_worker.is_none() {
            let wake: wandur_core::macros::worker::Wake = match &self.waker {
                Some(w) => Arc::clone(w),
                None => Arc::new(|| {}),
            };
            // Without a thread, triggers match on this thread (see run_triggers).
            self.trigger_worker = TriggerWorker::spawn(&format!("wandur-triggers {}", self.id), wake).ok();
        }
        if let Some(worker) = &self.trigger_worker {
            worker.load(library.clone());
        }
        self.macros.reload(library, now, gate);
        self.follow_lines();
    }

    /// Adopt the world's saved scripts (a new session, or Scripts > Reload saved rules): every
    /// script stops and the enabled ones start again.
    pub fn set_scripts(&mut self, scripts: Vec<ScriptDefinition>, now: Instant) {
        self.scripts.set_library(scripts);
        self.consume_pending_commands();
        self.run_scripts(now);
    }

    /// A panel widget's callback (a click, a toggle, a submitted input, a list choice) for the
    /// script that declared the panel. Privacy, rate limits and the send policy still apply.
    pub fn panel_event(&mut self, script: &str, panel: &str, widget: &str, event: PanelEvent, text: Option<&str>) {
        let Some(json) = self.panels.event(script, panel, widget, event, text) else {
            return;
        };
        let now = Instant::now();
        self.scripts
            .publish_to(script, ScriptEvent::new(EventKind::Panel, json), now);
        self.run_scripts(now);
    }

    /// A pack script's send policy changed (Save world in the editor): a running script starts
    /// again under it.
    pub fn set_script_send_policy(&mut self, id: &str, restricted: bool, now: Instant) {
        let protocol = &self.protocol;
        self.scripts
            .set_restricted_send(id, restricted, now, &|| protocol.cache.seed_json());
        self.run_scripts(now);
    }

    /// Allow or stop Lua scripts (the preference): the enabled Lua scripts start or stop.
    pub fn set_lua_scripts(&mut self, on: bool, now: Instant) {
        let protocol = &self.protocol;
        self.scripts.set_lua_enabled(on, now, &|| protocol.cache.seed_json());
        self.run_scripts(now);
    }

    /// The Scripts menu's switch for one script: this session only, not saved.
    pub fn switch_script(&mut self, id: &str, on: bool, now: Instant) {
        let protocol = &self.protocol;
        self.scripts.set_enabled(id, on, now, &|| protocol.cache.seed_json());
        self.run_scripts(now);
    }

    /// Follow the session's state, run script timers and carry out what the scripts answered.
    fn run_scripts(&mut self, now: Instant) {
        let gate = self.macro_gate();
        let protocol = &self.protocol;
        let reports = self.scripts.update(gate, now, &|| protocol.cache.seed_json());
        self.script_effects.extend(reports);
        self.scripts.tick(now);
        let mut effects = std::mem::take(&mut self.script_effects);
        let mut outcomes = std::mem::take(&mut self.script_outcomes);
        self.scripts.poll(now, &mut effects, &mut outcomes);
        for effect in effects.drain(..) {
            self.script_effect(effect);
        }
        for outcome in outcomes.drain(..) {
            self.command_answered(outcome, now);
        }
        self.script_effects = effects;
        self.script_outcomes = outcomes;
        self.send_script_reports();
        // A stopped script keeps no panels (a restart declares them again).
        if !self.panels.is_empty() {
            let scripts = &self.scripts;
            self.panels
                .retain_scripts(|id| scripts.entry(id).is_some_and(|e| e.is_running()));
        }
        if !self.scripts.is_active() {
            self.consume_pending_commands();
        }
        self.follow_lines();
    }

    fn script_effect(&mut self, effect: Effect) {
        match effect {
            Effect::Send { command, .. } => {
                let gate = self.macro_gate();
                if !gate.connected || gate.private || gate.login || !self.send_line(&command) {
                    return;
                }
                self.script_commands_sent += 1;
                if self.echo_commands {
                    self.terminal.feed_local(&format!("{command}\n"), LOCAL_ECHO_COLOR);
                }
                self.flush_demo();
            }
            Effect::Echo { text, .. } => {
                let lead = if self.terminal.at_line_start() { "" } else { "\n" };
                self.terminal
                    .feed_local(&format!("{lead}{}{text}\n", t(S::ScriptOutputPrefix)), LOCAL_ECHO_COLOR);
                let gate = self.protocol_gate();
                let secrets = self.protocol.secrets().to_vec();
                self.protocol.console.append(
                    wandur_core::diagnostics::console::ConsoleKind::Script,
                    &text,
                    gate.blocked(),
                    &secrets,
                );
                if let Some(recorder) = &mut self.recorder {
                    recorder.script(&text, gate.blocked());
                }
            }
            Effect::Report(name) => self.protocol.request_script_report(&name),
            Effect::Panel { script, json } => match PanelAction::parse(&json) {
                Ok(action) => {
                    self.panels.apply(&script, action, Instant::now());
                }
                // Malformed data is the script's error (C# `ScriptPanelRejected`).
                Err(error) => self.scripts.reject(&script, &tf(S::ScriptPanelRejected, &[&error])),
            },
        }
    }

    /// Ask the world for the MSDP variables scripts read.
    fn send_script_reports(&mut self) {
        let gate = self.protocol_gate();
        let connected = self.link_connected();
        let requests = self.protocol.take_script_reports(gate, connected);
        if requests.is_empty() {
            return;
        }
        if let Link::Net(conn) = &self.link
            && let Some(session) = conn.session()
        {
            for (option, payload) in requests {
                session.send_subnegotiation(option, &payload);
            }
        }
    }

    /// The scripts answered a typed command: an alias took it, or it goes out as typed.
    fn command_answered(&mut self, outcome: CommandOutcome, now: Instant) {
        let Some(at) = self.pending_commands.iter().position(|(t, _)| *t == outcome.ticket) else {
            return;
        };
        // Answers come in the order commands were typed: one still waiting before this one lost
        // its answer (the script thread failed), and is consumed as C# consumes it.
        self.pending_commands.drain(..at);
        let (_, line) = self.pending_commands.pop_front().expect("found above");
        if !outcome.consumed {
            self.send_as_typed(line, now);
        }
    }

    /// Commands still waiting when the scripts stopped or reloaded are dropped: as in C#, a
    /// command that can no longer be judged is consumed, never sent as typed.
    fn consume_pending_commands(&mut self) {
        self.pending_commands.clear();
    }

    /// Server text for scripts that wait for prompts: a public GA or EOR prompt with new text.
    fn publish_prompt(&mut self, prompt: &Prompt, now: Instant) {
        if prompt.source != PromptSource::Mark
            || prompt.password
            || self.prompt_private
            || self.macro_gate().private
            || self.login_running()
            || self.prompt_published_at == Some(self.text_pieces)
            || !self.scripts.wants_events()
        {
            return;
        }
        self.prompt_published_at = Some(self.text_pieces);
        self.scripts
            .publish(ScriptEvent::new(EventKind::Prompt, prompt.text.as_str()), now);
    }

    /// The footer's Macros switch: this session only, not saved.
    pub fn switch_macros(&mut self, on: bool, now: Instant) {
        let gate = self.macro_gate();
        self.macros.switch(on, now, gate);
        self.follow_lines();
    }

    /// Learn words for completion or not (Settings > Input). Off keeps what was learned; the
    /// composer offers nothing while the setting is off.
    pub fn set_learning(&mut self, on: bool) {
        self.learn_words = on;
    }

    /// Line events are on while triggers read them.
    fn follow_lines(&mut self) {
        self.terminal
            .set_line_events(self.macros.wants_lines() || self.scripts.is_active());
    }

    /// Server text as applied to the grid: completion learns it unless input is private or a
    /// login runs (then the line it belongs to is not learned).
    fn observe_words(&mut self, text: &str) {
        if !self.learn_words || text.is_empty() {
            return;
        }
        let gate = self.macro_gate();
        if gate.private || gate.login {
            self.completions.interrupt();
        } else {
            self.completions.observe(text);
        }
    }

    /// Server text as applied to the grid: public lines are classified for the Channels panel;
    /// private input or a running login drops the line in flight (C#: the same gate as the
    /// agent feed).
    fn observe_channels(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let gate = self.macro_gate();
        if gate.private || gate.login {
            self.channels.reset();
        } else {
            self.channels.receive_text(text, std::time::SystemTime::now());
        }
    }

    /// The saved world's channel rules or codebase changed (taught, or edited in the world
    /// editor): they apply from the next line.
    pub fn set_channel_rules(&mut self, rules: &[ChannelRule], codebase: &str) {
        self.channels.configure(rules, codebase);
    }

    /// The newest non-empty transcript lines, plain, oldest first (the teaching preview).
    pub fn recent_lines(&self, count: usize) -> Vec<String> {
        let transcript = self.terminal.transcript();
        let mut lines: Vec<String> = transcript
            .lines()
            .rev()
            .map(|l| wandur_core::channels::rules::strip(l).trim_end().to_string())
            .filter(|l| !l.is_empty())
            .take(count)
            .collect();
        lines.reverse();
        lines
    }

    /// Complete lines from the last output go to the triggers (on the worker thread), unless
    /// input is private (then they are dropped unseen).
    fn run_triggers(&mut self, now: Instant) {
        let macros = self.macros.wants_lines();
        if !macros && !self.scripts.is_active() {
            return;
        }
        self.terminal.take_line_texts(&mut self.lines);
        if self.lines.is_empty() {
            return;
        }
        let gate = self.macro_gate();
        if gate.private || gate.login {
            self.terminal.recycle_texts(&mut self.lines);
            return;
        }
        self.scripts.feed_lines(&self.lines, now);
        if !macros {
            self.terminal.recycle_texts(&mut self.lines);
            return;
        }
        let generation = self.macros.generation();
        let lines = std::mem::take(&mut self.lines);
        let refused = match &self.trigger_worker {
            Some(worker) => worker.submit(generation, lines),
            None => Err(Refused::Gone(lines)),
        };
        match refused {
            Ok(()) => {}
            Err(Refused::Full(mut lines)) => {
                self.macros.stop_triggers(t(S::ScriptQueueOverflow));
                self.terminal.recycle_texts(&mut lines);
            }
            Err(Refused::Gone(mut lines)) => {
                let mut out = std::mem::take(&mut self.macro_out);
                for line in &lines {
                    self.macros.on_line(line, now, gate, &mut out);
                }
                self.terminal.recycle_texts(&mut lines);
                self.send_from_macros(&mut out);
                self.macro_out = out;
            }
        }
    }

    /// Send the commands of triggers the worker matched since the last call.
    fn take_trigger_results(&mut self, now: Instant) {
        let Some(worker) = &self.trigger_worker else { return };
        let mut matched = Vec::new();
        while let Some(m) = worker.poll() {
            matched.push(m);
        }
        if matched.is_empty() {
            return;
        }
        let mut out = std::mem::take(&mut self.macro_out);
        for mut m in matched {
            let gate = self.macro_gate();
            self.macros.on_hits(m.generation, &m.hits, now, gate, &mut out);
            self.terminal.recycle_texts(&mut m.lines);
            // Keep the emptied buffer for the next batch.
            if self.lines.capacity() < m.lines.capacity() {
                self.lines = m.lines;
            }
        }
        self.send_from_macros(&mut out);
        self.macro_out = out;
    }

    /// Send what macros produced, as the C# script path does: echoed (unless private), never in
    /// the history, and never while input is private or the session is not connected.
    fn send_from_macros(&mut self, commands: &mut Vec<String>) {
        for command in commands.drain(..) {
            let gate = self.macro_gate();
            if !gate.connected || gate.private || gate.login {
                break;
            }
            if !self.send_line(&command) {
                break;
            }
            self.macro_commands_sent += 1;
            if self.echo_commands {
                self.terminal.feed_local(&format!("{command}\n"), LOCAL_ECHO_COLOR);
            }
            self.flush_demo();
        }
        commands.clear();
    }

    /// Follow the connection and run due timers.
    fn run_macro_timers(&mut self, now: Instant) {
        self.take_trigger_results(now);
        let gate = self.macro_gate();
        let mut out = std::mem::take(&mut self.macro_out);
        self.macros.tick(now, gate, &mut out);
        self.send_from_macros(&mut out);
        self.macro_out = out;
        self.follow_lines();
    }

    /// A function key in the command input. Returns whether a shortcut took it.
    pub fn press_key(&mut self, key: &str, now: Instant) -> bool {
        let gate = self.macro_gate();
        match self.macros.on_key(key, now, gate) {
            Some(mut commands) => {
                self.send_from_macros(&mut commands);
                self.terminal.scroll_to_bottom();
                true
            }
            None => false,
        }
    }

    /// Apply everything the network delivered since the last call, run the reconnect and prompt
    /// timers, and return the characters of server text applied. Cheap when nothing happened.
    pub fn pump(&mut self, now: Instant) -> usize {
        let Link::Net(conn) = &mut self.link else {
            self.follow_history();
            self.follow_map(now);
            self.agent_poll(now);
            return 0;
        };
        conn.drain(now, &mut self.drained, &mut self.notices);
        let mut chars = 0;
        if !self.drained.is_empty() {
            let drained = std::mem::take(&mut self.drained);
            if let Some(recorder) = &mut self.recorder {
                recorder.set_secrets(self.protocol.secrets());
            }
            let mut at = 0;
            for (offset, event) in &drained.events {
                let gate = self.protocol_gate();
                self.record_received(&drained.text[at..*offset]);
                self.protocol.receive_text(&drained.text[at..*offset], gate);
                self.terminal.feed(&drained.text.as_bytes()[at..*offset]);
                self.prompt_line.push(&drained.text[at..*offset]);
                self.observe_words(&drained.text[at..*offset]);
                self.observe_channels(&drained.text[at..*offset]);
                self.observe_agent(&drained.text[at..*offset]);
                let walk = self.walk_gate();
                self.map.track_output(&drained.text[at..*offset], walk, now);
                if *offset > at {
                    self.text_pieces += 1;
                }
                // Lines are judged with the privacy that held when they arrived.
                self.run_triggers(now);
                at = *offset;
                self.apply(event);
                // Scripts see a privacy change where it happened in the stream.
                let gate = self.macro_gate();
                let protocol = &self.protocol;
                let reports = self.scripts.update(gate, now, &|| protocol.cache.seed_json());
                self.script_effects.extend(reports);
            }
            let gate = self.protocol_gate();
            self.record_received(&drained.text[at..]);
            self.protocol.receive_text(&drained.text[at..], gate);
            self.terminal.feed(&drained.text.as_bytes()[at..]);
            self.prompt_line.push(&drained.text[at..]);
            self.observe_words(&drained.text[at..]);
            self.observe_channels(&drained.text[at..]);
            self.observe_agent(&drained.text[at..]);
            let walk = self.walk_gate();
            self.map.track_output(&drained.text[at..], walk, now);
            if drained.text.len() > at {
                self.text_pieces += 1;
            }
            self.completions.flush();
            self.run_triggers(now);
            if !drained.text.is_empty() {
                self.prompts.on_output(now, !self.terminal.at_line_start());
            }
            chars = drained.text.chars().count();
            self.chars_received += chars as u64;
            self.drained = drained;
            self.refresh_prompt_private();
            self.follow_reported_character();
            self.agent_privacy();
        }
        self.run_login(now);
        self.refresh_subscriptions();
        let Link::Net(conn) = &mut self.link else {
            return chars;
        };
        if conn.poll(now, &mut self.notices) {
            self.new_connection();
        }
        for notice in std::mem::take(&mut self.notices) {
            let text = match notice {
                Notice::RetryScheduled { delay, attempt } => tf(
                    S::ReconnectingInSeconds,
                    &[&format!("{:.0}", delay.as_secs_f32().ceil()), &attempt],
                ),
                Notice::Reconnecting { attempt } => tf(S::ReconnectingToAttempt, &[&self.endpoint, &attempt]),
                Notice::GaveUp { attempts } => tf(S::GaveUpReconnecting, &[&attempts]),
            };
            self.notice(&text);
        }
        if self.prompts.poll(now) {
            self.prompt(PromptSource::Quiet);
        }
        self.status = self.link_status();
        self.run_macro_timers(now);
        self.run_scripts(now);
        self.follow_history();
        self.follow_map(now);
        self.agent_poll(now);
        chars
    }

    fn link_status(&self) -> Status {
        match &self.link {
            Link::Net(conn) => Status::of(conn.state()),
            Link::Demo { running: true, .. } => Status::Demo,
            Link::Demo { running: false, .. } => Status::Closed {
                reason: wandur_core::demo::ENDED.into(),
            },
        }
    }

    /// Connected to a server, or the demo is running (without building a status).
    fn link_connected(&self) -> bool {
        match &self.link {
            Link::Net(conn) => matches!(conn.state(), ConnectionState::Connected { .. }),
            Link::Demo { running, .. } => *running,
        }
    }

    /// Whether this is the offline demo.
    pub fn is_demo(&self) -> bool {
        matches!(self.link, Link::Demo { .. })
    }

    /// Whether a lost connection reconnects by itself.
    pub fn auto_reconnect(&self) -> bool {
        matches!(&self.link, Link::Net(conn) if conn.auto_reconnect)
    }

    /// Send a line to the server, or to the demo world (whose answer lands in the grid at once).
    /// Prompt matching starts again from the text after it.
    fn send_line(&mut self, line: &str) -> bool {
        self.send_line_from(line, Origin::Person)
    }

    fn send_line_from(&mut self, line: &str, origin: Origin) -> bool {
        // While the agent has control nothing else sends (the C# `_agentOwnsControl`).
        if self.agent.has_control() && origin != Origin::Agent {
            return false;
        }
        // Any command but the walk's own takes over from a walk (C# `MapWalkManualCommand`).
        if origin != Origin::Walk {
            self.map.stop_walk(WalkStatus::ManualCommand);
        }
        // The console logs it with the privacy it was sent under; a private input is remembered
        // so its echo is masked in Diagnostics.
        let gate = self.protocol_gate();
        if origin != Origin::Login {
            self.map.track_command(line, gate.private, Instant::now());
        }
        if !self.is_demo() {
            if gate.private {
                self.protocol.remember_secret(line);
            }
            self.protocol.sent(line, gate.private, gate);
        }
        if let Some(recorder) = &mut self.recorder {
            recorder.set_secrets(self.protocol.secrets());
            recorder.sent(line, gate.blocked());
        }
        self.prompt_line.clear();
        self.prompt_private = false;
        match &mut self.link {
            Link::Net(conn) => conn.send_line(line),
            Link::Demo { running: false, .. } => false,
            Link::Demo { world, .. } => {
                let reply = world.command(line);
                let name = world.room_name();
                self.pending_demo = Some((reply, name));
                true
            }
        }
    }

    /// The demo's answer goes after the local echo, as a server's would.
    fn flush_demo(&mut self) {
        if let Some((reply, name)) = self.pending_demo.take() {
            self.record_received(&reply);
            self.terminal.feed(reply.as_bytes());
            let walk = self.walk_gate();
            self.map.track_output(&reply, walk, Instant::now());
            self.set_name(name);
            self.observe_agent(&reply);
            // The demo's lines teach completion as a server's do.
            self.observe_words(&reply);
            self.completions.flush();
        }
    }

    /// When the tab needs a frame even without input or output (a reconnect or a prompt timer).
    pub fn deadline(&self) -> Option<Instant> {
        let scripts = self.scripts.deadline();
        let save = self.maps.as_ref().and(self.map.save_deadline());
        let map = [self.map.walk_deadline(), save, self.agent.deadline()]
            .into_iter()
            .flatten()
            .min();
        let Link::Net(conn) = &self.link else {
            return [self.macros.deadline(), scripts, map].into_iter().flatten().min();
        };
        let login = self
            .login
            .as_ref()
            .map(|l| l.protocol_deadline.unwrap_or_else(|| l.sequence.deadline()));
        [
            conn.deadline(),
            self.prompts.deadline(),
            self.macros.deadline(),
            login,
            scripts,
            map,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// A client message in the transcript, on its own line.
    fn notice(&mut self, text: &str) {
        let lead = if self.terminal.at_line_start() { "" } else { "\n" };
        self.terminal.feed_local(&format!("{lead}[{text}]\n"), LOCAL_ECHO_COLOR);
    }

    fn new_connection(&mut self) {
        // Every connection is a history session of its own.
        self.end_history();
        self.prompts.reset();
        self.end_private_and_login();
        self.gmcp_enabled = false;
        self.mssp = None;
        self.protocol.connected();
        // Every connection starts with a fresh agent memory (C# `Configure` at start).
        self.agent_new_connection();
    }

    /// The session's privacy as protocol data sees it.
    pub fn protocol_gate(&self) -> ProtocolGate {
        ProtocolGate {
            private: self.private_input(),
            login: self.login_running(),
        }
    }

    /// A world that reports its character's name through the mapping names the session, as in
    /// C# (it wins over the saved username).
    fn follow_reported_character(&mut self) {
        if let Some(name) = self.protocol.reported_character()
            && name != self.character
        {
            self.character = name;
            self.label = label(&self.name, &self.character);
        }
    }

    /// Ask for the mapped variables again once a private or login stretch is over.
    fn refresh_subscriptions(&mut self) {
        let gate = self.protocol_gate();
        let connected = self.link_connected();
        let requests = self.protocol.take_refresh(gate, connected);
        if requests.is_empty() {
            return;
        }
        if let Link::Net(conn) = &self.link
            && let Some(session) = conn.session()
        {
            for (option, payload) in requests {
                session.send_subnegotiation(option, &payload);
            }
        }
    }

    /// A refreshed protocol mapping for this session (the directory reloaded).
    pub fn set_mapping(&mut self, mapping: WorldMapping) {
        if mapping.is_for(&self.endpoint) {
            self.protocol.set_mapping(mapping);
        }
    }

    fn prompt(&mut self, source: PromptSource) {
        let prompt = Prompt::new(self.terminal.current_line(), source);
        if prompt.text.trim().is_empty() {
            return;
        }
        self.password_prompt = prompt.password;
        self.prompt_count += 1;
        self.publish_prompt(&prompt, Instant::now());
        self.last_prompt = Some(prompt);
    }

    fn apply(&mut self, event: &SessionEvent) {
        match event {
            SessionEvent::Connected { .. } => {
                self.map.connection_started();
                self.new_connection();
                self.begin_history();
                self.start_login();
                self.focus_input = true;
            }
            SessionEvent::Closed { reason } => {
                self.agent_reset(AgentStop::Paused);
                self.map.stop_walk(WalkStatus::Disconnected);
                self.save_map(true);
                self.end_history();
                self.end_private_and_login();
                let reason = reason.clone();
                self.notice(&reason);
            }
            SessionEvent::ServerEcho(on) => {
                self.server_echo = *on;
                if *on {
                    // Even a burst that ends at once never reaches the agent (C#
                    // `PrivateIntervalReceived`).
                    self.agent_reset(AgentStop::Paused);
                    // A private stretch began: a walk never sends into it, even when it ends
                    // within the same burst.
                    self.map.stop_walk(WalkStatus::Private);
                    // Never keep a typed password in a visible draft from before.
                    self.history_pos = None;
                }
            }
            SessionEvent::PromptMark => {
                self.map.prompt_seen(Instant::now());
                self.prompts.on_mark();
                self.prompt(PromptSource::Mark);
            }
            SessionEvent::GmcpEnabled => {
                self.gmcp_enabled = true;
                self.protocol.gmcp_enabled = true;
                self.map.set_gmcp(OptionState::Enabled);
            }
            SessionEvent::MsdpEnabled => {
                self.protocol.msdp_enabled = true;
                self.map.set_msdp(OptionState::Enabled);
            }
            SessionEvent::GmcpDisabled => self.map.set_gmcp(OptionState::Disabled),
            SessionEvent::MsdpDisabled => self.map.set_msdp(OptionState::Disabled),
            SessionEvent::Msdp(payload) => {
                if let Some(room) = wandur_core::map::decode::from_msdp(payload) {
                    let walk = self.walk_gate();
                    self.map.observe_room(room, walk, Instant::now());
                }
                let gate = self.protocol_gate();
                self.protocol.receive_msdp(payload, gate, std::time::SystemTime::now());
                if !gate.blocked() {
                    self.agent.feed_msdp(payload);
                }
                if !gate.blocked()
                    && self.scripts.wants_events()
                    && !self.protocol.carries_secret_text(&String::from_utf8_lossy(payload))
                {
                    let now = Instant::now();
                    for (variable, json) in wandur_core::protocol::state::msdp_values(payload) {
                        self.scripts.publish(ScriptEvent::msdp(&variable, &json), now);
                    }
                }
            }
            SessionEvent::Gmcp(message) if gmcp_login::is_private(&message.package) => {
                // Login frames go to login handling (and Diagnostics, redacted) only: never
                // channels, the map or scripts.
                let gate = self.protocol_gate();
                self.protocol.receive_gmcp(message, gate, std::time::SystemTime::now());
                self.gmcp_messages += 1;
                match gmcp_login::decode(message) {
                    Some(GmcpLogin::Offer { password_supported }) => self.gmcp_login_offer(password_supported),
                    Some(GmcpLogin::Result { success }) => self.gmcp_login_result(success),
                    None => {}
                }
            }
            SessionEvent::Gmcp(message) => {
                let gate = self.protocol_gate();
                self.protocol.receive_gmcp(message, gate, std::time::SystemTime::now());
                self.gmcp_messages += 1;
                // Channel messages are public output only, as in C#.
                let public = !gate.private && !gate.login;
                if public && self.scripts.wants_events() {
                    let raw = String::from_utf8_lossy(&message.raw);
                    if !self.protocol.carries_secret_text(&raw) {
                        self.scripts
                            .publish(ScriptEvent::new(EventKind::Gmcp, raw.trim()), Instant::now());
                    }
                }
                if public {
                    self.channels.receive_gmcp(message, std::time::SystemTime::now());
                    self.agent.feed_gmcp(message);
                }
                // The game's official map (the app offers it), whatever the privacy, as rooms.
                if let Some(url) = wandur_core::map::official::client_map_url(message) {
                    self.client_map = Some(url);
                }
                // Rooms update the map whatever the privacy, as in C#.
                if let Some(room) = wandur_core::map::decode::from_gmcp(message) {
                    let walk = self.walk_gate();
                    self.map.observe_room(room, walk, Instant::now());
                }
            }
            SessionEvent::Mssp(table) => {
                // A table from a private stretch is skipped entirely, as in C#.
                let gate = self.protocol_gate();
                if self.protocol.receive_mssp(table, gate, std::time::SystemTime::now()) {
                    // Its CODEBASE picks the channel family while the world names none.
                    if let Some(codebase) = table.first("CODEBASE") {
                        self.channels.use_server_codebase(codebase);
                    }
                    self.mssp = Some(table.clone());
                }
            }
            SessionEvent::Subnegotiation { .. } => {}
        }
    }

    /// The name shown in tabs, the Workspace list and the status bar: "World · Character" (or a
    /// name the person gave the session).
    pub fn title(&self) -> &str {
        self.custom_name.as_deref().unwrap_or(&self.label)
    }

    /// The world's name (or the session's own name, when renamed), without the character.
    pub fn world_name(&self) -> &str {
        self.custom_name.as_deref().unwrap_or(&self.name)
    }

    /// The character playing, or "".
    pub fn character(&self) -> &str {
        &self.character
    }

    /// The window title while this session is shown: "Character · World · App", as in C#.
    pub fn window_title(&self, app: &str) -> String {
        if self.character.is_empty() {
            format!("{} · {app}", self.world_name())
        } else {
            format!("{} · {} · {app}", self.character, self.world_name())
        }
    }

    fn set_name(&mut self, name: &str) {
        if self.name != name {
            self.name = name.to_string();
            self.label = label(&self.name, &self.character);
        }
    }

    /// Private input off, auto-login stopped (a new connection, or the old one closed).
    fn end_private_and_login(&mut self) {
        self.server_echo = false;
        self.password_prompt = false;
        self.prompt_private = false;
        self.manual_private = false;
        self.prompt_line.clear();
        self.stop_login();
        self.gmcp_login_handled = false;
        self.pending_offer = None;
    }

    /// Whether automatic login is running (the password is being read, or being sent): macros
    /// stay quiet and no line reaches them.
    pub fn login_running(&self) -> bool {
        self.login.is_some() || self.login_read.is_some()
    }

    fn stop_login(&mut self) {
        self.login = None;
        self.login_read = None;
    }

    /// A connection opened: read the saved password for auto-login, on its own thread (the
    /// vault may block, or ask the person to unlock it).
    fn start_login(&mut self) {
        let Some(config) = self.login_config.clone() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let waker = self.waker.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("wandur-vault {}", self.id))
            .spawn(move || {
                let _ = tx.send(config.vault.read(&config.key));
                if let Some(wake) = waker {
                    wake();
                }
            });
        match spawned {
            Ok(_) => self.login_read = Some(rx),
            Err(_) => self.notice(t(S::CouldNotReadTheSavedPasswordUnlockYourSystem)),
        }
    }

    /// Follow auto-login: take the vault's answer, answer a GMCP offer that waited for it, end a
    /// GMCP login the server never confirmed, and answer the current prompt.
    fn run_login(&mut self, now: Instant) {
        if let Some(rx) = &self.login_read {
            let read = match rx.try_recv() {
                Ok(read) => Some(read),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(VaultError(String::new()))),
            };
            if let Some(read) = read {
                self.login_read = None;
                self.password_read(read, now);
                if let Some(supported) = self.pending_offer.take() {
                    self.gmcp_login_offer(supported);
                }
            }
        }
        let Some(login) = &self.login else { return };
        match login.protocol_deadline {
            Some(deadline) if now >= deadline => {
                self.login = None;
                self.notice(t(S::GmcpLoginTimedOut));
            }
            Some(_) => {}
            None => self.try_auto_login(now),
        }
    }

    fn password_read(&mut self, read: Result<Option<String>, VaultError>, now: Instant) {
        let Some(config) = &self.login_config else { return };
        let password = match read {
            Ok(Some(password)) if wandur_core::login::vault::validate_password(&password).is_ok() => password,
            Ok(None) => {
                self.notice(t(S::SavedPasswordWasnTFoundLogInManuallyOr));
                return;
            }
            _ => {
                self.notice(t(S::CouldNotReadTheSavedPasswordUnlockYourSystem));
                return;
            }
        };
        self.protocol.remember_secret(&password);
        let sequence = AutoLoginSequence::new(&config.username_prompt, &config.password_prompt, now)
            .unwrap_or_else(|_| AutoLoginSequence::with_defaults(now));
        self.login = Some(LoginAttempt {
            username: config.username.clone(),
            password,
            sequence,
            protocol_deadline: None,
        });
    }

    /// The text handshake: send the username or password the current prompt asks for. Neither
    /// is echoed or kept in the history; the transcript only moves to a new line.
    fn try_auto_login(&mut self, now: Instant) {
        if !self.link_connected() {
            return;
        }
        let Some(login) = &mut self.login else { return };
        if login.protocol_deadline.is_some() {
            return;
        }
        let step = login.sequence.next(self.prompt_line.current(), now);
        let text = match step {
            LoginStep::None => {
                if login.sequence.finished() {
                    self.login = None;
                }
                return;
            }
            LoginStep::Username => login.username.clone(),
            LoginStep::Password => login.password.clone(),
        };
        let finished = login.sequence.finished();
        if self.send_line_from(&text, Origin::Login) {
            self.login_sends += 1;
            self.terminal.feed_local("\n", LOCAL_ECHO_COLOR);
            if finished {
                self.login = None;
            }
        } else {
            self.login = None;
            self.notice(t(S::AutoLoginCouldNotSendCredentialsLogInManually));
        }
    }

    /// `Char.Login.Default`: answer once, with the saved credentials when the server takes
    /// password credentials and the text handshake has not begun, else with `{}` so the server's
    /// own login screen takes over.
    fn gmcp_login_offer(&mut self, password_supported: bool) {
        if self.gmcp_login_handled {
            return;
        }
        if self.login_read.is_some() {
            self.pending_offer.get_or_insert(password_supported);
            return;
        }
        self.gmcp_login_handled = true;
        let now = Instant::now();
        let use_credentials = password_supported
            && self
                .login
                .as_ref()
                .is_some_and(|l| l.protocol_deadline.is_none() && !l.sequence.started() && !l.sequence.expired(now));
        let message = match (&self.login, use_credentials) {
            (Some(login), true) => gmcp_login::credentials_message(Some(&login.username), Some(&login.password)),
            _ => gmcp_login::credentials_message(None, None),
        };
        let sent = match &self.link {
            Link::Net(conn) => conn.session().is_some_and(|s| s.send_gmcp(&message)),
            Link::Demo { .. } => false,
        };
        if !use_credentials {
            return;
        }
        if sent {
            self.login_sends += 1;
            self.prompt_line.clear();
            if let Some(login) = &mut self.login {
                login.protocol_deadline = Some(now + wandur_core::login::GMCP_RESULT_TIMEOUT);
            }
        } else {
            self.login = None;
            self.notice(t(S::AutoLoginCouldNotSendCredentialsLogInManually));
        }
    }

    /// `Char.Login.Result`: a finished authentication is never followed by another automatic
    /// attempt; a rejection says so and leaves the login to the person.
    fn gmcp_login_result(&mut self, success: bool) {
        self.gmcp_login_handled = true;
        if self.login.take().is_none() {
            return;
        }
        self.prompt_line.clear();
        if !success {
            self.notice(t(S::GmcpLoginRejected));
        }
    }

    /// Whether the prompt line asks for a password (the world's pattern).
    fn refresh_prompt_private(&mut self) {
        let prompt = self.prompt_line.current();
        self.prompt_private = self.link_connected()
            && prompt.chars().count() <= wandur_core::login::sequence::MAX_PATTERN
            && self.password_pattern.is_match(prompt);
    }

    /// The footer padlock and Session > Private Input.
    pub fn set_manual_private(&mut self, on: bool) {
        self.manual_private = on;
        self.agent_privacy();
    }

    /// Send a command that did not come from the input line (a channel reply). Not echoed when
    /// input is private; kept out of the history.
    pub fn send_command(&mut self, line: &str) -> bool {
        // Manual input takes over from the agent and the login handshake.
        self.agent_reset(AgentStop::Manual);
        self.stop_login();
        let private = self.private_input();
        if !self.send_line(line) {
            return false;
        }
        if self.echo_commands && !private {
            self.terminal.feed_local(&format!("{line}\n"), LOCAL_ECHO_COLOR);
        }
        self.flush_demo();
        true
    }

    /// Input is private: turned on by the person, the server turned echo off, or the prompt
    /// asks for a password.
    pub fn private_input(&self) -> bool {
        self.manual_private || self.server_echo || self.password_prompt || self.prompt_private
    }

    /// Connected to a server, or the demo is running.
    pub fn is_connected(&self) -> bool {
        matches!(self.status, Status::Connected { .. } | Status::Demo)
    }

    pub fn is_closed(&self) -> bool {
        matches!(self.status, Status::Closed { .. } | Status::Waiting { .. })
    }

    /// Send the input line. Private input is not echoed or remembered. An alias replaces the
    /// command: the typed command goes into the history (not the transcript) and the alias's
    /// commands are sent and echoed, as in C#.
    pub fn submit(&mut self) {
        self.submit_at(Instant::now());
    }

    pub fn submit_at(&mut self, now: Instant) {
        let line = std::mem::take(&mut self.input);
        if let Err(line) = self.send_typed(line, now) {
            self.input = line;
        }
    }

    /// Send a command as if typed, leaving the draft alone (the composer's Look and Commands
    /// buttons). Returns whether it went out.
    pub fn run_command(&mut self, line: &str) -> bool {
        self.send_typed(line.to_string(), Instant::now()).is_ok()
    }

    /// A command from the person: aliases, private input, echo and history as for the command
    /// line. Hands the line back if it could not be sent.
    fn send_typed(&mut self, line: String, now: Instant) -> Result<(), String> {
        // Manual input takes over from the agent (C# `ResetAgentContext("AgentManual")`) and
        // the login handshake.
        self.agent_reset(AgentStop::Manual);
        self.stop_login();
        let gate = self.macro_gate();
        if gate.connected
            && !gate.private
            && let Some(mut commands) = self.macros.on_command(&line, now, gate)
        {
            self.remember(line);
            self.send_from_macros(&mut commands);
            self.history_pos = None;
            self.draft.clear();
            self.terminal.scroll_to_bottom();
            return Ok(());
        }
        let private = self.private_input();
        if gate.connected
            && !private
            && let Some(ticket) = self.scripts.command(&line, now)
        {
            // A script's alias may take it: it goes out when the scripts have answered.
            self.pending_commands.push_back((ticket, line.clone()));
            self.remember(line);
            self.history_pos = None;
            self.draft.clear();
            self.terminal.scroll_to_bottom();
            return Ok(());
        }
        self.send_typed_now(line, private)
    }

    /// A typed command no alias took, sent once the scripts answered.
    fn send_as_typed(&mut self, line: String, _now: Instant) {
        let private = self.private_input();
        if private || !self.macro_gate().connected {
            return;
        }
        if self.send_line(&line) {
            if self.echo_commands {
                self.terminal.feed_local(&format!("{line}\n"), LOCAL_ECHO_COLOR);
            }
            self.flush_demo();
            self.terminal.scroll_to_bottom();
        }
    }

    fn send_typed_now(&mut self, line: String, private: bool) -> Result<(), String> {
        if !self.send_line(&line) {
            return Err(line);
        }
        if private {
            // Not echoed by anyone; advance the transcript like a terminal would.
            self.terminal.feed_local("\n", LOCAL_ECHO_COLOR);
            self.password_prompt = false;
        } else {
            if self.echo_commands {
                self.terminal.feed_local(&format!("{line}\n"), LOCAL_ECHO_COLOR);
            }
            self.remember(line);
        }
        self.history_pos = None;
        self.draft.clear();
        self.flush_demo();
        self.terminal.scroll_to_bottom();
        Ok(())
    }

    /// Keep a sent command for Up and Down (not blank, not twice in a row).
    fn remember(&mut self, line: String) {
        if self.learn_words {
            self.completions.learn_line(&line);
            self.completions.flush();
        }
        if !line.trim().is_empty() && self.history.last() != Some(&line) {
            self.history.push(line);
            if self.history.len() > HISTORY_LIMIT {
                self.history.remove(0);
            }
        }
    }

    /// The commands kept for Up and Down, oldest first.
    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// Older command (Up). Keeps the unsent draft to restore later.
    pub fn history_back(&mut self) {
        if self.private_input() || self.history.is_empty() {
            return;
        }
        let pos = match self.history_pos {
            None => {
                self.draft = std::mem::take(&mut self.input);
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(p) => p - 1,
        };
        self.history_pos = Some(pos);
        self.input = self.history[pos].clone();
    }

    /// Newer command (Down); past the newest, the draft comes back.
    pub fn history_forward(&mut self) {
        let Some(pos) = self.history_pos else { return };
        if pos + 1 < self.history.len() {
            self.history_pos = Some(pos + 1);
            self.input = self.history[pos + 1].clone();
        } else {
            self.history_pos = None;
            self.input = std::mem::take(&mut self.draft);
        }
    }

    pub fn disconnect(&mut self) {
        self.agent_reset(AgentStop::Paused);
        self.disconnecting = true;
        match &mut self.link {
            Link::Net(conn) => conn.disconnect(),
            Link::Demo { running, .. } => {
                if std::mem::take(running) {
                    self.notice(wandur_core::demo::ENDED);
                }
            }
        }
        self.end_history();
        self.status = self.link_status();
        // Timers stop at once, not at the next frame.
        self.run_macro_timers(Instant::now());
        self.run_scripts(Instant::now());
    }

    /// Open a new connection to the same endpoint, keeping the transcript (the demo starts
    /// again in the inn).
    pub fn reconnect(&mut self) {
        self.disconnecting = false;
        match &mut self.link {
            Link::Net(conn) => {
                let text = tf(S::ReconnectingTo, &[&self.endpoint]);
                conn.reconnect();
                self.notice(&text);
                self.new_connection();
                self.status = Status::Connecting;
            }
            Link::Demo { world, running } => {
                *running = true;
                let opening = world.start();
                let name = world.room_name();
                self.terminal.feed(b"\n");
                self.terminal.feed(opening.as_bytes());
                self.map.connection_started();
                let walk = self.walk_gate();
                self.map.track_output(&opening, walk, Instant::now());
                self.set_name(name);
                self.status = Status::Demo;
                self.end_history();
                self.begin_history();
                self.record_received(&opening);
            }
        }
    }

    /// Empty the transcript (Session > Clear Transcript).
    pub fn clear_transcript(&mut self) {
        self.terminal.clear();
        self.channels.reset();
        self.mark_seen();
    }

    /// The grid changed size: tell the server (NAWS) if it asked.
    pub fn resized(&mut self, size: TermSize) {
        if let Link::Net(conn) = &mut self.link {
            conn.set_window_size(
                size.columns.min(u16::MAX as usize) as u16,
                size.rows.min(u16::MAX as usize) as u16,
            );
        }
    }

    /// Whether output arrived since the tab was last shown.
    pub fn has_unseen(&self) -> bool {
        self.terminal.lines_total() > self.seen_lines || self.terminal.revision() > self.seen_revision
    }

    /// Completed lines that arrived since the tab was last shown.
    pub fn unseen_lines(&self) -> u64 {
        self.terminal.lines_total().saturating_sub(self.seen_lines)
    }

    pub fn mark_seen(&mut self) {
        self.seen_lines = self.terminal.lines_total();
        self.seen_revision = self.terminal.revision();
    }

    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Saved history settings changed (or the store became available).
    pub fn set_history(&mut self, config: Option<HistoryConfig>) {
        let show_notice = config.as_ref().is_some_and(|c| c.show_notice);
        let enabled = config.as_ref().is_some_and(|c| c.enabled);
        let retention = config.as_ref().map(|c| c.retention_days);
        self.history_config = config;
        if !show_notice && self.strip == Some(StripNotice::HistoryRecording) {
            self.strip = None;
        }
        if !enabled {
            // Turning recording off keeps what was saved and stops new capture.
            self.end_history();
        } else if self.recorder.is_none() && self.link_connected() {
            // Turning it on while connected starts a new history session (no backfill).
            self.begin_history();
        }
        if let (Some(recorder), Some(days)) = (&mut self.recorder, retention) {
            recorder.update_retention(days);
        }
    }

    /// Whether this connection is being recorded (tests and the probe).
    pub fn is_recording(&self) -> bool {
        self.recorder.as_ref().is_some_and(|r| !r.is_faulted())
    }

    /// The history session id of this connection, while it records.
    pub fn history_session(&self) -> Option<&str> {
        self.recorder.as_ref().map(HistoryRecorder::session_id)
    }

    /// Ask the recorder to write what it holds; the receiver hears when it is written.
    pub fn flush_history(&mut self) -> Option<Receiver<()>> {
        self.recorder.as_mut().map(HistoryRecorder::flush_and_wait)
    }

    /// Recorder threads of ended connections, for the app to wait on at exit.
    pub fn take_history_drains(&mut self) -> Vec<std::thread::JoinHandle<()>> {
        std::mem::take(&mut self.recorder_drains)
    }

    /// Start recording this connection, when history is on.
    fn begin_history(&mut self) {
        let Some(config) = &self.history_config else { return };
        if self.recorder.is_some() || !config.enabled {
            return;
        }
        let world_key = if self.is_demo() {
            "demo".to_string()
        } else {
            wandur_core::db::worlds::endpoint_key(&self.endpoint.host, self.endpoint.port)
                .unwrap_or_else(|_| self.endpoint.to_string())
        };
        let session = HistorySession::start(&world_key, self.world_name(), &self.character, (config.clock)());
        let show_notice = config.show_notice;
        match HistoryRecorder::start(
            Arc::clone(&config.store),
            session,
            config.retention_days,
            Arc::clone(&config.clock),
        ) {
            Ok(mut recorder) => {
                recorder.set_secrets(self.protocol.secrets());
                self.recorder = Some(recorder);
                if self.strip.is_none() && show_notice {
                    self.strip = Some(StripNotice::HistoryRecording);
                }
            }
            Err(_) => self.strip = Some(StripNotice::Text(t(S::HistoryRecordingFailed).into())),
        }
    }

    /// Stop recording: the last line and the end time are written on the recorder's thread.
    pub fn end_history(&mut self) {
        if let Some(mut recorder) = self.recorder.take() {
            recorder.update_character(&self.character);
            if let Some(handle) = recorder.complete() {
                self.recorder_drains.retain(|h| !h.is_finished());
                self.recorder_drains.push(handle);
            }
        }
    }

    /// Server text as applied: recorded, or a private marker while input is private or a login
    /// runs.
    fn record_received(&mut self, text: &str) {
        let blocked = self.protocol_gate().blocked();
        if let Some(recorder) = &mut self.recorder {
            recorder.received(text, blocked);
        }
    }

    /// Once a frame: the character, privacy that began without text, a failure notice, and the
    /// end of a connection that closed.
    fn follow_history(&mut self) {
        if self.recorder.is_none() {
            return;
        }
        if matches!(self.status, Status::Closed { .. } | Status::Waiting { .. }) {
            self.end_history();
            return;
        }
        let blocked = self.protocol_gate().blocked();
        let Some(recorder) = &mut self.recorder else { return };
        recorder.update_character(&self.character);
        if blocked {
            recorder.hide();
        }
        if recorder.take_failure() {
            // Recording pauses for this connection; the session goes on.
            self.strip = Some(StripNotice::Text(t(S::HistoryRecordingFailed).into()));
        }
    }
}

/// How a reset leaves the agent's status: Paused (privacy, a disconnect) or Paused for manual
/// control (a typed command, a walk).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentStop {
    Paused,
    Manual,
}

/// The session as the agent's runner sees it.
#[cfg(feature = "agent")]
struct TabGateway<'a>(&'a mut SessionTab);

#[cfg(feature = "agent")]
impl wandur_core::agent::AgentGateway for TabGateway<'_> {
    fn observe(&mut self) -> wandur_core::agent::AgentObservation {
        self.0.agent_observation()
    }

    fn set_agent_control(&mut self, enabled: bool) {
        self.0.set_agent_control(enabled);
    }

    fn send(&mut self, command: &str, expected: &wandur_core::agent::AgentObservation) -> bool {
        self.0.agent_send(command, expected)
    }
}

/// The agent's part of a session (the C# `WorkspaceController.Agent`). Without the `agent`
/// feature these do nothing.
impl SessionTab {
    /// Public server text for the agent's observation; nothing while input is private or a
    /// login runs.
    fn observe_agent(&mut self, text: &str) {
        if text.is_empty() || !self.agent.available() {
            return;
        }
        let gate = self.macro_gate();
        if gate.private || gate.login {
            return;
        }
        let rules = Arc::clone(self.channels.rules());
        self.agent.feed_text(text, &rules);
    }

    /// Input became private or a login runs: the context is dropped and a run stops.
    fn agent_privacy(&mut self) {
        if (self.private_input() || self.login_running()) && self.agent.has_context() {
            self.agent_reset(AgentStop::Paused);
        }
    }

    /// Stop the agent and drop its context (C# `ResetAgentContext`).
    pub fn agent_reset(&mut self, stop: AgentStop) {
        if !self.agent.available() {
            return;
        }
        #[cfg(feature = "agent")]
        {
            let status = match stop {
                AgentStop::Paused => wandur_core::agent::AgentStatus::Paused,
                AgentStop::Manual => wandur_core::agent::AgentStatus::Manual,
            };
            self.with_runner(|runner, gateway| runner.stop(status, gateway));
        }
        #[cfg(not(feature = "agent"))]
        let _ = stop;
        self.agent.reset_feed();
    }

    /// A walk starts: the agent stops (its context stays).
    fn agent_stop_for_walk(&mut self) {
        #[cfg(feature = "agent")]
        if self.agent.available() {
            self.with_runner(|runner, gateway| runner.stop(wandur_core::agent::AgentStatus::Manual, gateway));
        }
    }

    /// A new connection: a fresh memory and context.
    fn agent_new_connection(&mut self) {
        if !self.agent.available() {
            return;
        }
        #[cfg(feature = "agent")]
        self.with_runner(|runner, gateway| runner.clear_memory(gateway));
        self.agent.reset_feed();
    }

    /// Move a run along (each pump).
    fn agent_poll(&mut self, now: Instant) {
        #[cfg(feature = "agent")]
        if self.agent.is_busy() {
            self.with_runner(|runner, gateway| runner.poll(gateway, now));
        }
        #[cfg(not(feature = "agent"))]
        let _ = now;
    }
}

#[cfg(feature = "agent")]
impl SessionTab {
    /// Set up the agent for this session's world (the app calls it at open).
    pub fn configure_agent(
        &mut self,
        services: Arc<crate::agent_session::AgentServices>,
        world: wandur_core::agent::AgentWorld,
    ) {
        let waker = self.waker.clone();
        self.agent.configure(services, world, waker);
    }

    /// Run the runner with this session as its gateway.
    fn with_runner<R>(&mut self, f: impl FnOnce(&mut wandur_core::agent::AgentRunner, &mut TabGateway<'_>) -> R) -> R {
        let mut runner = std::mem::take(&mut self.agent.runner);
        let result = f(&mut runner, &mut TabGateway(self));
        self.agent.runner = runner;
        result
    }

    /// Whether the agent may send now: connected, logged in, input public, server echo off.
    fn agent_can_act(&self) -> bool {
        self.link_connected()
            && !self.disconnecting
            && !self.private_input()
            && !self.login_running()
            && !self.server_echo
    }

    fn agent_observation(&mut self) -> wandur_core::agent::AgentObservation {
        let rules = Arc::clone(self.channels.rules());
        let (revision, generation) = self.agent.revision();
        wandur_core::agent::AgentObservation {
            revision,
            generation,
            text: self.agent.observation_text(&rules),
            can_act: self.agent_can_act(),
        }
    }

    /// The agent takes control (no other source sends, scripts stop, a walk stops) or gives it
    /// back (scripts stay stopped until switched or reloaded).
    fn set_agent_control(&mut self, enabled: bool) {
        self.agent.owns_control = enabled;
        if enabled {
            self.map.stop_walk(WalkStatus::Stopped);
            self.consume_pending_commands();
        }
        self.scripts.set_suspended(enabled);
        self.follow_lines();
    }

    /// Send an allowed command for the agent, if the world is still the one it decided on.
    fn agent_send(&mut self, command: &str, expected: &wandur_core::agent::AgentObservation) -> bool {
        let now = self.agent_observation();
        if !self.agent.owns_control || !expected.same_as(&now) {
            return false;
        }
        if !self.send_line_from(command, Origin::Agent) {
            return false;
        }
        self.agent_commands_sent += 1;
        if self.echo_commands {
            self.terminal.feed_local(&format!("{command}\n"), LOCAL_ECHO_COLOR);
        }
        self.flush_demo();
        self.terminal.scroll_to_bottom();
        true
    }

    /// Play, Preview or Step with the selected goal and the world's profile as saved now.
    pub fn agent_start(&mut self, mode: wandur_core::agent::AgentRunMode) {
        if !self.agent.can_start() {
            return;
        }
        let Some(profile) = self.agent.profile() else {
            self.agent.runner.note_status(wandur_core::agent::AgentStatus::Failed);
            return;
        };
        let goal = self.agent.goal();
        let now = Instant::now();
        self.with_runner(|runner, gateway| runner.start(&profile, &goal, mode, gateway, now));
        // A Preview may already be answered by a fast server on the next pump.
        self.agent_poll(now);
    }

    /// The Stop button.
    pub fn agent_stop(&mut self) {
        self.with_runner(|runner, gateway| runner.stop(wandur_core::agent::AgentStatus::Stopped, gateway));
    }

    /// Clear memory (More controls).
    pub fn agent_clear_memory(&mut self) {
        self.with_runner(|runner, gateway| runner.clear_memory(gateway));
    }

    /// Choose another goal for this connection: a run stops and the memory is cleared.
    pub fn agent_select_goal(&mut self, index: usize) {
        if self.agent.select_goal(index) {
            self.agent_clear_memory();
        }
    }

    /// The world's agent settings were saved: stop, clear the memory and read the goals again.
    pub fn agent_profile_saved(&mut self, profile: &wandur_core::agent::AgentProfile) {
        self.agent_clear_memory();
        self.agent.profile_saved(profile);
    }
}

/// Loopback sessions for the tests of this module and of the views.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::net::TcpListener;
    use std::time::Duration;

    pub fn local_tab_with(options: TabOptions) -> (SessionTab, std::net::TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = Endpoint::new("127.0.0.1", listener.local_addr().unwrap().port());
        let tab = SessionTab::open(1, endpoint, &options, Arc::new(|| {}));
        let (server, _) = listener.accept().unwrap();
        (tab, server)
    }

    pub fn local_tab() -> (SessionTab, std::net::TcpStream) {
        local_tab_with(TabOptions {
            scrollback: 100,
            ..TabOptions::default()
        })
    }

    pub fn pump_until(tab: &mut SessionTab, done: impl Fn(&SessionTab) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(tab) {
            assert!(Instant::now() < deadline, "timed out with status {:?}", tab.status);
            tab.pump(Instant::now());
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::time::Duration;

    use super::test_support::*;

    #[test]
    fn output_lands_in_the_grid_and_marks_activity() {
        let (mut tab, mut server) = local_tab();
        pump_until(&mut tab, SessionTab::is_connected);
        tab.mark_seen();
        server.write_all(b"\x1b[32mHello\x1b[0m\r\n> ").unwrap();
        pump_until(&mut tab, |t| t.terminal.current_line() == ">");
        assert!(tab.terminal.transcript().ends_with("Hello\n>"));
        assert!(tab.has_unseen());
        assert_eq!(tab.unseen_lines(), 1);
        tab.mark_seen();
        assert!(!tab.has_unseen());
        drop(server);
        pump_until(&mut tab, SessionTab::is_closed);
        assert!(tab.terminal.transcript().contains("[Server closed the connection]"));
    }

    #[test]
    fn private_input_is_not_echoed_or_remembered() {
        let (mut tab, mut server) = local_tab();
        pump_until(&mut tab, SessionTab::is_connected);
        server.write_all(&[255, 251, 1]).unwrap();
        server.write_all(b"Password: ").unwrap();
        pump_until(&mut tab, |t| t.server_echo);
        tab.input = "hunter2".into();
        tab.submit();
        assert_eq!(tab.history_len(), 0);
        assert!(!tab.terminal.transcript().contains("hunter2"));
        let mut got = Vec::new();
        let mut buf = [0u8; 64];
        while !got.ends_with(b"hunter2\r\n") {
            let n = server.read(&mut buf).unwrap();
            assert!(n > 0);
            got.extend_from_slice(&buf[..n]);
        }
    }

    #[test]
    fn password_prompt_without_echo_off_is_masked_once() {
        let (mut tab, mut server) = local_tab_with(TabOptions {
            prompt_quiet: Duration::from_millis(30),
            ..TabOptions::default()
        });
        pump_until(&mut tab, SessionTab::is_connected);
        server.write_all(b"Welcome\r\nPassword: ").unwrap();
        pump_until(&mut tab, |t| t.last_prompt.is_some());
        let prompt = tab.last_prompt.clone().unwrap();
        assert_eq!(prompt.text, "Password:");
        assert_eq!(prompt.source, PromptSource::Quiet);
        assert!(tab.private_input());
        tab.input = "secret".into();
        tab.submit();
        assert!(!tab.terminal.transcript().contains("secret"));
        assert_eq!(tab.history_len(), 0);
        assert!(!tab.private_input(), "only the next command is masked");
    }

    #[test]
    fn prompt_marks_report_the_current_line() {
        let (mut tab, mut server) = local_tab();
        pump_until(&mut tab, SessionTab::is_connected);
        server.write_all(b"text\r\n\x1b[32m<100hp>\x1b[0m \xff\xf9").unwrap();
        pump_until(&mut tab, |t| t.prompt_count == 1);
        let prompt = tab.last_prompt.clone().unwrap();
        assert_eq!(prompt.text, "<100hp>");
        assert_eq!(prompt.source, PromptSource::Mark);
        assert!(!prompt.password);
    }

    #[test]
    fn history_navigation_restores_the_draft() {
        let (mut tab, _server) = local_tab();
        pump_until(&mut tab, SessionTab::is_connected);
        for cmd in ["north", "look", "look"] {
            tab.input = cmd.into();
            tab.submit();
        }
        assert_eq!(tab.history_len(), 2);
        tab.input = "dra".into();
        tab.history_back();
        assert_eq!(tab.input, "look");
        tab.history_back();
        assert_eq!(tab.input, "north");
        tab.history_back();
        assert_eq!(tab.input, "north");
        tab.history_forward();
        assert_eq!(tab.input, "look");
        tab.history_forward();
        assert_eq!(tab.input, "dra");
        assert!(tab.terminal.transcript().contains("north"));
    }

    #[test]
    fn manual_reconnect_keeps_the_transcript() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = Endpoint::new("127.0.0.1", listener.local_addr().unwrap().port());
        let mut tab = SessionTab::open(1, endpoint, &TabOptions::default(), Arc::new(|| {}));
        let (mut first, _) = listener.accept().unwrap();
        first.write_all(b"first session\r\n").unwrap();
        drop(first);
        pump_until(&mut tab, SessionTab::is_closed);
        tab.reconnect();
        let (mut second, _) = listener.accept().unwrap();
        second.write_all(b"second session\r\n").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("second session"));
        let text = tab.terminal.transcript();
        assert!(text.contains("first session") && text.contains("[Reconnecting to"));
        assert!(tab.is_connected());
    }

    fn macro_library() -> Vec<SavedMacro> {
        use wandur_core::macros::{MacroDefinition, MacroKind};
        let saved = |id: &str, definition| SavedMacro {
            id: id.into(),
            name: id.into(),
            enabled: true,
            definition,
        };
        vec![
            saved(
                "refill",
                MacroDefinition::new(MacroKind::Trigger, "Your lantern gutters", "fill lantern"),
            ),
            saved("ford", MacroDefinition::new(MacroKind::Alias, "ford", "east\neast")),
            saved("look", MacroDefinition::new(MacroKind::Shortcut, "F2", "look")),
        ]
    }

    /// Read what the server received until `want` lines have arrived.
    fn read_lines(server: &mut std::net::TcpStream, want: usize) -> Vec<String> {
        server.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 256];
        while got.iter().filter(|&&b| b == b'\n').count() < want {
            let n = server.read(&mut buf).unwrap();
            assert!(n > 0, "connection closed");
            got.extend_from_slice(&buf[..n]);
        }
        // Drop telnet negotiation (IAC, command, option).
        let mut plain = Vec::new();
        let mut i = 0;
        while i < got.len() {
            if got[i] == 255 {
                i += 3;
            } else {
                plain.push(got[i]);
                i += 1;
            }
        }
        let got = plain;
        String::from_utf8_lossy(&got)
            .split("\r\n")
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// Over loopback: a trigger answers a public server line; an alias replaces the typed
    /// command (the alias goes into the history, the replacement into the transcript, as C#);
    /// a function key sends its macro; private input and its lines set nothing off.
    #[test]
    fn macros_answer_lines_aliases_and_keys_over_loopback() {
        let (mut tab, mut server) = local_tab();
        tab.set_macros(Some("w".into()), macro_library(), Instant::now());
        pump_until(&mut tab, |t| t.is_connected() && t.macros.is_active());
        assert!(tab.macros.wants_lines());

        server.write_all(b"Your lantern gutters and dims.\r\n").unwrap();
        pump_until(&mut tab, |t| t.macro_commands_sent == 1);
        assert_eq!(read_lines(&mut server, 1), ["fill lantern"]);
        assert!(tab.terminal.transcript().contains("fill lantern"), "echoed");
        assert_eq!(tab.history_len(), 0, "trigger commands stay out of the history");

        tab.input = "ford".into();
        tab.submit();
        assert_eq!(read_lines(&mut server, 2), ["east", "east"]);
        assert_eq!(tab.history(), ["ford"]);
        let text = tab.terminal.transcript();
        assert!(!text.contains("ford"), "the alias itself is not echoed");
        assert!(text.contains("east\neast"));
        tab.input = "ford now".into();
        tab.submit();
        assert_eq!(read_lines(&mut server, 1), ["ford now"]);

        assert!(tab.press_key("F2", Instant::now()));
        assert!(!tab.press_key("F3", Instant::now()));
        assert_eq!(read_lines(&mut server, 1), ["look"]);

        // Echo off: the line that arrives while private never reaches the trigger, and the
        // alias is sent as typed (it is a password now).
        server.write_all(&[255, 251, 1]).unwrap();
        server
            .write_all(b"Your lantern gutters (private)\r\nPassword: ")
            .unwrap();
        pump_until(&mut tab, |t| {
            t.server_echo && t.terminal.transcript().contains("Password:")
        });
        tab.pump(Instant::now());
        tab.input = "ford".into();
        tab.submit();
        assert_eq!(read_lines(&mut server, 1), ["ford"]);
        assert!(!tab.press_key("F2", Instant::now()));
        server.write_all(&[255, 252, 1]).unwrap();
        server.write_all(b"Welcome back.\r\n").unwrap();
        pump_until(&mut tab, |t| {
            !t.server_echo && t.terminal.transcript().contains("Welcome back.")
        });
        assert_eq!(tab.macro_commands_sent, 4, "nothing more was sent");

        // The session switch turns them off; on again they answer.
        tab.switch_macros(false, Instant::now());
        tab.input = "ford".into();
        tab.submit();
        assert_eq!(read_lines(&mut server, 1), ["ford"]);
        tab.switch_macros(true, Instant::now());
        server.write_all(b"Your lantern gutters again.\r\n").unwrap();
        pump_until(&mut tab, |t| t.macro_commands_sent == 5);
        assert_eq!(read_lines(&mut server, 1), ["fill lantern"]);
    }

    /// A timer sends over loopback, and stops when the session disconnects.
    #[test]
    fn a_timer_sends_and_stops_on_disconnect() {
        use wandur_core::macros::{MacroDefinition, MacroKind};
        let (mut tab, mut server) = local_tab();
        let timer = SavedMacro {
            id: "t".into(),
            name: "t".into(),
            enabled: true,
            definition: MacroDefinition::new(MacroKind::Timer, "", "score").every(1),
        };
        tab.set_macros(Some("w".into()), vec![timer], Instant::now());
        pump_until(&mut tab, |t| t.macros.is_active());
        let started = Instant::now();
        assert!(tab.deadline().is_some_and(|d| d > started), "a full interval first");
        pump_until(&mut tab, |t| t.macro_commands_sent == 1);
        assert!(started.elapsed() >= Duration::from_millis(900));
        assert_eq!(read_lines(&mut server, 1), ["score"]);
        tab.disconnect();
        assert!(!tab.macros.is_active());
        assert!(tab.macros.deadline().is_none());
        let until = Instant::now() + Duration::from_millis(1300);
        while Instant::now() < until {
            tab.pump(Instant::now());
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(tab.macro_commands_sent, 1);
    }

    /// A tab whose world logs in automatically with `password` from a memory vault.
    fn login_tab(
        username: &str,
        password: Option<&str>,
        password_prompt: &str,
    ) -> (SessionTab, std::net::TcpStream, Arc<wandur_core::login::MemoryVault>) {
        let vault = Arc::new(wandur_core::login::MemoryVault::new());
        if let Some(password) = password {
            vault.write("login-key", password).unwrap();
        }
        let options = TabOptions {
            name: "Test".into(),
            scrollback: 100,
            character: username.into(),
            password_prompt: password_prompt.into(),
            login: Some(LoginConfig {
                username: username.into(),
                key: "login-key".into(),
                username_prompt: wandur_core::login::DEFAULT_USERNAME_PROMPT.into(),
                password_prompt: password_prompt.into(),
                vault: Arc::clone(&vault) as Arc<dyn PasswordVault>,
            }),
            ..TabOptions::default()
        };
        let (tab, server) = local_tab_with(options);
        (tab, server, vault)
    }

    /// Nothing waits to be read on the server side (after a short grace period).
    fn server_got_nothing(server: &mut std::net::TcpStream) -> bool {
        server.set_read_timeout(Some(Duration::from_millis(150))).unwrap();
        let mut buf = [0u8; 256];
        let quiet = match server.read(&mut buf) {
            Ok(n) => {
                // Telnet negotiation alone does not count.
                buf[..n].iter().all(|&b| b >= 240)
            }
            Err(e) => matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut),
        };
        server.set_read_timeout(None).unwrap();
        quiet
    }

    /// Pump for a while (the UI keeps drawing frames).
    fn pump_for(tab: &mut SessionTab, ms: u64) {
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until {
            tab.pump(Instant::now());
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// LoginTests.FragmentedColoredPromptsSendCredentialsOnceWithoutEchoOrHistory, over
    /// loopback: split, coloured prompts get the username and then the password once each; a
    /// second password prompt gets nothing; neither value is echoed or kept in the history; the
    /// macros stay quiet while the login runs.
    #[test]
    fn fragmented_colored_prompts_send_credentials_once_without_echo_or_history() {
        for (first, second) in [("Pass", "word: "), ("(P)", "assword: ")] {
            let (mut tab, mut server, _vault) = login_tab(
                "test-user",
                Some("p@ss-test-only"),
                wandur_core::login::DEFAULT_PASSWORD_PROMPT,
            );
            assert!(tab.echo_commands, "local echo is on");
            pump_until(&mut tab, |t| t.is_connected() && t.login.is_some());
            assert!(tab.macro_gate().login, "macros are held while the login runs");
            server.write_all(b"\x1b[1;36mUserna").unwrap();
            pump_until(&mut tab, |t| t.terminal.transcript().contains("Userna"));
            assert!(server_got_nothing(&mut server), "a partial prompt gets nothing");
            server.write_all(b"me: \x1b[0m").unwrap();
            pump_until(&mut tab, |t| t.login_sends == 1);
            assert_eq!(read_lines(&mut server, 1), ["test-user"]);
            server.write_all(b"\r\n\x1b[0m ").unwrap();
            pump_for(&mut tab, 100);
            server.write_all(format!("\x1b[1;30m{first}").as_bytes()).unwrap();
            pump_until(&mut tab, |t| t.terminal.transcript().contains(first));
            assert!(server_got_nothing(&mut server));
            server.write_all(format!("{second}\x1b[0m").as_bytes()).unwrap();
            pump_until(&mut tab, |t| t.login_sends == 2);
            assert_eq!(read_lines(&mut server, 1), ["p@ss-test-only"]);
            assert!(!tab.login_running(), "the handshake is over");
            server.write_all(b"Wrong password.\r\nPassword: ").unwrap();
            pump_until(&mut tab, |t| t.terminal.transcript().contains("Wrong password."));
            pump_for(&mut tab, 120);
            assert!(server_got_nothing(&mut server), "no retry for {first}{second}");
            assert_eq!(tab.history_len(), 0);
            let text = tab.terminal.transcript();
            assert!(
                !text.contains("p@ss-test-only") && !text.contains("test-user"),
                "{text}"
            );
            assert_eq!(tab.login_sends, 2);
        }
    }

    /// Over loopback: no trigger fires on lines that arrive while auto-login runs; once it is
    /// over, the same line sets the trigger off.
    #[test]
    fn nothing_fires_on_login_lines() {
        use wandur_core::macros::{MacroDefinition, MacroKind};
        let (mut tab, mut server, _vault) =
            login_tab("odo", Some("secret-x"), wandur_core::login::DEFAULT_PASSWORD_PROMPT);
        let trigger = SavedMacro {
            id: "greet".into(),
            name: "greet".into(),
            enabled: true,
            definition: MacroDefinition::new(MacroKind::Trigger, "Welcome", "wave"),
        };
        tab.set_macros(Some("w".into()), vec![trigger], Instant::now());
        pump_until(&mut tab, |t| {
            t.is_connected() && t.login.is_some() && t.macros.is_active()
        });
        server.write_all(b"Welcome to the station.\r\nName: ").unwrap();
        pump_until(&mut tab, |t| t.login_sends == 1);
        server.write_all(b"Welcome back.\r\nPassword: ").unwrap();
        pump_until(&mut tab, |t| t.login_sends == 2 && !t.login_running());
        assert_eq!(read_lines(&mut server, 2), ["odo", "secret-x"]);
        pump_for(&mut tab, 80);
        assert_eq!(tab.macro_commands_sent, 0, "login lines set nothing off");
        server.write_all(b"Welcome aboard.\r\n").unwrap();
        pump_until(&mut tab, |t| t.macro_commands_sent == 1);
        assert_eq!(read_lines(&mut server, 1), ["wave"]);
    }

    /// LoginTests.ManualInputCancelsAutoLoginAndMissingPasswordsAllowManualConnection.
    #[test]
    fn manual_input_cancels_auto_login_and_a_missing_password_allows_manual_play() {
        let (mut tab, mut server, vault) = login_tab(
            "saved-name",
            Some("saved-secret"),
            wandur_core::login::DEFAULT_PASSWORD_PROMPT,
        );
        pump_until(&mut tab, |t| t.is_connected() && t.login.is_some());
        tab.input = "manual-name".into();
        tab.submit();
        assert_eq!(read_lines(&mut server, 1), ["manual-name"]);
        server.write_all(b"Name: ").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("Name:"));
        pump_for(&mut tab, 80);
        assert!(server_got_nothing(&mut server), "manual input took over");
        assert_eq!(tab.login_sends, 0);

        // The saved password is gone: the session connects, says so, and plays by hand.
        vault.delete("login-key").unwrap();
        drop(server);
        pump_until(&mut tab, SessionTab::is_closed);
        let listener_tab = login_tab("saved-name", None, wandur_core::login::DEFAULT_PASSWORD_PROMPT);
        let (mut tab, _server, _) = listener_tab;
        pump_until(&mut tab, |t| t.is_connected() && !t.login_running());
        assert!(tab.is_connected());
        assert!(tab.terminal.transcript().contains("wasn't found"));
        assert!(!tab.macro_gate().login);
    }

    /// A store that cannot be read: a readable notice, manual play, nothing sent.
    #[test]
    fn a_locked_vault_is_a_readable_notice() {
        let (mut tab, mut server, vault) =
            login_tab("odo", Some("secret"), wandur_core::login::DEFAULT_PASSWORD_PROMPT);
        vault.set_locked(true);
        pump_until(&mut tab, |t| t.is_connected() && !t.login_running());
        assert!(
            tab.terminal
                .transcript()
                .contains(t(S::CouldNotReadTheSavedPasswordUnlockYourSystem))
        );
        server.write_all(b"Name: ").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("Name:"));
        assert!(server_got_nothing(&mut server));
    }

    /// LoginTests.ManualPasswordsStayPrivateForNewlineAndCustomPrompts: a prompt that ends its
    /// line still makes the input private, and so does the world's own pattern.
    #[test]
    fn manual_passwords_stay_private_for_newline_and_custom_prompts() {
        for (prompt, pattern) in [
            ("Password:\r\n", wandur_core::login::DEFAULT_PASSWORD_PROMPT),
            ("Secret?\r\n", r"^Secret\?$"),
        ] {
            let (mut tab, mut server) = local_tab_with(TabOptions {
                password_prompt: pattern.into(),
                ..TabOptions::default()
            });
            pump_until(&mut tab, SessionTab::is_connected);
            server.write_all(prompt.as_bytes()).unwrap();
            pump_until(&mut tab, |t| t.terminal.transcript().contains(prompt.trim()));
            assert!(tab.private_input(), "{prompt:?}");
            tab.input = "manual-secret".into();
            tab.submit();
            assert_eq!(read_lines(&mut server, 1), ["manual-secret"]);
            assert!(!tab.terminal.transcript().contains("manual-secret"));
            assert_eq!(tab.history_len(), 0);
            assert!(!tab.private_input(), "the next prompt decides again");
        }
    }

    /// Read raw bytes until `needle` has arrived, pumping the tab meanwhile (the UI answers
    /// login offers); returns everything read.
    fn read_until_contains(tab: &mut SessionTab, server: &mut std::net::TcpStream, needle: &[u8]) -> Vec<u8> {
        server.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut got = Vec::new();
        let mut buf = [0u8; 512];
        while !got.windows(needle.len()).any(|w| w == needle) {
            assert!(
                Instant::now() < deadline,
                "timed out; got {:?}",
                String::from_utf8_lossy(&got)
            );
            tab.pump(Instant::now());
            match server.read(&mut buf) {
                Ok(0) => panic!("closed; got {:?}", String::from_utf8_lossy(&got)),
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => panic!("{e}"),
            }
        }
        server.set_read_timeout(None).unwrap();
        got
    }

    fn gmcp_frame(message: &str) -> Vec<u8> {
        let mut out = vec![255, 250, 201];
        out.extend_from_slice(message.as_bytes());
        out.extend_from_slice(&[255, 240]);
        out
    }

    fn smaug_tab() -> (SessionTab, std::net::TcpStream) {
        local_tab_with(TabOptions {
            scrollback: 200,
            codebase: "SMAUG 1.4a".into(),
            ..TabOptions::default()
        })
    }

    /// ChannelPanelTests.ChannelLinesAppearOnTheirOwnTabsAndStayInTheTranscript over loopback: a
    /// SMAUG world's printed channels and a GMCP channel each land on their own tab (private
    /// first), with speaker and text cut out, and every line stays in the transcript.
    #[test]
    fn channel_lines_appear_on_their_own_tabs_and_stay_in_the_transcript() {
        let (mut tab, mut server) = smaug_tab();
        pump_until(&mut tab, SessionTab::is_connected);
        server
            .write_all(
                b"You are standing in a wide green field.\r\n[OOC] Aldric: anyone selling a lantern?\r\nJorunn tells you 'bring the brass key'\r\n",
            )
            .unwrap();
        server.write_all(&[255, 251, 201]).unwrap();
        server
            .write_all(&gmcp_frame(
                r#"Comm.Channel.Text {"channel":"chat","talker":"Brenna","text":"[CHAT] Brenna: who wants to group up?"}"#,
            ))
            .unwrap();
        // The world prints the same line right after the package: it is not shown twice.
        server.write_all(b"[CHAT] Brenna: who wants to group up?\r\n").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("group up?"));
        let titles: Vec<&str> = tab.channels.log.tabs().iter().map(|t| t.title()).collect();
        assert_eq!(titles, ["All", "tell", "ooc", "chat"]);
        let tabs = tab.channels.log.tabs();
        assert_eq!(tabs[0].messages.len(), 3);
        let tell = &tabs[1].messages[0];
        assert_eq!(
            (tell.speaker.as_str(), tell.text.as_str()),
            ("Jorunn", "bring the brass key")
        );
        assert!(tell.private);
        assert_eq!(tabs[1].reply("hi").as_deref(), Some("tell Jorunn hi"));
        let ooc = &tabs[2].messages[0];
        assert_eq!(
            (ooc.speaker.as_str(), ooc.text.as_str()),
            ("Aldric", "anyone selling a lantern?")
        );
        let chat = &tabs[3].messages[0];
        assert_eq!(
            (chat.speaker.as_str(), chat.text.as_str()),
            ("Brenna", "who wants to group up?")
        );
        assert_eq!(tabs[3].messages.len(), 1);
        let transcript = tab.terminal.transcript();
        for line in [
            "You are standing in a wide green field.",
            "[OOC] Aldric: anyone selling a lantern?",
            "Jorunn tells you 'bring the brass key'",
            "[CHAT] Brenna: who wants to group up?",
        ] {
            assert!(transcript.contains(line), "{line}");
        }
    }

    /// Channels see public output only: lines and GMCP channel messages that arrive while
    /// input is private are not mirrored, and the line in flight is dropped (C#).
    #[test]
    fn private_input_keeps_lines_out_of_the_channels() {
        let (mut tab, mut server) = smaug_tab();
        pump_until(&mut tab, SessionTab::is_connected);
        tab.set_manual_private(true);
        server.write_all(&[255, 251, 201]).unwrap();
        server.write_all(b"[OOC] Aldric: my secret\r\n").unwrap();
        server
            .write_all(&gmcp_frame(
                r#"Comm.Channel.Text {"channel":"chat","talker":"B","text":"x"}"#,
            ))
            .unwrap();
        server.write_all(b"[OOC] Aldric: still sec").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("still sec"));
        tab.set_manual_private(false);
        server.write_all(b"ret\r\n[OOC] Aldric: public again\r\n").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("public again"));
        let all = &tab.channels.log.tabs()[0];
        assert_eq!(all.messages.len(), 1);
        assert_eq!(all.messages[0].text, "public again");
    }

    /// MSSP CODEBASE picks the family while the world names none (C# `UseServerCodebase`).
    #[test]
    fn the_server_codebase_picks_the_family_for_the_session() {
        let (mut tab, mut server) = local_tab();
        pump_until(&mut tab, SessionTab::is_connected);
        let mut mssp = vec![255, 250, 70, 1];
        mssp.extend_from_slice(b"CODEBASE");
        mssp.push(2);
        mssp.extend_from_slice(b"ROM 2.4b6");
        mssp.extend_from_slice(&[255, 240]);
        server.write_all(&[255, 251, 70]).unwrap();
        server.write_all(&mssp).unwrap();
        server.write_all(b"Mira questions 'where is the smith?'\r\n").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("smith"));
        assert_eq!(tab.channels.codebase(), Some("ROM 2.4b6"));
        assert_eq!(tab.channels.log.tabs()[1].channel, "question");
    }

    /// GMCP `Char.Login`: credentials go once over GMCP when offered; a rejection says so and
    /// is never followed by a text attempt; the login frames reach neither channels nor map.
    #[test]
    fn a_rejected_gmcp_login_stops_without_a_text_retry() {
        let (mut tab, mut server, _vault) =
            login_tab("odo", Some("lantern-pass"), wandur_core::login::DEFAULT_PASSWORD_PROMPT);
        pump_until(&mut tab, |t| t.is_connected() && t.login.is_some());
        server.write_all(&[255, 251, 201]).unwrap();
        let hello = read_until_contains(&mut tab, &mut server, b"Char.Login 1");
        assert!(String::from_utf8_lossy(&hello).contains("Core.Supports.Set"));
        server
            .write_all(&gmcp_frame(r#"Char.Login.Default {"type":["password-credentials"]}"#))
            .unwrap();
        let got = read_until_contains(&mut tab, &mut server, b"}\xff\xf0");
        let text = String::from_utf8_lossy(&got);
        assert!(
            text.contains(r#"Char.Login.Credentials {"account":"odo","password":"lantern-pass"}"#),
            "{text}"
        );
        assert_eq!(tab.login_sends, 1);
        // A text prompt while the GMCP login waits for its result gets nothing.
        server.write_all(b"Name: ").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("Name:"));
        assert!(server_got_nothing(&mut server));
        server
            .write_all(&gmcp_frame(r#"Char.Login.Result {"success":false}"#))
            .unwrap();
        pump_until(&mut tab, |t| !t.login_running());
        assert!(tab.terminal.transcript().contains(t(S::GmcpLoginRejected)));
        server.write_all(b"\r\nName: ").unwrap();
        pump_for(&mut tab, 120);
        assert!(server_got_nothing(&mut server), "no text retry after a rejection");
        // A second offer is not answered with credentials either.
        server
            .write_all(&gmcp_frame(r#"Char.Login.Default {"type":["password-credentials"]}"#))
            .unwrap();
        pump_for(&mut tab, 120);
        assert!(server_got_nothing(&mut server));
        assert_eq!(tab.login_sends, 1);
        assert!(tab.channels.log.is_empty());
        assert!(!tab.terminal.transcript().contains("lantern-pass"));
    }

    /// Without auto-login an offer is declined with `{}`, handing the login back to the server.
    #[test]
    fn a_gmcp_offer_without_auto_login_is_declined() {
        let (mut tab, mut server) = local_tab();
        pump_until(&mut tab, SessionTab::is_connected);
        server.write_all(&[255, 251, 201]).unwrap();
        read_until_contains(&mut tab, &mut server, b"Core.Supports.Set");
        server
            .write_all(&gmcp_frame(r#"Char.Login.Default {"type":["password-credentials"]}"#))
            .unwrap();
        let got = read_until_contains(&mut tab, &mut server, b"Char.Login.Credentials {}");
        assert!(!String::from_utf8_lossy(&got).contains("account"));
        assert_eq!(tab.login_sends, 0);
    }

    /// The padlock: input is masked, not echoed and not remembered while it is on; it turns off
    /// on disconnect.
    #[test]
    fn manual_private_input_masks_and_resets_on_disconnect() {
        let (mut tab, mut server) = local_tab();
        pump_until(&mut tab, SessionTab::is_connected);
        assert!(!tab.private_input());
        tab.set_manual_private(true);
        assert!(tab.private_input() && tab.macro_gate().private);
        tab.input = "my-secret-word".into();
        tab.submit();
        assert_eq!(read_lines(&mut server, 1), ["my-secret-word"]);
        assert!(!tab.terminal.transcript().contains("my-secret-word"));
        assert_eq!(tab.history_len(), 0);
        assert!(tab.manual_private, "it stays on until turned off");
        tab.disconnect();
        pump_until(&mut tab, SessionTab::is_closed);
        assert!(!tab.manual_private && !tab.private_input());
    }

    /// SessionCharacterTests: the character names the tab ("World · Character") and the
    /// window title ("Character · World · App"); without one, the world's name alone.
    #[test]
    fn the_character_names_the_tab_and_the_window_title() {
        let (tab, _server) = local_tab_with(TabOptions {
            name: "The Lantern Road".into(),
            character: " Odo ".into(),
            ..TabOptions::default()
        });
        assert_eq!(tab.title(), "The Lantern Road · Odo");
        assert_eq!(tab.character(), "Odo");
        assert_eq!(tab.window_title("Wandur"), "Odo · The Lantern Road · Wandur");
        let (plain, _server) = local_tab_with(TabOptions {
            name: "Starfall Reach".into(),
            ..TabOptions::default()
        });
        assert_eq!(plain.title(), "Starfall Reach");
        assert_eq!(plain.window_title("Wandur"), "Starfall Reach · Wandur");
        assert!(!plain.window_title("Wandur").contains('\u{2014}'));
    }

    /// The offline demo plays through the input line: the commands echo, the answers follow,
    /// every room can be reached, and the tab takes the room's name.
    #[test]
    fn the_offline_demo_plays_through_the_input_line() {
        let mut tab = SessionTab::demo(1, &TabOptions::default());
        assert!(tab.is_demo() && tab.is_connected());
        assert_eq!(tab.title(), "The Lantern & the Rain");
        assert!(tab.terminal.transcript().contains("W A N D U R"));
        assert_eq!(tab.pump(Instant::now()), 0);
        assert!(tab.deadline().is_none());
        let mut visited = vec![tab.title().to_string()];
        for command in ["north", "up", "down", "south", "east", "east"] {
            tab.input = command.into();
            tab.submit();
            visited.push(tab.title().to_string());
        }
        assert_eq!(
            visited,
            [
                "The Lantern & the Rain",
                "The Old Market",
                "Above the Rooftops",
                "The Old Market",
                "The Lantern & the Rain",
                "A Bridge of Moss & Stone",
                "The Edge of the Wood"
            ]
        );
        let text = tab.terminal.transcript();
        assert!(text.contains("east\n\nThe Edge of the Wood"), "echo, then the answer");
        assert!(text.contains("\"Every world begins with a first step.\""));
        assert_eq!(tab.history_len(), 5, "east twice is kept once");
        tab.disconnect();
        assert!(tab.is_closed());
        assert!(tab.terminal.transcript().ends_with("[Demo ended]"));
        tab.input = "look".into();
        tab.submit();
        assert_eq!(tab.input, "look", "nothing is sent once the demo ended");
        tab.reconnect();
        assert!(tab.is_connected());
        assert_eq!(tab.title(), "The Lantern & the Rain");
        tab.clear_transcript();
        assert!(!tab.terminal.has_text());
    }

    /// CompletionTests.ObserveLearnsCompleteLines... and the composer's privacy rules: complete
    /// public lines teach their words (escape sequences resolved by the grid, a line split across
    /// reads waits for its end), lines received while private are never learned, sent commands
    /// are, and the setting stops learning.
    #[test]
    fn completion_learns_public_lines_and_sent_commands_only() {
        let (mut tab, mut server) = local_tab();
        pump_until(&mut tab, SessionTab::is_connected);
        let words = |tab: &mut SessionTab, prefix: &str| {
            assert!(tab.completions.settle(Duration::from_secs(5)));
            tab.completions.with(|w| w.suggest(prefix, 8))
        };
        let all = |tab: &mut SessionTab| {
            assert!(tab.completions.settle(Duration::from_secs(5)));
            tab.completions.with(|w| w.words())
        };
        server.write_all(b"\x1b[32mA Vicious Wom").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("Wom"));
        assert!(words(&mut tab, "wom").is_empty(), "an unfinished line teaches nothing");
        server.write_all(b"prat\x1b[0m scurries past.\r\nA bantha ").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("bantha"));
        assert_eq!(words(&mut tab, "wom"), ["Womprat"]);
        assert!(words(&mut tab, "ban").is_empty());
        server.write_all(b"wanders by.\n").unwrap();
        pump_until(&mut tab, |t| !t.completions.with(|w| w.suggest("ban", 1)).is_empty());
        assert_eq!(words(&mut tab, "ban"), ["bantha"]);

        // Received while private: shown, never learned.
        tab.set_manual_private(true);
        server.write_all(b"A Sneaky Hunter lurks.\r\n").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("Hunter"));
        tab.set_manual_private(false);
        assert!(words(&mut tab, "hun").is_empty());

        // A sent command is learned, as words and as a history line.
        tab.input = "look zyxxyz".into();
        tab.submit();
        assert!(all(&mut tab).contains(&"zyxxyz".to_string()));
        assert_eq!(tab.history().last().map(String::as_str), Some("look zyxxyz"));
        // A private command is neither.
        tab.set_manual_private(true);
        tab.input = "qwertyuiop".into();
        tab.submit();
        tab.set_manual_private(false);
        assert!(!all(&mut tab).contains(&"qwertyuiop".to_string()));

        // With suggestions turned off nothing more is learned.
        tab.set_learning(false);
        server.write_all(b"A Gloomy Ghast drifts.\r\n").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("Ghast"));
        tab.pump(Instant::now());
        assert!(words(&mut tab, "gho").is_empty() && words(&mut tab, "gha").is_empty());
    }

    fn script(id: &str, source: &str) -> ScriptDefinition {
        ScriptDefinition {
            id: id.into(),
            name: id.into(),
            source: source.into(),
            enabled: true,
            restricted_send: false,
            runtime: Default::default(),
        }
    }

    /// Over loopback: the state seed is in place before a script's first line (values that came
    /// before it started), reading an unknown MSDP variable asks the world once, and GMCP, MSDP
    /// and GA prompts arrive as events.
    #[test]
    fn scripts_start_from_the_seed_and_get_protocol_events_over_loopback() {
        let (mut tab, mut server) = local_tab();
        pump_until(&mut tab, SessionTab::is_connected);
        server.write_all(&[255, 251, 201, 255, 251, 69]).unwrap();
        read_until_contains(&mut tab, &mut server, b"REPORTABLE_VARIABLES");
        server.write_all(&gmcp_frame(r#"Char.Vitals {"hp":42}"#)).unwrap();
        server.write_all(&msdp_frame("HEALTH", "100")).unwrap();
        pump_until(&mut tab, |t| {
            t.protocol.cache.msdp("HEALTH").is_some() && t.protocol.cache.gmcp_count() > 0
        });
        tab.set_scripts(
            vec![script(
                "s",
                r#"mud.echo("seed " + mud.state.get("msdp.HEALTH") + " " + mud.state.get("gmcp.Char.Vitals.hp"));
                   mud.echo("level " + mud.state.get("msdp.LEVEL"));
                   mud.on(Events.Gmcp, e => mud.echo("gmcp " + e.package + " " + e.data.hp));
                   mud.on(Events.Msdp, e => mud.echo("msdp " + e.variable + "=" + e.value));
                   mud.on(Events.Prompt, e => mud.echo("prompt " + e.text));"#,
            )],
            Instant::now(),
        );
        pump_until(&mut tab, |t| {
            t.terminal.transcript().contains("[script] level undefined")
        });
        assert!(tab.terminal.transcript().contains("[script] seed 100 42"));
        // The script read LEVEL: the world is asked for it, once.
        let asked = read_until_contains(&mut tab, &mut server, b"LEVEL");
        assert!(asked.windows(7).any(|w| w == b"\x01REPORT"));
        server.write_all(&gmcp_frame(r#"Char.Vitals {"hp":7}"#)).unwrap();
        server.write_all(&msdp_frame("LEVEL", "12")).unwrap();
        server.write_all(b"HP 7 > \xff\xf9").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("[script] prompt HP 7 >"));
        let text = tab.terminal.transcript();
        assert!(text.contains("[script] gmcp Char.Vitals 7"), "{text}");
        assert!(text.contains("[script] msdp LEVEL=12"), "{text}");
        assert_eq!(text.matches("[script] prompt").count(), 1);
        // A second mark with nothing new raises nothing.
        server.write_all(b"\xff\xf9").unwrap();
        server.write_all(b"\r\nDone.\r\n").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("Done."));
        tab.pump(Instant::now());
        assert_eq!(tab.terminal.transcript().matches("[script] prompt").count(), 1);
    }

    /// Over loopback: a script never sees private input. A line that arrives while the server
    /// has echo off, the password typed then, and a GMCP message in that stretch reach no
    /// script; an alias does not take a private command (it goes out as typed).
    #[test]
    fn scripts_never_see_private_input_over_loopback() {
        let (mut tab, mut server) = local_tab();
        pump_until(&mut tab, SessionTab::is_connected);
        server.write_all(&[255, 251, 201]).unwrap();
        tab.set_scripts(
            vec![script(
                "spy",
                r#"mud.on(Events.Line, e => mud.echo("line " + e.text));
                   mud.on(Events.Gmcp, e => mud.echo("gmcp " + e.package));
                   mud.alias(/^(.*)$/, m => mud.echo("typed " + m[1]));"#,
            )],
            Instant::now(),
        );
        pump_until(&mut tab, |t| t.scripts.entries[0].is_running());
        server.write_all(b"Public line\r\n").unwrap();
        pump_until(&mut tab, |t| {
            t.terminal.transcript().contains("[script] line Public line")
        });
        server.write_all(&[255, 251, 1]).unwrap();
        server.write_all(b"Private line\r\nPassword: ").unwrap();
        server.write_all(&gmcp_frame(r#"Room.Info {"name":"Vault"}"#)).unwrap();
        pump_until(&mut tab, |t| {
            t.server_echo && t.terminal.transcript().contains("Password:")
        });
        tab.input = "hunter2".into();
        tab.submit();
        let sent = read_until_contains(&mut tab, &mut server, b"hunter2");
        assert!(
            String::from_utf8_lossy(&sent).contains("hunter2"),
            "a private command goes out as typed"
        );
        server.write_all(&[255, 252, 1]).unwrap();
        server.write_all(b"\r\nWelcome.\r\n").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("[script] line Welcome."));
        let text = tab.terminal.transcript();
        assert!(!text.contains("[script] line Private"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(!text.contains("[script] gmcp Room.Info"), "{text}");
        assert!(!text.contains("[script] typed"), "{text}");
        // Public again: the alias takes typed commands (and echoes, sending nothing).
        tab.input = "look".into();
        tab.submit();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("[script] typed look"));
        assert_eq!(tab.history().last().map(String::as_str), Some("look"));
    }

    /// A runaway script fails alone; the session and the other script carry on.
    #[test]
    fn a_runaway_script_fails_alone_over_loopback() {
        let (mut tab, mut server) = local_tab();
        pump_until(&mut tab, SessionTab::is_connected);
        tab.set_scripts(
            vec![
                script("loop", "mud.on(Events.Line, () => { for (;;) {} });"),
                script("ok", "mud.trigger(/^ping$/, () => mud.send('pong'));"),
            ],
            Instant::now(),
        );
        pump_until(&mut tab, |t| t.scripts.entries.iter().all(|e| e.is_running()));
        server.write_all(b"ping\r\n").unwrap();
        read_until_contains(&mut tab, &mut server, b"pong");
        pump_until(&mut tab, |t| t.scripts.entries[0].error.is_some());
        assert!(tab.scripts.entries[1].is_running());
        assert!(tab.is_connected());
        server.write_all(b"ping\r\n").unwrap();
        read_until_contains(&mut tab, &mut server, b"pong");
        assert_eq!(tab.script_commands_sent, 2);
    }

    fn msdp_frame(variable: &str, value: &str) -> Vec<u8> {
        let mut out = vec![255, 250, 69, 1];
        out.extend_from_slice(variable.as_bytes());
        out.push(2);
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(&[255, 240]);
        out
    }

    fn msdp_mapping(port: u16) -> WorldMapping {
        use wandur_core::protocol::mapping::{FieldBinding, FieldReference, MappingEndpoint, MappingTarget};
        let bind = |v: &str, entity: &str, category: &str, key: &str, member: &str, conversion: &str| FieldBinding {
            source: FieldReference {
                protocol: "MSDP".into(),
                package: "MSDP".into(),
                path: format!("/{v}"),
            },
            target: MappingTarget {
                entity: entity.into(),
                category: category.into(),
                key: key.into(),
                member: member.into(),
            },
            label: key.into(),
            conversion: conversion.into(),
            scale: 1.0,
        };
        WorldMapping {
            schema_version: 1,
            world_id: "test".into(),
            endpoint: MappingEndpoint {
                host: "127.0.0.1".into(),
                port: i64::from(port),
                use_tls: false,
            },
            schema_fingerprint: "a".repeat(64),
            revision: 1,
            generated_at: "2026-10-08T12:00:00Z".into(),
            provenance: "deterministic".into(),
            provisional: true,
            bindings: vec![
                bind("HEALTH", "character", "resource", "health", "current", "number"),
                bind("HEALTHMAX", "character", "resource", "health", "maximum", "number"),
                bind("OPPONENTHEALTH", "opponent", "resource", "health", "current", "number"),
                bind("OPPONENTNAME", "opponent", "identity", "name", "value", "text"),
                bind("CHARACTERNAME", "character", "identity", "name", "value", "text"),
            ],
        }
    }

    /// Port of `ConsoleShowsTheRawStreamSentCommandsAndPrivateMarkersAndCanPauseAndClear` (the
    /// session side): the raw stream with visible escapes, sent commands, one `[private]` marker
    /// for the password exchange, and the typed password masked when the world echoes it.
    #[test]
    fn the_console_shows_the_raw_stream_sent_commands_and_private_markers() {
        let (mut tab, mut server) = local_tab();
        pump_until(&mut tab, SessionTab::is_connected);
        server.write_all(b"\x1b[32mA green line\x1b[0m\r\n").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("A green line"));
        tab.input = "look".into();
        tab.submit();
        read_lines(&mut server, 1);
        server.write_all(&[255, 251, 1]).unwrap();
        server.write_all(b"Password: ").unwrap();
        pump_until(&mut tab, |t| {
            t.private_input() && t.terminal.transcript().contains("Password:")
        });
        tab.input = "hunter2".into();
        tab.submit();
        read_lines(&mut server, 1);
        server.write_all(&[255, 252, 1]).unwrap();
        server.write_all(b"\r\nChecking...\r\n").unwrap();
        pump_until(&mut tab, |t| {
            !t.private_input() && t.terminal.transcript().contains("Checking")
        });
        server
            .write_all(b"Welcome back, your password hunter2 is weak.\r\n")
            .unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("is weak"));
        let console = tab.protocol.console.render(0);
        assert!(
            console.contains("<< \u{241b}[32mA green line\u{241b}[0m\u{240d}\u{240a}"),
            "{console}"
        );
        assert!(!console.contains('\u{1b}'));
        assert!(console.contains(">> look"));
        assert!(console.contains("Welcome back, your password [redacted] is weak."));
        assert!(!console.contains("hunter2"));
        assert_eq!(console.matches("[private]").count(), 1, "{console}");
        assert!(!console.contains("Password"));
        // The transcript itself is untouched.
        assert!(tab.terminal.transcript().contains("hunter2 is weak"));
        // A new connection starts an empty console.
        tab.reconnect();
        assert!(tab.protocol.console.is_empty());
    }

    /// `MappedVitalsFollowTheWorldWhileAGmcpLoginLingers`: a GMCP login answered but never
    /// confirmed keeps auto-login running; mapped MSDP vitals still reach the game state, the
    /// strip and the cache, and the reported name names the session.
    #[test]
    fn mapped_vitals_follow_the_world_while_a_gmcp_login_lingers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let vault = Arc::new(wandur_core::login::MemoryVault::new());
        vault.write("login-key", "secret").unwrap();
        let options = TabOptions {
            name: "Jedi world".into(),
            scrollback: 100,
            character: "tester".into(),
            login: Some(LoginConfig {
                username: "tester".into(),
                key: "login-key".into(),
                username_prompt: wandur_core::login::DEFAULT_USERNAME_PROMPT.into(),
                password_prompt: wandur_core::login::DEFAULT_PASSWORD_PROMPT.into(),
                vault: Arc::clone(&vault) as Arc<dyn PasswordVault>,
            }),
            mapping: Some(msdp_mapping(port)),
            ..TabOptions::default()
        };
        let mut tab = SessionTab::open(1, Endpoint::new("127.0.0.1", port), &options, Arc::new(|| {}));
        let (mut server, _) = listener.accept().unwrap();
        pump_until(&mut tab, |t| t.is_connected() && t.login.is_some());
        server.write_all(&[255, 251, 201, 255, 251, 69]).unwrap();
        read_until_contains(&mut tab, &mut server, b"REPORTABLE_VARIABLES");
        server
            .write_all(&gmcp_frame(
                r#"Char.Login.Default {"type":["password-credentials"],"version":1}"#,
            ))
            .unwrap();
        read_until_contains(&mut tab, &mut server, b"Char.Login.Credentials");
        assert!(tab.login_running() && !tab.private_input());
        let mut fight = Vec::new();
        for (v, value) in [
            ("CHARACTERNAME", "Tester"),
            ("HEALTH", "980"),
            ("HEALTHMAX", "1000"),
            ("OPPONENTNAME", "A Vicious Womprat"),
            ("OPPONENTHEALTH", "30"),
        ] {
            fight.extend(msdp_frame(v, value));
        }
        server.write_all(&fight).unwrap();
        pump_until(&mut tab, |t| t.protocol.cache.msdp("OPPONENTHEALTH").is_some());
        assert!(tab.login_running(), "the login still lingers");
        assert_eq!(tab.protocol.cache.msdp("HEALTH"), Some("\"980\""));
        let engine = tab.protocol.bindings().unwrap();
        assert_eq!(
            engine.character().resources["health"].current.as_ref().unwrap().value,
            980.0
        );
        assert_eq!(
            engine.opponent().resources["health"].current.as_ref().unwrap().value,
            30.0
        );
        assert_eq!(engine.opponent().identity["name"].value, "A Vicious Womprat");
        let cards = crate::vitals_view::cards(tab.protocol.bindings(), tab.is_connected());
        assert_eq!(cards.len(), 1, "the opponent has no maximum yet: no card");
        assert_eq!(cards[0].tip(), "Health: 980 / 1000");
        // The world reported the character: it names the session.
        assert_eq!(tab.title(), "Jedi world · Tester");
        // Diagnostics has the login offer (redacted copy) and every variable.
        assert!(
            tab.protocol
                .messages
                .entries()
                .any(|e| e.title() == "Char.Login.Default")
        );
        assert!(tab.protocol.messages.kinds().iter().any(|k| k.name == "OPPONENTHEALTH"));
        assert!(tab.protocol.cache.gmcp("Char.Login.Default").is_none());
    }

    /// When a private stretch ends (a text login with echo off), the mapped variables are asked
    /// for again with REPORT and SEND, since their first report usually came during the login.
    #[test]
    fn the_mapped_variables_are_asked_for_again_when_the_password_is_done() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let options = TabOptions {
            scrollback: 100,
            mapping: Some(msdp_mapping(port)),
            ..TabOptions::default()
        };
        let mut tab = SessionTab::open(1, Endpoint::new("127.0.0.1", port), &options, Arc::new(|| {}));
        let (mut server, _) = listener.accept().unwrap();
        pump_until(&mut tab, SessionTab::is_connected);
        server.write_all(&[255, 251, 69]).unwrap();
        server.write_all(&[255, 251, 1]).unwrap();
        server.write_all(b"Password: ").unwrap();
        read_until_contains(&mut tab, &mut server, b"REPORTABLE_VARIABLES");
        pump_until(&mut tab, |t| t.private_input());
        assert_eq!(tab.protocol.refreshes_sent, 0);
        server.write_all(&[255, 252, 1]).unwrap();
        server.write_all(b"\r\nWelcome.\r\n").unwrap();
        let got = read_until_contains(&mut tab, &mut server, b"\x01SEND\x02HEALTH");
        assert!(String::from_utf8_lossy(&got).contains("\u{1}REPORT\u{2}HEALTH\u{2}HEALTHMAX"));
        assert_eq!(tab.protocol.refreshes_sent, 1);
    }

    // Session history over loopback (the C# HistoryNoticeTests and recorder wiring).

    fn history_dir(name: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.superpowers/test-data")
            .join(format!("history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn history_tab(
        dir: &std::path::Path,
    ) -> (
        SessionTab,
        std::net::TcpStream,
        Arc<wandur_core::history::SqliteHistoryStore>,
    ) {
        let (db, _) = wandur_core::db::Database::open(dir).unwrap();
        let store = Arc::new(wandur_core::history::SqliteHistoryStore::new(db));
        let dyn_store: Arc<dyn HistoryStore> = store.clone();
        let (tab, server) = local_tab_with(TabOptions {
            scrollback: 100,
            name: "Copper Harbor".into(),
            character: "Mira".into(),
            history: Some(HistoryConfig {
                store: dyn_store,
                enabled: true,
                retention_days: 30,
                show_notice: true,
                clock: wandur_core::history::system_clock(),
            }),
            ..TabOptions::default()
        });
        (tab, server, store)
    }

    fn flush(tab: &mut SessionTab) {
        if let Some(done) = tab.flush_history() {
            done.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }

    fn search(store: &wandur_core::history::SqliteHistoryStore, query: &str) -> Vec<String> {
        use wandur_core::history::HistoryStore as _;
        store
            .search(query, &Default::default(), 0, 100)
            .unwrap()
            .into_iter()
            .map(|h| h.entry.text)
            .collect()
    }

    #[test]
    fn a_session_records_public_lines_and_commands_and_never_private_input() {
        let dir = history_dir("record");
        let (mut tab, mut server, store) = history_tab(&dir);
        pump_until(&mut tab, |t| t.is_connected() && t.is_recording());
        assert_eq!(tab.strip, Some(StripNotice::HistoryRecording));
        server
            .write_all(b"\x1b[32mA brass lantern\x1b[0m glows.\r\nPassword: ")
            .unwrap();
        server.write_all(&[255, 251, 1]).unwrap();
        pump_until(&mut tab, |t| t.server_echo);
        tab.input = "hunter2".into();
        tab.submit();
        server.write_all(&[255, 252, 1]).unwrap();
        server
            .write_all(b"\r\nWelcome back; your password hunter2 is weak.\r\nThe harbor is quiet.\r\n")
            .unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("harbor is quiet"));
        tab.input = "look".into();
        tab.submit();
        flush(&mut tab);
        assert_eq!(search(&store, "lantern"), ["A brass lantern glows."]);
        assert_eq!(search(&store, "look"), ["look"]);
        assert_eq!(search(&store, "quiet"), ["The harbor is quiet."]);
        assert!(search(&store, "hunter2").is_empty());
        // The echo of the private input is masked.
        assert_eq!(
            search(&store, "Welcome"),
            ["Welcome back; your password [redacted] is weak."]
        );
        let id = tab.history_session().unwrap().to_string();
        use wandur_core::history::HistoryStore as _;
        let entries = store.entries(&id, 0, 100).unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.kind == wandur_core::history::EntryKind::Private)
        );
        let session = &store.sessions(&Default::default(), 0, 10).unwrap()[0];
        assert_eq!(session.world_name, "Copper Harbor");
        assert_eq!(session.character_name, "Mira");
        assert!(session.world_key.starts_with("127.0.0.1:"));
        // Closing the connection ends the history session with its end time.
        drop(server);
        pump_until(&mut tab, |t| t.is_closed() && !t.is_recording());
        for handle in tab.take_history_drains() {
            handle.join().unwrap();
        }
        assert!(
            store.sessions(&Default::default(), 0, 10).unwrap()[0]
                .ended_at
                .is_some()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_storage_error_pauses_recording_with_a_notice_and_the_session_stays_connected() {
        let dir = history_dir("failure");
        let (mut tab, mut server, store) = history_tab(&dir);
        pump_until(&mut tab, |t| t.is_connected() && t.is_recording());
        // Every insert now fails, as a full disk or a broken file would.
        store
            .database()
            .connect()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_history BEFORE INSERT ON history_entries BEGIN SELECT RAISE(ABORT, 'disk'); END",
            )
            .unwrap();
        server.write_all(b"First line.\r\n").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("First line."));
        flush(&mut tab);
        pump_until(&mut tab, |t| matches!(t.strip, Some(StripNotice::Text(_))));
        assert_eq!(tab.strip, Some(StripNotice::Text(t(S::HistoryRecordingFailed).into())));
        assert!(!tab.is_recording());
        // Play goes on.
        server.write_all(b"Second line.\r\n").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("Second line."));
        assert!(tab.is_connected());
        tab.input = "look".into();
        tab.submit();
        let mut got = Vec::new();
        let mut buf = [0u8; 64];
        while !got.ends_with(b"look\r\n") {
            let n = server.read(&mut buf).unwrap();
            assert!(n > 0);
            got.extend_from_slice(&buf[..n]);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn turning_history_off_stops_recording_and_on_again_starts_a_new_session() {
        let dir = history_dir("toggle");
        let (mut tab, mut server, store) = history_tab(&dir);
        pump_until(&mut tab, |t| t.is_connected() && t.is_recording());
        let first = tab.history_session().unwrap().to_string();
        let mut config = tab.history_config.clone().unwrap();
        config.enabled = false;
        tab.set_history(Some(config.clone()));
        assert!(!tab.is_recording());
        server.write_all(b"Unrecorded line.\r\n").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("Unrecorded line."));
        config.enabled = true;
        config.show_notice = false;
        tab.strip = None;
        tab.set_history(Some(config));
        assert!(tab.is_recording());
        assert_ne!(tab.history_session(), Some(first.as_str()));
        assert_eq!(tab.strip, None, "Don't show again hides the reminder");
        server.write_all(b"Recorded again.\r\n").unwrap();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("Recorded again."));
        flush(&mut tab);
        for handle in tab.take_history_drains() {
            handle.join().unwrap();
        }
        assert!(search(&store, "Unrecorded").is_empty());
        assert_eq!(search(&store, "again"), ["Recorded again."]);
        use wandur_core::history::HistoryStore as _;
        assert_eq!(store.sessions(&Default::default(), 0, 10).unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
