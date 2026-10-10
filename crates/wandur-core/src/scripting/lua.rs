//! Lua world scripts (vendored Lua 5.4 through `mlua`) on Wandur's host API, as the C# client's
//! `LuaScriptEngine` runs them. The host API is not written twice: each Lua script gets a
//! JavaScript engine of its own running the bootstrap with no user code, and the Lua globals
//! `mud` and `Events` are proxies onto it. Both languages therefore share one implementation of
//! every host operation and of its output, panel, state and send limits.
//!
//! Lua gets its own limits on top, equal to the JavaScript ones: 300 ms of wall clock and one
//! million VM instructions per load or event (a count hook every 1,000 instructions), and a
//! 64 MiB heap. A limit cannot be caught: `pcall`, `xpcall` and `coroutine.resume` raise it
//! again (see `lua/prelude.lua`). The sandbox has the basic, string, table, math, coroutine and
//! bit32 libraries and a clock-only `os`; `print` writes to the script output.
//!
//! A script marked Mudlet-compatible also runs `lua/mudlet.lua`, the clean-room Mudlet layer
//! (docs/mudlet-layer.md), with three host helpers: plain-text and Perl-style patterns as
//! JavaScript regular expressions, and compiling code given as text (never bytecode).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::chunk::ChunkMode;
use mlua::{
    Function as LuaFunction, HookTriggers, IntoLuaMulti, Lua, LuaOptions, MultiValue, StdLib, Table, Value as LuaValue,
    VmState, WeakLua,
};
use rquickjs::function::Rest;
use rquickjs::{Array, Ctx, Exception, Function, Object, Persistent, Value};

use super::engine::{Engine, SOURCE_TOO_LARGE, describe as describe_js};
use super::{Compatibility, EventKind, ScriptEvent, ScriptResult, limits};
use crate::mudlet::regex as mudlet_regex;

/// VM instructions one load or event may run. Several instructions make up one JavaScript
/// statement, so this sits above the JavaScript engine's 100,000; the clock usually bites first.
pub const MAX_INSTRUCTIONS: u64 = 1_000_000;
/// Heap per Lua state.
pub const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
/// How often the count hook runs, in VM instructions.
const HOOK_INTERVAL: u32 = 1000;
/// How deeply nested a value may be when it crosses between the languages; deeper parts arrive
/// as nil or undefined.
pub const MAX_DEPTH: usize = 16;
/// Entries copied across in one crossing, in total.
pub const MAX_ENTRIES: usize = 20_000;

pub const INSTRUCTION_LIMIT: &str = "Lua script exceeded its instruction limit.";
pub const TIME_LIMIT: &str = "Lua script exceeded its time limit.";
pub const MEMORY_LIMIT_MESSAGE: &str = "Lua script exceeded its memory limit.";
const OUTSIDE_CALL: &str = "The host API is reachable only while the script runs.";

const PRELUDE: &str = include_str!("lua/prelude.lua");
const MUDLET: &str = include_str!("lua/mudlet.lua");

/// Gives the native names to the host API proxies and defines `print`. Its arguments are the
/// host's `mud` and `Events` as tables and the two Lua-only helpers.
const INSTALL: &str = r##"
local hostMud, events, regex, match = ...
local copy = {}
for key, value in pairs(hostMud) do copy[key] = value end
copy.regex, copy.match = regex, match
mud, Events = copy, events
local echo, concat, select, tostring = copy.echo, table.concat, select, tostring
function print(...)
  local parts = {}
  for index = 1, select("#", ...) do parts[index] = tostring((select(index, ...))) end
  echo(concat(parts, "\t"))
end
"##;

/// Turns a raw host proxy, which answers `true, results...` or `false, message`, into a
/// function that returns the results or raises the message as a plain Lua error.
const LIFT: &str = r#"
local error = error
local function check(ok, ...)
  if ok then return ... end
  error((...), 2)
end
return function(raw)
  return function(...) return check(raw(...)) end
end
"#;

/// The limits of the load or event being run.
#[derive(Default)]
struct Budget {
    started: Cell<Option<Instant>>,
    instructions: Cell<u64>,
    breach: Cell<Option<&'static str>>,
}

impl Budget {
    fn start(&self) {
        self.started.set(Some(Instant::now()));
        self.instructions.set(0);
        self.breach.set(None);
    }

    fn stop(&self) {
        self.started.set(None);
    }

    /// The count hook: once a limit is passed every later hook fails too.
    fn count(&self) -> Option<&'static str> {
        if let Some(breach) = self.breach.get() {
            return Some(breach);
        }
        let started = self.started.get()?;
        let used = self.instructions.get() + u64::from(HOOK_INTERVAL);
        self.instructions.set(used);
        let breach = if used > MAX_INSTRUCTIONS {
            INSTRUCTION_LIMIT
        } else if started.elapsed() > Duration::from_millis(limits::CALLBACK_TIMEOUT_MILLISECONDS) {
            TIME_LIMIT
        } else {
            return None;
        };
        self.breach.set(Some(breach));
        Some(breach)
    }
}

/// A value of the host engine held for Lua (a RegExp from `mud.regex`, say): it turns back
/// into the original when it is passed back.
struct JsHandle(Persistent<Value<'static>>);

/// What the proxies on both sides share: the context of the call in progress, the conversion
/// budget, and the helpers.
struct Bridge {
    lua: WeakLua,
    /// The host engine's context for the call in progress, innermost last. Entries exist only
    /// while that call runs (see [`Bridge::enter`]).
    contexts: RefCell<Vec<Ctx<'static>>>,
    entries: Cell<usize>,
    lift: LuaFunction,
    is_plain: RefCell<Option<Persistent<Function<'static>>>>,
}

/// Pops the context [`Bridge::enter`] pushed.
struct Entered<'a>(&'a Bridge);

impl Drop for Entered<'_> {
    fn drop(&mut self) {
        self.0.contexts.borrow_mut().pop();
    }
}

impl Bridge {
    /// Make `ctx` the context Lua's calls into the host use until the guard drops.
    fn enter<'a>(&'a self, ctx: &Ctx<'_>) -> Entered<'a> {
        // SAFETY: the context is only reachable through `contexts` while the guard lives, and
        // the guard lives inside the `Context::with` scope (or the host function call) that
        // gave `ctx`, on this thread. Values made from it in a proxy are dropped before the
        // proxy returns, so nothing outlives the real lifetime.
        let ctx: Ctx<'static> = unsafe { std::mem::transmute::<Ctx<'_>, Ctx<'static>>(ctx.clone()) };
        self.contexts.borrow_mut().push(ctx);
        Entered(self)
    }

    fn current(&self) -> Option<Ctx<'static>> {
        self.contexts.borrow().last().cloned()
    }

    fn reset(&self) {
        self.entries.set(0);
    }

    /// Count one more copied entry; false once the crossing has copied too many.
    fn take_entry(&self) -> bool {
        let used = self.entries.get() + 1;
        self.entries.set(used);
        used <= MAX_ENTRIES
    }

    fn is_plain<'js>(&self, ctx: &Ctx<'js>, object: &Value<'js>) -> bool {
        let Some(test) = self.is_plain.borrow().clone() else {
            return false;
        };
        test.restore(ctx)
            .and_then(|f| f.call::<_, bool>((object.clone(),)))
            .unwrap_or(false)
    }

    // JavaScript to Lua.

    fn to_lua<'js>(
        self: &Rc<Self>,
        lua: &Lua,
        ctx: &Ctx<'js>,
        value: Value<'js>,
        depth: usize,
    ) -> mlua::Result<LuaValue> {
        if value.is_undefined() || value.is_null() {
            return Ok(LuaValue::Nil);
        }
        if let Some(b) = value.as_bool() {
            return Ok(LuaValue::Boolean(b));
        }
        if let Some(n) = value.as_number() {
            return Ok(number_to_lua(n));
        }
        if let Some(s) = value.as_string() {
            let text = s.to_string().unwrap_or_default();
            return Ok(LuaValue::String(lua.create_string(&text)?));
        }
        if depth >= MAX_DEPTH || !self.take_entry() {
            return Ok(LuaValue::Nil);
        }
        if let Some(function) = value.as_function() {
            return self.lua_proxy(lua, ctx, function.clone(), None).map(LuaValue::Function);
        }
        if let Some(array) = value.as_array() {
            let length = array.len().min(MAX_ENTRIES);
            let table = lua.create_table_with_capacity(length, 0)?;
            for index in 0..length {
                let item: Value = array.get(index).unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
                table.raw_set(index + 1, self.to_lua(lua, ctx, item, depth + 1)?)?;
            }
            return Ok(LuaValue::Table(table));
        }
        if let Some(object) = value.as_object()
            && self.is_plain(ctx, &value)
        {
            let table = lua.create_table()?;
            let receiver = table.to_pointer() as usize;
            for key in object.keys::<String>().flatten() {
                let item: Value = object
                    .get(key.as_str())
                    .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
                let converted = match item.as_function() {
                    Some(method) => LuaValue::Function(self.lua_proxy(lua, ctx, method.clone(), Some(receiver))?),
                    None => self.to_lua(lua, ctx, item, depth + 1)?,
                };
                table.raw_set(key, converted)?;
                if !self.take_entry() {
                    break;
                }
            }
            return Ok(LuaValue::Table(table));
        }
        let handle = JsHandle(Persistent::save(ctx, value));
        Ok(LuaValue::UserData(lua.create_any_userdata(handle)?))
    }

    /// A Lua function that calls a host function. A method call with a colon passes the table
    /// first; host functions take no receiver, so that argument is dropped.
    fn lua_proxy<'js>(
        self: &Rc<Self>,
        lua: &Lua,
        ctx: &Ctx<'js>,
        function: Function<'js>,
        receiver: Option<usize>,
    ) -> mlua::Result<LuaFunction> {
        let saved = Persistent::save(ctx, function);
        let bridge = Rc::clone(self);
        let raw = lua.create_function(move |lua, args: MultiValue| {
            let Some(ctx) = bridge.current() else {
                return (false, OUTSIDE_CALL).into_lua_multi(lua);
            };
            let function = saved.clone().restore(&ctx).map_err(mlua::Error::external)?;
            let mut args = args.into_vec();
            if let (Some(receiver), Some(LuaValue::Table(first))) = (receiver, args.first())
                && first.to_pointer() as usize == receiver
            {
                args.remove(0);
            }
            let mut converted = Vec::with_capacity(args.len());
            for arg in args {
                bridge.reset();
                match bridge.to_js(&ctx, arg, 0) {
                    Ok(value) => converted.push(value),
                    Err(message) => return (false, message).into_lua_multi(lua),
                }
            }
            match function.call::<_, Value>((Rest(converted),)) {
                Ok(result) => {
                    bridge.reset();
                    let value = bridge.to_lua(lua, &ctx, result, 0)?;
                    (true, value).into_lua_multi(lua)
                }
                Err(error) => (false, describe_js(&ctx, error)).into_lua_multi(lua),
            }
        })?;
        self.lift.call(raw)
    }

    // Lua to JavaScript.

    fn to_js<'js>(self: &Rc<Self>, ctx: &Ctx<'js>, value: LuaValue, depth: usize) -> Result<Value<'js>, String> {
        let fail = |e: rquickjs::Error| e.to_string();
        match value {
            LuaValue::Nil => return Ok(Value::new_undefined(ctx.clone())),
            LuaValue::Boolean(b) => return Ok(Value::new_bool(ctx.clone(), b)),
            LuaValue::Integer(i) => return Ok(Value::new_number(ctx.clone(), i as f64)),
            LuaValue::Number(n) => return Ok(Value::new_number(ctx.clone(), n)),
            LuaValue::String(s) => {
                let text = s.to_string_lossy();
                return rquickjs::String::from_str(ctx.clone(), &text)
                    .map(|s| s.into_value())
                    .map_err(fail);
            }
            _ => {}
        }
        if depth >= MAX_DEPTH || !self.take_entry() {
            return Ok(Value::new_undefined(ctx.clone()));
        }
        match value {
            LuaValue::Function(function) => self.js_proxy(ctx, function).map_err(fail),
            LuaValue::Table(table) => self.table_to_js(ctx, table, depth),
            LuaValue::UserData(data) => match data.borrow::<JsHandle>() {
                Ok(handle) => handle.0.clone().restore(ctx).map_err(fail),
                Err(_) => Err(cannot_pass("userdata")),
            },
            other => Err(cannot_pass(other.type_name())),
        }
    }

    fn table_to_js<'js>(self: &Rc<Self>, ctx: &Ctx<'js>, table: Table, depth: usize) -> Result<Value<'js>, String> {
        let fail = |e: rquickjs::Error| e.to_string();
        let pairs: Vec<(LuaValue, LuaValue)> = table.pairs::<LuaValue, LuaValue>().filter_map(Result::ok).collect();
        let length = table.raw_len();
        let is_array = pairs.len() == length
            && pairs
                .iter()
                .all(|(key, _)| matches!(key, LuaValue::Integer(i) if *i >= 1 && (*i as usize) <= length));
        if is_array {
            let array = Array::new(ctx.clone()).map_err(fail)?;
            for index in 1..=length.min(MAX_ENTRIES) {
                let item: LuaValue = table.raw_get(index).unwrap_or(LuaValue::Nil);
                array.set(index - 1, self.to_js(ctx, item, depth + 1)?).map_err(fail)?;
            }
            return Ok(array.into_value());
        }
        let object = Object::new(ctx.clone()).map_err(fail)?;
        for (key, item) in pairs {
            let key = match key {
                LuaValue::String(s) => s.to_string_lossy(),
                LuaValue::Integer(i) => i.to_string(),
                LuaValue::Number(n) => format_number(n),
                _ => continue,
            };
            object.set(key, self.to_js(ctx, item, depth + 1)?).map_err(fail)?;
            if !self.take_entry() {
                break;
            }
        }
        Ok(object.into_value())
    }

    /// A host-side function that calls a Lua function (a trigger callback, a timer, a panel
    /// button) and gives back its first result.
    fn js_proxy<'js>(self: &Rc<Self>, ctx: &Ctx<'js>, function: LuaFunction) -> rquickjs::Result<Value<'js>> {
        let bridge = Rc::clone(self);
        let proxy = Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, args: Rest<Value<'js>>| -> rquickjs::Result<Value<'js>> {
                let _entered = bridge.enter(&ctx);
                let Some(lua) = bridge.lua.try_upgrade() else {
                    return Err(Exception::throw_message(&ctx, OUTSIDE_CALL));
                };
                let mut converted = Vec::with_capacity(args.0.len());
                for arg in args.0 {
                    bridge.reset();
                    match bridge.to_lua(&lua, &ctx, arg, 0) {
                        Ok(value) => converted.push(value),
                        Err(error) => return Err(Exception::throw_message(&ctx, &describe(&error))),
                    }
                }
                match function.call::<MultiValue>(MultiValue::from_vec(converted)) {
                    Ok(results) => match results.into_iter().next() {
                        Some(first) => {
                            bridge.reset();
                            bridge
                                .to_js(&ctx, first, 0)
                                .map_err(|message| Exception::throw_message(&ctx, &message))
                        }
                        None => Ok(Value::new_undefined(ctx.clone())),
                    },
                    Err(error) => Err(Exception::throw_message(&ctx, &describe(&error))),
                }
            },
        )?;
        Ok(proxy.into_value())
    }
}

fn cannot_pass(kind: &str) -> String {
    format!("This value cannot be passed to the host API: {kind}.")
}

/// JavaScript numbers that are whole become Lua integers, so `"HP " .. 12` reads "HP 12".
fn number_to_lua(n: f64) -> LuaValue {
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 && !(n == 0.0 && n.is_sign_negative()) {
        LuaValue::Integer(n as i64)
    } else {
        LuaValue::Number(n)
    }
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        n.to_string()
    }
}

/// A readable message for a Lua error: the innermost cause, without mlua's labels.
fn describe(error: &mlua::Error) -> String {
    match error {
        mlua::Error::CallbackError { cause, .. } => describe(cause),
        mlua::Error::WithContext { cause, .. } => describe(cause),
        mlua::Error::RuntimeError(message) => message.clone(),
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::MemoryError(_) => MEMORY_LIMIT_MESSAGE.to_string(),
        mlua::Error::ExternalError(inner) => inner.to_string(),
        other => other.to_string(),
    }
}

/// One Lua script's engine. Field order is drop order: the Lua state (and every host value it
/// holds) before the host engine.
pub struct LuaEngine {
    lua: Lua,
    bridge: Rc<Bridge>,
    budget: Rc<Budget>,
    host: Engine,
}

impl Drop for LuaEngine {
    /// Host functions made into Lua proxies keep the bridge alive until the host engine goes, so
    /// the bridge lets go of its own host value first. (The Lua state, dropped next, releases
    /// every other host value it holds; the host engine is dropped last.)
    fn drop(&mut self) {
        self.bridge.is_plain.borrow_mut().take();
    }
}

impl LuaEngine {
    /// Create the engine and run `source` (after `seed`, as for JavaScript). Returns the engine
    /// when the source ran, and what the source did.
    pub fn load(
        source: &str,
        restricted_send: bool,
        compatibility: Compatibility,
        seed: Option<&str>,
    ) -> (Option<LuaEngine>, ScriptResult) {
        if source.len() > limits::SOURCE_BYTES {
            return (None, ScriptResult::failed(SOURCE_TOO_LARGE));
        }
        let (host, boot) = Engine::load("", restricted_send, seed);
        let Some(host) = host else {
            return (None, boot);
        };
        let budget = Rc::new(Budget::default());
        let (lua, bridge) = match sandbox(&budget) {
            Ok(created) => created,
            Err(error) => return (None, ScriptResult::failed(describe(&error))),
        };
        let mut engine = LuaEngine {
            lua,
            bridge,
            budget,
            host,
        };
        let ran = engine.host.with_context(|ctx| {
            let _entered = engine.bridge.enter(ctx);
            engine.budget.start();
            let result = engine.install(ctx, compatibility).and_then(|()| {
                engine
                    .lua
                    .load(source)
                    .set_name("=script")
                    .set_mode(ChunkMode::Text)
                    .exec()
            });
            engine.budget.stop();
            result
        });
        if let Err(error) = ran {
            return (None, ScriptResult::failed(engine.explain(&describe(&error))));
        }
        let flushed = engine.host.dispatch(&ScriptEvent::new(EventKind::Flush, ""));
        if flushed.error.is_some() {
            return (None, flushed);
        }
        (Some(engine), flushed)
    }

    /// Deliver one event. An error means the engine must not be used again.
    pub fn dispatch(&mut self, event: &ScriptEvent) -> ScriptResult {
        self.budget.start();
        let mut result = self.host.dispatch(event);
        self.budget.stop();
        if let Some(error) = result.error.take() {
            return ScriptResult::failed(self.explain(&error));
        }
        result
    }

    /// The limit's reason wins over the message it surfaced as.
    fn explain(&self, error: &str) -> String {
        match self.budget.breach.get() {
            Some(breach) => breach.to_string(),
            None if error.contains("not enough memory") => MEMORY_LIMIT_MESSAGE.to_string(),
            None => error.to_string(),
        }
    }

    /// The host API as `mud` and `Events`, the Lua helpers, and the Mudlet layer when asked.
    fn install<'js>(&self, ctx: &Ctx<'js>, compatibility: Compatibility) -> mlua::Result<()> {
        let js = |e: rquickjs::Error| mlua::Error::RuntimeError(describe_js(ctx, e));
        let is_plain: Function = ctx
            .eval("(value => { const p = Object.getPrototypeOf(value); return p === Object.prototype || p === null; })")
            .map_err(js)?;
        *self.bridge.is_plain.borrow_mut() = Some(Persistent::save(ctx, is_plain));
        // Lua has no regular expression literal: mud.regex(source, flags) builds the RegExp
        // the host matches with, and mud.match(regex, text) is regex.exec(text).
        let regex: Value = ctx
            .eval("((source, flags) => new RegExp(source, flags === undefined ? '' : flags))")
            .map_err(js)?;
        let matcher: Value = ctx
            .eval(
                "((regex, text) => { if (!(regex instanceof RegExp)) throw new TypeError('mud.match needs a pattern from mud.regex.'); \
                 regex.lastIndex = 0; return regex.exec(String(text)); })",
            )
            .map_err(js)?;
        let globals = ctx.globals();
        let mud: Value = globals.get("mud").map_err(js)?;
        let events: Value = globals.get("Events").map_err(js)?;
        let lua = &self.lua;
        let convert = |value: Value<'js>| {
            self.bridge.reset();
            self.bridge.to_lua(lua, ctx, value, 0)
        };
        let args = (convert(mud)?, convert(events)?, convert(regex)?, convert(matcher)?);
        lua.load(INSTALL).set_name("=install").call::<()>(args)?;
        if compatibility == Compatibility::Mudlet {
            install_mudlet_helpers(lua)?;
            lua.load(MUDLET).set_name("=mudlet").set_mode(ChunkMode::Text).exec()?;
        }
        Ok(())
    }
}

/// A Lua state with the sandbox, the limits hook and the lift helper.
fn sandbox(budget: &Rc<Budget>) -> mlua::Result<(Lua, Rc<Bridge>)> {
    let lua = Lua::new_with(
        StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::COROUTINE | StdLib::OS,
        LuaOptions::default(),
    )?;
    lua.set_memory_limit(MEMORY_LIMIT)?;
    let hook_budget = Rc::clone(budget);
    // Lua 5.4 gives a new coroutine the hook of the thread that creates it, and the global
    // hook's callback serves every thread, so scripts' coroutines count against the budget.
    lua.set_global_hook(
        HookTriggers::new().every_nth_instruction(HOOK_INTERVAL),
        move |_, _| match hook_budget.count() {
            Some(breach) => Err(mlua::Error::RuntimeError(breach.to_string())),
            None => Ok(VmState::Continue),
        },
    )?;
    let breach_budget = Rc::clone(budget);
    let breached = lua.create_function(move |_, ()| Ok(breach_budget.breach.get()))?;
    lua.load(PRELUDE).set_name("=sandbox").call::<()>(breached)?;
    let lift: LuaFunction = lua.load(LIFT).set_name("=lift").eval()?;
    let bridge = Rc::new(Bridge {
        lua: lua.weak(),
        contexts: RefCell::new(Vec::new()),
        entries: Cell::new(0),
        lift,
        is_plain: RefCell::new(None),
    });
    Ok((lua, bridge))
}

/// `__wandur`, the Mudlet layer's host helpers; the layer takes them and clears the global.
fn install_mudlet_helpers(lua: &Lua) -> mlua::Result<()> {
    let helpers = lua.create_table()?;
    helpers.set(
        "literal",
        lua.create_function(|_, (text, from_start, to_end): (String, Option<bool>, Option<bool>)| {
            Ok(mudlet_regex::literal(&text, from_start.unwrap_or(false), to_end.unwrap_or(false)).source)
        })?,
    )?;
    helpers.set(
        "perl",
        lua.create_function(|lua, pattern: String| match mudlet_regex::from_perl(&pattern) {
            Some(p) => (p.source, p.flags).into_lua_multi(lua),
            None => LuaValue::Nil.into_lua_multi(lua),
        })?,
    )?;
    helpers.set("compile", lua.create_function(compile)?)?;
    lua.globals().set("__wandur", helpers)
}

/// Compiles Lua source text, never bytecode, into the script's own globals (or a table the
/// script passes, which can hold only what the script could reach). The result is an ordinary
/// function of the script: it runs under the caller's limits, in the same sandbox. Returns the
/// function, or nil and a message, as Lua's `load` does.
fn compile(
    lua: &Lua,
    (chunk, name, mode, environment): (LuaValue, Option<String>, Option<String>, LuaValue),
) -> mlua::Result<MultiValue> {
    let refuse = |message: &str| (LuaValue::Nil, message.to_string()).into_lua_multi(lua);
    let LuaValue::String(text) = chunk else {
        return refuse("code given as text must be a string");
    };
    let environment = match environment {
        LuaValue::Nil => None,
        LuaValue::Table(table) => Some(table),
        _ => return refuse("the environment must be a table"),
    };
    if !mode.as_deref().unwrap_or("t").contains('t') {
        return refuse("only Lua source text can be loaded, not binary chunks");
    }
    let bytes = text.as_bytes();
    // A precompiled chunk starts with the escape character; Wandur never loads one.
    if bytes.first() == Some(&0x1b) {
        return refuse("binary chunks cannot be loaded");
    }
    if bytes.len() > limits::SOURCE_BYTES {
        return refuse("code given as text exceeds 256 KiB");
    }
    let mut name = name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "text code".to_string());
    if name.chars().count() > 64 {
        name = name.chars().take(64).collect();
    }
    let mut chunk = lua
        .load(&bytes[..])
        .set_name(format!("={name}"))
        .set_mode(ChunkMode::Text);
    if let Some(environment) = environment {
        chunk = chunk.set_environment(environment);
    }
    match chunk.into_function() {
        Ok(function) => function.into_lua_multi(lua),
        Err(error) => refuse(&describe(&error)),
    }
}

#[cfg(test)]
#[path = "lua/tests.rs"]
mod tests;
