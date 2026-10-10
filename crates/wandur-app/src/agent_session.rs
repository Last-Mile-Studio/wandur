//! One session's agent (the C# `WorkspaceController.Agent` and `AgentSessionViewModel`): the
//! world's profile, this connection's goal selection, the runner, and the observation the model
//! sees.
//!
//! The observation is built from public output only: the session feeds server text while input
//! is public and no login runs, and resets the context (a new generation, the text dropped, a
//! run stopped) when a private stretch or a login begins, when the connection closes and when a
//! command is typed. Chat lines (the channel rules' verdict, as in the Channels panel) are left
//! out, and do not count as fresh output while the agent waits. Recognized GMCP room and vitals
//! messages and decoded MSDP rooms are added as protocol context.
//!
//! Without the `agent` feature the session keeps an empty stand-in with the same calls.

#[cfg(feature = "agent")]
pub use imp::*;

#[cfg(not(feature = "agent"))]
pub use stub::*;

#[cfg(feature = "agent")]
mod imp {
    use std::sync::Arc;
    use std::time::Instant;

    use wandur_core::agent::profile::{self, AgentGoal, AgentProfile};
    use wandur_core::agent::{AgentProfileStore, AgentRunner, AgentStatus, AgentWorld, ProviderRegistry, credentials};
    use wandur_core::channels::RuleSet;
    use wandur_core::channels::rules::strip;
    use wandur_core::login::PasswordVault;
    use wandur_core::protocol::GmcpMessage;
    use wandur_core::session::Waker;

    /// Raw public text kept (bytes): far more than the lines the model sees.
    const RAW_LIMIT: usize = 32 * 1024;
    /// Lines of recent output the model sees (the C# agent terminal's 60 rows).
    const LINES: usize = 60;
    /// The longest output text in an observation (characters, the newest kept).
    const TEXT_LIMIT: usize = 8000;
    /// The longest protocol context (characters).
    const PROTOCOL_LIMIT: usize = 4000;
    /// Between the protocol context and the output (English, text for the model as in C#).
    const OUTPUT_HEADING: &str = "\nRecent world output:\n";

    /// What the agent needs from the app: profiles, providers and the vault.
    pub struct AgentServices {
        pub store: Arc<dyn AgentProfileStore>,
        pub providers: ProviderRegistry,
        pub vault: Arc<dyn PasswordVault>,
        /// The settings look models up by themselves (scenes turn it off).
        pub discover: bool,
    }

    impl std::fmt::Debug for AgentServices {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("AgentServices")
                .field("providers", &self.providers)
                .finish_non_exhaustive()
        }
    }

    impl AgentServices {
        /// A runner whose key comes from this vault.
        pub fn runner(&self, waker: Option<Waker>) -> AgentRunner {
            let vault = Arc::clone(&self.vault);
            AgentRunner::new(
                self.providers.clone(),
                Arc::new(move |p: &AgentProfile| credentials::read(vault.as_ref(), p)),
                waker,
            )
        }
    }

    /// Public output and protocol context, with the revision and generation the runner checks.
    #[derive(Debug, Default)]
    struct Feed {
        raw: String,
        protocol: String,
        revision: u64,
        generation: u64,
        /// The text built for `revision`.
        built: Option<(u64, String)>,
    }

    impl Feed {
        /// Public server text. `judge` (while a run waits) leaves chat out of the revision.
        fn push(&mut self, text: &str, judge: Option<&RuleSet>) {
            if text.is_empty() {
                return;
            }
            let line_start = self.raw.rfind('\n').map_or(0, |i| i + 1);
            self.raw.push_str(text);
            let fresh = match judge {
                None => true,
                Some(rules) => self.raw[line_start..].split('\n').any(|line| {
                    let plain = strip(line);
                    let plain = plain.trim_end_matches('\r');
                    !plain.trim().is_empty() && !rules.is_channel_line(plain)
                }),
            };
            if self.raw.len() > RAW_LIMIT {
                let mut cut = self.raw.len() - RAW_LIMIT;
                while !self.raw.is_char_boundary(cut) {
                    cut += 1;
                }
                let cut = self.raw[cut..].find('\n').map_or(cut, |i| cut + i + 1);
                self.raw.drain(..cut);
            }
            if fresh {
                self.revision += 1;
            }
        }

        fn protocol(&mut self, text: &str) {
            let text: String = text.chars().take(PROTOCOL_LIMIT).collect();
            if text != self.protocol {
                self.protocol = text;
                self.revision += 1;
            }
        }

        fn reset(&mut self) {
            self.generation += 1;
            self.raw.clear();
            self.protocol.clear();
            self.revision += 1;
            self.built = None;
        }

        fn is_empty(&self) -> bool {
            self.raw.is_empty() && self.protocol.is_empty()
        }

        /// The observation text: protocol context, then the recent output without chat.
        fn text(&mut self, rules: &RuleSet) -> String {
            if let Some((revision, text)) = &self.built
                && *revision == self.revision
            {
                return text.clone();
            }
            let lines: Vec<&str> = self.raw.split('\n').collect();
            let recent = &lines[lines.len().saturating_sub(LINES + 1)..];
            let mut plain = String::new();
            for line in recent {
                let line = strip(line);
                let line = line.trim_end_matches('\r');
                if rules.is_channel_line(line) {
                    continue;
                }
                if !plain.is_empty() {
                    plain.push('\n');
                }
                plain.push_str(line);
            }
            let count = plain.chars().count();
            if count > TEXT_LIMIT {
                plain = plain.chars().skip(count - TEXT_LIMIT).collect();
            }
            let text = [self.protocol.as_str(), OUTPUT_HEADING, &plain].concat();
            self.built = Some((self.revision, text.clone()));
            text
        }
    }

    /// One session's agent.
    #[derive(Debug, Default)]
    pub struct AgentSession {
        services: Option<Arc<AgentServices>>,
        world: Option<AgentWorld>,
        /// This connection's goals: the world's, with this session's own selection.
        pub goals: Vec<AgentGoal>,
        pub runner: AgentRunner,
        feed: Feed,
        /// The agent has control (Step or Play): nothing else sends.
        pub owns_control: bool,
        /// Moves when the goals or the selection change (the UI redraws).
        pub goals_revision: u64,
    }

    impl AgentSession {
        /// Set up for a world (a new connection): memory cleared, the profile's goals with its
        /// default selection. A profile that cannot be read leaves the agent unavailable.
        pub fn configure(&mut self, services: Arc<AgentServices>, world: AgentWorld, waker: Option<Waker>) {
            self.runner = services.runner(waker);
            match services.store.load(&world) {
                Ok(profile) => {
                    self.goals = profile::goals_of(&profile);
                    self.world = Some(world);
                }
                Err(_) => {
                    self.goals.clear();
                    self.world = None;
                    self.runner.note_status(AgentStatus::Failed);
                }
            }
            self.services = Some(services);
            self.goals_revision += 1;
        }

        /// Whether this build and session have an agent.
        pub fn available(&self) -> bool {
            self.services.is_some()
        }

        pub fn world(&self) -> Option<&AgentWorld> {
            self.world.as_ref()
        }

        /// The world's profile as saved now.
        pub fn profile(&self) -> Option<AgentProfile> {
            let services = self.services.as_ref()?;
            services.store.load(self.world.as_ref()?).ok()
        }

        /// The selected goal's instructions ("" when none is selected).
        pub fn goal(&self) -> String {
            profile::compose(&self.goals).unwrap_or_default()
        }

        pub fn has_goal(&self) -> bool {
            !self.goal().trim().is_empty()
        }

        /// Play, Preview and Step are possible.
        pub fn can_start(&self) -> bool {
            !self.runner.is_busy() && self.world.is_some() && self.has_goal()
        }

        /// Choose the goal at `index` for this connection (the radio button); `false` when it
        /// was already chosen. The caller clears the working memory (a goal change stops a run).
        pub fn select_goal(&mut self, index: usize) -> bool {
            if index >= self.goals.len() || self.goals[index].enabled {
                return false;
            }
            for (i, goal) in self.goals.iter_mut().enumerate() {
                goal.enabled = i == index;
            }
            self.goals_revision += 1;
            true
        }

        /// The world's agent settings were saved: the goals are read again, keeping this
        /// session's selection when that goal still exists (the C# `LoadGoals(preserve)`).
        pub fn profile_saved(&mut self, profile: &AgentProfile) {
            let selected = self.goals.iter().find(|g| g.enabled).map(|g| g.id);
            let had_goals = !self.goals.is_empty();
            let mut goals = profile::goals_of(profile);
            let keep = had_goals && selected.is_none_or(|id| goals.iter().any(|g| g.id == id));
            if keep {
                for goal in &mut goals {
                    goal.enabled = Some(goal.id) == selected;
                }
            }
            self.goals = goals;
            self.goals_revision += 1;
        }

        /// Public server text.
        pub fn feed_text(&mut self, text: &str, rules: &RuleSet) {
            if self.services.is_none() {
                return;
            }
            let judge = self.runner.is_busy().then_some(rules);
            self.feed.push(text, judge);
        }

        /// A public GMCP message: room and vitals messages only, never login or other packages.
        pub fn feed_gmcp(&mut self, message: &GmcpMessage) {
            if self.services.is_none() {
                return;
            }
            let room = message.package.eq_ignore_ascii_case("Room.Info");
            let vitals = message.package.eq_ignore_ascii_case("Char.Vitals");
            if room || vitals {
                let raw = String::from_utf8_lossy(&message.raw);
                self.feed.protocol(raw.trim());
            }
        }

        /// A public MSDP payload: a decoded room only.
        pub fn feed_msdp(&mut self, payload: &[u8]) {
            if self.services.is_none() {
                return;
            }
            if let Some(room) = wandur_core::map::decode::from_msdp(payload) {
                let json = serde_json::json!({
                    "Id": room.server_id,
                    "Name": room.name,
                    "Description": room.description,
                    "Area": room.area,
                    "Exits": room
                        .exits
                        .iter()
                        .map(|(direction, to)| (direction.clone(), serde_json::json!(to)))
                        .collect::<serde_json::Map<_, _>>(),
                    "Environment": room.environment,
                });
                self.feed.protocol(&json.to_string());
            }
        }

        /// Whether there is context to drop (C# resets only then, or while busy).
        pub fn has_context(&self) -> bool {
            !self.feed.is_empty() || self.runner.is_busy()
        }

        /// A new generation: the text is dropped. (The caller stops the runner.)
        pub fn reset_feed(&mut self) {
            self.feed.reset();
        }

        pub fn revision(&self) -> (u64, u64) {
            (self.feed.revision, self.feed.generation)
        }

        /// The observation text.
        pub fn observation_text(&mut self, rules: &RuleSet) -> String {
            self.feed.text(rules)
        }

        pub fn is_busy(&self) -> bool {
            self.runner.is_busy()
        }

        /// The agent has control (Step or Play): no other command source sends.
        pub fn has_control(&self) -> bool {
            self.owns_control
        }

        pub fn deadline(&self) -> Option<Instant> {
            self.runner.deadline()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn rules() -> RuleSet {
            RuleSet::new(vec![wandur_core::channels::ChannelRule::new(
                "chat",
                r"^\[Chat\] (?<speaker>\w+): (?<text>.*)$",
                None,
            )])
        }

        #[test]
        fn the_feed_keeps_public_lines_without_chat_and_counts_fresh_output() {
            let rules = rules();
            let mut feed = Feed::default();
            feed.push("\x1b[1mA quiet room.\x1b[0m\r\n[Chat] Wren: hello\n", None);
            let first = feed.revision;
            assert_eq!(feed.text(&rules), "\nRecent world output:\nA quiet room.\n");
            // While a run waits, chat alone is not fresh output; anything else is.
            feed.push("[Chat] Bastian: anyone?\n", Some(&rules));
            assert_eq!(feed.revision, first);
            feed.push("A doorway leads north.\n> ", Some(&rules));
            assert_eq!(feed.revision, first + 1);
            assert!(feed.text(&rules).ends_with("A doorway leads north.\n> "));
            feed.protocol("Room.Info {\"num\":1}");
            assert!(
                feed.text(&rules)
                    .starts_with("Room.Info {\"num\":1}\nRecent world output:\n")
            );
            let generation = feed.generation;
            feed.reset();
            assert_eq!(feed.generation, generation + 1);
            assert!(feed.is_empty());
        }

        #[test]
        fn the_feed_is_bounded() {
            let rules = rules();
            let mut feed = Feed::default();
            for i in 0..5000 {
                feed.push(&format!("line {i} {}\n", "x".repeat(40)), None);
            }
            assert!(feed.raw.len() <= RAW_LIMIT);
            let text = feed.text(&rules);
            assert!(text.chars().count() <= TEXT_LIMIT + OUTPUT_HEADING.len());
            assert!(text.contains("line 4999"));
            assert!(!text.contains("line 4900 "), "sixty lines at most");
            feed.protocol(&"p".repeat(5000));
            assert_eq!(feed.protocol.chars().count(), PROTOCOL_LIMIT);
        }
    }
}

#[cfg(not(feature = "agent"))]
mod stub {
    use std::time::Instant;

    use wandur_core::channels::RuleSet;
    use wandur_core::protocol::GmcpMessage;

    /// No agent in this build.
    #[derive(Debug, Default)]
    pub struct AgentSession;

    impl AgentSession {
        pub fn available(&self) -> bool {
            false
        }
        pub fn feed_text(&mut self, _: &str, _: &RuleSet) {}
        pub fn feed_gmcp(&mut self, _: &GmcpMessage) {}
        pub fn feed_msdp(&mut self, _: &[u8]) {}
        pub fn has_context(&self) -> bool {
            false
        }
        pub fn reset_feed(&mut self) {}
        pub fn is_busy(&self) -> bool {
            false
        }
        pub fn has_control(&self) -> bool {
            false
        }
        pub fn deadline(&self) -> Option<Instant> {
            None
        }
    }
}
