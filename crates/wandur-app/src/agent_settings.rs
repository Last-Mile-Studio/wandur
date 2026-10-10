//! The world editor's Agent settings section (the C# `AgentSettingsView`, `AgentGoalsEditor`
//! and `AgentProfileViewModel`): server address and provider, model discovery, the optional
//! API key, the Instructions, Goals and Allowed commands tabs, and the run limits.
//!
//! Everything is a draft until Save world, like the other sections. The settings never start
//! the agent or send a command. Models load by themselves when the section opens and after the
//! address, the provider or the key change (after a short pause in typing); a saved key is
//! used for discovery only while the address and provider are the saved ones.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::{RichText, Ui};
use wandur_core::agent::profile::{self, AgentGoal, AgentProfile, LMSTUDIO_NATIVE, OPENAI_COMPATIBLE, TEMPLATES};
use wandur_core::agent::{AgentError, AgentWorld, CancelToken, credentials};
use wandur_core::l10n::{S, t, tf};

use crate::agent_session::AgentServices;
use crate::theme::Theme;
use crate::widgets::{self, Icon};

/// The providers in list order, with their names.
pub const PROVIDERS: [(&str, S); 2] = [
    (OPENAI_COMPATIBLE, S::AgentOpenAiCompatible),
    (LMSTUDIO_NATIVE, S::AgentLmStudioNative),
];
/// The pause after an edit before models are looked up.
pub const DISCOVERY_DELAY: Duration = Duration::from_millis(450);
/// Discovery waits at most this long for the server.
const DISCOVERY_TIMEOUT: i32 = 10;

/// The tabs under the connection fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditorTab {
    #[default]
    Instructions,
    Goals,
    Commands,
}

/// The goal editor's two Markdown tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GoalTab {
    #[default]
    Description,
    Rules,
}

/// The run limits, as edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_decisions: i32,
    pub max_run_seconds: i32,
    pub action_interval_seconds: i32,
    pub response_timeout_seconds: i32,
    pub max_input_characters: i32,
    pub max_output_tokens: i32,
}

impl Limits {
    fn of(p: &AgentProfile) -> Self {
        Self {
            max_decisions: p.max_decisions,
            max_run_seconds: p.max_run_seconds,
            action_interval_seconds: p.action_interval_seconds,
            response_timeout_seconds: p.response_timeout_seconds,
            max_input_characters: p.max_input_characters,
            max_output_tokens: p.max_output_tokens,
        }
    }
}

/// What is compared to tell whether the draft changed.
#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    provider: usize,
    endpoint: String,
    model: String,
    system_prompt: String,
    commands: String,
    json_mode: bool,
    limits: Limits,
    goals: Vec<AgentGoal>,
}

/// A model lookup in flight, by generation (an older one's answer is dropped).
struct Discovery {
    generation: u64,
    reply: Receiver<Result<Vec<String>, AgentError>>,
    cancel: CancelToken,
}

/// What Save world writes for the agent.
#[derive(Clone, Debug)]
pub struct AgentChange {
    pub profile: AgentProfile,
    pub new_key: String,
    pub forget: bool,
}

/// The section's draft and view state.
pub struct AgentDraft {
    services: Arc<AgentServices>,
    saved: AgentProfile,
    pub server_address: String,
    pub endpoint: String,
    pub provider: usize,
    pub model: String,
    pub system_prompt: String,
    pub goals: Vec<AgentGoal>,
    pub selected_goal: Option<usize>,
    pub commands: String,
    pub api_key: String,
    pub forget_key: bool,
    pub json_mode: bool,
    pub limits: Limits,
    /// Models the server offered.
    pub models: Vec<String>,
    pub error: Option<String>,
    pub status: Option<S>,
    pub discovering: bool,
    discovery: Option<Discovery>,
    generation: u64,
    discover_at: Option<Instant>,
    baseline: Snapshot,
    /// Model lookups started (tests).
    pub lookups: u64,
    pub tab: EditorTab,
    pub goal_tab: GoalTab,
    pub credentials_open: bool,
    pub limits_open: bool,
    templates_open: bool,
    /// Wakes the UI when a lookup answers.
    ctx: Option<egui::Context>,
    /// Heights measured in the last frame (the editors take what is left).
    connection_height: f32,
    limits_height: f32,
}

impl Clone for AgentDraft {
    /// A copy without the lookup in flight (the world editor's draft is cloned in tests).
    fn clone(&self) -> Self {
        Self {
            services: Arc::clone(&self.services),
            saved: self.saved.clone(),
            server_address: self.server_address.clone(),
            endpoint: self.endpoint.clone(),
            provider: self.provider,
            model: self.model.clone(),
            system_prompt: self.system_prompt.clone(),
            goals: self.goals.clone(),
            selected_goal: self.selected_goal,
            commands: self.commands.clone(),
            api_key: self.api_key.clone(),
            forget_key: self.forget_key,
            json_mode: self.json_mode,
            limits: self.limits,
            models: self.models.clone(),
            error: self.error.clone(),
            status: self.status,
            discovering: false,
            discovery: None,
            generation: self.generation,
            discover_at: self.discover_at,
            baseline: self.baseline.clone(),
            lookups: self.lookups,
            tab: self.tab,
            goal_tab: self.goal_tab,
            credentials_open: self.credentials_open,
            limits_open: self.limits_open,
            templates_open: false,
            ctx: self.ctx.clone(),
            connection_height: self.connection_height,
            limits_height: self.limits_height,
        }
    }
}

impl std::fmt::Debug for AgentDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentDraft")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("goals", &self.goals.len())
            .finish_non_exhaustive()
    }
}

/// The address as shown: the endpoint without the API path the provider adds.
pub fn without_api_path(value: &str) -> String {
    let address = value.trim().trim_end_matches('/');
    for suffix in ["/api/v1", "/v1"] {
        if address.len() >= suffix.len() && address[address.len() - suffix.len()..].eq_ignore_ascii_case(suffix) {
            return address[..address.len() - suffix.len()].to_string();
        }
    }
    address.to_string()
}

/// The endpoint for an address and provider: `http://` when no scheme is given, and the
/// provider's API path.
pub fn compose_endpoint(address: &str, provider: usize) -> String {
    let address = without_api_path(address);
    if address.trim().is_empty() {
        return String::new();
    }
    let address = if address.contains("://") {
        address
    } else {
        ["http://", &address].concat()
    };
    [address.as_str(), if provider == 1 { "/api/v1" } else { "/v1" }].concat()
}

/// The message for a failed lookup.
pub fn discovery_error(error: &AgentError) -> String {
    match error {
        AgentError::Http(Some(401 | 403)) => t(S::AgentDiscoveryUnauthorized).into(),
        AgentError::Http(Some(404)) | AgentError::InvalidResponse => t(S::AgentDiscoveryWrongApi).into(),
        AgentError::Http(Some(code)) => tf(S::AgentDiscoveryServerError, &[code]),
        AgentError::Http(None) => t(S::AgentDiscoveryUnreachable).into(),
        AgentError::Timeout => t(S::AgentDiscoveryTimedOut).into(),
        AgentError::Invalid(_) => t(S::AgentServerAddressInvalid).into(),
        _ => t(S::AgentDiscoveryFailed).into(),
    }
}

impl AgentDraft {
    /// The section for `world` (its saved profile), or for a new world (the defaults). Models
    /// are looked up shortly after.
    pub fn open(services: Arc<AgentServices>, world: Option<&AgentWorld>, ctx: Option<egui::Context>) -> Self {
        let saved = world.and_then(|w| services.store.load(w).ok()).unwrap_or_default();
        Self::with_profile(services, saved, ctx)
    }

    pub fn with_profile(services: Arc<AgentServices>, saved: AgentProfile, ctx: Option<egui::Context>) -> Self {
        let provider = PROVIDERS.iter().position(|(k, _)| *k == saved.provider).unwrap_or(0);
        let goals = profile::goals_of(&saved);
        let mut draft = Self {
            services,
            server_address: without_api_path(&saved.endpoint),
            endpoint: saved.endpoint.clone(),
            provider,
            model: saved.model.clone(),
            system_prompt: saved.system_prompt.clone(),
            selected_goal: (!goals.is_empty()).then_some(0),
            goals,
            commands: saved.commands.clone(),
            api_key: String::new(),
            forget_key: false,
            json_mode: saved.json_mode,
            limits: Limits::of(&saved),
            models: Vec::new(),
            error: None,
            status: None,
            discovering: false,
            discovery: None,
            generation: 0,
            discover_at: None,
            baseline: Snapshot {
                provider: 0,
                endpoint: String::new(),
                model: String::new(),
                system_prompt: String::new(),
                commands: String::new(),
                json_mode: false,
                limits: Limits::of(&saved),
                goals: Vec::new(),
            },
            lookups: 0,
            tab: EditorTab::default(),
            goal_tab: GoalTab::default(),
            credentials_open: false,
            limits_open: false,
            templates_open: false,
            ctx,
            connection_height: 260.0,
            limits_height: 48.0,
            saved,
        };
        draft.baseline = draft.snapshot();
        if draft.services.discover {
            draft.schedule(Instant::now());
        }
        draft
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            provider: self.provider,
            endpoint: self.endpoint.clone(),
            model: self.model.clone(),
            system_prompt: self.system_prompt.clone(),
            commands: self.commands.clone(),
            json_mode: self.json_mode,
            limits: self.limits,
            goals: self.goals.clone(),
        }
    }

    pub fn has_unsaved_changes(&self) -> bool {
        !self.api_key.is_empty() || self.forget_key || self.snapshot() != self.baseline
    }

    /// The OpenAI-compatible provider can ask for JSON mode; the native one cannot.
    pub fn supports_json_mode(&self) -> bool {
        self.provider == 0
    }

    pub fn endpoint_help(&self) -> &'static str {
        t(if self.provider == 1 {
            S::AgentNativeEndpointHelp
        } else {
            S::AgentCompatibleEndpointHelp
        })
    }

    /// The profile as it would be saved.
    pub fn draft(&self) -> AgentProfile {
        AgentProfile {
            provider: PROVIDERS[self.provider.min(PROVIDERS.len() - 1)].0.into(),
            endpoint: self.endpoint.trim().to_string(),
            model: self.model.trim().to_string(),
            system_prompt: self.system_prompt.clone(),
            default_goal: String::new(),
            goals: self.goals.clone(),
            commands: self.commands.clone(),
            json_mode: self.supports_json_mode() && self.json_mode,
            max_decisions: self.limits.max_decisions,
            max_run_seconds: self.limits.max_run_seconds,
            action_interval_seconds: self.limits.action_interval_seconds,
            response_timeout_seconds: self.limits.response_timeout_seconds,
            max_input_characters: self.limits.max_input_characters,
            max_output_tokens: self.limits.max_output_tokens,
            ..self.saved.clone()
        }
    }

    /// Check the draft before anything is saved (a model is required, as in C#).
    pub fn validate(&self) -> Result<(), String> {
        profile::validate(&self.draft(), true).map_err(|_| t(S::AgentInvalidSettings).to_string())
    }

    /// What Save world writes, when the section changed.
    pub fn change(&self) -> Option<AgentChange> {
        self.has_unsaved_changes().then(|| AgentChange {
            profile: self.draft(),
            new_key: self.api_key.clone(),
            forget: self.forget_key,
        })
    }

    /// Save the change for `world` (the key into the vault, the profile into the store).
    /// Returns the profile as saved, or the message to show.
    pub fn save(&mut self, world: &AgentWorld) -> Result<Option<AgentProfile>, String> {
        let Some(change) = self.change() else {
            return Ok(None);
        };
        let saved = credentials::save(
            self.services.store.as_ref(),
            self.services.vault.as_ref(),
            world,
            change.profile,
            &change.new_key,
            change.forget,
        )
        .map_err(|e| match e {
            AgentError::Invalid(_) => t(S::AgentInvalidSettings).to_string(),
            _ => t(S::AgentSaveFailed).to_string(),
        });
        match saved {
            Ok(profile) => {
                self.saved = profile.clone();
                self.api_key.clear();
                self.forget_key = false;
                self.baseline = self.snapshot();
                self.status = Some(S::AgentSettingsSaved);
                self.error = None;
                Ok(Some(profile))
            }
            Err(e) => {
                self.error = Some(e.clone());
                Err(e)
            }
        }
    }

    /// Something the lookup depends on changed: drop the models and look again shortly.
    fn connection_changed(&mut self, now: Instant) {
        self.cancel_discovery();
        self.models.clear();
        self.error = None;
        self.status = None;
        self.schedule(now);
    }

    fn schedule(&mut self, now: Instant) {
        self.discover_at = Some(now + DISCOVERY_DELAY);
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint_after(DISCOVERY_DELAY);
        }
    }

    fn cancel_discovery(&mut self) {
        if let Some(d) = self.discovery.take() {
            d.cancel.cancel();
        }
        self.generation += 1;
        self.discovering = false;
    }

    pub fn set_server_address(&mut self, address: &str, now: Instant) {
        self.server_address = address.to_string();
        self.endpoint = compose_endpoint(address, self.provider);
        self.connection_changed(now);
    }

    /// Set the endpoint itself (an older profile's full address); the address follows.
    pub fn set_endpoint(&mut self, endpoint: &str, now: Instant) {
        self.endpoint = endpoint.to_string();
        self.server_address = without_api_path(endpoint);
        self.connection_changed(now);
    }

    pub fn set_provider(&mut self, provider: usize, now: Instant) {
        if provider == self.provider || provider >= PROVIDERS.len() {
            return;
        }
        self.provider = provider;
        if !self.supports_json_mode() {
            self.json_mode = false;
        }
        self.endpoint = compose_endpoint(&self.server_address, provider);
        self.connection_changed(now);
    }

    pub fn set_api_key(&mut self, key: &str, now: Instant) {
        self.api_key = key.to_string();
        if !key.is_empty() {
            self.forget_key = false;
        }
        self.connection_changed(now);
    }

    pub fn set_forget_key(&mut self, forget: bool, now: Instant) {
        self.forget_key = forget;
        if forget {
            self.api_key.clear();
        }
        self.connection_changed(now);
    }

    /// Choose a discovered model: it becomes the model id.
    pub fn choose_model(&mut self, model: &str) {
        self.model = model.to_string();
    }

    /// Look the models up now (the refresh button, or after the pause).
    pub fn discover(&mut self) {
        self.discover_at = None;
        self.cancel_discovery();
        self.error = None;
        let generation = self.generation;
        let mut draft = AgentProfile {
            provider: PROVIDERS[self.provider.min(PROVIDERS.len() - 1)].0.into(),
            endpoint: self.endpoint.clone(),
            model: String::new(),
            ..self.saved.clone()
        };
        draft.response_timeout_seconds = draft.response_timeout_seconds.min(DISCOVERY_TIMEOUT);
        if profile::validate(&draft, false).is_err() {
            self.error = Some(t(S::AgentServerAddressInvalid).into());
            self.status = None;
            return;
        }
        let provider = match self.services.providers.resolve(&draft.provider) {
            Ok(p) => p,
            Err(e) => {
                self.error = Some(discovery_error(&e));
                return;
            }
        };
        // A saved key is only sent to the address and provider it was saved for.
        let typed = (!self.api_key.trim().is_empty()).then(|| self.api_key.clone());
        let use_saved = typed.is_none()
            && !self.forget_key
            && draft.endpoint == self.saved.endpoint
            && draft.provider == self.saved.provider;
        let saved = self.saved.clone();
        let vault = Arc::clone(&self.services.vault);
        let cancel = CancelToken::new();
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        let thread_cancel = cancel.clone();
        let spawned = std::thread::Builder::new()
            .name("wandur-agent-models".into())
            .spawn(move || {
                let key = match typed {
                    Some(key) => Ok(Some(key)),
                    None if use_saved => credentials::read(vault.as_ref(), &saved),
                    None => Ok(None),
                };
                let answer = key.and_then(|key| provider.list_models(&draft, key.as_deref(), &thread_cancel));
                if tx.send(answer).is_ok()
                    && let Some(ctx) = ctx
                {
                    ctx.request_repaint();
                }
            });
        if spawned.is_err() {
            self.error = Some(t(S::AgentDiscoveryFailed).into());
            return;
        }
        self.lookups += 1;
        self.discovering = true;
        self.status = Some(S::AgentDiscovering);
        self.discovery = Some(Discovery {
            generation,
            reply: rx,
            cancel,
        });
    }

    /// Start a lookup that is due, and take the answer of one that came.
    pub fn poll(&mut self, now: Instant) {
        if self.discover_at.is_some_and(|at| now >= at) {
            self.discover();
        }
        let Some(discovery) = &self.discovery else { return };
        let answer = match discovery.reply.try_recv() {
            Err(TryRecvError::Empty) => return,
            Ok(answer) => answer,
            Err(TryRecvError::Disconnected) => Err(AgentError::Storage(String::new())),
        };
        let current = discovery.generation == self.generation;
        self.discovery = None;
        if !current {
            return;
        }
        self.discovering = false;
        match answer {
            Ok(models) => {
                if !models.contains(&self.model) && self.model.trim().is_empty() && models.len() == 1 {
                    self.model = models[0].clone();
                }
                self.status = Some(if models.is_empty() {
                    S::AgentNoModels
                } else {
                    S::AgentModelsLoaded
                });
                self.models = models;
            }
            Err(error) => {
                self.error = Some(discovery_error(&error));
                self.status = None;
            }
        }
    }

    /// When the next lookup is due (for a repaint).
    pub fn deadline(&self) -> Option<Instant> {
        self.discover_at
    }

    /// The section is closed: nothing more is looked up and the typed key is forgotten.
    pub fn close(&mut self) {
        self.discover_at = None;
        self.cancel_discovery();
        self.api_key.clear();
    }

    fn edited(&mut self) {
        self.status = None;
        self.error = None;
    }

    pub fn add_goal(&mut self) {
        if self.goals.len() >= profile::MAX_GOALS {
            return;
        }
        self.goals.push(AgentGoal::new("", false));
        self.selected_goal = Some(self.goals.len() - 1);
        self.edited();
    }

    /// Add an editable copy of a starter goal (never selected; the commands do not change).
    pub fn add_template(&mut self, key: &str) {
        if self.goals.len() >= profile::MAX_GOALS {
            return;
        }
        if let Ok(goal) = profile::template(key) {
            self.goals.push(goal);
            self.selected_goal = Some(self.goals.len() - 1);
            self.edited();
        }
    }

    pub fn delete_goal(&mut self) {
        let Some(i) = self.selected_goal.filter(|&i| i < self.goals.len()) else {
            return;
        };
        self.goals.remove(i);
        self.selected_goal = (!self.goals.is_empty()).then_some(0);
        self.edited();
    }

    /// "Selected by default for new sessions": one goal at most.
    pub fn set_default(&mut self, index: usize, on: bool) {
        if index >= self.goals.len() {
            return;
        }
        for (i, goal) in self.goals.iter_mut().enumerate() {
            if i == index {
                goal.enabled = on;
            } else if on {
                goal.enabled = false;
            }
        }
        self.edited();
    }
}

/// A small muted label above a field.
fn caption(ui: &mut Ui, text: &str, theme: &Theme) {
    ui.label(RichText::new(text).size(11.0).color(theme.muted));
    crate::a11y::set_pending(ui, text);
}

fn help(ui: &mut Ui, text: &str, theme: &Theme) {
    ui.add(egui::Label::new(RichText::new(text).size(11.0).color(theme.muted)).wrap());
}

/// A row of tab headers with the selected one underlined.
fn tab_headers<T: Copy + PartialEq>(ui: &mut Ui, tabs: &[(T, &str)], selected: &mut T, theme: &Theme) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for &(tab, label) in tabs {
            let on = *selected == tab;
            let color = if on { theme.text } else { theme.muted };
            let galley = ui
                .painter()
                .layout_no_wrap(label.to_string(), egui::FontId::proportional(12.0), color);
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(galley.size().x + 24.0, 32.0), egui::Sense::click());
            ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
            if on {
                ui.painter().hline(
                    rect.shrink2(egui::vec2(10.0, 0.0)).x_range(),
                    rect.bottom() - 1.5,
                    egui::Stroke::new(2.0, theme.accent),
                );
            }
            response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, label));
            if response.clicked() {
                *selected = tab;
            }
        }
    });
}

/// A labelled number field with its range.
fn limit(ui: &mut Ui, label: &str, value: &mut i32, range: std::ops::RangeInclusive<i32>, width: f32, theme: &Theme) {
    ui.vertical(|ui| {
        ui.set_width(width);
        caption(ui, label, theme);
        crate::a11y::named(
            ui.add_sized(
                egui::vec2(width, 28.0),
                egui::DragValue::new(value).range(range).speed(1.0),
            ),
            label,
        );
    });
}

/// Draw the section.
pub fn show(ui: &mut Ui, draft: &mut AgentDraft, theme: &Theme) {
    if draft.ctx.is_none() {
        draft.ctx = Some(ui.ctx().clone());
    }
    let now = Instant::now();
    draft.poll(now);
    if let Some(at) = draft.deadline() {
        ui.ctx().request_repaint_after(at.saturating_duration_since(now));
    }
    if draft.discovering {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
    let full = ui.max_rect();
    // The toolbar: refresh (discover models) and the title.
    let bar = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), 30.0));
    ui.painter().rect_filled(bar, 0.0, theme.shell);
    ui.painter()
        .hline(bar.x_range(), bar.bottom(), egui::Stroke::new(1.0, theme.border));
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(bar.shrink2(egui::vec2(8.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if widgets::tool_button(ui, Icon::Reconnect, None, theme, true)
                .on_hover_text(t(S::AgentDiscoverModels))
                .clicked()
            {
                draft.discover();
            }
            ui.label(RichText::new(t(S::AgentSettings)).size(12.0).color(theme.text));
        },
    );
    // The footer: progress, error, status, unsaved.
    let mut footer_lines = Vec::new();
    if let Some(error) = &draft.error {
        footer_lines.push((error.clone(), theme.error, 12.0));
    }
    if let Some(status) = draft.status {
        footer_lines.push((t(status).to_string(), theme.muted, 11.0));
    }
    if draft.has_unsaved_changes() {
        footer_lines.push((t(S::ScriptUnsaved).to_string(), theme.muted, 11.0));
    }
    let footer_height = if footer_lines.is_empty() && !draft.discovering {
        0.0
    } else {
        8.0 + footer_lines.len() as f32 * 17.0 + if draft.discovering { 5.0 } else { 0.0 }
    };
    let body = egui::Rect::from_min_max(
        egui::pos2(full.left(), bar.bottom() + 1.0),
        egui::pos2(full.right(), full.bottom() - footer_height),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        egui::ScrollArea::vertical()
            .id_salt("agent-settings-scroll")
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| form(ui, draft, theme, body.height(), now));
            });
    });
    if footer_height > 0.0 {
        let foot = egui::Rect::from_min_max(egui::pos2(full.left() + 12.0, body.bottom() + 4.0), full.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(foot), |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            if draft.discovering {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width() - 12.0, 3.0), egui::Sense::hover());
                let phase = (ui.input(|i| i.time) * 0.8).fract() as f32;
                let w = rect.width() * 0.3;
                let x = rect.left() + (rect.width() + w) * phase - w;
                ui.painter().rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(x.max(rect.left()), rect.top()),
                        egui::pos2((x + w).min(rect.right()), rect.bottom()),
                    ),
                    1.0,
                    theme.accent,
                );
            }
            for (text, color, size) in footer_lines {
                ui.label(RichText::new(text).size(size).color(color));
            }
        });
    }
}

fn form(ui: &mut Ui, draft: &mut AgentDraft, theme: &Theme, viewport: f32, now: Instant) {
    let width = ui.available_width();
    ui.spacing_mut().item_spacing.y = 4.0;
    let top = ui.cursor().top();
    // Connection.
    caption(ui, t(S::AgentServerAddress), theme);
    let mut address = draft.server_address.clone();
    if crate::a11y::named(
        ui.add(
            egui::TextEdit::singleline(&mut address)
                .id_salt("agent-server-address")
                .desired_width(width)
                .margin(egui::Margin::symmetric(6, 5)),
        ),
        t(S::AgentServerAddress),
    )
    .changed()
    {
        draft.set_server_address(&address, now);
    }
    help(ui, t(S::AgentServerAddressHelp), theme);
    ui.add_space(3.0);
    caption(ui, t(S::AgentHostingProvider), theme);
    let mut provider = draft.provider;
    crate::a11y::named_combo(
        egui::ComboBox::from_id_salt("agent-provider")
            .width(width)
            .selected_text(RichText::new(t(PROVIDERS[provider].1)).size(12.0))
            .show_ui(ui, |ui| {
                for (i, (_, name)) in PROVIDERS.iter().enumerate() {
                    ui.selectable_value(&mut provider, i, t(*name));
                }
            }),
        t(S::AgentHostingProvider),
    );
    if provider != draft.provider {
        draft.set_provider(provider, now);
    }
    help(ui, draft.endpoint_help(), theme);
    ui.add_space(3.0);
    let half = (width - 8.0) / 2.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.vertical(|ui| {
            ui.set_width(half);
            caption(ui, t(S::AgentModelId), theme);
            if crate::a11y::named(
                ui.add(
                    egui::TextEdit::singleline(&mut draft.model)
                        .id_salt("agent-model")
                        .char_limit(256)
                        .desired_width(half)
                        .margin(egui::Margin::symmetric(6, 5)),
                ),
                t(S::AgentModelId),
            )
            .changed()
            {
                draft.edited();
            }
        });
        ui.vertical(|ui| {
            ui.set_width(half);
            caption(ui, t(S::AgentDiscoveredModels), theme);
            let shown = if draft.models.contains(&draft.model) {
                RichText::new(draft.model.clone()).size(12.0)
            } else {
                RichText::new(t(S::AgentChooseModel)).size(12.0).color(theme.muted)
            };
            let mut chosen = None;
            crate::a11y::named_combo(
                egui::ComboBox::from_id_salt("agent-models")
                    .width(half)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for model in &draft.models {
                            if ui.selectable_label(*model == draft.model, model.as_str()).clicked() {
                                chosen = Some(model.clone());
                            }
                        }
                    }),
                t(S::AgentDiscoveredModels),
            );
            if let Some(model) = chosen {
                draft.choose_model(&model);
            }
        });
    });
    ui.add_space(4.0);
    let mut open = draft.credentials_open;
    widgets::expander(ui, &mut open, t(S::AgentCredentials), width, theme, |ui| {
        caption(ui, t(S::AgentApiKey), theme);
        let mut key = draft.api_key.clone();
        if crate::a11y::named(
            ui.add(
                egui::TextEdit::singleline(&mut key)
                    .id_salt("agent-api-key")
                    .password(true)
                    .desired_width(ui.available_width())
                    .margin(egui::Margin::symmetric(6, 5)),
            ),
            t(S::AgentApiKey),
        )
        .changed()
        {
            draft.set_api_key(&key, now);
        }
        help(ui, t(S::AgentKeyHelp), theme);
        let mut forget = draft.forget_key;
        if ui
            .checkbox(&mut forget, RichText::new(t(S::AgentForgetKey)).size(12.0))
            .changed()
        {
            draft.set_forget_key(forget, now);
        }
    });
    draft.credentials_open = open;
    ui.add_space(8.0);
    draft.connection_height = ui.cursor().top() - top;
    // The editors take what is left of the view, at least 140 points.
    let editors = (viewport - draft.connection_height - draft.limits_height - 40.0).max(140.0);
    let editors_top = ui.cursor().top();
    tab_headers(
        ui,
        &[
            (EditorTab::Instructions, t(S::AgentSystemPrompt)),
            (EditorTab::Goals, t(S::AgentGoals)),
            (EditorTab::Commands, t(S::AgentCommands)),
        ],
        &mut draft.tab,
        theme,
    );
    let body_height = editors - 36.0;
    match draft.tab {
        EditorTab::Instructions => {
            if text_tab(
                ui,
                t(S::AgentSystemPromptHelp),
                "agent-instructions",
                &mut draft.system_prompt,
                body_height,
                theme,
            ) {
                draft.edited();
            }
        }
        EditorTab::Commands => {
            if text_tab(
                ui,
                t(S::AgentCommandsHelp),
                "agent-commands",
                &mut draft.commands,
                body_height,
                theme,
            ) {
                draft.edited();
            }
        }
        EditorTab::Goals => goals_editor(ui, draft, body_height, theme),
    }
    let used = ui.cursor().top() - editors_top;
    if used < editors {
        ui.add_space(editors - used);
    }
    ui.add_space(8.0);
    let limits_top = ui.cursor().top();
    let mut open = draft.limits_open;
    widgets::expander(ui, &mut open, t(S::AgentLimits), width, theme, |ui| {
        let column = (ui.available_width() - 16.0) / 3.0;
        let l = &mut draft.limits;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            limit(
                ui,
                t(S::AgentMaxDecisions),
                &mut l.max_decisions,
                1..=1000,
                column,
                theme,
            );
            limit(
                ui,
                t(S::AgentMaxRunSeconds),
                &mut l.max_run_seconds,
                1..=86_400,
                column,
                theme,
            );
            limit(
                ui,
                t(S::AgentActionInterval),
                &mut l.action_interval_seconds,
                1..=3600,
                column,
                theme,
            );
        });
        ui.add_space(6.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            limit(
                ui,
                t(S::AgentResponseTimeout),
                &mut l.response_timeout_seconds,
                1..=600,
                column,
                theme,
            );
            limit(
                ui,
                t(S::AgentMaxInputCharacters),
                &mut l.max_input_characters,
                1024..=128_000,
                column,
                theme,
            );
            limit(
                ui,
                t(S::AgentMaxOutputTokens),
                &mut l.max_output_tokens,
                64..=8192,
                column,
                theme,
            );
        });
        if draft.supports_json_mode() {
            ui.add_space(6.0);
            ui.checkbox(&mut draft.json_mode, RichText::new(t(S::AgentJsonMode)).size(12.0));
        }
    });
    draft.limits_open = open;
    draft.limits_height = ui.cursor().top() - limits_top;
}

/// A tab with a help line and a multi-line text box filling `height`. Returns whether the text
/// changed.
fn text_tab(ui: &mut Ui, help_text: &str, id: &str, text: &mut String, height: f32, theme: &Theme) -> bool {
    ui.add_space(4.0);
    help(ui, help_text, theme);
    ui.add_space(4.0);
    let rest = (height - 30.0).max(80.0);
    let width = ui.available_width();
    let mut changed = false;
    egui::ScrollArea::vertical()
        .id_salt((id, "scroll"))
        .max_height(rest)
        .min_scrolled_height(rest)
        .auto_shrink(false)
        .show(ui, |ui| {
            changed = crate::a11y::named(
                ui.add_sized(
                    egui::vec2(width - 2.0, rest),
                    egui::TextEdit::multiline(text)
                        .id_salt(id)
                        .font(egui::FontId::proportional(12.0))
                        .margin(egui::Margin::symmetric(6, 5)),
                ),
                help_text,
            )
            .changed();
        });
    changed
}

/// The Goals tab (the C# `AgentGoalsEditor`): help, the toolbar (add, delete, Add template),
/// the goal list beside the name, the Markdown description and rules, and the default box.
fn goals_editor(ui: &mut Ui, draft: &mut AgentDraft, height: f32, theme: &Theme) {
    ui.add_space(2.0);
    help(ui, t(S::AgentGoalsEditHelp), theme);
    let selected = draft.selected_goal.filter(|&i| i < draft.goals.len());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if widgets::tool_button(ui, Icon::Plus, None, theme, draft.goals.len() < profile::MAX_GOALS)
            .on_hover_text(t(S::AgentAddGoal))
            .clicked()
        {
            draft.add_goal();
        }
        if widgets::tool_button(ui, Icon::Trash, None, theme, selected.is_some())
            .on_hover_text(t(S::AgentDeleteGoal))
            .clicked()
        {
            draft.delete_goal();
        }
        let templates = ui.add(
            egui::Button::new(RichText::new([t(S::AgentAddTemplate), "  ⌄"].concat()).size(12.0))
                .min_size(egui::vec2(0.0, 30.0)),
        );
        if templates.clicked() {
            draft.templates_open = !draft.templates_open;
        }
        if draft.templates_open {
            let mut open = true;
            let mut chosen = None;
            egui::Popup::from_response(&templates)
                .id(egui::Id::new("agent-templates"))
                .open_bool(&mut open)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                .show(|ui| {
                    ui.set_min_width(180.0);
                    for (key, name) in TEMPLATES {
                        if ui.button(RichText::new(t(name)).size(12.0)).clicked() {
                            chosen = Some(key);
                        }
                    }
                });
            if let Some(key) = chosen {
                draft.add_template(key);
                open = false;
            }
            draft.templates_open = open;
        }
    });
    let width = ui.available_width();
    let rest = (height - 110.0).max(130.0);
    let list_width = (width - 8.0) / 4.0;
    let fields_width = width - 8.0 - list_width;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        // The list.
        let (list_rect, _) = ui.allocate_exact_size(egui::vec2(list_width, rest), egui::Sense::hover());
        ui.painter().rect_filled(list_rect, 2.0, ui.visuals().extreme_bg_color);
        let list_ui = egui::UiBuilder::new()
            .max_rect(list_rect)
            .layout(egui::Layout::top_down(egui::Align::Min));
        ui.scope_builder(list_ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("agent-goal-list")
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (i, goal) in draft.goals.iter().enumerate() {
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(list_width, 35.0), egui::Sense::click());
                        if Some(i) == selected {
                            ui.painter().rect_filled(rect, 2.0, theme.selection_fill());
                        } else if response.hovered() {
                            ui.painter().rect_filled(rect, 2.0, theme.hover_fill());
                        }
                        let name = widgets::clipped(
                            ui,
                            goal.display_name(),
                            egui::FontId::proportional(12.0),
                            theme.text,
                            list_width - 20.0,
                            1,
                        );
                        let at = egui::pos2(rect.left() + 10.0, rect.center().y - name.size().y / 2.0);
                        ui.painter().galley(at, name, theme.text);
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::WidgetType::SelectableLabel,
                                true,
                                Some(i) == selected,
                                goal.display_name(),
                            )
                        });
                        if response.clicked() {
                            draft.selected_goal = Some(i);
                        }
                    }
                });
        });
        // The fields.
        ui.vertical(|ui| {
            ui.set_width(fields_width);
            ui.add_enabled_ui(selected.is_some(), |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                let mut empty = String::new();
                let goal_index = selected.unwrap_or(0);
                let mut changed = false;
                {
                    let name = match selected {
                        Some(i) => &mut draft.goals[i].name,
                        None => &mut empty,
                    };
                    changed |= crate::a11y::named(
                        ui.add(
                            egui::TextEdit::singleline(name)
                                .id_salt("agent-goal-name")
                                .hint_text(t(S::AgentGoalName))
                                .char_limit(profile::MAX_GOAL_NAME)
                                .desired_width(fields_width)
                                .margin(egui::Margin::symmetric(8, 5)),
                        ),
                        t(S::AgentGoalName),
                    )
                    .changed();
                }
                tab_headers(
                    ui,
                    &[
                        (GoalTab::Description, t(S::AgentGoalDescription)),
                        (GoalTab::Rules, t(S::AgentGoalRules)),
                    ],
                    &mut draft.goal_tab,
                    theme,
                );
                let editor_height = (rest - 80.0).max(80.0);
                let tab = draft.goal_tab;
                let text = match (selected, tab) {
                    (Some(i), GoalTab::Description) => &mut draft.goals[i].text,
                    (Some(i), GoalTab::Rules) => &mut draft.goals[i].rules,
                    (None, _) => &mut empty,
                };
                let id = egui::Id::new(("agent-goal-markdown", goal_index, tab == GoalTab::Rules));
                changed |= crate::markdown_editor::markdown_editor(ui, id, text, theme, editor_height).changed();
                ui.label(RichText::new(t(S::AgentGoalRulesHelp)).size(10.0).color(theme.muted));
                if changed {
                    draft.edited();
                }
            });
        });
    });
    ui.add_space(4.0);
    let mut on = selected.is_some_and(|i| draft.goals[i].enabled);
    if ui
        .add_enabled(
            selected.is_some(),
            egui::Checkbox::new(&mut on, RichText::new(t(S::AgentGoalDefaultEnabled)).size(12.0)),
        )
        .changed()
        && let Some(i) = selected
    {
        draft.set_default(i, on);
    }
}

#[cfg(test)]
mod tests {
    //! The C# `AgentSettingsTests` cases, against fake providers and a memory store and vault.

    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    use wandur_core::agent::decision_codec::{AgentDecision, AgentRequest};
    use wandur_core::agent::{AgentModelProvider, AgentProfileStore, MemoryAgentProfileStore, ProviderRegistry};
    use wandur_core::login::{MemoryVault, PasswordVault};

    /// A provider that answers model lists from a script, counting calls.
    #[derive(Default)]
    struct Models {
        calls: AtomicU64,
        endpoint: Mutex<Option<String>>,
        key: Mutex<Option<Option<String>>>,
        answer: Mutex<Option<Result<Vec<String>, AgentError>>>,
        /// Hold the answer until released.
        hold: Mutex<Option<mpsc::Receiver<()>>>,
    }

    struct Fake(Arc<Models>, &'static str);

    impl AgentModelProvider for Fake {
        fn key(&self) -> &str {
            self.1
        }
        fn decide(&self, _: &AgentRequest, _: Option<&str>, _: &CancelToken) -> Result<AgentDecision, AgentError> {
            Err(AgentError::InvalidResponse)
        }
        fn list_models(&self, p: &AgentProfile, key: Option<&str>, _: &CancelToken) -> Result<Vec<String>, AgentError> {
            self.0.calls.fetch_add(1, Ordering::SeqCst);
            *self.0.endpoint.lock().unwrap() = Some(p.endpoint.clone());
            *self.0.key.lock().unwrap() = Some(key.map(str::to_string));
            let hold = self.0.hold.lock().unwrap().take();
            if let Some(hold) = hold {
                let _ = hold.recv();
            }
            self.0
                .answer
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| Ok(vec!["gemma-12b".into()]))
        }
    }

    /// A vault that counts reads (the C# `CredentialReads`).
    #[derive(Default)]
    struct CountingVault {
        inner: MemoryVault,
        reads: AtomicU64,
    }

    impl PasswordVault for CountingVault {
        fn name(&self) -> String {
            self.inner.name()
        }
        fn read(&self, key: &str) -> Result<Option<String>, wandur_core::login::VaultError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.inner.read(key)
        }
        fn write(&self, key: &str, password: &str) -> Result<(), wandur_core::login::VaultError> {
            self.inner.write(key, password)
        }
        fn delete(&self, key: &str) -> Result<(), wandur_core::login::VaultError> {
            self.inner.delete(key)
        }
    }

    struct Setup {
        models: Arc<Models>,
        vault: Arc<CountingVault>,
        store: Arc<MemoryAgentProfileStore>,
        services: Arc<AgentServices>,
        world: AgentWorld,
    }

    fn setup() -> Setup {
        let models = Arc::new(Models::default());
        let vault = Arc::new(CountingVault::default());
        let store = Arc::new(MemoryAgentProfileStore::new());
        let world = AgentWorld::Key("world".into());
        // A saved profile with a saved key (C# `CredentialId = Guid.NewGuid()`, "saved-secret").
        let saved = credentials::save(
            store.as_ref(),
            vault.as_ref(),
            &world,
            store.load(&world).unwrap(),
            "saved-secret",
            false,
        )
        .unwrap();
        vault.reads.store(0, Ordering::SeqCst);
        assert!(saved.credential_id.is_some());
        let services = Arc::new(AgentServices {
            store: Arc::clone(&store) as Arc<dyn AgentProfileStore>,
            providers: ProviderRegistry::new(vec![
                Arc::new(Fake(Arc::clone(&models), OPENAI_COMPATIBLE)),
                Arc::new(Fake(Arc::clone(&models), LMSTUDIO_NATIVE)),
            ]),
            vault: Arc::clone(&vault) as Arc<dyn PasswordVault>,
            discover: true,
        });
        Setup {
            models,
            vault,
            store,
            services,
            world,
        }
    }

    fn open(s: &Setup) -> AgentDraft {
        AgentDraft::open(Arc::clone(&s.services), Some(&s.world), None)
    }

    /// Poll until `done` (lookups answer on their own thread).
    fn settle(draft: &mut AgentDraft, done: impl Fn(&AgentDraft) -> bool) {
        let wall = Instant::now();
        while !done(draft) {
            draft.poll(Instant::now() + Duration::from_secs(1));
            std::thread::sleep(Duration::from_millis(2));
            assert!(wall.elapsed() < Duration::from_secs(5), "timed out");
        }
    }

    #[test]
    fn templates_are_editable_copies_and_one_default_at_most() {
        let s = setup();
        let mut d = open(&s);
        d.model = "gemma".into();
        d.add_template("explore");
        let first = d.selected_goal.unwrap();
        d.add_template("explore");
        let second = d.selected_goal.unwrap();
        assert_ne!(d.goals[first].id, d.goals[second].id);
        assert!(d.goals[first].text.contains("##"));
        assert!(d.goals[first].rules.contains("- "));
        d.set_default(first, true);
        d.set_default(second, true);
        assert!(!d.goals[first].enabled);
        assert!(d.goals[second].enabled);
        d.goals[second].text = "# Custom objective\nFind **a room** with `look`.".into();
        d.goals[second].rules = "- Do not attack.\n- Stop after one observation.".into();
        assert_ne!(d.goals[first].text, d.goals[second].text);
        let saved = d.save(&s.world).unwrap().unwrap();
        assert!(d.error.is_none());
        assert_eq!(saved.goals[1].rules, d.goals[second].rules);
        assert_eq!(s.store.load(&s.world).unwrap().goals[1].rules, d.goals[second].rules);
        assert!(!d.has_unsaved_changes());
    }

    #[test]
    fn goals_migrate_from_legacy_settings_and_save_independent_defaults() {
        let s = setup();
        let legacy = AgentProfile {
            default_goal: "Explore".into(),
            model: "gemma".into(),
            ..s.store.load(&s.world).unwrap()
        };
        s.store.save(&s.world, &legacy).unwrap();
        let mut d = open(&s);
        assert_eq!(d.goals.len(), 1);
        assert_eq!(d.goals[0].text, "Explore");
        d.add_goal();
        let i = d.selected_goal.unwrap();
        d.goals[i].text = "Find food".into();
        d.set_default(i, false);
        d.save(&s.world).unwrap();
        let stored = s.store.load(&s.world).unwrap();
        assert!(stored.default_goal.is_empty());
        assert_eq!(stored.goals.len(), 2);
        assert!(!stored.goals[1].enabled);
        let mut reopened = open(&s);
        assert_eq!(reopened.goals[1].text, "Find food");
        reopened.selected_goal = Some(0);
        reopened.delete_goal();
        reopened.save(&s.world).unwrap();
        let stored = s.store.load(&s.world).unwrap();
        assert_eq!(stored.goals.len(), 1);
        assert_eq!(stored.goals[0].text, "Find food");
    }

    #[test]
    fn host_and_provider_changes_wait_for_a_pause_and_ignore_unfinished_command_rows() {
        let s = setup();
        let mut d = open(&s);
        let now = Instant::now();
        d.set_provider(1, now);
        d.set_server_address("host-one:5555", now);
        d.commands = "unfinished row".into();
        d.set_server_address("host-two:9999", now);
        d.poll(now + Duration::from_millis(100));
        assert_eq!(s.models.calls.load(Ordering::SeqCst), 0, "still typing");
        settle(&mut d, |d| !d.models.is_empty());
        assert_eq!(
            s.models.endpoint.lock().unwrap().as_deref(),
            Some("http://host-two:9999/api/v1")
        );
        assert_eq!(s.models.calls.load(Ordering::SeqCst), 1);
        assert_eq!(d.model, "gemma-12b", "the only model is chosen when none is set");
        assert_eq!(
            s.vault.reads.load(Ordering::SeqCst),
            0,
            "no saved key for another address"
        );
        assert_eq!(d.status, Some(S::AgentModelsLoaded));
    }

    #[test]
    fn a_failed_lookup_says_why_and_stops_being_busy() {
        for (failure, expected) in [
            (
                AgentError::Http(Some(401)),
                t(S::AgentDiscoveryUnauthorized).to_string(),
            ),
            (AgentError::Http(None), t(S::AgentDiscoveryUnreachable).to_string()),
            (AgentError::InvalidResponse, t(S::AgentDiscoveryWrongApi).to_string()),
            (AgentError::Http(Some(500)), tf(S::AgentDiscoveryServerError, &[&500])),
            (AgentError::Timeout, t(S::AgentDiscoveryTimedOut).to_string()),
        ] {
            let s = setup();
            *s.models.answer.lock().unwrap() = Some(Err(failure));
            let mut d = open(&s);
            d.discover();
            settle(&mut d, |d| !d.discovering);
            assert_eq!(d.error.as_deref(), Some(expected.as_str()));
            assert!(!d.discovering);
        }
    }

    #[test]
    fn closing_before_the_pause_sends_nothing_and_models_load_on_open() {
        let s = setup();
        let mut d = open(&s);
        d.close();
        d.poll(Instant::now() + Duration::from_secs(1));
        assert_eq!(s.models.calls.load(Ordering::SeqCst), 0);
        let mut d = open(&s);
        settle(&mut d, |d| !d.models.is_empty());
        assert!(d.models.contains(&"gemma-12b".to_string()));
    }

    #[test]
    fn the_native_provider_switches_the_path_and_is_saved() {
        let s = setup();
        let mut d = open(&s);
        d.model = "gemma".into();
        d.json_mode = true;
        let now = Instant::now();
        d.set_provider(1, now);
        assert_eq!(d.endpoint, "http://localhost:1234/api/v1");
        assert!(!d.json_mode, "the native API has no JSON mode");
        d.save(&s.world).unwrap();
        let stored = s.store.load(&s.world).unwrap();
        assert_eq!(stored.provider, LMSTUDIO_NATIVE);
        assert!(!stored.json_mode);
        let mut reopened = open(&s);
        assert_eq!(reopened.provider, 1);
        reopened.set_provider(0, now);
        assert_eq!(reopened.endpoint, "http://localhost:1234/v1");
        reopened.set_endpoint("http://localhost:4567/custom/prefix", now);
        reopened.set_provider(1, now);
        assert_eq!(reopened.endpoint, "http://localhost:4567/custom/prefix/api/v1");
        assert_eq!(compose_endpoint("192.168.1.20:1234", 0), "http://192.168.1.20:1234/v1");
        assert_eq!(
            compose_endpoint("https://models.example.net/", 1),
            "https://models.example.net/api/v1"
        );
        assert_eq!(without_api_path("http://localhost:1234/V1"), "http://localhost:1234");
        assert_eq!(compose_endpoint("  ", 0), "");
    }

    #[test]
    fn forgetting_the_key_keeps_the_saved_key_out_of_lookups() {
        let s = setup();
        let mut d = open(&s);
        let now = Instant::now();
        d.set_api_key("new-secret", now);
        d.set_forget_key(true, now);
        assert!(d.api_key.is_empty());
        d.discover();
        settle(&mut d, |d| !d.discovering);
        assert_eq!(*s.models.key.lock().unwrap(), Some(None));
        assert_eq!(s.vault.reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn closing_drops_a_pending_lookup_and_the_typed_key() {
        let s = setup();
        let (release, hold) = mpsc::channel();
        *s.models.hold.lock().unwrap() = Some(hold);
        *s.models.answer.lock().unwrap() = Some(Ok(vec!["stale-model".into()]));
        let mut d = open(&s);
        d.set_api_key("secret", Instant::now());
        d.discover();
        d.close();
        release.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        d.poll(Instant::now());
        assert!(d.models.is_empty());
        assert!(d.api_key.is_empty());
    }

    #[test]
    fn saving_passes_the_key_to_the_vault_and_clears_the_draft_key() {
        let s = setup();
        let mut d = open(&s);
        d.model = "gemma-12b".into();
        d.set_api_key("new-secret", Instant::now());
        let saved = d.save(&s.world).unwrap().unwrap();
        assert!(d.error.is_none());
        assert_eq!(
            credentials::read(s.vault.as_ref(), &saved).unwrap().as_deref(),
            Some("new-secret")
        );
        assert_eq!(s.store.load(&s.world).unwrap().model, "gemma-12b");
        assert!(d.api_key.is_empty());
        assert!(!d.has_unsaved_changes());
        assert_eq!(s.vault.inner.len(), 1, "the replaced key was removed");
    }

    #[test]
    fn a_lookup_never_sends_the_saved_key_to_an_edited_address() {
        let s = setup();
        let mut d = open(&s);
        d.discover();
        settle(&mut d, |d| !d.discovering);
        assert_eq!(*s.models.key.lock().unwrap(), Some(Some("saved-secret".into())));
        d.set_endpoint("http://localhost:4567/v1", Instant::now());
        d.discover();
        settle(&mut d, |d| !d.discovering);
        assert_eq!(*s.models.key.lock().unwrap(), Some(None));
        assert_eq!(s.vault.reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_address_change_drops_an_older_lookup_even_if_it_answers() {
        let s = setup();
        let (release, hold) = mpsc::channel();
        *s.models.hold.lock().unwrap() = Some(hold);
        *s.models.answer.lock().unwrap() = Some(Ok(vec!["old-model".into()]));
        let mut d = open(&s);
        d.discover();
        d.set_endpoint("http://localhost:4567/v1", Instant::now());
        release.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        d.poll(Instant::now());
        assert!(d.models.is_empty());
    }

    #[test]
    fn an_invalid_catalog_is_not_saved() {
        let s = setup();
        let mut d = open(&s);
        d.model = "gemma-12b".into();
        d.commands = "look | look;quit | Invalid".into();
        assert!(d.validate().is_err());
        let before = s.store.load(&s.world).unwrap();
        assert!(d.save(&s.world).is_err());
        assert!(d.error.is_some());
        assert_eq!(s.store.load(&s.world).unwrap(), before);
    }

    #[test]
    fn an_invalid_address_is_explained() {
        let s = setup();
        let mut d = open(&s);
        d.set_server_address("http://user:pw@host:1", Instant::now());
        d.discover();
        assert_eq!(d.error.as_deref(), Some(t(S::AgentServerAddressInvalid)));
        assert_eq!(s.models.calls.load(Ordering::SeqCst), 0);
    }
}
