//! The C# `ScriptSessionTests` cases and the session rules, against the real engine thread.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;

const PUBLIC: Gate = Gate {
    connected: true,
    private: false,
    login: false,
};
const PRIVATE: Gate = Gate {
    connected: true,
    private: true,
    login: false,
};

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

fn no_seed() -> Option<String> {
    None
}

struct Harness {
    scripts: SessionScripts,
    effects: Vec<Effect>,
    commands: Vec<CommandOutcome>,
}

impl Harness {
    fn new(definitions: Vec<ScriptDefinition>) -> Self {
        let mut scripts = SessionScripts::new("test-session-scripts", Arc::new(|| {}));
        scripts.set_library(definitions);
        Self {
            scripts,
            effects: Vec::new(),
            commands: Vec::new(),
        }
    }

    fn update(&mut self, gate: Gate) {
        let reports = self.scripts.update(gate, Instant::now(), &no_seed);
        self.effects.extend(reports);
    }

    fn poll(&mut self) {
        self.scripts.poll(Instant::now(), &mut self.effects, &mut self.commands);
    }

    fn until(&mut self, what: &str, done: impl Fn(&Harness) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(2));
            self.poll();
        }
    }

    fn running(&mut self) {
        self.update(PUBLIC);
        self.until("the scripts to run", |h| {
            h.scripts
                .entries
                .iter()
                .all(|e| !e.enabled || e.state == RunState::Running || e.error.is_some())
        });
    }

    fn echoes(&self) -> Vec<String> {
        self.effects
            .iter()
            .filter_map(|e| match e {
                Effect::Echo { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn sends(&self) -> Vec<String> {
        self.effects
            .iter()
            .filter_map(|e| match e {
                Effect::Send { command, .. } => Some(command.clone()),
                _ => None,
            })
            .collect()
    }
}

/// C# `EventsPublishedWhileTheScriptLoadsAreDeliveredOnceItRuns`: what is published while the
/// load is pending reaches the script once it runs, in order, and the seed is there first.
#[test]
fn events_published_while_a_script_loads_reach_it_once_it_runs() {
    let mut h = Harness::new(vec![script(
        "watch",
        "mud.echo('seeded:' + mud.state.get('msdp.HEALTH')); mud.on(Events.Msdp, e => mud.echo(e.variable + '=' + e.value));",
    )]);
    let seed = || Some(r#"{"gmcp":{},"msdp":{"HEALTH":"900"}}"#.to_string());
    h.scripts.update(PUBLIC, Instant::now(), &seed);
    assert_eq!(h.scripts.entries[0].state, RunState::Loading);
    assert_eq!(h.scripts.status(&h.scripts.entries[0]), ScriptStatus::Busy);
    let now = Instant::now();
    h.scripts.publish(ScriptEvent::msdp("LEVELCOMBAT", "\"4\""), now);
    h.scripts.publish(ScriptEvent::msdp("HEALTH", "\"1000\""), now);
    // A long login replay is queued behind the load, not lost.
    for i in 0..300 {
        h.scripts
            .publish(ScriptEvent::msdp(&format!("VAR{i}"), &format!("\"{i}\"")), now);
    }
    h.scripts.publish(ScriptEvent::msdp("LEVELCOMBAT", "\"5\""), now);
    h.until("303 events", |h| h.echoes().len() == 304);
    let echoes = h.echoes();
    assert_eq!(echoes[0], "seeded:900", "the seed arrives before the source runs");
    assert_eq!(echoes[1], "LEVELCOMBAT=4");
    assert_eq!(echoes[303], "LEVELCOMBAT=5");
    assert!(h.scripts.entries[0].is_running());

    // A privacy change while a load is pending invalidates what was queued with it.
    let mut h = Harness::new(vec![script("watch", "mud.on(Events.Msdp, e => mud.echo(e.variable));")]);
    h.update(PUBLIC);
    h.scripts.publish(ScriptEvent::msdp("MANA", "\"1\""), Instant::now());
    h.update(PRIVATE);
    h.update(PUBLIC);
    h.until("the load", |h| h.scripts.entries[0].is_running());
    std::thread::sleep(Duration::from_millis(50));
    h.poll();
    assert!(h.echoes().is_empty(), "{:?}", h.effects);
}

/// C# `FragmentedServerLinesAndPrivateInputAreIsolatedFromScripts`: private input reaches no
/// script, neither as lines nor as commands.
#[test]
fn private_input_never_reaches_a_script() {
    let mut h = Harness::new(vec![script(
        "spy",
        "mud.on(Events.Line, e => mud.echo('line:' + e.text)); mud.alias(/.*/, m => mud.echo('cmd:' + m[0]));",
    )]);
    h.running();
    h.scripts.feed_lines(&["Hello".into()], Instant::now());
    h.until("the first line", |h| h.echoes().len() == 1);
    h.update(PRIVATE);
    assert_eq!(h.scripts.status(&h.scripts.entries[0]), ScriptStatus::Paused);
    h.scripts.feed_lines(&["private text".into()], Instant::now());
    assert_eq!(
        h.scripts.command("secret", Instant::now()),
        None,
        "private commands bypass aliases"
    );
    h.update(PUBLIC);
    h.scripts.feed_lines(&["Public".into()], Instant::now());
    h.until("the public line", |h| h.echoes().len() == 2);
    assert_eq!(h.echoes(), ["line:Hello", "line:Public"]);
    h.scripts.set_enabled("spy", false, Instant::now(), &no_seed);
    h.scripts.feed_lines(&["stopped".into()], Instant::now());
    assert!(!h.scripts.entries[0].is_running());
    assert_eq!(h.scripts.status(&h.scripts.entries[0]), ScriptStatus::Disabled);
    assert!(h.sends().is_empty());
}

/// C# `StopDiscardsAnInFlightSendAndSavedSourceDoesNotAutoRun`: a command in flight when its
/// script stops is consumed and its send discarded.
#[test]
fn stopping_discards_an_in_flight_send_and_consumes_the_command() {
    let mut h = Harness::new(vec![script(
        "heal",
        "mud.alias(/^heal$/, () => mud.send('cast heal'));",
    )]);
    h.running();
    let ticket = h
        .scripts
        .command("heal", Instant::now())
        .expect("the alias script wants it");
    h.scripts.set_enabled("heal", false, Instant::now(), &no_seed);
    std::thread::sleep(Duration::from_millis(50));
    h.poll();
    // The stopped thread may never answer; a command it does answer is consumed.
    assert!(h.commands.iter().all(|c| c.ticket == ticket && c.consumed));
    assert!(h.sends().is_empty());
    assert!(!h.scripts.entries[0].is_running());
}

/// C# `EnteringPrivateModeInvalidatesEffectsEvenIfPrivacyEndsBeforeTheWorkerReplies`. The reply
/// is applied at the next poll, after both privacy changes, whenever the thread produced it.
#[test]
fn private_mode_invalidates_effects_even_when_it_ends_before_the_reply() {
    let mut h = Harness::new(vec![script(
        "heal",
        "mud.alias(/^heal$/, () => mud.send('cast heal'));",
    )]);
    h.running();
    let ticket = h.scripts.command("heal", Instant::now()).unwrap();
    h.update(PRIVATE);
    h.update(PUBLIC);
    h.until("the command", |h| !h.commands.is_empty());
    assert_eq!(h.commands, [CommandOutcome { ticket, consumed: true }]);
    assert!(h.sends().is_empty());
    assert!(h.scripts.entries[0].is_running());
}

/// C# `WorkerErrorsRemainVisibleAcrossAPrivacyTransition`.
#[test]
fn an_error_stays_visible_across_a_privacy_change() {
    let mut h = Harness::new(vec![script(
        "bad",
        "mud.alias(/^heal$/, () => { throw new Error('Script failed deliberately'); });",
    )]);
    h.running();
    h.scripts.command("heal", Instant::now()).unwrap();
    h.update(PRIVATE);
    h.update(PUBLIC);
    h.until("the command", |h| !h.commands.is_empty());
    assert!(h.commands[0].consumed);
    let entry = &h.scripts.entries[0];
    assert!(!entry.is_running());
    assert!(entry.error.as_deref().unwrap().contains("Script failed deliberately"));
    assert!(entry.log.contains("Script failed deliberately"));
    assert_eq!(h.scripts.status(entry), ScriptStatus::Failed);
}

#[test]
fn an_unmatched_command_is_not_consumed_and_a_script_without_aliases_is_not_asked() {
    let mut h = Harness::new(vec![
        script(
            "greet",
            "mud.alias(/^greet (.+)$/, m => mud.send('say Hello, ' + m[1] + '!'));",
        ),
        script("quiet", "mud.on(Events.Line, () => {});"),
    ]);
    h.running();
    let ticket = h.scripts.command("look", Instant::now()).unwrap();
    h.until("look", |h| !h.commands.is_empty());
    assert_eq!(
        h.commands,
        [CommandOutcome {
            ticket,
            consumed: false
        }]
    );
    let ticket = h.scripts.command("greet Wren", Instant::now()).unwrap();
    h.until("greet", |h| h.commands.len() == 2);
    assert_eq!(h.commands[1], CommandOutcome { ticket, consumed: true });
    assert_eq!(h.sends(), ["say Hello, Wren!"]);
    let mut h = Harness::new(vec![script("quiet", "mud.on(Events.Line, () => {});")]);
    h.running();
    assert_eq!(
        h.scripts.command("look", Instant::now()),
        None,
        "no alias: sent at once"
    );
}

#[test]
fn a_runaway_script_fails_alone_and_sending_too_fast_stops_a_script() {
    let mut h = Harness::new(vec![
        script("loop", "mud.on(Events.Line, () => { while (true) {} });"),
        script("ok", "mud.on(Events.Line, e => mud.echo(e.text));"),
        script(
            "flood",
            "mud.alias(/^spam$/, () => { for (let i = 0; i < 21; i++) mud.send('look'); });",
        ),
    ]);
    h.running();
    h.scripts.feed_lines(&["go".into()], Instant::now());
    h.until("the line", |h| {
        h.echoes().len() == 1 && h.scripts.entries[0].error.is_some()
    });
    assert!(
        h.scripts.entries[0]
            .error
            .as_deref()
            .unwrap()
            .contains(crate::scripting::engine::TIMED_OUT)
            || h.scripts.entries[0]
                .error
                .as_deref()
                .unwrap()
                .contains(crate::scripting::engine::TOO_MANY_STATEMENTS)
    );
    assert!(h.scripts.entries[1].is_running());
    h.scripts.command("spam", Instant::now()).unwrap();
    h.until("spam", |h| !h.commands.is_empty());
    assert_eq!(h.sends().len(), 20, "twenty a second");
    assert_eq!(
        h.scripts.entries[2].error.as_deref(),
        Some(tf(S::ScriptFailed, &[&t(S::ScriptRateExceeded)]).as_str())
    );
}

#[test]
fn too_many_waiting_events_stop_every_script() {
    let mut h = Harness::new(vec![script("a", "mud.on(Events.Line, () => {});")]);
    h.update(PUBLIC);
    let lines: Vec<String> = (0..=limits::QUEUED_EVENTS_PER_SESSION).map(|i| i.to_string()).collect();
    h.scripts.feed_lines(&lines, Instant::now());
    let entry = &h.scripts.entries[0];
    assert_eq!(entry.state, RunState::Stopped);
    assert_eq!(
        entry.error.as_deref(),
        Some(tf(S::ScriptFailed, &[&t(S::ScriptQueueOverflow)]).as_str())
    );
}

#[test]
fn a_failed_thread_restarts_the_scripts_at_most_three_times_in_five_minutes() {
    let mut h = Harness::new(vec![script("a", "mud.alias(/^x$/, () => mud.echo('x'));")]);
    h.running();
    assert_eq!(h.scripts.starts, 1);
    for restart in 1..=3 {
        h.scripts.crash_thread();
        h.until("the failure", |h| !h.scripts.has_thread());
        assert_eq!(
            h.scripts.entries[0].error.as_deref(),
            Some(tf(S::ScriptFailed, &[&t(S::ScriptWorkerFailed)]).as_str())
        );
        h.running();
        assert!(h.scripts.entries[0].is_running(), "restart {restart}");
        assert_eq!(h.scripts.entries[0].error, None);
        assert_eq!(h.scripts.starts, restart + 1);
    }
    h.scripts.crash_thread();
    h.until("the fourth failure", |h| !h.scripts.has_thread());
    h.update(PUBLIC);
    assert!(!h.scripts.has_thread(), "no fourth restart");
    assert_eq!(
        h.scripts.entries[0].error.as_deref(),
        Some(tf(S::ScriptFailed, &[&t(S::ScriptWorkerRestartLimit)]).as_str())
    );
    // The switch still starts it by hand.
    h.scripts.set_enabled("a", true, Instant::now(), &no_seed);
    h.until("the manual start", |h| h.scripts.entries[0].is_running());
}

#[test]
fn timers_reports_and_disconnect() {
    let mut h = Harness::new(vec![script(
        "t",
        "mud.every(1, () => mud.echo('tick')); mud.state.get('msdp.ROOMEXITS');",
    )]);
    h.running();
    assert_eq!(h.effects, [Effect::Report("ROOMEXITS".into())]);
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1300) {
        h.scripts.tick(Instant::now());
        h.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(h.echoes(), ["tick"], "one interval, no burst");
    // A private stretch ends: the variables read are asked for again.
    h.effects.clear();
    h.update(PRIVATE);
    h.update(PUBLIC);
    assert_eq!(h.effects, [Effect::Report("ROOMEXITS".into())]);
    h.update(Gate::default());
    assert!(!h.scripts.is_active());
    assert!(!h.scripts.has_thread());
    assert_eq!(h.scripts.status(&h.scripts.entries[0]), ScriptStatus::Waiting);
    h.running();
    assert!(h.scripts.entries[0].is_running(), "a new connection starts it again");
}

#[test]
fn a_pack_script_may_send_only_from_aliases_and_its_error_is_named() {
    let mut definition = script(
        "pack",
        "mud.alias(/^go$/, () => mud.send('north')); mud.on(Events.Line, () => mud.send('look'));",
    );
    definition.restricted_send = true;
    let mut h = Harness::new(vec![definition]);
    h.running();
    h.scripts.command("go", Instant::now()).unwrap();
    h.until("go", |h| !h.commands.is_empty());
    assert_eq!(h.sends(), ["north"]);
    h.scripts.feed_lines(&["You see a road.".into()], Instant::now());
    h.until("the refusal", |h| h.scripts.entries[0].error.is_some());
    assert_eq!(
        h.scripts.entries[0].error.as_deref(),
        Some(tf(S::ScriptFailed, &[&t(S::ScriptPackSendRefused)]).as_str())
    );
}

/// A panel callback reaches only the script that declared the panel; a button click may send
/// under the pack policy, a toggle may not; allowing sending restarts the script under the
/// lifted policy; a rejected panel instruction stops the script with its message.
#[test]
fn panel_callbacks_go_to_their_script_and_the_send_policy_can_be_lifted() {
    let source = r#"
        const p = mud.panel("ship");
        p.button("flee", { onClick: () => mud.send("flee") });
        p.toggle("auto", { onChange: () => mud.send("repair") });
        mud.alias(/^hi$/, () => mud.echo("hi from " + "pack"));
    "#;
    let mut pack = script("pack", source);
    pack.restricted_send = true;
    let other = script(
        "other",
        "mud.on(Events.Line, () => {}); mud.alias(/^x$/, () => mud.echo('other'));",
    );
    let mut h = Harness::new(vec![pack, other]);
    h.running();
    let click = super::super::panels::event_json("ship", "flee", super::super::panels::PanelEvent::Click, None);
    assert!(h.scripts.publish_to(
        "pack",
        ScriptEvent::new(EventKind::Panel, click.clone()),
        Instant::now()
    ));
    h.until("the click", |h| !h.sends().is_empty());
    assert_eq!(h.sends(), ["flee"]);
    assert!(
        !h.scripts
            .publish_to("missing", ScriptEvent::new(EventKind::Panel, click), Instant::now()),
        "no such script"
    );

    let toggle = super::super::panels::event_json("ship", "auto", super::super::panels::PanelEvent::Change(true), None);
    h.scripts.publish_to(
        "pack",
        ScriptEvent::new(EventKind::Panel, toggle.clone()),
        Instant::now(),
    );
    h.until("the refusal", |h| h.scripts.entries[0].error.is_some());
    assert!(h.scripts.entries[1].is_running(), "the other script runs on");

    // Allowed: the script starts again and the toggle may send.
    h.scripts.set_restricted_send("pack", false, Instant::now(), &no_seed);
    h.until("the restart", |h| h.scripts.entries[0].is_running());
    assert!(h.scripts.entries[0].error.is_none());
    h.scripts
        .publish_to("pack", ScriptEvent::new(EventKind::Panel, toggle), Instant::now());
    h.until("the repair", |h| h.sends().len() == 2);
    assert_eq!(h.sends(), ["flee", "repair"]);

    h.scripts.reject("other", "A panel instruction was rejected: x");
    assert!(!h.scripts.entries[1].is_running());
    assert!(h.scripts.entries[1].error.as_deref().unwrap().contains("rejected: x"));
}

/// The C# `LuaScriptLibraryTests` rules: a Lua script runs only while Lua scripts are allowed;
/// switching it on while they are not says so; allowing them starts it, and stopping them stops
/// it without touching the JavaScript beside it.
#[cfg(feature = "lua")]
#[test]
fn lua_scripts_run_only_while_the_preference_allows_them() {
    let lua = ScriptDefinition {
        runtime: Runtime::LUA,
        ..script("lua", "mud.alias('^camp$', function() mud.echo('lua camp') end)")
    };
    let js = script("js", "mud.alias(/^js$/, () => mud.echo('js'));");
    let mut h = Harness::new(vec![lua, js]);
    h.update(PUBLIC);
    h.until("the JavaScript to run", |h| h.scripts.entries[1].is_running());
    assert_eq!(h.scripts.entries[0].state, RunState::Stopped, "Lua is off by default");
    assert_eq!(h.scripts.entries[1].state, RunState::Running);
    h.scripts.set_enabled("lua", true, Instant::now(), &no_seed);
    assert_eq!(h.scripts.entries[0].error.as_deref(), Some(t(S::LuaScriptsTurnedOff)));
    h.scripts.set_lua_enabled(true, Instant::now(), &no_seed);
    assert_eq!(h.scripts.entries[0].error, None);
    h.running();
    assert_eq!(h.scripts.entries[0].state, RunState::Running);
    let ticket = h
        .scripts
        .command("camp", Instant::now())
        .expect("an alias script waits");
    h.until("the alias", |h| h.commands.iter().any(|c| c.ticket == ticket));
    assert!(h.echoes().contains(&"lua camp".to_string()));
    h.scripts.set_lua_enabled(false, Instant::now(), &no_seed);
    assert_eq!(h.scripts.entries[0].state, RunState::Stopped);
    assert_eq!(h.scripts.entries[1].state, RunState::Running);
}

/// C# `SetSuspended`: an agent's Step or Play stops every script; giving control back starts
/// none (a restart could replay top-level sends); switching a script on, or a reload, does.
#[test]
fn an_agent_suspends_scripts_and_they_resume_only_when_asked() {
    let mut h = Harness::new(vec![script("a", "mud.echo('started');")]);
    h.running();
    assert!(h.scripts.is_active());
    h.scripts.set_suspended(true);
    assert!(h.scripts.is_suspended());
    assert!(!h.scripts.is_active());
    h.update(PUBLIC);
    assert!(!h.scripts.is_active(), "nothing starts while suspended");
    h.scripts.set_suspended(false);
    h.update(PUBLIC);
    assert!(!h.scripts.is_active(), "giving control back starts nothing");
    h.scripts.set_enabled("a", true, Instant::now(), &no_seed);
    h.running();
    assert!(h.scripts.entries[0].is_running(), "the switch starts it again");
    h.scripts.set_suspended(true);
    h.scripts.set_library(vec![script("a", "mud.echo('again');")]);
    h.update(PUBLIC);
    assert!(!h.scripts.is_active());
    h.scripts.set_suspended(false);
    h.scripts.set_library(vec![script("a", "mud.echo('again');")]);
    h.running();
    assert!(h.scripts.entries[0].is_running(), "a reload starts it again");
}
