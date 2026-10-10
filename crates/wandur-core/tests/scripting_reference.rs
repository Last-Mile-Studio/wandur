//! `docs/scripting-reference.json` (the C# client's machine-readable API, kept in this repository)
//! against the engine: every global and member it lists exists with its kind, every event
//! constant has its value, every limit has its value here, and the widget kinds and panel
//! methods match.

#![cfg(feature = "scripting")]

use serde_json::Value;
use wandur_core::scripting::engines::EngineSet;
use wandur_core::scripting::{ActionKind, EventKind, PANEL_WIDGET_KINDS, ScriptEvent, limits};

fn reference() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/scripting-reference.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("the reference")).expect("valid JSON")
}

/// Evaluate `expression` in a fresh script and return what it echoed.
fn eval(expression: &str) -> String {
    let mut set = EngineSet::default();
    let result = set.load("probe", &format!("mud.echo(String({expression}));"), false);
    assert_eq!(result.error, None, "{expression}");
    let echoes: Vec<_> = result.actions.iter().filter(|a| a.kind == ActionKind::Echo).collect();
    assert_eq!(echoes.len(), 1, "{expression}");
    echoes[0].text.clone()
}

fn kind_of(kind: &str) -> &'static str {
    match kind {
        "function" => "function",
        "namespace" => "object",
        "constant" => "string",
        other => panic!("unknown kind {other}"),
    }
}

#[test]
fn every_global_member_and_event_constant_exists() {
    let reference = reference();
    let globals = reference["globals"].as_array().unwrap();
    assert_eq!(globals.len(), 2);
    for global in globals {
        let name = global["name"].as_str().unwrap();
        assert_eq!(eval(&format!("typeof {name}")), "object", "{name}");
        assert_eq!(eval(&format!("Object.isFrozen({name})")), "true", "{name} is frozen");
        let members = global["members"].as_array().unwrap();
        for member in members {
            let member_name = member["name"].as_str().unwrap();
            let path = format!("{name}.{member_name}");
            assert_eq!(
                eval(&format!("typeof {path}")),
                kind_of(member["kind"].as_str().unwrap()),
                "{path}"
            );
            if let Some(value) = member["value"].as_str() {
                assert_eq!(eval(&path), value, "{path}");
            }
            for nested in member["members"].as_array().into_iter().flatten() {
                let nested_path = format!("{path}.{}", nested["name"].as_str().unwrap());
                assert_eq!(
                    eval(&format!("typeof {nested_path}")),
                    kind_of(nested["kind"].as_str().unwrap()),
                    "{nested_path}"
                );
            }
        }
        // Nothing beyond the reference.
        assert_eq!(
            eval(&format!("Object.keys({name}).length")),
            members.len().to_string(),
            "{name} has only the listed members"
        );
    }
    // The events: every constant subscribes, and its payload arrives with the listed fields.
    for event in reference["events"].as_array().unwrap() {
        let constant = event["constant"].as_str().unwrap();
        let value = event["value"].as_str().unwrap();
        assert_eq!(eval(&format!("Events.{constant}")), value);
        let kind = match value {
            "line" => EventKind::Line,
            "prompt" => EventKind::Prompt,
            "gmcp" => EventKind::Gmcp,
            "msdp" => EventKind::Msdp,
            "key" => EventKind::Key,
            other => panic!("unknown event {other}"),
        };
        let text = match kind {
            EventKind::Gmcp => r#"Char.Vitals {"hp":1}"#,
            EventKind::Msdp => r#"{"variable":"HEALTH","value":"9"}"#,
            _ => "F2",
        };
        let fields: Vec<&str> = event["payload"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap())
            .collect();
        let mut set = EngineSet::default();
        let source = format!("mud.on(Events.{constant}, e => mud.echo(Object.keys(e).sort().join(',')));");
        assert_eq!(set.load("e", &source, false).error, None);
        let result = set.dispatch_one("e", &ScriptEvent::new(kind, text));
        let mut sorted = fields.clone();
        sorted.sort_unstable();
        assert_eq!(result.actions[0].text, sorted.join(","), "{constant}");
    }
}

#[test]
fn every_limit_is_the_engines() {
    let reference = reference();
    let listed = reference["limits"].as_object().unwrap();
    assert_eq!(listed.len(), limits::ALL.len(), "same limits");
    for (name, value) in listed {
        let ours = limits::ALL
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("limit {name} is missing"));
        assert_eq!(Some(ours.1), value.as_u64(), "{name}");
    }
    assert_eq!(
        reference["state"]["report"]["maximumPerScript"].as_u64(),
        Some(limits::MSDP_REPORTS_PER_SCRIPT as u64)
    );
    // Limits the engine enforces itself, checked at their edges.
    let over = |source: &str| {
        let mut set = EngineSet::default();
        set.load("limit", source, false).error.unwrap_or_default()
    };
    assert!(over("for (let i = 0; i < 33; i++) mud.echo('x');").contains("Event action limit"));
    assert!(over("for (let i = 0; i < 257; i++) mud.on(Events.Line, () => {});").contains("256 hooks"));
    assert!(over("mud.every(0.5, () => {});").contains("Timer interval"));
    assert!(over("mud.echo('x'.repeat(8193));").contains("Echo exceeds"));
    assert!(over("mud.send('x'.repeat(4097));").contains("Commands must contain"));
    assert!(over("for (let i = 0; i < 9; i++) mud.panel('p' + i);").contains("8 panels"));
    assert!(over("mud.state.get('x'.repeat(513));").contains("512 characters"));
    assert!(over("eval('1')").contains("Code generation"));
    assert!(over("new Function('return 1')").contains("Code generation"));
    assert!(over("(function(){}).constructor('return 1')").contains("Code generation"));
    assert_eq!(
        over("const a = []; while (true) a.push('x'.repeat(1 << 20));"),
        "out of memory"
    );
    assert_eq!(
        over("function f(n) { return f(n + 1) + 1; } f(0);"),
        "Maximum call stack size exceeded"
    );
    assert_eq!(over(&"// padding\n".repeat(24_000)), "Source exceeds 256 KiB.");
    // Events past their size are refused; a seed may be larger.
    let mut set = EngineSet::default();
    set.load("big", "mud.on(Events.Line, () => {});", false);
    let event = ScriptEvent::new(EventKind::Line, "x".repeat(limits::EVENT_CHARACTERS + 1));
    assert!(set.dispatch_one("big", &event).error.is_some());
    assert!(
        set.seed(&format!(r#"{{"msdp":{{"BIG":"{}"}}}}"#, "x".repeat(40_000)))
            .is_empty()
    );
}

#[test]
fn panel_widgets_and_methods_match() {
    let reference = reference();
    let widgets: Vec<&str> = reference["panel"]["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["kind"].as_str().unwrap())
        .collect();
    assert_eq!(widgets, PANEL_WIDGET_KINDS);
    let methods: Vec<String> = reference["panel"]["methods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap().to_string())
        .collect();
    for name in widgets.iter().map(|s| s.to_string()).chain(methods) {
        assert_eq!(eval(&format!("typeof mud.panel('p').{name}")), "function", "{name}");
    }
    let bars = reference["panel"]["bars"]["error"].as_str().unwrap();
    let mut set = EngineSet::default();
    let result = set.load("bars", "mud.panel('b', { dock: 'bars' }).button('x');", false);
    assert_eq!(result.error.as_deref(), Some(bars));
}
