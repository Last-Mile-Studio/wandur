//! Script panels per frame: ten panels in the session rail (two scripts), one open with a dozen
//! widgets, drawn headless and tessellated, while scripts re-send every widget each frame with
//! unchanged values, with changed gauge values, or not at all. Unchanged values should cost what
//! no updates cost: the host keeps every revision and the rail builds no new layout.
//!
//! `WANDUR_MICRO_ONLY_PANELS=1 wandur-bench micro --label NAME` (run with `--release`).

use std::time::Instant;

use egui::{RawInput, Rect, pos2, vec2};
use wandur_app::panel_view::{self, RailState};
use wandur_app::sysstat;
use wandur_app::theme::Theme;
use wandur_core::scripting::panels::{PanelAction, PanelHost};

use crate::micro::Row;

/// The instructions of panel `i` of `script`, with gauge values from `tick`.
fn declarations(i: usize, tick: u64) -> Vec<String> {
    let p = format!("p{i}");
    let mut out = vec![format!(
        r#"{{"panel":"{p}","action":"create","title":"&YPanel {i}&D","dock":"right"}}"#
    )];
    for g in 0..3 {
        out.push(format!(
            r#"{{"panel":"{p}","action":"widget","widget":"g{g}","kind":"gauge","props":{{"label":"&CGauge {g}&D","value":{},"max":100,"warn":0.3}}}}"#,
            (tick + g) % 100
        ));
    }
    out.push(format!(
        r#"{{"panel":"{p}","action":"widget","widget":"name","kind":"label","props":{{"text":"&228A Vicious Womprat&D in &Rred&D"}}}}"#
    ));
    out.push(format!(
        r#"{{"panel":"{p}","action":"widget","widget":"note","kind":"text","props":{{"text":"The road is clear to the ford."}}}}"#
    ));
    out.push(format!(
        r#"{{"panel":"{p}","action":"widget","widget":"flee","kind":"button","props":{{"label":"&YFlee&D now"}}}}"#
    ));
    out.push(format!(
        r#"{{"panel":"{p}","action":"widget","widget":"auto","kind":"toggle","props":{{"label":"Auto repair","value":true}}}}"#
    ));
    out.push(format!(
        r#"{{"panel":"{p}","action":"widget","widget":"crew","kind":"list","props":{{"title":"Crew","items":["Ann","&GBo&D","Cy","Dee"]}}}}"#
    ));
    let rows: Vec<String> = (0..6).map(|r| format!(r#"["&WSkill {r}&D","{}"]"#, r * 7)).collect();
    out.push(format!(
        r#"{{"panel":"{p}","action":"widget","widget":"skills","kind":"table","props":{{"title":"Skills","columns":["Skill","Level"],"rows":[{}]}}}}"#,
        rows.join(",")
    ));
    out.push(format!(
        r#"{{"panel":"{p}","action":"widget","widget":"s","kind":"separator"}}"#
    ));
    out.push(format!(
        r#"{{"panel":"{p}","action":"widget","widget":"say","kind":"input","props":{{"placeholder":"Say..."}}}}"#
    ));
    out
}

fn script_of(i: usize) -> &'static str {
    if i < 8 { "first" } else { "second" }
}

#[derive(Clone, Copy, PartialEq)]
enum Updates {
    None,
    Unchanged,
    Changed,
}

pub fn panels(rows: &mut Vec<Row>) {
    let ctx = egui::Context::default();
    wandur_app::fonts::install(&ctx);
    let theme = Theme::preset("Hull");
    for (name, updates) in [
        ("ten panels, no updates", Updates::None),
        (
            "ten panels, every widget re-sent unchanged each frame",
            Updates::Unchanged,
        ),
        (
            "ten panels, every widget re-sent, gauges changed each frame",
            Updates::Changed,
        ),
    ] {
        let mut host = PanelHost::default();
        for i in 0..10 {
            for json in declarations(i, 0) {
                host.apply(script_of(i), PanelAction::parse(&json).unwrap(), Instant::now());
            }
        }
        let mut state = RailState::default();
        let frames = 400u64;
        let mut times = Vec::new();
        let mut draws = Vec::new();
        let mut applies = Vec::new();
        let mut allocated = Vec::new();
        let mut built_before = 0;
        for frame in 0..frames + 20 {
            if frame == 20 {
                built_before = state.layouts_built;
            }
            let a0 = sysstat::allocated_bytes();
            let start = Instant::now();
            let apply_start = Instant::now();
            if updates != Updates::None {
                let tick = if updates == Updates::Changed { frame } else { 0 };
                for i in 0..10 {
                    for json in declarations(i, tick) {
                        host.apply(script_of(i), PanelAction::parse(&json).unwrap(), Instant::now());
                    }
                }
            }
            let applied = apply_start.elapsed().as_secs_f64() * 1000.0;
            let draw_start = Instant::now();
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(panel_view::RAIL_WIDTH, 760.0))),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                    panel_view::sync(&mut host, &mut state);
                    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(panel_view::RAIL_WIDTH, 760.0));
                    panel_view::show(ui, rect, &host, &mut state, &theme);
                });
            });
            out.textures_delta.clear();
            let _ = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
            if frame >= 20 {
                draws.push(draw_start.elapsed().as_secs_f64() * 1000.0);
                applies.push(applied);
                times.push(start.elapsed().as_secs_f64() * 1000.0);
                allocated.push((sysstat::allocated_bytes() - a0) as f64 / 1024.0);
            }
        }
        times.sort_by(f64::total_cmp);
        draws.sort_by(f64::total_cmp);
        applies.sort_by(f64::total_cmp);
        let mean = draws.iter().sum::<f64>() / draws.len() as f64;
        assert_eq!(state.drawn.len(), 11, "the open panel's widgets are drawn");
        rows.push(Row {
            name: format!("Rail frame, {name}"),
            unit: "one frame",
            ms: draws[draws.len() / 2],
            kb: allocated.iter().sum::<f64>() / allocated.len() as f64,
            note: format!(
                "draw and tessellate: mean {mean:.3} ms, p95 {:.3} ms; parse and apply {:.3} ms; whole frame {:.3} ms; {} layouts built over {frames} frames",
                draws[draws.len() * 95 / 100],
                applies[applies.len() / 2],
                times[times.len() / 2],
                state.layouts_built - built_before
            ),
        });
    }
}
