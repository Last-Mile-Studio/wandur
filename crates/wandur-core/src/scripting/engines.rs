//! The engines of one session, one per script, keyed by the id the host gives each script (the
//! C# `ScriptEngineSet`). Every call runs on the calling thread, one engine at a time, so no
//! script's callbacks interleave with another's. An engine that fails is dropped and its error
//! goes back for that id only; the others are untouched.

use std::collections::HashMap;

use super::engine::Engine;
use super::{EventKind, Runtime, ScriptEvent, ScriptResult, UNKNOWN_LANGUAGE};

/// What a Lua script gets from a build without the `lua` feature.
pub const LUA_NOT_BUILT: &str = "This build has no Lua engine (the lua feature is off).";

/// One script's engine, in its language.
enum AnyEngine {
    JavaScript(Engine),
    #[cfg(feature = "lua")]
    Lua(Box<super::lua::LuaEngine>),
}

impl AnyEngine {
    fn dispatch(&mut self, event: &ScriptEvent) -> ScriptResult {
        match self {
            AnyEngine::JavaScript(engine) => engine.dispatch(event),
            #[cfg(feature = "lua")]
            AnyEngine::Lua(engine) => engine.dispatch(event),
        }
    }
}

#[derive(Default)]
pub struct EngineSet {
    engines: HashMap<String, AnyEngine>,
    seed: Option<String>,
}

impl EngineSet {
    pub fn len(&self) -> usize {
        self.engines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.engines.is_empty()
    }

    pub fn is_loaded(&self, id: &str) -> bool {
        self.engines.contains_key(id)
    }

    /// Keep the host's protocol cache for engines loaded later and apply it to the running ones
    /// (no callback runs). Returns the engines that could not take it, now dropped.
    pub fn seed(&mut self, state: &str) -> Vec<(String, ScriptResult)> {
        self.seed = Some(state.to_string());
        let event = ScriptEvent::new(EventKind::State, state);
        let mut failed = Vec::new();
        self.engines.retain(|id, engine| {
            let mut result = engine.dispatch(&event);
            if result.error.is_none() {
                return true;
            }
            result.actions.clear();
            failed.push((id.clone(), result));
            false
        });
        failed
    }

    /// Create the JavaScript engine for a script (replacing one the id had) and run its source.
    pub fn load(&mut self, id: &str, source: &str, restricted_send: bool) -> ScriptResult {
        self.load_with(id, source, restricted_send, Runtime::JAVASCRIPT)
    }

    /// Create the engine for a script in its language (replacing one the id had) and run its
    /// source. A runtime that does not fit together (JavaScript with a layer) is refused.
    pub fn load_with(&mut self, id: &str, source: &str, restricted_send: bool, runtime: Runtime) -> ScriptResult {
        self.engines.remove(id);
        if !runtime.is_valid() {
            return ScriptResult::failed(UNKNOWN_LANGUAGE);
        }
        let seed = self.seed.as_deref();
        let (engine, result) = if runtime.is_lua() {
            Self::load_lua(source, restricted_send, runtime, seed)
        } else {
            let (engine, result) = Engine::load(source, restricted_send, seed);
            (engine.map(AnyEngine::JavaScript), result)
        };
        if let Some(engine) = engine {
            self.engines.insert(id.to_string(), engine);
        }
        result
    }

    #[cfg(feature = "lua")]
    fn load_lua(
        source: &str,
        restricted_send: bool,
        runtime: Runtime,
        seed: Option<&str>,
    ) -> (Option<AnyEngine>, ScriptResult) {
        let (engine, result) = super::lua::LuaEngine::load(source, restricted_send, runtime.compatibility, seed);
        (engine.map(|e| AnyEngine::Lua(Box::new(e))), result)
    }

    #[cfg(not(feature = "lua"))]
    fn load_lua(_: &str, _: bool, _: Runtime, _: Option<&str>) -> (Option<AnyEngine>, ScriptResult) {
        (None, ScriptResult::failed(LUA_NOT_BUILT))
    }

    /// Deliver one event to the named engines in order: one result per id. An id without an
    /// engine gets an empty result; a failed engine is dropped and its actions discarded.
    pub fn dispatch(&mut self, ids: &[String], event: &ScriptEvent) -> Vec<ScriptResult> {
        ids.iter().map(|id| self.dispatch_one(id, event)).collect()
    }

    pub fn dispatch_one(&mut self, id: &str, event: &ScriptEvent) -> ScriptResult {
        let Some(engine) = self.engines.get_mut(id) else {
            return ScriptResult::default();
        };
        let mut result = engine.dispatch(event);
        if result.error.is_some() {
            self.engines.remove(id);
            result.actions.clear();
        }
        result
    }

    pub fn stop(&mut self, id: &str) {
        self.engines.remove(id);
    }

    pub fn clear(&mut self) {
        self.engines.clear();
    }
}

#[cfg(all(test, feature = "scripting"))]
mod tests {
    use super::*;
    use crate::scripting::{ActionKind, ScriptAction};
    use std::time::{Duration, Instant};

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn echo(text: &str) -> ScriptAction {
        ScriptAction::new(ActionKind::Echo, text)
    }

    /// C# `WorkerHostsTwoEnginesAndAnswersEveryRequestWithTheScriptId`.
    #[test]
    fn two_engines_answer_in_the_order_asked_and_keep_their_own_globals() {
        let mut set = EngineSet::default();
        let a = set.load(
            "a",
            "let n = 0; mud.alias('x', () => mud.echo('a' + ++n)); mud.trigger(/^go$/, () => mud.echo('a-line'));",
            false,
        );
        assert_eq!(a.error, None);
        let b = set.load(
            "b",
            "mud.trigger(/^go$/, () => mud.echo('b-line')); mud.alias('x', () => mud.echo('b'));",
            false,
        );
        assert_eq!(b.error, None);
        let line = set.dispatch(&ids(&["a", "b"]), &ScriptEvent::new(EventKind::Line, "go"));
        assert_eq!(line[0].actions, [echo("a-line")]);
        assert_eq!(line[1].actions, [echo("b-line")]);
        let command = set.dispatch(&ids(&["a"]), &ScriptEvent::new(EventKind::Command, "x"));
        assert_eq!(command[0].actions, [echo("a1")]);
        assert!(command[0].handled);
        set.stop("a");
        let after = set.dispatch(&ids(&["a", "b"]), &ScriptEvent::new(EventKind::Command, "x"));
        assert!(
            after[0].is_empty(),
            "the stopped script answers empty, without an error"
        );
        assert!(after[1].handled);
        assert_eq!(after[1].actions, [echo("b")]);
    }

    /// C# `ASeedReachesEnginesLoadedBeforeAndAfterItWithoutFiringCallbacks`.
    #[test]
    fn a_seed_reaches_engines_loaded_before_and_after_it_without_callbacks() {
        let mut set = EngineSet::default();
        let source = r#"
            mud.on(Events.Msdp, () => mud.echo("fired"));
            mud.alias(/^read$/, () => mud.echo("health:" + mud.state.get("msdp.HEALTH")));
            mud.echo("load:" + mud.state.get("msdp.HEALTH"));
        "#;
        let early = set.load("early", source, false);
        assert_eq!(
            early.actions,
            [ScriptAction::new(ActionKind::Report, "HEALTH"), echo("load:undefined")]
        );
        assert!(set.seed(r#"{"msdp":{"HEALTH":"100"}}"#).is_empty());
        let late = set.load("late", source, false);
        assert_eq!(late.actions[0], echo("load:100"));
        let read = set.dispatch(&ids(&["early", "late"]), &ScriptEvent::new(EventKind::Command, "read"));
        assert_eq!(read[0].actions, [echo("health:100")]);
        assert_eq!(read[1].actions, [echo("health:100")]);
        assert!(set.seed(r#"{"msdp":{"HEALTH":"7"}}"#).is_empty());
        let read = set.dispatch(&ids(&["early", "late"]), &ScriptEvent::new(EventKind::Command, "read"));
        assert_eq!(read[0].actions, [echo("health:7")]);
        assert_eq!(read[1].actions, [echo("health:7")]);
        assert_eq!(set.len(), 2);
    }

    /// C# `ARunawayCallbackStopsThatScriptOnlyAndTheOtherKeepsRunning`.
    #[test]
    fn a_runaway_callback_stops_that_script_only() {
        let mut set = EngineSet::default();
        let looping = "mud.trigger(/^go$/, () => { mud.send('discard'); while (true) {} });";
        assert_eq!(set.load("loop", looping, false).error, None);
        assert_eq!(
            set.load(
                "fine",
                "let n = 0; mud.trigger(/^go$/, () => mud.echo('fine' + ++n));",
                false
            )
            .error,
            None
        );
        let started = Instant::now();
        let reply = set.dispatch(&ids(&["loop", "fine"]), &ScriptEvent::new(EventKind::Line, "go"));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(reply[0].error.is_some());
        assert!(reply[0].actions.is_empty(), "a failed callback's actions are discarded");
        assert_eq!(reply[1].actions, [echo("fine1")]);
        assert!(!set.is_loaded("loop"));
        assert!(set.is_loaded("fine"));
        let reply = set.dispatch(&ids(&["loop", "fine"]), &ScriptEvent::new(EventKind::Line, "go"));
        assert!(reply[0].is_empty());
        assert_eq!(reply[1].actions, [echo("fine2")]);
    }

    fn kinds(result: &ScriptResult) -> Vec<String> {
        result
            .actions
            .iter()
            .map(|a| {
                format!(
                    "{}:{}",
                    if a.kind == ActionKind::Send { "send" } else { "echo" },
                    a.text
                )
            })
            .collect()
    }

    /// C# `ScriptHookRemovalTests.RegistrationsReturnIdsAndRemovedHooksStopRunning`.
    #[test]
    fn registrations_return_ids_and_removed_hooks_stop_running() {
        let mut set = EngineSet::default();
        let loaded = set.load(
            "s",
            r#"
            const ferry = mud.trigger(/ferry/, () => { mud.send('board ferry'); mud.remove(ferry); });
            const alias = mud.alias(/^hi$/, () => mud.send('wave'));
            const listener = mud.on(Events.Line, () => mud.echo('seen'));
            mud.alias(/^quiet$/, () => { mud.echo(String(mud.remove(listener)) + String(mud.remove(listener))); });
            mud.echo([typeof ferry, ferry !== alias, alias !== listener].join(','));
            "#,
            false,
        );
        assert_eq!(loaded.error, None);
        assert_eq!(kinds(&loaded), ["echo:number,true,true"]);
        let line = |set: &mut EngineSet, text: &str| set.dispatch_one("s", &ScriptEvent::new(EventKind::Line, text));
        // Line listeners run before triggers.
        assert_eq!(
            kinds(&line(&mut set, "The ferry arrives.")),
            ["echo:seen", "send:board ferry"]
        );
        assert_eq!(kinds(&line(&mut set, "The ferry arrives.")), ["echo:seen"]);
        let quiet = set.dispatch_one("s", &ScriptEvent::new(EventKind::Command, "quiet"));
        assert_eq!(kinds(&quiet), ["echo:truefalse"]);
        assert!(line(&mut set, "anything").actions.is_empty());
        assert!(
            set.dispatch_one("s", &ScriptEvent::new(EventKind::Command, "hi"))
                .handled
        );
    }

    /// C# `AHookRemovedDuringAnEventDoesNotRunLaterInThatEvent`.
    #[test]
    fn a_hook_removed_during_an_event_does_not_run_later_in_it() {
        let mut set = EngineSet::default();
        let source = "let second; mud.trigger(/x/, () => { mud.remove(second); mud.echo('first'); }); second = mud.trigger(/x/, () => mud.echo('second'));";
        assert_eq!(set.load("s", source, false).error, None);
        let result = set.dispatch_one("s", &ScriptEvent::new(EventKind::Line, "x"));
        assert_eq!(kinds(&result), ["echo:first"]);
    }

    /// C# `AfterRunsOnceAndAcceptsFractionsWhileEveryKeepsItsMinimum`.
    #[test]
    fn after_runs_once_and_accepts_fractions_while_every_keeps_its_minimum() {
        let mut set = EngineSet::default();
        let source = "mud.after(0.5, () => mud.send('once')); mud.after(0, () => mud.send('next tick'));";
        assert_eq!(set.load("s", source, false).error, None);
        assert_eq!(
            kinds(&set.dispatch_one("s", &ScriptEvent::tick(100))),
            ["send:next tick"]
        );
        assert_eq!(kinds(&set.dispatch_one("s", &ScriptEvent::tick(500))), ["send:once"]);
        assert!(set.dispatch_one("s", &ScriptEvent::tick(5000)).actions.is_empty());
        assert!(set.load("t", "mud.after(-1, () => {})", false).error.is_some());
        assert!(set.load("u", "mud.every(0.5, () => {})", false).error.is_some());
    }

    /// C# `RemovingHooksFreesTheirPlaceInTheBudget`.
    #[test]
    fn removing_hooks_frees_their_place_in_the_budget() {
        let mut set = EngineSet::default();
        let churn = r#"
            for (let round = 0; round < 10; round++) {
                const ids = [];
                for (let i = 0; i < 200; i++) ids.push(mud.trigger('x', () => {}));
                for (const id of ids) mud.remove(id);
            }
            mud.echo('ok');
        "#;
        assert_eq!(set.load("s", churn, false).error, None);
        assert!(
            set.load("t", "for (let i = 0; i < 257; i++) mud.trigger('x', () => {});", false)
                .error
                .is_some()
        );
    }
}
