//! The C# `LuaScriptEngineTests`: Lua scripts on the shared host API, the sandbox and limits,
//! and the Mudlet layer. Every script here is fictional (The Lantern Road; Wren, Odo and
//! Bastian). The layer's messages say "Wandur" where the C# ones say "WMC".

use std::time::{Duration, Instant};

use super::*;
use crate::scripting::engines::EngineSet;
use crate::scripting::{ActionKind, Runtime};

fn load(source: &str, compatibility: Compatibility) -> (Option<LuaEngine>, ScriptResult) {
    LuaEngine::load(source, false, compatibility, None)
}

fn loaded(source: &str, compatibility: Compatibility) -> LuaEngine {
    let (engine, result) = load(source, compatibility);
    assert_eq!(result.error, None, "{source}");
    engine.expect("running")
}

fn native(source: &str) -> LuaEngine {
    loaded(source, Compatibility::None)
}

fn mudlet(source: &str) -> LuaEngine {
    loaded(source, Compatibility::Mudlet)
}

fn error_of(source: &str, compatibility: Compatibility) -> String {
    let (engine, result) = load(source, compatibility);
    assert!(engine.is_none(), "{source} should fail");
    result.error.unwrap_or_else(|| panic!("{source} should fail"))
}

fn actions(result: &ScriptResult) -> Vec<String> {
    result
        .actions
        .iter()
        .map(|a| {
            let kind = match a.kind {
                ActionKind::Send => "send",
                ActionKind::Echo => "echo",
                ActionKind::Panel => "panel",
                ActionKind::Report => "report",
            };
            format!("{kind}:{}", a.text)
        })
        .collect()
}

fn line(text: &str) -> ScriptEvent {
    ScriptEvent::new(EventKind::Line, text)
}

fn command(text: &str) -> ScriptEvent {
    ScriptEvent::new(EventKind::Command, text)
}

fn gmcp(text: &str) -> ScriptEvent {
    ScriptEvent::new(EventKind::Gmcp, text)
}

#[test]
fn a_native_lua_script_uses_the_same_host_api_as_javascript() {
    let mut engine = native(
        r#"
        local seen = 0
        mud.trigger(mud.regex("^(\\w+) arrives from the (\\w+)\\.$"), function(m)
          seen = seen + 1
          mud.send("wave " .. m[2])
        end)
        mud.alias("^camp$", function() mud.send("make camp"); mud.send("rest") end)
        mud.on(Events.Gmcp, function(event)
          if event.package == "Char.Vitals" then mud.echo("HP " .. event.data.hp .. " of " .. mud.state.get("gmcp.Char.Vitals.maxhp")) end
        end)
        local road = mud.panel("road", { title = "Road", dock = "right" })
        road:gauge("hp", { label = "Health", value = 30, max = 40 })
        road.label("note", { text = "Lit by lanterns" })
        print("loaded", seen, send == nil, tempTrigger == nil)
        "#,
    );
    assert_eq!(
        actions(&engine.dispatch(&line("Odo arrives from the east."))),
        ["send:wave Odo"]
    );
    let camp = engine.dispatch(&command("camp"));
    assert!(camp.handled);
    assert_eq!(actions(&camp), ["send:make camp", "send:rest"]);
    assert!(!engine.dispatch(&command("look")).handled);
    assert_eq!(
        actions(&engine.dispatch(&gmcp(r#"Char.Vitals {"hp":12,"maxhp":40}"#))),
        ["echo:HP 12 of 40"]
    );

    let (_, loaded_result) = load(
        "print('a', 1, nil, true) local p = mud.panel('road') p:gauge('hp', { value = 1 })",
        Compatibility::None,
    );
    assert_eq!(loaded_result.error, None);
    let all = actions(&loaded_result);
    assert!(all.contains(&"echo:a\t1\tnil\ttrue".to_string()), "{all:?}");
    assert!(
        loaded_result
            .actions
            .iter()
            .any(|a| a.kind == ActionKind::Panel && a.text.contains("\"gauge\""))
    );
    // A native script has none of Mudlet's globals.
    let (_, bare) = load(
        "print('loaded', 0, send == nil, tempTrigger == nil)",
        Compatibility::None,
    );
    assert!(actions(&bare).contains(&"echo:loaded\t0\ttrue\ttrue".to_string()));
}

#[test]
fn hooks_timers_and_removal_come_from_the_shared_api() {
    let mut engine = native(
        r#"
        local count = 0
        local id
        id = mud.every(1, function() count = count + 1; mud.echo("tick " .. count); if count == 2 then mud.remove(id) end end)
        mud.after(0.5, function() mud.send("once") end)
        "#,
    );
    assert_eq!(actions(&engine.dispatch(&ScriptEvent::tick(500))), ["send:once"]);
    assert_eq!(actions(&engine.dispatch(&ScriptEvent::tick(1000))), ["echo:tick 1"]);
    assert_eq!(actions(&engine.dispatch(&ScriptEvent::tick(2000))), ["echo:tick 2"]);
    assert!(engine.dispatch(&ScriptEvent::tick(3000)).actions.is_empty());
}

#[test]
fn the_sandbox_has_no_files_processes_or_code_from_text() {
    for source in [
        "return io.open('/etc/hosts')",
        "os.execute('ls')",
        "return os.getenv('HOME').x",
        "debug.sethook()",
        "require('socket')",
        "load('return 1')()",
        "loadstring('return 1')()",
        "dofile('/etc/hosts')",
        "loadfile('/etc/hosts')",
        "string.dump(print)",
        "collectgarbage('stop')",
        "package.loadlib('x', 'y')",
        "os.remove('/tmp/x')",
        "os.exit(1)",
    ] {
        error_of(source, Compatibility::None);
    }
}

#[test]
fn the_clock_part_of_os_and_pure_libraries_remain() {
    let (_, result) = load(
        r#"
        print(type(os.time()), type(os.clock()), type(os.date('%Y')), math.floor(2.7), string.format('%03d', 7), bit32.band(6, 3), table.concat({ 'a', 'b' }, '-'))
        local co = coroutine.wrap(function(a) local b = coroutine.yield(a + 1) return b * 2 end)
        print(co(1), co(5))
        "#,
        Compatibility::None,
    );
    assert_eq!(result.error, None);
    assert_eq!(
        actions(&result),
        ["echo:number\tnumber\tstring\t2\t007\t2\ta-b", "echo:2\t10"]
    );
}

/// The runaway loop test: Lua scripts obey the same limits as JavaScript, and pcall cannot
/// hide them.
#[test]
fn runaway_scripts_stop_at_the_limits_and_pcall_cannot_hide_them() {
    for (source, reason) in [
        ("while true do end", "instruction"),
        ("while true do pcall(function() while true do end end) end", ""),
        (
            "while true do xpcall(function() while true do end end, function(e) return e end) end",
            "",
        ),
        ("local s = 'x' while true do s = s .. s end", ""),
        ("local t = {} for i = 1, 1e9 do t[i] = {} end", ""),
        (
            "local co = coroutine.create(function() local i = 0 while true do i = i + 1 end end) coroutine.resume(co)",
            "",
        ),
        ("return string.rep('x', 1e9)", "string.rep"),
        ("local function f() return f() + 1 end f()", ""),
    ] {
        let started = Instant::now();
        let error = error_of(source, Compatibility::None);
        assert!(error.contains(reason), "{source}: {error}");
        assert!(started.elapsed() < Duration::from_secs(5), "{source}: {error}");
    }
}

#[test]
fn a_callback_failure_stops_the_script_and_limits_apply_per_event() {
    let mut engine = native(
        r#"
        mud.trigger("boom", function() error("the lantern went out") end)
        mud.trigger("spin", function() while true do end end)
        "#,
    );
    let failed = engine.dispatch(&line("boom"));
    assert!(
        failed.error.as_deref().unwrap_or("").contains("the lantern went out"),
        "{failed:?}"
    );

    let mut spinning = native("mud.trigger('spin', function() while true do end end)");
    let started = Instant::now();
    let timed_out = spinning.dispatch(&line("spin"));
    assert_eq!(timed_out.error.as_deref(), Some(INSTRUCTION_LIMIT));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn output_and_hook_budgets_are_the_javascript_ones() {
    assert!(error_of("for i = 1, 33 do mud.send('look') end", Compatibility::None).contains("action limit"));
    assert!(
        error_of(
            "for i = 1, 257 do mud.trigger('x', function() end) end",
            Compatibility::None
        )
        .contains("256 hooks")
    );
    error_of("mud.send('two\\nlines')", Compatibility::None);
    error_of("mud.every(0.5, function() end)", Compatibility::None);
}

#[test]
fn the_engine_set_loads_lua_by_runtime_and_refuses_what_does_not_fit() {
    let mut set = EngineSet::default();
    let lua = set.load_with(
        "lua",
        "mud.alias('^hi$', function() mud.send('wave') end)",
        false,
        Runtime::LUA,
    );
    assert_eq!(lua.error, None);
    assert!(set.dispatch_one("lua", &command("hi")).handled);
    let javascript_with_layer = Runtime {
        language: crate::scripting::Language::JavaScript,
        compatibility: Compatibility::Mudlet,
    };
    assert_eq!(
        set.load_with("y", "", false, javascript_with_layer).error.as_deref(),
        Some(crate::scripting::UNKNOWN_LANGUAGE)
    );
    assert_eq!(crate::scripting::Language::parse(Some("python")), None);
    // A load that names no language is JavaScript, as every load before Lua was.
    assert_eq!(set.load("js", "mud.echo('js')", false).error, None);
    // Lua, Mudlet Lua and JavaScript side by side, and a runaway Lua script fails alone.
    assert_eq!(
        set.load_with(
            "mudlet",
            "tempTrigger('lantern', function() send('warm hands') end)",
            false,
            Runtime::MUDLET
        )
        .error,
        None
    );
    assert_eq!(
        set.load("js2", "mud.trigger('lantern', () => mud.echo('js saw it'));", false)
            .error,
        None
    );
    let results = set.dispatch(
        &["lua".into(), "mudlet".into(), "js2".into()],
        &line("A lantern flickers."),
    );
    assert_eq!(actions(&results[1]), ["send:warm hands"]);
    assert_eq!(actions(&results[2]), ["echo:js saw it"]);
    assert!(
        set.load_with("runaway", "while true do end", false, Runtime::LUA)
            .error
            .is_some()
    );
    assert!(set.is_loaded("lua") && set.is_loaded("mudlet") && set.is_loaded("js2"));
}

#[test]
fn a_state_seed_reaches_lua_before_the_first_line() {
    let mut set = EngineSet::default();
    assert!(set.seed(r#"{"gmcp":{"Char.Vitals":{"hp":7}},"msdp":{}}"#).is_empty());
    let result = set.load_with(
        "lua",
        "print(mud.state.get('gmcp.Char.Vitals.hp'))",
        false,
        Runtime::LUA,
    );
    assert_eq!(actions(&result), ["echo:7"]);
}

#[test]
fn a_restricted_pack_script_in_lua_keeps_the_send_policy() {
    let (_, result) = LuaEngine::load("mud.send('look')", true, Compatibility::None, None);
    assert!(result.error.unwrap().contains("Pack send policy"));
}

// The three fictional Mudlet scripts of the prototype.

#[test]
fn mudlet_script_one_a_trigger_sends_with_a_capture() {
    let mut engine = mudlet(
        r#"
        -- Wren's loot helper
        tempRegexTrigger("^(\\w+) drops (?:a|an) (.+)\\.$", function()
          send("get " .. matches[3])
          if matches[2] == "Odo" then send("thank Odo") end
        end)
        "#,
    );
    assert_eq!(
        actions(&engine.dispatch(&line("Odo drops a lantern oil."))),
        ["send:get lantern oil", "send:thank Odo"]
    );
    assert_eq!(
        actions(&engine.dispatch(&line("Bastian drops a copper coin."))),
        ["send:get copper coin"]
    );
    assert!(engine.dispatch(&line("Wren waves.")).actions.is_empty());
}

#[test]
fn mudlet_script_two_a_temp_timer_chain() {
    let (engine, first) = load(
        r#"
        -- Bastian's campfire routine
        local steps = { "gather wood", "build fire", "light fire" }
        local function step(index)
          if index > #steps then echo("The campfire is ready.\n") return end
          send(steps[index])
          tempTimer(1.5, function() step(index + 1) end)
        end
        step(1)
        "#,
        Compatibility::Mudlet,
    );
    assert_eq!(first.error, None);
    assert_eq!(actions(&first), ["send:gather wood"]);
    let mut engine = engine.unwrap();
    assert!(engine.dispatch(&ScriptEvent::tick(1400)).actions.is_empty());
    assert_eq!(actions(&engine.dispatch(&ScriptEvent::tick(1500))), ["send:build fire"]);
    assert_eq!(actions(&engine.dispatch(&ScriptEvent::tick(3000))), ["send:light fire"]);
    assert_eq!(
        actions(&engine.dispatch(&ScriptEvent::tick(4500))),
        ["echo:The campfire is ready."]
    );
    assert!(engine.dispatch(&ScriptEvent::tick(9000)).actions.is_empty());
}

#[test]
fn mudlet_script_three_a_gmcp_vitals_handler_echoes() {
    let mut engine = mudlet(
        r#"
        -- Odo's vitals watch
        function onVitals()
          local vitals = gmcp.Char.Vitals
          cecho("<green>HP " .. vitals.hp .. "/" .. vitals.maxhp .. "<reset>\n")
          if tonumber(vitals.hp) < tonumber(vitals.maxhp) / 4 then send("quaff tonic") end
        end
        registerAnonymousEventHandler("gmcp.Char.Vitals", "onVitals")
        "#,
    );
    assert_eq!(
        actions(&engine.dispatch(&gmcp(r#"Char.Vitals {"hp":"30","maxhp":"40"}"#))),
        ["echo:HP 30/40"]
    );
    assert_eq!(
        actions(&engine.dispatch(&gmcp(r#"Char.Vitals {"hp":"8","maxhp":"40"}"#))),
        ["echo:HP 8/40", "send:quaff tonic"]
    );
    assert!(
        engine
            .dispatch(&gmcp(r#"Room.Info {"name":"The Lantern Road"}"#))
            .actions
            .is_empty()
    );
}

#[test]
fn mudlet_triggers_can_be_switched_killed_and_expire() {
    let mut engine = mudlet(
        r#"
        ferry = tempTrigger("The ferry arrives", function() send("board ferry") end, 1)
        local toll = tempTrigger("tollkeeper", function() send("pay toll") end)
        tempRegexTrigger("(?i)^odo waves", function() send("wave odo") end)
        tempAlias("^ff$", function() expandAlias("greet Wren;;greet Odo") end)
        tempAlias("^greet (\\w+)$", function() send("say Hello, " .. matches[2]) end)
        function stopToll() return killTrigger(toll) end
        function pauseToll() disableTrigger(toll) end
        "#,
    );
    assert_eq!(
        actions(&engine.dispatch(&line("The ferry arrives."))),
        ["send:board ferry"]
    );
    assert!(engine.dispatch(&line("The ferry arrives.")).actions.is_empty());
    assert_eq!(
        actions(&engine.dispatch(&line("The tollkeeper waits."))),
        ["send:pay toll"]
    );
    assert_eq!(actions(&engine.dispatch(&line("ODO waves."))), ["send:wave odo"]);
    assert_eq!(
        actions(&engine.dispatch(&command("ff"))),
        ["send:say Hello, Wren", "send:say Hello, Odo"]
    );
}

#[test]
fn what_mudlet_has_and_wandur_does_not_says_so() {
    let unsupported = |source: &str, expected: &str| {
        let error = error_of(source, Compatibility::Mudlet);
        assert!(error.contains(expected), "{source}: {error}");
    };
    unsupported("selectString('Wren', 1)", "not supported in Wandur yet: selectString");
    unsupported("Geyser.Label:new({})", "not supported in Wandur yet: Geyser.Label");
    unsupported("setfenv(1, {})", "not supported in Wandur yet: setfenv");
    unsupported("echo('map', 'x')", "not supported in Wandur yet: echo to a window");
    // Inside a trigger the failure is reported and the script carries on, as Mudlet does.
    let mut engine = mudlet(
        "tempTrigger('sparkle', function() selectString('sparkle', 1) end) tempTrigger('rain', function() send('cover lantern') end)",
    );
    let result = engine.dispatch(&line("sparkle in the rain"));
    assert_eq!(result.error, None);
    assert!(
        result
            .actions
            .iter()
            .any(|a| a.kind == ActionKind::Echo && a.text.contains("not supported in Wandur yet: selectString"))
    );
    assert!(actions(&result).contains(&"send:cover lantern".to_string()));
    assert!(engine.dispatch(&line("rain")).error.is_none(), "still running");
}

#[test]
fn mudlet_code_given_as_text_runs() {
    let (engine, first) = load(
        r#"
        tempTrigger("lantern", [[send("warm hands")]])
        tempRegexTrigger("^(\\w+) drops a (.+)\\.$", "send('get ' .. matches[3])")
        tempTimer(1, [[send("rest")]])
        local add = loadstring("local a, b = ... return a + b")
        local twice = load("return 2 * x", "twice", "t", { x = 21 })
        local pieces, i = { "return ", "'pie", "ces'" }, 0
        local reader = load(function() i = i + 1 return pieces[i] end)
        print(add(1, 2), twice(), reader())
        function camp() send("make camp") end
        tempTrigger("dusk", "camp")
        "#,
        Compatibility::Mudlet,
    );
    assert_eq!(first.error, None);
    assert_eq!(actions(&first), ["echo:3\t42\tpieces"]);
    let mut engine = engine.unwrap();
    assert_eq!(
        actions(&engine.dispatch(&line("A lantern flickers."))),
        ["send:warm hands"]
    );
    assert_eq!(
        actions(&engine.dispatch(&line("Odo drops a coil of rope."))),
        ["send:get coil of rope"]
    );
    assert_eq!(actions(&engine.dispatch(&line("It is dusk."))), ["send:make camp"]);
    assert_eq!(actions(&engine.dispatch(&ScriptEvent::tick(1000))), ["send:rest"]);
    // A syntax error is reported where the code is given, as Lua's load reports it.
    assert!(error_of("tempTrigger('x', [[send(]])", Compatibility::Mudlet).contains("tempTrigger"));
    let (_, result) = load(
        "local f, e = loadstring('send(') print(f == nil, type(e) == 'string')",
        Compatibility::Mudlet,
    );
    assert_eq!(actions(&result)[0], "echo:true\ttrue");
}

#[test]
fn mudlet_text_code_is_never_bytecode() {
    let (_, result) = load(
        r#"
        local f1, e1 = load("\27Lua\84\0\1\4\8\4\8\0")
        local f2, e2 = loadstring("\27Lua\84\0\1\4\8\4\8\0")
        local f3, e3 = load("return 1", "x", "b")
        print(f1 == nil, e1, f2 == nil, e2, f3 == nil, e3, string.dump == nil)
        "#,
        Compatibility::Mudlet,
    );
    assert_eq!(result.error, None);
    assert_eq!(
        actions(&result)[0],
        "echo:true\tbinary chunks cannot be loaded\ttrue\tbinary chunks cannot be loaded\ttrue\tonly Lua source text can be loaded, not binary chunks\ttrue"
    );
}

#[test]
fn only_mudlet_scripts_compile_text() {
    for source in [
        "load('return 1')",
        "loadstring('return 1')",
        "tempTrigger('x', 'send(1)')",
    ] {
        error_of(source, Compatibility::None);
    }
}

#[test]
fn mudlet_text_code_cannot_leave_the_sandbox() {
    for name in [
        "io",
        "os.execute",
        "os.getenv",
        "os.remove",
        "debug",
        "require",
        "package",
        "dofile",
        "loadfile",
        "string.dump",
        "collectgarbage",
        "__wandur",
    ] {
        let (_, result) = load(
            &format!(
                r#"
                local direct = loadstring("return {name}")()
                local nested = loadstring("return loadstring('return {name}')()")()
                local fresh = load("return {name}", "x", "t", _G)()
                print(direct == nil, nested == nil, fresh == nil)
                "#
            ),
            Compatibility::Mudlet,
        );
        assert_eq!(result.error, None, "{name}");
        assert_eq!(actions(&result)[0], "echo:true\ttrue\ttrue", "{name}");
        // Calling the missing thing from text code fails like calling it from the script.
        error_of(&format!("loadstring([[{name}('x')]])()"), Compatibility::Mudlet);
    }
}

#[test]
fn mudlet_text_code_runs_under_the_scripts_limits() {
    for (source, reason) in [
        ("loadstring('while true do end')()", "instruction"),
        ("pcall(loadstring('while true do end'))", "instruction"),
        ("while true do pcall(load('while true do end')) end", "instruction"),
        ("loadstring('local s = \"x\" while true do s = s .. s end')()", ""),
    ] {
        let started = Instant::now();
        let error = error_of(source, Compatibility::Mudlet);
        assert!(error.contains(reason), "{source}: {error}");
        assert!(started.elapsed() < Duration::from_secs(5), "{source}");
    }
    // In a trigger too: the limit stops the script, which Mudlet's error reporting cannot catch.
    let mut engine = mudlet("tempTrigger('spin', [[while true do end]])");
    let error = engine.dispatch(&line("spin")).error.unwrap_or_default();
    assert!(error.contains("Lua script exceeded its"), "{error}");
}

#[test]
fn lua51_names_are_provided_where_safe() {
    let (_, result) = load(
        r#"
        local a, b = unpack({ 1, 2 })
        print(a + b, table.getn({ 1, 2, 3 }), math.mod(7, 3), string.trim("  lantern "), #string.split("a,b,c", ","), table.contains({ x = { "Odo" } }, "Odo"))
        display({ name = "Wren" })
        "#,
        Compatibility::Mudlet,
    );
    assert_eq!(result.error, None);
    assert_eq!(actions(&result)[0], "echo:3\t3\t1\tlantern\t3\ttrue");
    assert!(result.actions[1].text.contains("name = \"Wren\""));
}

#[test]
fn values_cross_both_ways_with_their_shapes() {
    let (_, result) = load(
        r#"
        local m = mud.match(mud.regex("^(\\w+) (\\w+)?$"), "Wren ")
        print(m[1], m[2], m[3] == nil, #m)
        local state = mud.state.snapshot()
        print(type(state.gmcp), next(state.gmcp) == nil)
        print(mud.format({ "a", "b" }), mud.format({ hp = 3 }))
        print(pcall(mud.panel, "road", { dock = "nowhere" }))
        "#,
        Compatibility::None,
    );
    assert_eq!(result.error, None);
    let all = actions(&result);
    assert_eq!(all[0], "echo:Wren \tWren\ttrue\t2");
    assert_eq!(all[1], "echo:table\ttrue");
    assert_eq!(all[2], "echo:a, b\thp: 3");
    assert!(
        all[3].starts_with("echo:false\t") && all[3].contains("left, right or bars"),
        "{all:?}"
    );
}

#[test]
fn a_coroutine_cannot_be_passed_to_the_host() {
    let error = error_of("mud.echo(coroutine.create(function() end))", Compatibility::None);
    assert!(error.contains("cannot be passed to the host API: thread"), "{error}");
}
