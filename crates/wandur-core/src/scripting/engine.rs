//! One script's engine: a QuickJS runtime and context of its own, the bootstrap that defines
//! `mud` and `Events`, and the C# limits around every call into it:
//!
//! - 300 ms per call (load or event), regex matching included: QuickJS asks an interrupt handler
//!   every 10,000 calls or backward jumps, and every 10,000 regex steps.
//! - About 100,000 statements per call: the handler counts its polls, and the eleventh in one
//!   call ends it (Jint counts statements; QuickJS only offers the poll, see the ruling in the
//!   ledger).
//! - 64 MiB of heap for the script, a 256 KiB JavaScript stack (Jint limits recursion to a depth
//!   of 64 instead), sources up to 256 KiB, event text up to 32,768 characters and a state seed up
//!   to 1 MiB.
//!
//! An engine that fails (a thrown error, a limit, a malformed result) is not used again: the
//! error goes back as the result and the caller drops the engine.

use super::{EventKind, ScriptEvent, ScriptResult, js_length, limits};

/// What a script without the engine gets.
pub const NOT_BUILT: &str = "This build has no JavaScript engine (the scripting feature is off).";
pub const SOURCE_TOO_LARGE: &str = "Source exceeds 256 KiB.";
pub const STATE_TOO_LARGE: &str = "State seed exceeds 1048576 characters.";
pub const EVENT_TOO_LARGE: &str = "Event text exceeds 32768 characters.";
pub const TIMED_OUT: &str = "The script ran longer than 300 ms.";
pub const TOO_MANY_STATEMENTS: &str = "The script ran more than 100000 statements.";
pub const INVALID_RESULT: &str = "Invalid script result.";

/// Heap per script.
pub const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
/// JavaScript stack per script.
pub const STACK_LIMIT: usize = 256 * 1024;
/// Interrupt polls (about 10,000 operations each) one call may take: about 100,000 statements.
pub const MAX_POLLS: u32 = 10;

#[cfg(feature = "lua")]
pub(crate) use quickjs::describe;
#[cfg(feature = "scripting")]
pub use quickjs::{Engine, RegexCheck};

#[cfg(not(feature = "scripting"))]
pub use stub::Engine;

fn check_event(event: &ScriptEvent) -> Option<ScriptResult> {
    if event.kind == EventKind::State {
        (js_length(&event.text) > limits::STATE_SEED_CHARACTERS).then(|| ScriptResult::failed(STATE_TOO_LARGE))
    } else {
        (js_length(&event.text) > limits::EVENT_CHARACTERS).then(|| ScriptResult::failed(EVENT_TOO_LARGE))
    }
}

#[cfg(feature = "scripting")]
mod quickjs {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use rquickjs::context::EvalOptions;
    use rquickjs::{Context, Ctx, Function, Persistent, Runtime, Value};
    use serde::Deserialize;

    use super::*;
    use crate::scripting::ScriptAction;

    const BOOTSTRAP: &str = include_str!("bootstrap.js");

    /// Why the interrupt handler stopped a call.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Stop {
        None,
        Time,
        Statements,
    }

    struct Guard {
        deadline: Cell<Option<Instant>>,
        polls: Cell<u32>,
        stop: Cell<Stop>,
    }

    impl Guard {
        fn poll(&self) -> bool {
            let Some(deadline) = self.deadline.get() else {
                return false;
            };
            if Instant::now() >= deadline {
                self.stop.set(Stop::Time);
                return true;
            }
            let polls = self.polls.get() + 1;
            self.polls.set(polls);
            if polls > MAX_POLLS {
                self.stop.set(Stop::Statements);
                return true;
            }
            false
        }
    }

    #[derive(Deserialize)]
    struct Raw {
        handled: bool,
        actions: Vec<ScriptAction>,
        aliases: u32,
    }

    /// One script's engine. Field order is drop order: the dispatcher before its context, the
    /// context before its runtime.
    pub struct Engine {
        dispatch: Persistent<Function<'static>>,
        context: Context,
        _runtime: Runtime,
        guard: Rc<Guard>,
    }

    impl Engine {
        /// Create the engine and run `source`, after `seed` (the host's protocol cache) when
        /// there is one. Returns the engine when the source ran, and what the source did.
        pub fn load(source: &str, restricted_send: bool, seed: Option<&str>) -> (Option<Engine>, ScriptResult) {
            if source.len() > limits::SOURCE_BYTES {
                return (None, ScriptResult::failed(SOURCE_TOO_LARGE));
            }
            if let Some(seed) = seed
                && js_length(seed) > limits::STATE_SEED_CHARACTERS
            {
                return (None, ScriptResult::failed(STATE_TOO_LARGE));
            }
            let engine = match Self::create(restricted_send) {
                Ok(engine) => engine,
                Err(error) => return (None, ScriptResult::failed(error)),
            };
            if let Some(seed) = seed {
                // A seed fills mud.state before the first line of the script runs.
                let result = engine.call(&ScriptEvent::new(EventKind::State, seed));
                if result.error.is_some() {
                    return (None, result);
                }
            }
            let ran = engine.guarded(|| {
                engine.context.with(|ctx| {
                    let mut options = EvalOptions::default();
                    options.strict = false;
                    options.filename = Some("script.js".into());
                    match ctx.eval_with_options::<(), _>(source, options) {
                        Ok(()) => Ok(()),
                        Err(error) => Err(describe(&ctx, error)),
                    }
                })
            });
            if let Err(error) = ran {
                return (None, ScriptResult::failed(engine.explain(error)));
            }
            let result = engine.call(&ScriptEvent::new(EventKind::Flush, ""));
            if result.error.is_some() {
                return (None, result);
            }
            (Some(engine), result)
        }

        fn create(restricted_send: bool) -> Result<Engine, String> {
            let runtime = Runtime::new().map_err(|e| e.to_string())?;
            runtime.set_memory_limit(MEMORY_LIMIT);
            runtime.set_max_stack_size(STACK_LIMIT);
            let guard = Rc::new(Guard {
                deadline: Cell::new(None),
                polls: Cell::new(0),
                stop: Cell::new(Stop::None),
            });
            let handler = Rc::clone(&guard);
            runtime.set_interrupt_handler(Some(Box::new(move || handler.poll())));
            let context = Context::full(&runtime).map_err(|e| e.to_string())?;
            let bootstrap = BOOTSTRAP.replace("__RESTRICTED_SEND__", if restricted_send { "true" } else { "false" });
            let dispatch = context.with(|ctx| -> Result<Persistent<Function<'static>>, String> {
                let function: Function = ctx.eval(bootstrap).map_err(|e| describe(&ctx, e))?;
                Ok(Persistent::save(&ctx, function))
            })?;
            Ok(Engine {
                dispatch,
                context,
                _runtime: runtime,
                guard,
            })
        }

        /// Deliver one event. An error means the engine must not be used again.
        pub fn dispatch(&mut self, event: &ScriptEvent) -> ScriptResult {
            if let Some(refused) = check_event(event) {
                return refused;
            }
            self.call(event)
        }

        fn call(&self, event: &ScriptEvent) -> ScriptResult {
            let outcome = self.guarded(|| {
                self.context.with(|ctx| -> Result<Option<String>, String> {
                    let function = self.dispatch.clone().restore(&ctx).map_err(|e| e.to_string())?;
                    let value: Value = function
                        .call((event.kind.as_str(), event.text.as_str(), event.elapsed_ms as f64))
                        .map_err(|e| describe(&ctx, e))?;
                    if value.is_null() || value.is_undefined() {
                        return Ok(None);
                    }
                    match value.as_string() {
                        Some(text) => text.to_string().map(Some).map_err(|e| e.to_string()),
                        None => Err(INVALID_RESULT.to_string()),
                    }
                })
            });
            match outcome {
                Ok(None) => ScriptResult::default(),
                Ok(Some(json)) => match serde_json::from_str::<Raw>(&json) {
                    Ok(raw) => ScriptResult {
                        handled: raw.handled,
                        actions: raw.actions,
                        error: None,
                        aliases: Some(raw.aliases),
                    },
                    Err(_) => ScriptResult::failed(INVALID_RESULT),
                },
                Err(error) => ScriptResult::failed(self.explain(error)),
            }
        }

        /// Run `f` under the per-call limits.
        fn guarded<R>(&self, f: impl FnOnce() -> R) -> R {
            self.guard.polls.set(0);
            self.guard.stop.set(Stop::None);
            self.guard.deadline.set(Some(
                Instant::now() + Duration::from_millis(limits::CALLBACK_TIMEOUT_MILLISECONDS),
            ));
            let result = f();
            self.guard.deadline.set(None);
            result
        }

        /// Run `f` with the script's context under the per-call limits. The Lua engine runs its
        /// script this way, so its calls into the host API reach this context.
        #[cfg(feature = "lua")]
        pub(crate) fn with_context<R>(&self, f: impl FnOnce(&Ctx<'_>) -> R) -> R {
            self.guarded(|| self.context.with(|ctx| f(&ctx)))
        }

        /// The interrupt's reason wins over QuickJS's "interrupted".
        fn explain(&self, error: String) -> String {
            match self.guard.stop.get() {
                Stop::Time => TIMED_OUT.to_string(),
                Stop::Statements => TOO_MANY_STATEMENTS.to_string(),
                Stop::None => error,
            }
        }
    }

    /// JavaScript regular expressions checked and tried outside any script (the Mudlet importer
    /// keeps only patterns the engine accepts, as the C# converter asks Jint). Each check gets
    /// two seconds.
    pub struct RegexCheck {
        context: Context,
        _runtime: Runtime,
        deadline: Rc<Cell<Option<Instant>>>,
    }

    impl RegexCheck {
        pub fn new() -> Option<Self> {
            let runtime = Runtime::new().ok()?;
            runtime.set_memory_limit(MEMORY_LIMIT);
            let deadline: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
            let watch = Rc::clone(&deadline);
            runtime.set_interrupt_handler(Some(Box::new(move || watch.get().is_some_and(|d| Instant::now() >= d))));
            let context = Context::full(&runtime).ok()?;
            Some(Self {
                context,
                _runtime: runtime,
                deadline,
            })
        }

        fn run(&self, source: &str, flags: &str, text: Option<&str>) -> bool {
            self.deadline.set(Some(Instant::now() + Duration::from_secs(2)));
            let result = self.context.with(|ctx| -> rquickjs::Result<bool> {
                let test: Function = ctx.eval(
                    "((source, flags, text) => { const r = new RegExp(source, flags); return text === undefined ? true : r.test(text); })",
                )?;
                match text {
                    Some(text) => test.call((source, flags, text)),
                    None => test.call((source, flags)),
                }
            });
            self.deadline.set(None);
            result.unwrap_or(false)
        }

        /// The engine accepts this pattern.
        pub fn is_valid(&self, source: &str, flags: &str) -> bool {
            self.run(source, flags, None)
        }

        /// The pattern matches somewhere in `text` (false when it cannot run).
        pub fn matches(&self, source: &str, flags: &str, text: &str) -> bool {
            self.run(source, flags, Some(text))
        }
    }

    /// A readable message for a failed call: the thrown error's message (as Jint gives it), or
    /// the thrown value as text.
    pub(crate) fn describe(ctx: &Ctx<'_>, error: rquickjs::Error) -> String {
        if !error.is_exception() {
            return error.to_string();
        }
        let thrown = ctx.catch();
        if let Some(exception) = thrown.as_exception() {
            let message = exception.message().unwrap_or_default();
            if !message.is_empty() {
                return message;
            }
        }
        if let Some(text) = thrown.as_string().and_then(|s| s.to_string().ok()) {
            return text;
        }
        ctx.json_stringify(thrown)
            .ok()
            .flatten()
            .and_then(|s| s.to_string().ok())
            .unwrap_or_else(|| error.to_string())
    }
}

#[cfg(not(feature = "scripting"))]
mod stub {
    use super::*;

    /// Without the engine nothing loads.
    pub struct Engine;

    impl Engine {
        pub fn load(_source: &str, _restricted_send: bool, _seed: Option<&str>) -> (Option<Engine>, ScriptResult) {
            (None, ScriptResult::failed(NOT_BUILT))
        }

        pub fn dispatch(&mut self, event: &ScriptEvent) -> ScriptResult {
            check_event(event).unwrap_or_else(|| ScriptResult::failed(NOT_BUILT))
        }
    }
}
