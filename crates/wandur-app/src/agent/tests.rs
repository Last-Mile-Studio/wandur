//! The agent in a session over loopback (the C# `AgentSessionTests` and the runner rules that
//! need a real connection): a session tab against a socket the test plays the world on, and a
//! fake model the test controls.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use wandur_core::agent::decision_codec::{AgentDecision, AgentRequest};
use wandur_core::agent::profile::{AgentGoal, AgentProfile, OPENAI_COMPATIBLE};
use wandur_core::agent::{
    AgentError, AgentModelProvider, AgentProfileStore, AgentRunMode, AgentStatus, AgentWorld, CancelToken,
    MemoryAgentProfileStore, ProviderRegistry,
};
use wandur_core::login::{MemoryVault, PasswordVault};

use crate::agent_session::AgentServices;
use crate::session_tab::test_support::{local_tab_with, pump_until};
use crate::session_tab::{AgentStop, LoginConfig, SessionTab, TabOptions};

/// The fake model (the C# test `Services`): it records each request and answers `look`, or
/// waits for an answer the test gives (`pending`), ignoring cancellation.
#[derive(Default)]
struct Model {
    requests: Mutex<Vec<AgentRequest>>,
    pending: Mutex<Option<mpsc::Receiver<AgentDecision>>>,
    action: Mutex<Option<String>>,
}

struct Fake(Arc<Model>);

impl AgentModelProvider for Fake {
    fn key(&self) -> &str {
        OPENAI_COMPATIBLE
    }
    fn decide(&self, request: &AgentRequest, _: Option<&str>, _: &CancelToken) -> Result<AgentDecision, AgentError> {
        self.0.requests.lock().unwrap().push(request.clone());
        let pending = self.0.pending.lock().unwrap().take();
        if let Some(rx) = pending {
            return rx.recv().map_err(|_| AgentError::Cancelled);
        }
        let action = self.0.action.lock().unwrap().clone().unwrap_or_else(|| "look".into());
        Ok(AgentDecision::new(&action, "Observe", "remember room"))
    }
    fn list_models(&self, _: &AgentProfile, _: Option<&str>, _: &CancelToken) -> Result<Vec<String>, AgentError> {
        Ok(Vec::new())
    }
}

struct World {
    tab: SessionTab,
    server: TcpStream,
    model: Arc<Model>,
    store: Arc<MemoryAgentProfileStore>,
    services: Arc<AgentServices>,
}

fn profile() -> AgentProfile {
    AgentProfile {
        model: "test".into(),
        default_goal: "Explore".into(),
        response_timeout_seconds: 2,
        action_interval_seconds: 1,
        ..AgentProfile::default()
    }
}

fn agent_world() -> AgentWorld {
    AgentWorld::Key("world".into())
}

impl World {
    fn open() -> World {
        Self::open_with(TabOptions {
            scrollback: 200,
            ..TabOptions::default()
        })
    }

    fn open_with(options: TabOptions) -> World {
        let model = Arc::new(Model::default());
        let store = Arc::new(MemoryAgentProfileStore::with(&agent_world(), profile()));
        let services = Arc::new(AgentServices {
            store: Arc::clone(&store) as Arc<dyn AgentProfileStore>,
            providers: ProviderRegistry::new(vec![Arc::new(Fake(Arc::clone(&model)))]),
            vault: Arc::new(MemoryVault::new()),
            discover: false,
        });
        let (mut tab, server) = local_tab_with(options);
        tab.configure_agent(Arc::clone(&services), agent_world());
        pump_until(&mut tab, SessionTab::is_connected);
        World {
            tab,
            server,
            model,
            store,
            services,
        }
    }

    /// Server text, pumped until the session has it.
    fn output(&mut self, text: &str) {
        self.output_bytes(text.as_bytes());
        let needle = text.trim_end().lines().last().unwrap_or_default().to_string();
        pump_until(&mut self.tab, |t| t.terminal.transcript().contains(&needle));
    }

    fn output_bytes(&mut self, bytes: &[u8]) {
        self.server.write_all(bytes).unwrap();
        self.pump_for(60);
    }

    fn pump_for(&mut self, ms: u64) {
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until {
            self.tab.pump(Instant::now());
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The next command the client sent (telnet negotiation dropped), pumping the session
    /// while the server waits for it.
    fn command(&mut self) -> String {
        self.server.set_read_timeout(Some(Duration::from_millis(10))).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        let mut skip = 0;
        loop {
            assert!(Instant::now() < deadline, "no command arrived");
            self.tab.pump(Instant::now());
            match self.server.read(&mut byte) {
                Ok(1) => {}
                Ok(_) => panic!("connection closed"),
                Err(_) => continue,
            }
            if skip > 0 {
                skip -= 1;
                continue;
            }
            match byte[0] {
                255 => skip = 2,
                b'\n' => break,
                b'\r' => {}
                b => line.push(b),
            }
        }
        self.server.set_read_timeout(None).unwrap();
        String::from_utf8(line).unwrap()
    }

    /// Nothing but telnet negotiation reached the server (pumping meanwhile).
    fn server_got_nothing(&mut self) -> bool {
        self.server.set_read_timeout(Some(Duration::from_millis(10))).unwrap();
        let until = Instant::now() + Duration::from_millis(150);
        let mut got = Vec::new();
        let mut buf = [0u8; 256];
        while Instant::now() < until {
            self.tab.pump(Instant::now());
            if let Ok(n) = self.server.read(&mut buf) {
                got.extend_from_slice(&buf[..n]);
            }
        }
        self.server.set_read_timeout(None).unwrap();
        let mut data = Vec::new();
        let mut i = 0;
        while i < got.len() {
            if got[i] == 255 {
                i += 3;
            } else {
                data.push(got[i]);
                i += 1;
            }
        }
        data.is_empty()
    }

    fn requests(&self) -> Vec<AgentRequest> {
        self.model.requests.lock().unwrap().clone()
    }

    /// Hold the model's next answer until the returned sender gives one.
    fn hold(&self) -> mpsc::Sender<AgentDecision> {
        let (tx, rx) = mpsc::channel();
        *self.model.pending.lock().unwrap() = Some(rx);
        tx
    }

    fn until_idle(&mut self) {
        pump_until(&mut self.tab, |t| !t.agent.is_busy());
    }

    /// Wait until a model call is out.
    fn until_asked(&mut self, count: usize) {
        let model = Arc::clone(&self.model);
        pump_until(&mut self.tab, move |_| model.requests.lock().unwrap().len() >= count);
    }

    fn save(&mut self, profile: &AgentProfile) {
        self.store.save(&agent_world(), profile).unwrap();
        self.tab.agent_profile_saved(profile);
    }
}

#[test]
fn choosing_another_goal_replaces_the_selection_and_sends_its_description_and_rules() {
    let mut w = World::open();
    let goals = vec![
        AgentGoal::new("Explore the academy", true),
        AgentGoal {
            name: "Observe".into(),
            rules: "- Do not move\n- Stop after looking".into(),
            ..AgentGoal::new("Review the room", false)
        },
    ];
    let saved = AgentProfile {
        default_goal: String::new(),
        goals,
        ..profile()
    };
    w.save(&saved);
    w.tab.agent_select_goal(1);
    assert!(!w.tab.agent.goals[0].enabled);
    assert_eq!(w.tab.agent.goals.iter().filter(|g| g.enabled).count(), 1);
    w.tab.agent_start(AgentRunMode::Preview);
    w.until_idle();
    let goal = &w.requests()[0].goal;
    assert!(goal.contains("Review the room"));
    assert!(goal.contains("Do not move"));
    assert!(!goal.contains("Explore the academy"));
    assert!(
        w.store.load(&agent_world()).unwrap().goals[0].enabled,
        "the saved default is not changed"
    );
}

#[test]
fn the_chosen_goal_drives_requests_and_changing_it_stops_pending_work() {
    let mut w = World::open();
    let saved = AgentProfile {
        default_goal: String::new(),
        goals: vec![AgentGoal::new("Explore", true), AgentGoal::new("Find food", false)],
        ..profile()
    };
    w.save(&saved);
    assert_eq!(w.tab.agent.goals.len(), 2);
    w.tab.agent_start(AgentRunMode::Preview);
    w.until_idle();
    assert!(w.requests()[0].goal.contains("Explore"));
    assert!(!w.requests()[0].goal.contains("Find food"));
    let answer = w.hold();
    w.tab.agent_select_goal(1);
    w.tab.agent_start(AgentRunMode::Run);
    assert!(w.tab.agent.is_busy());
    w.until_asked(2);
    w.tab.agent_select_goal(0);
    assert!(!w.tab.agent.is_busy(), "a goal change stops the run");
    answer.send(AgentDecision::new("look", "", "")).unwrap();
    w.pump_for(80);
    assert!(w.server_got_nothing());
    assert!(!w.store.load(&agent_world()).unwrap().goals[1].enabled);
}

#[test]
fn preview_uses_public_context_without_sending_and_step_waits_for_output() {
    let mut w = World::open();
    w.output("A quiet room.\r\n");
    w.tab.agent_start(AgentRunMode::Preview);
    w.until_idle();
    assert!(w.requests()[0].observation.contains("A quiet room"));
    assert_eq!(w.tab.agent.runner.status(), AgentStatus::PreviewReady);
    assert!(w.server_got_nothing());
    w.tab.agent_start(AgentRunMode::Step);
    assert_eq!(w.command(), "look");
    assert_eq!(w.tab.agent.runner.status(), AgentStatus::Waiting);
    w.output("A doorway leads north.\r\n");
    w.until_idle();
    assert_eq!(w.tab.agent.runner.memory(), "remember room");
    assert_eq!(w.tab.agent.runner.status(), AgentStatus::Paused);
    assert!(w.server_got_nothing(), "Step sends one command");
    assert_eq!(w.tab.agent_commands_sent, 1);
}

#[test]
fn manual_input_cancels_an_uncooperative_model_and_still_sends_the_manual_command() {
    let mut w = World::open();
    let answer = w.hold();
    w.tab.agent_start(AgentRunMode::Run);
    assert!(w.tab.agent.is_busy());
    w.until_asked(1);
    w.tab.input = "score".into();
    w.tab.submit();
    assert_eq!(w.command(), "score");
    assert!(!w.tab.agent.is_busy());
    assert_eq!(w.tab.agent.runner.status(), AgentStatus::Manual);
    answer.send(AgentDecision::new("look", "stale", "stale")).unwrap();
    w.pump_for(60);
    assert!(w.server_got_nothing());
    assert_eq!(w.tab.agent.runner.memory(), "");
}

#[test]
fn private_input_and_echo_bursts_never_reach_the_model() {
    let mut w = World::open();
    let answer = w.hold();
    w.tab.agent_start(AgentRunMode::Run);
    w.until_asked(1);
    // A burst with echo off and on again in one write: the line between is private.
    let mut burst = vec![255, 251, 1];
    burst.extend_from_slice(b"private-secret\r\n");
    burst.extend_from_slice(&[255, 252, 1]);
    w.output_bytes(&burst);
    pump_until(&mut w.tab, |t| t.terminal.transcript().contains("private-secret"));
    assert!(!w.tab.agent.is_busy(), "the private stretch stopped the run");
    answer.send(AgentDecision::new("look", "stale", "stale")).unwrap();
    w.output("A public room.\r\n");
    w.tab.agent_start(AgentRunMode::Preview);
    w.until_idle();
    let seen = w.requests().last().unwrap().observation.clone();
    assert!(!seen.contains("private-secret"), "{seen}");
    assert!(seen.contains("A public room"));
    w.tab.set_manual_private(true);
    w.output("another-secret\r\n");
    w.tab.set_manual_private(false);
    w.output("public-again\r\n");
    w.tab.agent_start(AgentRunMode::Preview);
    w.until_idle();
    let seen = w.requests().last().unwrap().observation.clone();
    assert!(!seen.contains("another-secret"), "{seen}");
    assert!(seen.contains("public-again"));
    assert!(w.server_got_nothing());
}

#[test]
fn separate_connections_share_neither_memory_nor_goal_and_saving_stops_a_run() {
    let mut first = World::open();
    let mut second = World::open();
    first.tab.agent.goals[0].text = "first goal".into();
    second.tab.agent.goals[0].text = "second goal".into();
    let answer = first.hold();
    first.tab.agent_start(AgentRunMode::Run);
    first.until_asked(1);
    let saved = first.store.load(&agent_world()).unwrap();
    first.save(&saved);
    assert!(!first.tab.agent.is_busy(), "saving the settings stops the run");
    assert_eq!(second.tab.agent.goal(), "second goal");
    assert_eq!(second.tab.agent.runner.memory(), "");
    answer.send(AgentDecision::new("look", "", "")).unwrap();
    first.pump_for(60);
    assert!(first.server_got_nothing());
    assert!(second.server_got_nothing());
}

#[test]
fn only_allowed_commands_are_sent() {
    let mut w = World::open();
    *w.model.action.lock().unwrap() = Some("quit".into());
    w.tab.agent_start(AgentRunMode::Step);
    w.until_idle();
    assert_eq!(w.tab.agent.runner.status(), AgentStatus::Failed);
    assert!(w.server_got_nothing());
    // An allowed id sends its fixed command, not the id.
    let custom = AgentProfile {
        commands: "go_north | north | Move north".into(),
        ..profile()
    };
    w.save(&custom);
    *w.model.action.lock().unwrap() = Some("go_north".into());
    w.tab.agent_start(AgentRunMode::Step);
    assert_eq!(w.command(), "north");
}

#[test]
fn while_the_agent_has_control_macros_and_scripts_send_nothing() {
    use wandur_core::macros::{MacroDefinition, MacroKind, SavedMacro};
    let mut w = World::open();
    let trigger = SavedMacro {
        id: "greet".into(),
        name: "greet".into(),
        enabled: true,
        definition: MacroDefinition::new(MacroKind::Trigger, "Welcome", "wave"),
    };
    w.tab.set_macros(Some("w".into()), vec![trigger], Instant::now());
    let answer = w.hold();
    w.tab.agent_start(AgentRunMode::Run);
    w.until_asked(1);
    assert!(w.tab.agent.has_control());
    assert!(w.tab.scripts.is_suspended());
    w.output("Welcome aboard.\r\n");
    w.pump_for(80);
    assert_eq!(w.tab.macro_commands_sent, 0);
    assert!(w.server_got_nothing());
    w.tab.agent_stop();
    assert!(!w.tab.agent.has_control());
    assert!(!w.tab.scripts.is_suspended());
    drop(answer);
}

#[test]
fn walking_and_disconnecting_cancel_a_run() {
    let mut w = World::open();
    let answer = w.hold();
    w.tab.agent_start(AgentRunMode::Run);
    w.until_asked(1);
    // A walk takes over (an empty route: the walk itself fails, the agent stops anyway).
    w.tab.walk_route(&wandur_core::map::MapRoute {
        steps: Vec::new(),
        cost: 0.0,
    });
    assert!(!w.tab.agent.is_busy());
    assert_eq!(w.tab.agent.runner.status(), AgentStatus::Manual);
    drop(answer);
    let answer = w.hold();
    w.tab.agent_start(AgentRunMode::Run);
    w.until_asked(2);
    w.tab.disconnect();
    assert!(!w.tab.agent.is_busy());
    assert_eq!(w.tab.agent.runner.status(), AgentStatus::Paused);
    answer.send(AgentDecision::new("look", "", "")).unwrap();
    w.pump_for(60);
    assert_eq!(w.tab.agent_commands_sent, 0);
}

#[test]
fn a_closed_connection_cancels_a_run_and_a_new_one_starts_clean() {
    let mut w = World::open();
    w.tab.agent_start(AgentRunMode::Step);
    assert_eq!(w.command(), "look");
    w.output("Fresh output.\r\n");
    w.until_idle();
    assert_eq!(w.tab.agent.runner.memory(), "remember room");
    let answer = w.hold();
    w.tab.agent_start(AgentRunMode::Run);
    w.until_asked(2);
    w.server.shutdown(std::net::Shutdown::Both).unwrap();
    pump_until(&mut w.tab, |t| !t.agent.is_busy());
    assert_eq!(w.tab.agent.runner.status(), AgentStatus::Paused);
    drop(answer);
    w.tab.agent_start(AgentRunMode::Preview);
    assert_eq!(
        w.tab.agent.runner.status(),
        AgentStatus::Unavailable,
        "no run without a connection"
    );
}

/// Text sent while auto-login runs never reaches the model, and the agent cannot start
/// before the login is over.
#[test]
fn login_text_never_reaches_the_model() {
    let vault = Arc::new(MemoryVault::new());
    vault.write("login-key", "secret-x").unwrap();
    let options = TabOptions {
        scrollback: 200,
        character: "odo".into(),
        password_prompt: wandur_core::login::DEFAULT_PASSWORD_PROMPT.into(),
        login: Some(LoginConfig {
            username: "odo".into(),
            key: "login-key".into(),
            username_prompt: wandur_core::login::DEFAULT_USERNAME_PROMPT.into(),
            password_prompt: wandur_core::login::DEFAULT_PASSWORD_PROMPT.into(),
            vault: Arc::clone(&vault) as Arc<dyn PasswordVault>,
        }),
        ..TabOptions::default()
    };
    let mut w = World::open_with(options);
    pump_until(&mut w.tab, |t| t.login_running());
    w.tab.agent_start(AgentRunMode::Preview);
    assert_eq!(w.tab.agent.runner.status(), AgentStatus::Unavailable);
    w.output_bytes(b"Account notes: ledger-7731\r\nName: ");
    pump_until(&mut w.tab, |t| t.login_sends == 1);
    w.output_bytes(b"Password: ");
    pump_until(&mut w.tab, |t| t.login_sends == 2 && !t.login_running());
    w.output("You wake in the inn.\r\n");
    w.tab.agent_start(AgentRunMode::Preview);
    w.until_idle();
    let seen = w.requests()[0].observation.clone();
    assert!(!seen.contains("ledger-7731"), "{seen}");
    assert!(!seen.contains("secret-x"));
    assert!(seen.contains("You wake in the inn."));
}

/// Model calls never hold up the UI thread: every pump returns at once while the model thinks.
#[test]
fn pumps_never_wait_on_the_model() {
    let mut w = World::open();
    let answer = w.hold();
    w.tab.agent_start(AgentRunMode::Run);
    w.until_asked(1);
    let mut slowest = Duration::ZERO;
    for _ in 0..40 {
        let started = Instant::now();
        w.tab.pump(Instant::now());
        slowest = slowest.max(started.elapsed());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(slowest < Duration::from_millis(50), "{slowest:?}");
    assert!(w.tab.agent.is_busy());
    w.tab.agent_reset(AgentStop::Paused);
    drop(answer);
    let _ = &w.services;
}

/// AgentSessionTests.FooterFitsAlongsideOutputTabsAndShowsLiveStatus and
/// AgentSectionAndLiveMenuBindToTheOriginatingWorld: the footer shows Agent ▾, Play and the
/// live status; the menu shows the goals as radio buttons, Play and Stop, the status, Recent
/// decisions, More controls and Configure agent...; a click on another goal chooses it.
#[test]
fn the_footer_and_the_menu_show_the_agent_and_a_click_chooses_a_goal() {
    use crate::fonts::FallbackFonts;
    use crate::terminal_view::{PaintOptions, TermFonts, TerminalViewState, show};
    use crate::theme::Theme;
    use egui::{Event, PointerButton, RawInput, Rect, pos2, vec2};
    use wandur_core::l10n::{S, t};

    let mut w = World::open();
    let saved = AgentProfile {
        default_goal: String::new(),
        goals: vec![
            AgentGoal {
                name: "Scout the road".into(),
                ..AgentGoal::new("Walk east.", true)
            },
            AgentGoal {
                name: "Keep watch at the Rest".into(),
                ..AgentGoal::new("Stay.", false)
            },
        ],
        ..profile()
    };
    w.save(&saved);
    w.tab
        .agent
        .runner
        .note_activity("look: Check the crossroads", AgentStatus::Paused);
    w.tab.world = Some(0);
    let ctx = egui::Context::default();
    crate::fonts::install(&ctx);
    let theme = Theme::preset("Hull");
    let fonts = TermFonts::new(13.0);
    let mut fallback = FallbackFonts::new();
    let mut view = TerminalViewState::default();
    view.agent_menu = true;
    view.agent_activity_open = true;
    let options = PaintOptions::default();
    let mut frame = |tab: &mut SessionTab, view: &mut TerminalViewState, events: Vec<Event>| {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 700.0))),
            events,
            ..Default::default()
        };
        let mut texts = Vec::new();
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                show(ui, tab, view, &theme, &fonts, &mut fallback, &options);
            });
        });
        out.textures_delta.clear();
        for clipped in &out.shapes {
            if let egui::Shape::Text(text) = &clipped.shape {
                texts.push((
                    text.galley.text().to_string(),
                    Rect::from_min_size(text.pos, text.galley.size()),
                ));
            }
        }
        texts
    };
    frame(&mut w.tab, &mut view, vec![]);
    let texts = frame(&mut w.tab, &mut view, vec![]);
    let has = |s: &str| texts.iter().any(|(text, _)| text == s);
    for expected in [
        t(S::AgentMenu),
        t(S::AgentGoals),
        "Scout the road",
        "Keep watch at the Rest",
        t(S::AgentPlay),
        t(S::AgentStop),
        t(S::AgentPaused),
        t(S::AgentActivity),
        "look: Check the crossroads",
        t(S::AgentAdvanced),
        t(S::AgentConfigure),
        t(S::AgentScriptsPaused),
    ] {
        assert!(
            has(expected),
            "{expected:?} not drawn: {:?}",
            texts.iter().map(|t| &t.0).collect::<Vec<_>>()
        );
    }
    // The footer's status sits inside the window, on the footer row.
    let footer_status = texts
        .iter()
        .filter(|(text, _)| text == t(S::AgentPaused))
        .map(|(_, r)| *r)
        .max_by(|a, b| a.top().total_cmp(&b.top()))
        .unwrap();
    assert!(
        footer_status.right() < 900.0 && footer_status.top() > 600.0,
        "{footer_status:?}"
    );
    let target = texts
        .iter()
        .find(|(text, _)| text == "Keep watch at the Rest")
        .map(|(_, r)| r.center())
        .unwrap();
    frame(&mut w.tab, &mut view, vec![Event::PointerMoved(target)]);
    let press = |pressed| Event::PointerButton {
        pos: target,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(&mut w.tab, &mut view, vec![press(true)]);
    frame(&mut w.tab, &mut view, vec![press(false)]);
    assert!(w.tab.agent.goals[1].enabled, "the click chose the goal");
    assert!(!w.tab.agent.goals[0].enabled);
    assert_eq!(
        w.tab.agent.runner.status(),
        AgentStatus::Stopped,
        "a goal change clears the memory"
    );
    // Configure agent... asks for the world editor's Agent settings.
    let texts = frame(&mut w.tab, &mut view, vec![]);
    let configure = texts
        .iter()
        .find(|(text, _)| text == t(S::AgentConfigure))
        .map(|(_, r)| r.center())
        .unwrap();
    frame(&mut w.tab, &mut view, vec![Event::PointerMoved(configure)]);
    let click = |pressed| Event::PointerButton {
        pos: configure,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(&mut w.tab, &mut view, vec![click(true)]);
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 700.0))),
        events: vec![click(false)],
        ..Default::default()
    };
    let mut actions = None;
    let mut out = ctx.run_ui(input, |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            actions = Some(show(ui, &mut w.tab, &mut view, &theme, &fonts, &mut fallback, &options));
        });
    });
    out.textures_delta.clear();
    assert!(actions.unwrap().edit_agent);
    assert!(!view.agent_menu, "the menu closes");
}
