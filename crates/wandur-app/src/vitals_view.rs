//! The vitals strip under the transcript (the C# `ResourceBarsView`): one card per observed
//! current and maximum pair of the world's protocol mapping, the character's first, then the
//! opponent's, a vehicle's and the world's, each with its label, "current / maximum" and a bar.
//! Nothing is guessed: a value without a positive maximum has no card, and the strip hides when
//! there is no card or the session is not connected. Scripts add gauges after the mapped cards
//! with `mud.panel(id, { dock: "bars" })` ([`script_cards`]); their captions may carry colour codes.

use egui::{Color32, Rect, RichText, Ui};
use wandur_core::l10n::{S, t};
use wandur_core::protocol::binding::{BindingEngine, Entity, EntityState};

use crate::theme::Theme;

/// One card.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    /// `character:resource:health`.
    pub id: String,
    /// `Health`, or `Opponent · Health`.
    pub label: String,
    pub key: String,
    pub current: f64,
    pub maximum: f64,
    /// 0 to 100.
    pub percentage: f64,
    /// A script gauge's caption as declared (with colour codes), drawn coloured.
    pub coded: Option<String>,
    /// A script gauge at or below its warn share (drawn in the health colour).
    pub warned: bool,
}

impl Card {
    /// `296 / 380`.
    pub fn values(&self) -> String {
        format!("{} / {}", number(self.current), number(self.maximum))
    }

    /// The tooltip and accessible text: `Health: 296 / 380`.
    pub fn tip(&self) -> String {
        format!("{}: {}", self.label, self.values())
    }
}

/// A value with at most two decimals and no trailing zeros (the C# "0.##").
pub fn number(value: f64) -> String {
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" { "0".into() } else { text.into() }
}

fn priority(key: &str) -> u8 {
    match key {
        "health" => 0,
        "mana" | "spell_points" => 1,
        "movement" | "endurance" => 2,
        "energy" | "psionic_points" => 3,
        "shield" => 4,
        "hull" => 5,
        _ => 10,
    }
}

fn label(key: &str, fallback: &str) -> String {
    let known = match key {
        "health" => S::VitalsHealth,
        "mana" => S::VitalsMana,
        "movement" => S::VitalsMovement,
        "energy" => S::VitalsEnergy,
        "ammunition" => S::VitalsAmmunition,
        "shield" => S::VitalsShield,
        "hull" => S::VitalsHull,
        "fuel" => S::VitalsFuel,
        "experience" | "xp" => S::VitalsExperience,
        _ => return fallback.to_string(),
    };
    t(known).to_string()
}

/// The bar colour of a key (the C# `ResourceBar.ColorFor`); `None` follows the theme's accent.
pub fn color_for(key: &str) -> Option<Color32> {
    let hex = match key {
        "health" => 0xCE6474,
        "mana" | "spell_points" => 0x6399D1,
        "movement" | "endurance" | "energy" | "fuel" => 0xBD9B54,
        "psionic_points" => 0xAB7AC9,
        "shield" => 0x63B5BA,
        "hull" => 0x95A4BE,
        "experience" | "xp" => 0x6CAD8E,
        _ => return None,
    };
    Some(Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8))
}

/// The cards for a game state, in strip order. Empty when not connected.
pub fn cards(engine: Option<&BindingEngine>, connected: bool) -> Vec<Card> {
    let mut items = Vec::new();
    let Some(engine) = engine.filter(|_| connected) else {
        return items;
    };
    for (id, entity, prefix) in [
        ("character", Entity::Character, None),
        ("opponent", Entity::Opponent, Some(S::VitalsOpponent)),
        ("vehicle", Entity::Vehicle, Some(S::VitalsVehicle)),
        ("world", Entity::World, Some(S::VitalsWorld)),
    ] {
        add(&mut items, id, engine.entity(entity), prefix);
    }
    items
}

fn add(items: &mut Vec<Card>, id: &str, entity: &EntityState, prefix: Option<S>) {
    for (category, resources) in [("resource", &entity.resources), ("progression", &entity.progression)] {
        let mut keys: Vec<&String> = resources.keys().collect();
        keys.sort_by(|a, b| priority(a).cmp(&priority(b)).then_with(|| a.cmp(b)));
        for key in keys {
            let value = &resources[key];
            let (Some(current), Some(maximum), Some(percentage)) = (&value.current, &value.maximum, value.percentage())
            else {
                continue;
            };
            let (current, maximum) = (current.value, maximum.value);
            if current < 0.0 || maximum <= 0.0 || !current.is_finite() || !maximum.is_finite() {
                continue;
            }
            let name = label(key, &value.label);
            items.push(Card {
                coded: None,
                warned: false,
                id: format!("{id}:{category}:{key}"),
                label: match prefix {
                    Some(p) => format!("{} · {name}", t(p)),
                    None => name,
                },
                key: key.clone(),
                current,
                maximum,
                percentage: percentage.clamp(0.0, 100.0),
            });
        }
    }
}

/// The gauges of visible bars panels, in declaration order, after the mapped cards (C#
/// `ResourceBarsView.ScriptCards`). Labels declared on a bars panel are not drawn.
pub fn script_cards(host: &wandur_core::scripting::panels::PanelHost) -> Vec<Card> {
    host.bar_gauges()
        .map(|(panel, widget)| {
            let props = &widget.props;
            let raw = props.label.clone().unwrap_or_else(|| widget.id.clone());
            Card {
                id: format!("script:{}:{}:{}", panel.script, panel.id, widget.id),
                label: wandur_core::mudcolor::strip(&raw),
                key: String::new(),
                current: props.number,
                maximum: props.maximum,
                percentage: props.percentage(),
                coded: wandur_core::mudcolor::has_codes(&raw).then_some(raw),
                warned: props.warned(),
            }
        })
        .collect()
}

/// Height of one card.
pub const CARD_HEIGHT: f32 = 38.0;
/// Rows shown at most (the C# strip caps its height at 144).
const MAX_ROWS: usize = 3;

/// Columns for a width (one per 220 points, one to four).
pub fn columns(width: f32) -> usize {
    ((width.max(0.0) / 220.0) as usize).clamp(1, 4)
}

/// The strip's height for these cards at this width (0 when there are none).
pub fn height(cards: &[Card], width: f32) -> f32 {
    if cards.is_empty() {
        return 0.0;
    }
    let rows = cards.len().div_ceil(columns(width)).min(MAX_ROWS);
    rows as f32 * CARD_HEIGHT + 9.0
}

/// Draw the strip into the full width; it takes [`height`].
pub fn show(ui: &mut Ui, cards: &[Card], theme: &Theme) {
    let width = ui.available_width();
    let h = height(cards, width);
    if h == 0.0 {
        return;
    }
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, h), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme.shell);
    painter.hline(rect.x_range(), rect.top() + 0.5, egui::Stroke::new(1.0, theme.border));
    let cols = columns(width);
    let cell_w = width / cols as f32;
    let top = rect.top() + 5.0;
    for (i, card) in cards.iter().take(cols * MAX_ROWS).enumerate() {
        let (row, col) = (i / cols, i % cols);
        let cell = Rect::from_min_size(
            egui::pos2(rect.left() + col as f32 * cell_w, top + row as f32 * CARD_HEIGHT),
            egui::vec2(cell_w, CARD_HEIGHT),
        );
        card_ui(ui, cell, card, theme);
    }
}

fn card_ui(ui: &mut Ui, cell: Rect, card: &Card, theme: &Theme) {
    let label = match &card.coded {
        Some(raw) => crate::panel_view::mud_job(raw, egui::FontId::proportional(12.0), theme.text, theme),
        None => {
            egui::text::LayoutJob::simple_singleline(card.label.clone(), egui::FontId::proportional(12.0), theme.text)
        }
    };
    let color = if card.warned {
        color_for("health")
    } else {
        color_for(&card.key)
    };
    gauge_ui(
        ui,
        cell,
        &card.id,
        label,
        (&card.values(), egui::FontId::proportional(11.0)),
        card.percentage,
        color,
        theme,
        &card.tip(),
    );
}

/// One gauge card (the C# `ResourceBar`): the caption on the left, "current / maximum" on the
/// right in `values_font`, the bar under them; `color` `None` follows the theme's accent.
#[allow(clippy::too_many_arguments)]
pub fn gauge_ui(
    ui: &mut Ui,
    cell: Rect,
    id: &str,
    mut label: egui::text::LayoutJob,
    (values, values_font): (&str, egui::FontId),
    percentage: f64,
    color: Option<Color32>,
    theme: &Theme,
    tip: &str,
) {
    let response = ui.interact(cell, ui.id().with(("vital", id)), egui::Sense::hover());
    let painter = ui.painter_at(cell);
    painter.rect_filled(cell, 0.0, theme.panel);
    let inner = cell.shrink2(egui::vec2(10.0, 6.0));
    let values = painter.layout_no_wrap(values.to_string(), values_font, theme.muted);
    painter.galley(
        egui::pos2(inner.right() - values.size().x, inner.top() + 1.0),
        values.clone(),
        theme.muted,
    );
    let label_width = (inner.width() - values.size().x - 8.0).max(10.0);
    label.wrap = egui::text::TextWrapping::truncate_at_width(label_width);
    let galley = painter.layout_job(label);
    painter.galley(inner.left_top(), galley, theme.text);
    let bar = Rect::from_min_size(
        egui::pos2(inner.left(), inner.bottom() - 6.0),
        egui::vec2(inner.width(), 6.0),
    );
    painter.rect_filled(bar, 3.0, theme.border);
    let fill = Rect::from_min_size(
        bar.min,
        egui::vec2(bar.width() * (percentage / 100.0) as f32, bar.height()),
    );
    if fill.width() > 0.0 {
        painter.rect_filled(fill, 3.0, color.unwrap_or(theme.accent));
    }
    response
        .on_hover_text(RichText::new(tip))
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ProgressIndicator, true, tip));
}

#[cfg(test)]
mod tests {
    use super::*;
    use wandur_core::protocol::binding::ResourceState;

    #[test]
    fn numbers_read_as_the_csharp_strip_writes_them() {
        assert_eq!(number(296.0), "296");
        assert_eq!(number(12.5), "12.5");
        assert_eq!(number(1.0 / 3.0), "0.33");
        assert_eq!(number(0.0), "0");
    }

    /// Port of `OnlyValidPairsRenderAndDisconnectOrEmptyStateHidesTheStrip`, on the card list.
    #[test]
    fn only_valid_pairs_render_and_disconnect_or_empty_state_hides_the_strip() {
        let mut state = EntityState::default();
        state
            .resources
            .insert("health".into(), ResourceState::new("Health", Some(42.0), Some(100.0)));
        state
            .resources
            .insert("fuel".into(), ResourceState::new("Fuel", Some(7.0), None));
        state
            .resources
            .insert("shield".into(), ResourceState::new("Shield", Some(0.0), Some(0.0)));
        state
            .resources
            .insert("bad".into(), ResourceState::new("Bad", Some(f64::NAN), Some(100.0)));
        state
            .progression
            .insert("level".into(), ResourceState::new("Level", Some(20.0), None));
        state.progression.insert(
            "experience".into(),
            ResourceState::new("Experience", Some(250.0), Some(1000.0)),
        );
        let mut vehicle = EntityState::default();
        vehicle
            .resources
            .insert("hull".into(), ResourceState::new("Hull", Some(120.0), Some(100.0)));
        let mut items = Vec::new();
        add(&mut items, "character", &state, None);
        add(&mut items, "vehicle", &vehicle, Some(S::VitalsVehicle));
        assert_eq!(items.len(), 3);
        let pcts: Vec<f64> = items.iter().map(|c| c.percentage).collect();
        assert_eq!(pcts, [42.0, 25.0, 100.0]);
        assert_eq!(items[2].values(), "120 / 100");
        assert_eq!(items[2].label, "Vehicle · Hull");
        assert_eq!(items[0].tip(), "Health: 42 / 100");
        assert_eq!(cards(None, true), []);
        assert_eq!(height(&[], 800.0), 0.0);
    }

    #[test]
    fn cards_order_by_priority_and_the_strip_wraps_within_the_width() {
        let mut state = EntityState::default();
        for (key, c, m) in [
            ("movement", 189.0, 321.0),
            ("mana", 238.0, 326.0),
            ("health", 640.0, 640.0),
            ("ammo", 1.0, 2.0),
        ] {
            state
                .resources
                .insert(key.into(), ResourceState::new(key, Some(c), Some(m)));
        }
        let mut items = Vec::new();
        add(&mut items, "character", &state, None);
        let keys: Vec<&str> = items.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["health", "mana", "movement", "ammo"]);
        assert_eq!(items[3].label, "ammo");
        assert_eq!(columns(360.0), 1);
        assert_eq!(columns(960.0), 4);
        assert_eq!(columns(5000.0), 4);
        assert_eq!(height(&items, 960.0), CARD_HEIGHT + 9.0);
        assert!(height(&items, 360.0) <= 3.0 * CARD_HEIGHT + 9.0);
        assert_eq!(color_for("health"), Some(Color32::from_rgb(0xCE, 0x64, 0x74)));
        assert_eq!(color_for("ammo"), None);
    }

    /// C# `BarsGaugesFollowTheMappedVitalsAndLeaveWithTheirPanel`, on the card list: a bars
    /// panel's gauges become cards (labels do not), coded captions are kept for drawing, warn
    /// picks the health colour, hide and close take them away.
    #[test]
    fn bars_gauges_become_cards_and_leave_with_their_panel() {
        use std::time::Instant;
        use wandur_core::scripting::panels::{PanelAction, PanelHost};
        let mut host = PanelHost::default();
        let apply =
            |host: &mut PanelHost, json: &str| host.apply("s", PanelAction::parse(json).unwrap(), Instant::now());
        apply(
            &mut host,
            r#"{"panel":"ship","action":"create","title":"Ship","dock":"bars"}"#,
        );
        apply(
            &mut host,
            r#"{"panel":"ship","action":"widget","widget":"hull","kind":"gauge","props":{"label":"&CHull&D","value":25,"max":100,"warn":0.3}}"#,
        );
        apply(
            &mut host,
            r#"{"panel":"ship","action":"widget","widget":"note","kind":"label","props":{"text":"ignored"}}"#,
        );
        apply(
            &mut host,
            r#"{"panel":"side","action":"create","title":"Side","dock":"right"}"#,
        );
        apply(
            &mut host,
            r#"{"panel":"side","action":"widget","widget":"g","kind":"gauge","props":{"value":1}}"#,
        );
        let cards = script_cards(&host);
        assert_eq!(cards.len(), 1, "labels and rail panels are not in the strip");
        assert_eq!(cards[0].label, "Hull");
        assert_eq!(cards[0].coded.as_deref(), Some("&CHull&D"));
        assert_eq!(cards[0].values(), "25 / 100");
        assert_eq!(cards[0].tip(), "Hull: 25 / 100");
        assert!(cards[0].warned);
        apply(&mut host, r#"{"panel":"ship","action":"hide"}"#);
        assert!(script_cards(&host).is_empty());
        apply(&mut host, r#"{"panel":"ship","action":"show"}"#);
        assert_eq!(script_cards(&host).len(), 1);
        apply(&mut host, r#"{"panel":"ship","action":"close"}"#);
        assert!(script_cards(&host).is_empty());
    }
}
