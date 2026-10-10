//! The session footer's agent controls (the C# `SessionAutomationToolbar` agent part and
//! `AgentSessionView`): the Agent menu button, Play or Stop, the status, and the menu itself
//! with this connection's goal choice, Play and Stop, the status, Recent decisions, More
//! controls (Preview, Step, Clear memory) and Configure agent...

use egui::{RichText, Ui};
use wandur_core::agent::AgentRunMode;
use wandur_core::l10n::{S, t};

use crate::session_tab::SessionTab;
use crate::terminal_view::{TerminalViewState, ViewActions, footer_button};
use crate::theme::Theme;
use crate::widgets::{self, Icon};

/// Width of the menu's content (the C# flyout's 340).
pub const MENU_WIDTH: f32 = 340.0;
/// The least room the footer gives the agent status.
const STATUS_WIDTH: f32 = 120.0;
/// The agent status's widest room in the footer.
const STATUS_MAX_WIDTH: f32 = 280.0;
/// Room kept at the footer's right for the private input toggle.
const FOOTER_RIGHT: f32 = 130.0;

/// The footer's agent part: Agent ▾, Play or Stop, the status.
pub fn footer_controls(
    ui: &mut Ui,
    tab: &mut SessionTab,
    view: &mut TerminalViewState,
    theme: &Theme,
    actions: &mut ViewActions,
) {
    if !tab.agent.available() {
        return;
    }
    ui.add_space(6.0);
    let button = footer_button(ui, None, t(S::AgentMenu), view.agent_menu, true, theme).on_hover_text(t(S::AgentMenu));
    if button.clicked() {
        view.agent_menu = !view.agent_menu;
    }
    menu(&button, tab, view, theme, actions);
    let busy = tab.agent.is_busy();
    let (icon, enabled, hint) = if busy {
        (Icon::Stop, true, S::AgentStop)
    } else {
        (Icon::Play, tab.agent.can_start(), S::AgentPlay)
    };
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(24.0, 24.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    if enabled && response.hovered() {
        ui.painter().rect_filled(rect, 3.0, theme.hover_fill());
    }
    let color = if enabled { theme.text } else { theme.disabled_text() };
    let glyph = if busy { rect.shrink(4.0) } else { rect };
    widgets::paint_icon(ui, icon, glyph, color);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, t(hint)));
    let response = response.on_hover_text(t(hint));
    if response.clicked() {
        if busy {
            tab.agent_stop();
        } else {
            tab.agent_start(AgentRunMode::Run);
        }
    }
    let status = tab.agent.runner.status().label();
    // The whole status when the footer has room for it (the private input toggle at the right
    // keeps its place); an ellipsis only when it does not, with the status as a tooltip.
    let room = (ui.available_width() - FOOTER_RIGHT).clamp(STATUS_WIDTH, STATUS_MAX_WIDTH);
    let galley = widgets::clipped(ui, status, egui::FontId::proportional(11.0), theme.muted, room, 1);
    let elided = galley.elided;
    let (rect, response) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
    ui.painter().galley(rect.min, galley, theme.muted);
    if elided {
        response.on_hover_text(status);
    }
}

/// A compact button of the menu (the C# 28-point action buttons).
fn action(ui: &mut Ui, text: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).size(11.0)).min_size(egui::vec2(0.0, 28.0)),
    )
}

fn menu(
    button: &egui::Response,
    tab: &mut SessionTab,
    view: &mut TerminalViewState,
    theme: &Theme,
    actions: &mut ViewActions,
) {
    if !view.agent_menu {
        return;
    }
    let frame = egui::Frame::new()
        .shadow(egui::Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: egui::Color32::from_black_alpha(48),
        })
        .fill(theme.panel)
        .stroke(egui::Stroke::new(1.0, theme.border))
        .corner_radius(6)
        .inner_margin(egui::Margin::same(12));
    let mut open = true;
    egui::Popup::from_response(button)
        .id(egui::Id::new(("agent-menu", tab.id)))
        .open_bool(&mut open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::TOP_END)
        .align_alternatives(&[])
        .gap(4.0)
        .frame(frame)
        .show(|ui| {
            // The goal list and Recent decisions scroll on their own, so the menu stays within
            // the C# flyout's 580 points without a scroll of its own.
            ui.set_width(MENU_WIDTH);
            body(ui, tab, view, theme, actions);
        });
    crate::a11y::name_shown_popup(
        &button.ctx,
        egui::Id::new(("agent-menu", tab.id)),
        egui::accesskit::Role::Menu,
        t(S::AgentMenu),
    );
    if !open {
        view.agent_menu = false;
    }
}

fn body(ui: &mut Ui, tab: &mut SessionTab, view: &mut TerminalViewState, theme: &Theme, actions: &mut ViewActions) {
    ui.spacing_mut().item_spacing.y = 7.0;
    ui.label(RichText::new(t(S::AgentGoals)).size(13.0).color(theme.text));
    ui.add(egui::Label::new(RichText::new(t(S::AgentGoalsLiveHelp)).size(11.0).color(theme.muted)).wrap());
    let mut chosen = None;
    egui::ScrollArea::vertical()
        .id_salt("agent-menu-goals")
        .max_height(240.0)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            for (i, goal) in tab.agent.goals.iter().enumerate() {
                let name: String = goal.display_name().chars().take(200).collect();
                if ui.radio(goal.enabled, RichText::new(name).size(12.0)).clicked() {
                    chosen = Some(i);
                }
            }
        });
    if let Some(i) = chosen {
        tab.agent_select_goal(i);
    }
    if tab.agent.goals.is_empty() {
        ui.label(RichText::new(t(S::AgentGoalsEmpty)).size(12.0).color(theme.muted));
    }
    let can_start = tab.agent.can_start();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        if action(ui, t(S::AgentPlay), can_start).clicked() {
            tab.agent_start(AgentRunMode::Run);
        }
        if action(ui, t(S::AgentStop), true).clicked() {
            tab.agent_stop();
        }
    });
    ui.label(
        RichText::new(tab.agent.runner.status().label())
            .size(12.0)
            .color(theme.muted),
    );
    ui.add(egui::Label::new(RichText::new(t(S::AgentScriptsPaused)).size(11.0).color(theme.muted)).wrap());
    let width = ui.available_width();
    let activity = tab.agent.runner.activity_text();
    let mut open = view.agent_activity_open;
    widgets::expander(ui, &mut open, t(S::AgentActivity), width, theme, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("agent-menu-activity")
            .max_height(160.0)
            .show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(activity).size(11.0).color(theme.text)).wrap());
            });
    });
    view.agent_activity_open = open;
    let mut open = view.agent_more_open;
    let mut chosen = None;
    widgets::expander(ui, &mut open, t(S::AgentAdvanced), width, theme, |ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        if action(ui, t(S::AgentPreview), can_start).clicked() {
            chosen = Some(Some(AgentRunMode::Preview));
        }
        if action(ui, t(S::AgentStep), can_start).clicked() {
            chosen = Some(Some(AgentRunMode::Step));
        }
        if action(ui, t(S::AgentResetMemory), true).clicked() {
            chosen = Some(None);
        }
    });
    view.agent_more_open = open;
    match chosen {
        Some(Some(mode)) => tab.agent_start(mode),
        Some(None) => tab.agent_clear_memory(),
        None => {}
    }
    let configure = ui.add_enabled(
        tab.world.is_some(),
        egui::Button::new(RichText::new(t(S::AgentConfigure)).size(13.0)).min_size(egui::vec2(0.0, 32.0)),
    );
    if configure.clicked() {
        actions.edit_agent = true;
        view.agent_menu = false;
    }
}
