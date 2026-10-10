use super::*;

fn act(json: &str) -> PanelAction {
    PanelAction::parse(json).unwrap_or_else(|e| panic!("{json}: {e}"))
}

#[cfg(feature = "scripting")]
fn declared(source: &str) -> Vec<String> {
    let (engine, result) = crate::scripting::engine::Engine::load(source, false, None);
    assert!(engine.is_some(), "{:?}", result.error);
    result
        .actions
        .into_iter()
        .inspect(|a| assert_eq!(a.kind, crate::scripting::ActionKind::Panel))
        .map(|a| a.text)
        .collect()
}

#[cfg(feature = "scripting")]
fn load_error(source: &str) -> String {
    let (engine, result) = crate::scripting::engine::Engine::load(source, false, None);
    assert!(engine.is_none());
    assert!(result.actions.is_empty());
    result.error.unwrap_or_default()
}

/// C# `PanelDeclarationsBecomePanelActionsWithStableShapes`: every widget kind's instruction,
/// and the client parser accepts each one.
#[cfg(feature = "scripting")]
#[test]
fn panel_declarations_become_panel_actions_with_stable_shapes() {
    let actions = declared(
        r#"
        const p = mud.panel("ship", { title: "Ship", dock: "right" });
        p.gauge("hull", { label: "Hull", value: 0, max: 100 });
        p.label("system", { text: "In orbit" });
        p.button("flee", { label: "Flee", onClick: () => mud.send("flee") });
        p.toggle("auto", { label: "Auto repair", value: false, onChange: () => {} });
        p.input("say", { placeholder: "Say...", onSubmit: text => mud.send("say " + text) });
        p.list("crew", { title: "Crew", items: ["Ann", "Bo"], onSelect: item => mud.send("look " + item) });
        p.table("skills", { columns: ["Skill", "Level"], rows: [["Piloting", 12]] });
        p.separator("s1");
        p.group("g1", { title: "Combat", children: ["flee", "auto"] });
        p.remove("system");
        p.hide(); p.show();
        "#,
    );
    assert_eq!(
        actions,
        [
            r#"{"panel":"ship","action":"create","title":"Ship","dock":"right"}"#,
            r#"{"panel":"ship","action":"widget","widget":"hull","kind":"gauge","props":{"label":"Hull","value":0,"max":100}}"#,
            r#"{"panel":"ship","action":"widget","widget":"system","kind":"label","props":{"text":"In orbit"}}"#,
            r#"{"panel":"ship","action":"widget","widget":"flee","kind":"button","props":{"label":"Flee"}}"#,
            r#"{"panel":"ship","action":"widget","widget":"auto","kind":"toggle","props":{"label":"Auto repair","value":false}}"#,
            r#"{"panel":"ship","action":"widget","widget":"say","kind":"input","props":{"placeholder":"Say..."}}"#,
            r#"{"panel":"ship","action":"widget","widget":"crew","kind":"list","props":{"title":"Crew","items":["Ann","Bo"]}}"#,
            r#"{"panel":"ship","action":"widget","widget":"skills","kind":"table","props":{"columns":["Skill","Level"],"rows":[["Piloting","12"]]}}"#,
            r#"{"panel":"ship","action":"widget","widget":"s1","kind":"separator","props":{}}"#,
            r#"{"panel":"ship","action":"widget","widget":"g1","kind":"group","props":{"title":"Combat","children":["flee","auto"]}}"#,
            r#"{"panel":"ship","action":"remove","widget":"system"}"#,
            r#"{"panel":"ship","action":"hide"}"#,
            r#"{"panel":"ship","action":"show"}"#,
        ]
    );
    for action in &actions {
        act(action);
    }
    let table = act(&actions[7]);
    let Instruction::Widget { kind, props, .. } = table.instruction else {
        panic!()
    };
    assert_eq!(kind, WidgetKind::Table);
    assert_eq!(props.columns, ["Skill", "Level"]);
    assert_eq!(props.rows, [vec!["Piloting".to_string(), "12".into()]]);
}

/// C# `ABarsPanelDeclaresGaugesAndLabelsOnlyAndItsCreateActionCarriesTheBarsDock`.
#[cfg(feature = "scripting")]
#[test]
fn a_bars_panel_declares_gauges_and_labels_only_and_its_create_action_carries_the_bars_dock() {
    let actions = declared(
        r#"
        const bars = mud.panel("vitals", { title: "Vitals", dock: "bars" });
        bars.gauge("force", { label: "&CForce&D", value: 40, max: 80, warn: 0.25 });
        bars.label("note", { text: "ignored in the strip" });
        "#,
    );
    assert_eq!(
        actions[0],
        r#"{"panel":"vitals","action":"create","title":"Vitals","dock":"bars"}"#
    );
    assert_eq!(
        act(&actions[0]).instruction,
        Instruction::Create {
            title: "Vitals".into(),
            dock: Dock::Bars
        }
    );
    assert_eq!(actions.len(), 3);
    for source in [
        "mud.panel('v', { dock: 'bars' }).button('b', { label: 'x' });",
        "mud.panel('v', { dock: 'bars' }).text('t', { text: 'x' });",
        "mud.panel('v', { dock: 'bars' }).list('l', { items: [] });",
        // A panel that already holds a button cannot move into the strip.
        "mud.panel('v').button('b', { label: 'x' }); mud.panel('v', { dock: 'bars' });",
    ] {
        assert!(
            load_error(source).contains("A bars panel accepts only gauge and label widgets."),
            "{source}"
        );
    }
    assert!(load_error("mud.panel('p', { dock: 'top' });").contains("A panel docks to left, right or bars."));
    assert!(PanelAction::parse(r#"{"panel":"v","action":"create","title":"V","dock":"top"}"#).is_err());
    assert!(WidgetKind::Gauge.allowed_on_bars() && WidgetKind::Label.allowed_on_bars());
    assert!(WidgetKind::ALL.iter().filter(|k| k.allowed_on_bars()).count() == 2);
}

/// C# `PanelLimitViolationsStopTheScriptWithAnErrorAndNoActions`.
#[cfg(feature = "scripting")]
#[test]
fn panel_limit_violations_stop_the_script_with_an_error_and_no_actions() {
    for (source, expected) in [
        (
            "for (let i = 0; i < 9; i++) mud.panel('p' + i);",
            "Maximum 8 panels per script.",
        ),
        (
            "mud.panel('p').label('a', { text: 'x'.repeat(4097) });",
            "exceeds 4096 characters",
        ),
        (
            "mud.panel('p').list('a', { items: new Array(501).fill('x') });",
            "exceeds 500 items",
        ),
        (
            "mud.panel('p').table('a', { rows: new Array(501).fill(['x']) });",
            "rows exceeds 500 items.",
        ),
    ] {
        let error = load_error(source);
        assert!(error.contains(expected), "{source}: {error}");
    }
}

/// C# `FocusAndShowWithFocusEmitTheFocusActionAndTheParserAcceptsIt`.
#[cfg(feature = "scripting")]
#[test]
fn focus_and_show_with_focus_emit_the_focus_action_and_the_parser_accepts_it() {
    let actions = declared(
        r#"
        const p = mud.panel("combat", { title: "Combat" });
        p.focus();
        p.show({ focus: true });
        p.show();
        p.show({});
        p.hide().focus();
        "#,
    );
    assert_eq!(
        actions,
        [
            r#"{"panel":"combat","action":"create","title":"Combat","dock":"right"}"#,
            r#"{"panel":"combat","action":"focus"}"#,
            r#"{"panel":"combat","action":"show"}"#,
            r#"{"panel":"combat","action":"focus"}"#,
            r#"{"panel":"combat","action":"show"}"#,
            r#"{"panel":"combat","action":"show"}"#,
            r#"{"panel":"combat","action":"hide"}"#,
            r#"{"panel":"combat","action":"focus"}"#,
        ]
    );
    for action in &actions {
        assert_eq!(act(action).panel, "combat");
    }
    assert!(load_error("mud.panel('p').show(true);").contains("Show options must be an object."));
}

/// C# `InvalidPanelInstructionsAreRejectedByTheClientParser`, and the property checks.
#[test]
fn invalid_panel_instructions_are_rejected_by_the_client_parser() {
    for json in [
        r#"{"panel":"ship","action":"widget","widget":"a","kind":"nope","props":{}}"#,
        r#"{"panel":"ship","action":"nope"}"#,
        r#"{"panel":"bad id","action":"show"}"#,
        r#"{"panel":"ship","action":"create","title":"Ship","dock":"middle"}"#,
        "[]",
        "{",
        r#"{"panel":"-ship","action":"show"}"#,
        r#"{"panel":"ship","action":"widget","widget":"g","kind":"gauge","props":{"value":"7"}}"#,
        r#"{"panel":"ship","action":"widget","widget":"g","kind":"gauge","props":[]}"#,
        r#"{"panel":"ship","action":"widget","widget":"t","kind":"toggle","props":{"value":1}}"#,
        r#"{"panel":"ship","action":"widget","widget":"t","kind":"table","props":{"rows":[["a",1]]}}"#,
        r#"{"panel":"ship","action":"widget","widget":"t","kind":"table","props":{"rows":["a"]}}"#,
        r#"{"panel":"ship","action":"widget","widget":"l","kind":"list","props":{"items":"a"}}"#,
        r#"{"panel":"ship","action":"create","title":7,"dock":"right"}"#,
        r#"{"panel":"ship","action":"show","x":[[[[[[[[[1]]]]]]]]]}"#,
    ] {
        assert!(PanelAction::parse(json).is_err(), "{json}");
    }
    let long = format!(
        r#"{{"panel":"p","action":"widget","widget":"l","kind":"label","props":{{"text":"{}"}}}}"#,
        "x".repeat(4097)
    );
    assert!(PanelAction::parse(&long).unwrap_err().contains("4096"));
    let huge = format!(r#"{{"panel":"p","action":"show","pad":"{}"}}"#, "x".repeat(65_536));
    assert!(PanelAction::parse(&huge).unwrap_err().contains("65536"));
    assert!(is_identifier("lantern-watch") && is_identifier("a.b_c") && is_identifier(&"a".repeat(64)));
    assert!(!is_identifier("") && !is_identifier("_a") && !is_identifier(&"a".repeat(65)) && !is_identifier("é"));
}

#[test]
fn defaults_and_gauge_arithmetic() {
    let Instruction::Widget { props, .. } =
        act(r#"{"panel":"p","action":"widget","widget":"g","kind":"gauge","props":{"value":3}}"#).instruction
    else {
        panic!()
    };
    assert_eq!(props.maximum, 100.0);
    assert_eq!(props.percentage(), 3.0);
    assert!(!props.warned());
    let Instruction::Widget { props, .. } =
        act(r#"{"panel":"p","action":"widget","widget":"g","kind":"gauge","props":{"value":3,"max":10,"warn":0.3}}"#)
            .instruction
    else {
        panic!()
    };
    assert!(props.warned(), "3 of 10 is at the warn share");
    assert_eq!(measure(7.0, 10.0), "7 / 10");
    assert_eq!(measure(12.5, 1.0 / 3.0), "12.5 / 0.33");
    let Instruction::Widget { props, .. } =
        act(r#"{"panel":"p","action":"widget","widget":"b","kind":"button"}"#).instruction
    else {
        panic!()
    };
    assert_eq!(props.label, None, "without props a button shows its id (C#)");
}

#[test]
fn callback_messages_have_the_csharp_shape() {
    assert_eq!(
        event_json("ship", "flee", PanelEvent::Click, None),
        r#"{"panel":"ship","widget":"flee","event":"click","value":null}"#
    );
    assert_eq!(
        event_json("ship", "auto", PanelEvent::Change(true), None),
        r#"{"panel":"ship","widget":"auto","event":"change","value":true}"#
    );
    assert_eq!(
        event_json("ship", "say", PanelEvent::Submit, Some("hello \"you\"")),
        r#"{"panel":"ship","widget":"say","event":"submit","value":"hello \"you\""}"#
    );
    assert_eq!(
        event_json("ship", "crew", PanelEvent::Select, Some("Ann")),
        r#"{"panel":"ship","widget":"crew","event":"select","value":"Ann"}"#
    );
}

fn host_with(script: &str, actions: &[&str], now: Instant) -> PanelHost {
    let mut host = PanelHost::default();
    for a in actions {
        host.apply(script, act(a), now);
    }
    host
}

/// What the C# `ScriptPanelHost` and `ScriptPanel` do with instructions: declarations create,
/// updates change in place, stray updates are ignored, close and remove retire.
#[test]
fn the_host_builds_panels_and_updates_widgets_in_place() {
    let now = Instant::now();
    let mut host = host_with(
        "s1",
        &[
            r#"{"panel":"ship","action":"create","title":"&228A Vicious Womprat&D","dock":"right"}"#,
            r#"{"panel":"ship","action":"widget","widget":"hull","kind":"gauge","props":{"label":"Hull","value":10,"max":100}}"#,
            r#"{"panel":"ship","action":"widget","widget":"note","kind":"label","props":{"text":"In orbit"}}"#,
            r#"{"panel":"ghost","action":"widget","widget":"x","kind":"label","props":{"text":"never"}}"#,
        ],
        now,
    );
    assert_eq!(host.panels.len(), 1, "an update for an undeclared panel is ignored");
    let panel = &host.panels[0];
    assert_eq!(panel.title, "A Vicious Womprat", "titles lose their codes");
    assert_eq!(panel.widgets.len(), 2);
    let (panel_rev, hull_rev, host_rev) = (panel.revision, panel.widgets[0].revision, host.revision);

    // The same values again change nothing at all.
    let same = r#"{"panel":"ship","action":"widget","widget":"hull","kind":"gauge","props":{"label":"Hull","value":10,"max":100}}"#;
    for _ in 0..100 {
        assert!(!host.apply("s1", act(same), now));
    }
    assert!(!host.apply(
        "s1",
        act(r#"{"panel":"ship","action":"create","title":"&228A Vicious Womprat&D","dock":"right"}"#),
        now
    ));
    assert_eq!(host.revision, host_rev);

    // A new value moves the widget's revision only.
    assert!(host.apply(
        "s1",
        act(r#"{"panel":"ship","action":"widget","widget":"hull","kind":"gauge","props":{"label":"Hull","value":42,"max":100}}"#),
        now
    ));
    let panel = &host.panels[0];
    assert_eq!(panel.revision, panel_rev);
    assert!(panel.widgets[0].revision > hull_rev);
    assert_eq!(panel.widgets[0].props.number, 42.0);
    assert!(host.revision > host_rev);

    // Panels are per script: another script's "ship" is its own panel.
    host.apply(
        "s2",
        act(r#"{"panel":"ship","action":"create","title":"","dock":"left"}"#),
        now,
    );
    assert_eq!(host.panels.len(), 2);
    assert_eq!(host.panels[1].title, "ship", "an empty title shows the id");

    assert!(host.apply("s1", act(r#"{"panel":"ship","action":"remove","widget":"note"}"#), now));
    assert!(!host.apply("s1", act(r#"{"panel":"ship","action":"remove","widget":"note"}"#), now));
    assert_eq!(host.panels[0].widgets.len(), 1);
    assert!(host.apply("s1", act(r#"{"panel":"ship","action":"hide"}"#), now));
    assert!(!host.apply("s1", act(r#"{"panel":"ship","action":"hide"}"#), now));
    assert_eq!(host.rail().count(), 1);
    assert!(host.apply("s1", act(r#"{"panel":"ship","action":"show"}"#), now));
    assert!(host.apply("s1", act(r#"{"panel":"ship","action":"close"}"#), now));
    assert_eq!(host.panels.len(), 1);
    assert!(host.retain_scripts(|s| s != "s2"));
    assert!(host.is_empty());
}

#[test]
fn eight_panels_per_script_and_sixty_four_widgets_per_panel() {
    let now = Instant::now();
    let mut host = PanelHost::default();
    for i in 0..10 {
        host.apply(
            "s",
            act(&format!(
                r#"{{"panel":"p{i}","action":"create","title":"P","dock":"right"}}"#
            )),
            now,
        );
    }
    assert_eq!(host.panels.len(), 8);
    for i in 0..70 {
        host.apply(
            "s",
            act(&format!(
                r#"{{"panel":"p0","action":"widget","widget":"w{i}","kind":"separator"}}"#
            )),
            now,
        );
    }
    assert_eq!(host.panels[0].widgets.len(), 64);
}

/// C# focus rules: one request a second per panel; show again; nothing for a bars panel.
#[test]
fn focus_is_rate_limited_and_never_applies_to_bars() {
    let start = Instant::now();
    let mut host = host_with(
        "s",
        &[
            r#"{"panel":"cargo","action":"create","title":"Cargo","dock":"right"}"#,
            r#"{"panel":"cargo","action":"hide"}"#,
            r#"{"panel":"vitals","action":"create","title":"Vitals","dock":"bars"}"#,
        ],
        start,
    );
    let focus = || act(r#"{"panel":"cargo","action":"focus"}"#);
    assert!(host.apply("s", focus(), start));
    assert!(host.panels[0].visible, "focus shows a hidden panel");
    assert!(host.panels[0].take_focus_request());
    assert!(!host.panels[0].take_focus_request());
    assert!(!host.apply("s", focus(), start + Duration::from_millis(900)));
    assert!(!host.panels[0].focus_requested);
    assert!(host.apply("s", focus(), start + Duration::from_millis(1000)));
    assert!(host.panels[0].focus_requested);
    assert!(!host.apply("s", act(r#"{"panel":"vitals","action":"focus"}"#), start));
    assert!(!host.panels[1].focus_requested);
}

#[test]
fn bars_gauges_and_groups() {
    let now = Instant::now();
    let host = host_with(
        "s",
        &[
            r#"{"panel":"vitals","action":"create","title":"Vitals","dock":"bars"}"#,
            r#"{"panel":"vitals","action":"widget","widget":"force","kind":"gauge","props":{"label":"&CForce&D","value":40,"max":80}}"#,
            r#"{"panel":"vitals","action":"widget","widget":"note","kind":"label","props":{"text":"ignored"}}"#,
            r#"{"panel":"ship","action":"create","title":"Ship","dock":"right"}"#,
            r#"{"panel":"ship","action":"widget","widget":"flee","kind":"button","props":{"label":"Flee"}}"#,
            r#"{"panel":"ship","action":"widget","widget":"auto","kind":"toggle","props":{"label":"Auto"}}"#,
            r#"{"panel":"ship","action":"widget","widget":"g1","kind":"group","props":{"title":"Combat","children":["flee","g1","missing"]}}"#,
            r#"{"panel":"ship","action":"widget","widget":"g2","kind":"group","props":{"children":["flee","auto"]}}"#,
        ],
        now,
    );
    let gauges: Vec<&str> = host.bar_gauges().map(|(_, w)| w.id.as_str()).collect();
    assert_eq!(gauges, ["force"], "labels are not drawn in the strip");
    assert_eq!(host.rail().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["ship"]);
    let claimed = PanelHost::claimed(&host.panels[1]);
    assert_eq!(
        claimed.get("flee"),
        Some(&"g1"),
        "the first group to name a child keeps it"
    );
    assert_eq!(claimed.get("auto"), Some(&"g2"));
    assert_eq!(claimed.len(), 2);
    assert!(host.event("s", "ship", "flee", PanelEvent::Click, None).is_some());
    assert!(host.event("s", "ship", "gone", PanelEvent::Click, None).is_none());
    assert!(host.event("other", "ship", "flee", PanelEvent::Click, None).is_none());
}
