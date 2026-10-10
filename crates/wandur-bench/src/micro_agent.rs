//! The agent's cost on the UI thread: feeding public output into its observation (every chunk
//! of every session with an agent, run or not) and building the observation text a decision
//! sends (once per decision, and per frame while a run waits, cached by revision).

use std::sync::Arc;
use std::time::Instant;

use wandur_app::agent_session::{AgentServices, AgentSession};
use wandur_core::agent::{AgentWorld, MemoryAgentProfileStore, ProviderRegistry};
use wandur_core::channels::RuleSet;
use wandur_core::login::MemoryVault;

use crate::micro::Row;

pub fn agent(rows: &mut Vec<Row>) {
    let chunks = crate::generator::AnsiGenerator::new(42, true).chunks(15_000, 4096);
    let bytes: usize = chunks.iter().map(|c| c.len()).sum();
    let services = Arc::new(AgentServices {
        store: Arc::new(MemoryAgentProfileStore::new()),
        providers: ProviderRegistry::default(),
        vault: Arc::new(MemoryVault::new()),
        discover: false,
    });
    let rules = wandur_core::channels::families::for_world(&[], Some("SmaugFUSS"));
    let rules: &RuleSet = &rules;
    let mut times = Vec::new();
    for _ in 0..41 {
        let mut agent = AgentSession::default();
        agent.configure(Arc::clone(&services), AgentWorld::demo(), None);
        let start = Instant::now();
        for chunk in &chunks {
            agent.feed_text(chunk, rules);
        }
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    rows.push(Row {
        name: "Agent observation feed, 1 MB of flood".into(),
        unit: "1 MB",
        ms: times[times.len() / 2] * 1_000_000.0 / bytes as f64,
        kb: 0.0,
        note: format!("{bytes} bytes in {} chunks", chunks.len()),
    });
    let mut agent = AgentSession::default();
    agent.configure(Arc::clone(&services), AgentWorld::demo(), None);
    for chunk in &chunks {
        agent.feed_text(chunk, rules);
    }
    let mut times = Vec::new();
    for i in 0..201 {
        // A new line each time, so the text is built again (not the cached copy).
        agent.feed_text(&format!("A new line {i}\r\n"), rules);
        let start = Instant::now();
        let text = agent.observation_text(rules);
        times.push(start.elapsed().as_secs_f64() * 1000.0);
        assert!(text.contains("A new line"));
    }
    times.sort_by(f64::total_cmp);
    rows.push(Row {
        name: "Agent observation text (60 lines, chat filtered)".into(),
        unit: "build",
        ms: times[times.len() / 2],
        kb: 0.0,
        note: "after 1 MB of flood".into(),
    });
}
