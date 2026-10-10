//! Script panels as data (the C# `ScriptPanelContracts` and `ScriptPanels`): the instructions a
//! script's `mud.panel(...)` produces, checked again before anything is drawn, and the panels of
//! one session they build. The UI draws [`PanelHost`]; nothing here knows about widgets on screen.
//!
//! - [`PanelAction::parse`] re-validates one instruction (JSON from the script thread). Malformed
//!   data is a script error ([`crate::l10n::S::ScriptPanelRejected`]).
//! - [`PanelHost`] keeps the session's panels by script: at most eight per script, 64 widgets per
//!   panel, in declaration order. A widget update with the same kind and properties changes
//!   nothing (no revision moves), so a script that re-sends unchanged values on every event costs
//!   the view no new layout.
//! - `focus` is honoured at most once a second per panel, and never for a bars panel (nothing to
//!   bring to the front in the vitals strip).

use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use super::js_length;
use super::limits;
use crate::mudcolor;

/// A panel takes one focus request per this interval; later ones are dropped.
pub const FOCUS_INTERVAL: Duration = Duration::from_secs(1);
/// Most characters of any one text property.
pub const MAX_STRING: usize = limits::PROPERTY_STRING_CHARACTERS;

/// Where a panel lives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Dock {
    Left,
    #[default]
    Right,
    /// The panel's gauges join the vitals strip under the transcript.
    Bars,
}

impl Dock {
    pub fn as_str(self) -> &'static str {
        match self {
            Dock::Left => "left",
            Dock::Right => "right",
            Dock::Bars => "bars",
        }
    }

    fn parse(text: &str) -> Option<Dock> {
        match text {
            "left" => Some(Dock::Left),
            "right" => Some(Dock::Right),
            "bars" => Some(Dock::Bars),
            _ => None,
        }
    }
}

/// The widget kinds (`docs/scripting-reference.json`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WidgetKind {
    Gauge,
    Label,
    Text,
    List,
    Table,
    Button,
    Toggle,
    Input,
    Separator,
    Group,
}

impl WidgetKind {
    pub const ALL: [WidgetKind; 10] = [
        WidgetKind::Gauge,
        WidgetKind::Label,
        WidgetKind::Text,
        WidgetKind::List,
        WidgetKind::Table,
        WidgetKind::Button,
        WidgetKind::Toggle,
        WidgetKind::Input,
        WidgetKind::Separator,
        WidgetKind::Group,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            WidgetKind::Gauge => "gauge",
            WidgetKind::Label => "label",
            WidgetKind::Text => "text",
            WidgetKind::List => "list",
            WidgetKind::Table => "table",
            WidgetKind::Button => "button",
            WidgetKind::Toggle => "toggle",
            WidgetKind::Input => "input",
            WidgetKind::Separator => "separator",
            WidgetKind::Group => "group",
        }
    }

    fn parse(text: &str) -> Option<WidgetKind> {
        Self::ALL.into_iter().find(|k| k.as_str() == text)
    }

    /// The kinds a bars panel draws in the vitals strip (the script engine refuses the others;
    /// labels are accepted there and not drawn, as in C#).
    pub fn allowed_on_bars(self) -> bool {
        matches!(self, WidgetKind::Gauge | WidgetKind::Label)
    }
}

/// What a script declared for a widget. Unset members keep their defaults; the view reads only
/// the members its kind uses.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetProps {
    pub label: Option<String>,
    pub title: Option<String>,
    pub text: Option<String>,
    pub placeholder: Option<String>,
    pub value: Option<String>,
    pub number: f64,
    pub maximum: f64,
    pub warn: Option<f64>,
    pub on: bool,
    pub items: Vec<String>,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub children: Vec<String>,
}

impl Default for WidgetProps {
    fn default() -> Self {
        Self {
            label: None,
            title: None,
            text: None,
            placeholder: None,
            value: None,
            number: 0.0,
            maximum: 100.0,
            warn: None,
            on: false,
            items: Vec::new(),
            columns: Vec::new(),
            rows: Vec::new(),
            children: Vec::new(),
        }
    }
}

impl WidgetProps {
    /// A gauge's fill, 0 to 100.
    pub fn percentage(&self) -> f64 {
        if self.maximum > 0.0 {
            (self.number / self.maximum * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        }
    }

    /// At or below its warn share a gauge takes the health colour.
    pub fn warned(&self) -> bool {
        self.warn
            .is_some_and(|warn| self.maximum > 0.0 && self.number / self.maximum <= warn)
    }
}

/// What one instruction does.
#[derive(Clone, Debug, PartialEq)]
pub enum Instruction {
    Create {
        title: String,
        dock: Dock,
    },
    Widget {
        id: String,
        kind: WidgetKind,
        props: Box<WidgetProps>,
    },
    Remove {
        widget: String,
    },
    Show,
    Hide,
    Focus,
    Close,
}

/// One validated panel instruction.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelAction {
    pub panel: String,
    pub instruction: Instruction,
}

/// Why an instruction was refused (English, as the C# `FormatException` messages; the UI wraps
/// it in [`crate::l10n::S::ScriptPanelRejected`]).
pub type Rejected = String;

fn depth(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        Value::Object(map) => 1 + map.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// A panel or widget id: a letter or digit, then up to 63 of letters, digits, `_`, `.` and `-`.
pub fn is_identifier(text: &str) -> bool {
    let bytes = text.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

fn text(root: &Map<String, Value>, member: &str, limit: usize) -> Result<String, Rejected> {
    match root.get(member) {
        Some(Value::String(s)) if js_length(s) <= limit => Ok(s.clone()),
        Some(Value::String(_)) => Err(format!("{member} is too long.")),
        _ => Err(format!("{member} must be a string.")),
    }
}

fn name(root: &Map<String, Value>, member: &str) -> Result<String, Rejected> {
    let value = text(root, member, 64)?;
    if is_identifier(&value) {
        Ok(value)
    } else {
        Err("A panel or widget id is not valid.".into())
    }
}

fn string(value: &Value, name: &str) -> Result<String, Rejected> {
    match value {
        Value::String(s) if js_length(s) <= MAX_STRING => Ok(s.clone()),
        Value::String(_) => Err(format!("{name} exceeds 4096 characters.")),
        _ => Err(format!("{name} must contain strings.")),
    }
}

fn optional(props: &Map<String, Value>, member: &str) -> Result<Option<String>, Rejected> {
    props.get(member).map(|v| string(v, member)).transpose()
}

fn strings(props: &Map<String, Value>, member: &str, limit: usize) -> Result<Vec<String>, Rejected> {
    match props.get(member) {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) if items.len() <= limit => items.iter().map(|v| string(v, member)).collect(),
        Some(Value::Array(_)) => Err(format!("{member} exceeds its item limit.")),
        Some(_) => Err(format!("{member} must be an array.")),
    }
}

fn number(props: &Map<String, Value>, member: &str) -> Result<f64, Rejected> {
    match props.get(member) {
        None => Ok(if member == "max" { 100.0 } else { 0.0 }),
        Some(Value::Number(n)) => n
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("{member} must be a finite number.")),
        Some(_) => Err(format!("{member} must be a finite number.")),
    }
}

fn flag(props: &Map<String, Value>, member: &str) -> Result<bool, Rejected> {
    match props.get(member) {
        None => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(format!("{member} must be true or false.")),
    }
}

fn rows(props: &Map<String, Value>) -> Result<Vec<Vec<String>>, Rejected> {
    let Some(rows) = props.get("rows") else {
        return Ok(Vec::new());
    };
    let Value::Array(rows) = rows else {
        return Err("rows must be an array.".into());
    };
    if rows.len() > limits::TABLE_ROWS {
        return Err("rows exceeds 500 items.".into());
    }
    rows.iter()
        .map(|row| match row {
            Value::Array(cells) if cells.len() <= limits::TABLE_COLUMNS => {
                cells.iter().map(|c| string(c, "a cell")).collect()
            }
            Value::Array(_) => Err("A table row exceeds 32 cells.".into()),
            _ => Err("Each table row must be an array.".into()),
        })
        .collect()
}

fn properties(root: &Map<String, Value>, kind: WidgetKind) -> Result<WidgetProps, Rejected> {
    let Some(props) = root.get("props") else {
        return Ok(WidgetProps::default());
    };
    let Value::Object(props) = props else {
        return Err("Widget properties must be an object.".into());
    };
    let mut out = WidgetProps::default();
    match kind {
        WidgetKind::Gauge => {
            out.label = optional(props, "label")?;
            out.number = number(props, "value")?;
            out.maximum = number(props, "max")?;
            if props.contains_key("warn") {
                out.warn = Some(number(props, "warn")?);
            }
        }
        WidgetKind::Label | WidgetKind::Text => out.text = Some(optional(props, "text")?.unwrap_or_default()),
        WidgetKind::List => {
            out.title = optional(props, "title")?;
            out.items = strings(props, "items", limits::LIST_ITEMS)?;
        }
        WidgetKind::Table => {
            out.title = optional(props, "title")?;
            out.columns = strings(props, "columns", limits::TABLE_COLUMNS)?;
            out.rows = rows(props)?;
        }
        WidgetKind::Button => out.label = Some(optional(props, "label")?.unwrap_or_default()),
        WidgetKind::Toggle => {
            out.label = Some(optional(props, "label")?.unwrap_or_default());
            out.on = flag(props, "value")?;
        }
        WidgetKind::Input => {
            out.placeholder = optional(props, "placeholder")?;
            out.value = optional(props, "value")?;
        }
        WidgetKind::Group => {
            out.title = optional(props, "title")?;
            out.children = strings(props, "children", limits::GROUP_CHILDREN)?;
        }
        WidgetKind::Separator => {}
    }
    Ok(out)
}

impl PanelAction {
    /// Re-validate one instruction before anything is drawn.
    pub fn parse(json: &str) -> Result<PanelAction, Rejected> {
        if js_length(json) > limits::PANEL_ACTION_CHARACTERS {
            return Err("A panel action exceeds 65536 characters.".into());
        }
        let value: Value = serde_json::from_str(json).map_err(|_| "A panel action is not valid JSON.".to_string())?;
        if depth(&value) > 8 {
            return Err("A panel action is not valid JSON.".into());
        }
        let Value::Object(root) = value else {
            return Err("A panel action must be an object.".into());
        };
        let panel = name(&root, "panel")?;
        let action = text(&root, "action", 16)?;
        let instruction = match action.as_str() {
            "create" => {
                let dock = Dock::parse(&text(&root, "dock", 16)?)
                    .ok_or_else(|| "A panel docks to left, right or bars.".to_string())?;
                Instruction::Create {
                    title: text(&root, "title", MAX_STRING)?,
                    dock,
                }
            }
            "widget" => {
                let id = name(&root, "widget")?;
                let kind =
                    WidgetKind::parse(&text(&root, "kind", 16)?).ok_or_else(|| "Unknown widget kind.".to_string())?;
                Instruction::Widget {
                    id,
                    kind,
                    props: Box::new(properties(&root, kind)?),
                }
            }
            "remove" => Instruction::Remove {
                widget: name(&root, "widget")?,
            },
            "show" => Instruction::Show,
            "hide" => Instruction::Hide,
            "focus" => Instruction::Focus,
            "close" => Instruction::Close,
            _ => return Err("Unknown panel action.".into()),
        };
        Ok(PanelAction { panel, instruction })
    }
}

/// A widget callback's name (`docs/scripting-reference.json`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelEvent {
    Click,
    Change(bool),
    Submit,
    Select,
}

/// The callback message the client sends back to the script: `{"panel", "widget", "event",
/// "value"}`, the value a flag, a string or null.
pub fn event_json(panel: &str, widget: &str, event: PanelEvent, text: Option<&str>) -> String {
    let (name, value) = match event {
        PanelEvent::Click => ("click", Value::Null),
        PanelEvent::Change(on) => ("change", Value::Bool(on)),
        PanelEvent::Submit => ("submit", text.map_or(Value::Null, |t| Value::String(t.into()))),
        PanelEvent::Select => ("select", text.map_or(Value::Null, |t| Value::String(t.into()))),
    };
    serde_json::json!({"panel": panel, "widget": widget, "event": name, "value": value}).to_string()
}

/// A gauge's caption: `7 / 10` (at most two decimals, no trailing zeros, as C# "0.##").
pub fn measure(value: f64, maximum: f64) -> String {
    format!("{} / {}", short_number(value), short_number(maximum))
}

/// A number with at most two decimals and no trailing zeros.
pub fn short_number(value: f64) -> String {
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" { "0".into() } else { text.into() }
}

/// One widget a script declared.
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub id: String,
    pub kind: WidgetKind,
    pub props: WidgetProps,
    /// Moves when the kind or a property changed.
    pub revision: u64,
}

/// A panel one script declared.
#[derive(Clone, Debug)]
pub struct Panel {
    /// The script that declared it.
    pub script: String,
    pub id: String,
    /// The title without colour codes (rail headers are plain text); the panel id when empty.
    pub title: String,
    pub dock: Dock,
    pub visible: bool,
    /// A focus request waits for the rail to open the panel.
    pub focus_requested: bool,
    focused_at: Option<Instant>,
    pub widgets: Vec<Widget>,
    /// Moves when the title, dock, visibility or the set of widgets changed (not for a widget
    /// property: that moves the widget's revision).
    pub revision: u64,
}

impl Panel {
    pub fn is_bars(&self) -> bool {
        self.dock == Dock::Bars
    }

    pub fn widget(&self, id: &str) -> Option<&Widget> {
        self.widgets.iter().find(|w| w.id == id)
    }

    /// Consume the pending focus request (the rail opened the panel).
    pub fn take_focus_request(&mut self) -> bool {
        std::mem::take(&mut self.focus_requested)
    }
}

/// The panels a session's scripts declare. Panels belong to one session and go with the script
/// that created them.
#[derive(Clone, Debug, Default)]
pub struct PanelHost {
    pub panels: Vec<Panel>,
    /// Moves on every change to any panel or widget.
    pub revision: u64,
    next_revision: u64,
}

impl PanelHost {
    fn bump(&mut self) -> u64 {
        self.next_revision += 1;
        self.revision = self.next_revision;
        self.next_revision
    }

    pub fn is_empty(&self) -> bool {
        self.panels.is_empty()
    }

    /// The panel `script` declared as `id`.
    pub fn panel(&self, script: &str, id: &str) -> Option<&Panel> {
        self.panels.iter().find(|p| p.script == script && p.id == id)
    }

    pub fn panel_mut(&mut self, script: &str, id: &str) -> Option<&mut Panel> {
        self.panels.iter_mut().find(|p| p.script == script && p.id == id)
    }

    fn title(action: &PanelAction, title: &str) -> String {
        if title.is_empty() {
            action.panel.clone()
        } else {
            mudcolor::strip(title)
        }
    }

    /// Apply one instruction of `script`. Returns whether anything changed.
    pub fn apply(&mut self, script: &str, action: PanelAction, now: Instant) -> bool {
        let index = self
            .panels
            .iter()
            .position(|p| p.script == script && p.id == action.panel);
        if let Instruction::Close = action.instruction {
            if let Some(index) = index {
                self.panels.remove(index);
                self.bump();
                return true;
            }
            return false;
        }
        let Some(index) = index else {
            // Only a declaration creates a panel; a stray update for an unknown panel is ignored.
            let Instruction::Create { title, dock } = &action.instruction else {
                return false;
            };
            if self.panels.iter().filter(|p| p.script == script).count() >= limits::PANELS_PER_SCRIPT {
                return false;
            }
            let title = Self::title(&action, title);
            let dock = *dock;
            let revision = self.bump();
            self.panels.push(Panel {
                script: script.to_string(),
                id: action.panel,
                title,
                dock,
                visible: true,
                focus_requested: false,
                focused_at: None,
                widgets: Vec::new(),
                revision,
            });
            return true;
        };
        let changed = match action.instruction {
            Instruction::Create { ref title, dock } => {
                let title = Self::title(&action, title);
                let panel = &mut self.panels[index];
                if panel.title == title && panel.dock == dock {
                    false
                } else {
                    panel.title = title;
                    panel.dock = dock;
                    true
                }
            }
            Instruction::Widget { id, kind, props } => {
                let panel = &mut self.panels[index];
                match panel.widgets.iter().position(|w| w.id == id) {
                    Some(at) => {
                        let widget = &panel.widgets[at];
                        if widget.kind == kind && widget.props == *props {
                            return false;
                        }
                        let revision = self.bump();
                        let widget = &mut self.panels[index].widgets[at];
                        widget.kind = kind;
                        widget.props = *props;
                        widget.revision = revision;
                        return true;
                    }
                    None if panel.widgets.len() >= limits::WIDGETS_PER_PANEL => return false,
                    None => {
                        panel.widgets.push(Widget {
                            id,
                            kind,
                            props: *props,
                            revision: 0,
                        });
                        true
                    }
                }
            }
            Instruction::Remove { widget } => {
                let panel = &mut self.panels[index];
                let before = panel.widgets.len();
                panel.widgets.retain(|w| w.id != widget);
                panel.widgets.len() != before
            }
            Instruction::Show | Instruction::Hide => {
                let visible = action.instruction == Instruction::Show;
                let panel = &mut self.panels[index];
                if panel.visible == visible {
                    false
                } else {
                    panel.visible = visible;
                    true
                }
            }
            Instruction::Focus => {
                let panel = &mut self.panels[index];
                // Nothing to bring to the front in the strip. Elsewhere one request a second is
                // kept, so a script refreshing on every MSDP event cannot hold the tab hostage.
                if panel.is_bars()
                    || panel
                        .focused_at
                        .is_some_and(|at| now.saturating_duration_since(at) < FOCUS_INTERVAL)
                {
                    false
                } else {
                    panel.focused_at = Some(now);
                    panel.visible = true;
                    panel.focus_requested = true;
                    true
                }
            }
            Instruction::Close => unreachable!("handled above"),
        };
        if changed {
            let revision = self.bump();
            let panel = &mut self.panels[index];
            panel.revision = revision;
            for widget in &mut panel.widgets {
                if widget.revision == 0 {
                    widget.revision = revision;
                }
            }
        }
        changed
    }

    /// Retire one panel (the person closed it).
    pub fn close(&mut self, script: &str, id: &str) -> bool {
        let before = self.panels.len();
        self.panels.retain(|p| !(p.script == script && p.id == id));
        let changed = self.panels.len() != before;
        if changed {
            self.bump();
        }
        changed
    }

    /// Drop the panels of scripts for which `keep` is false (scripts that stopped).
    pub fn retain_scripts(&mut self, keep: impl Fn(&str) -> bool) -> bool {
        let before = self.panels.len();
        self.panels.retain(|p| keep(&p.script));
        let changed = self.panels.len() != before;
        if changed {
            self.bump();
        }
        changed
    }

    pub fn clear(&mut self) {
        if !self.panels.is_empty() {
            self.panels.clear();
            self.bump();
        }
    }

    /// The callback message for a widget event, when the panel and widget exist.
    pub fn event(
        &self,
        script: &str,
        panel: &str,
        widget: &str,
        event: PanelEvent,
        text: Option<&str>,
    ) -> Option<String> {
        let p = self.panel(script, panel)?;
        p.widget(widget)?;
        Some(event_json(panel, widget, event, text))
    }

    /// The panels the rail shows: visible, not bars, in declaration order.
    pub fn rail(&self) -> impl Iterator<Item = &Panel> {
        self.panels.iter().filter(|p| p.visible && !p.is_bars())
    }

    /// The gauges the vitals strip shows: the gauges of visible bars panels, in order.
    pub fn bar_gauges(&self) -> impl Iterator<Item = (&Panel, &Widget)> {
        self.panels
            .iter()
            .filter(|p| p.visible && p.is_bars())
            .flat_map(|p| p.widgets.iter().map(move |w| (p, w)))
            .filter(|(_, w)| w.kind == WidgetKind::Gauge)
    }

    /// Which children each group shows: a non-group widget named by a group of the same panel
    /// is drawn inside the first group that names it, not at the top level.
    pub fn claimed(panel: &Panel) -> std::collections::HashMap<&str, &str> {
        let mut claimed = std::collections::HashMap::new();
        for group in panel.widgets.iter().filter(|w| w.kind == WidgetKind::Group) {
            for child in &group.props.children {
                if child != &group.id
                    && panel.widget(child).is_some_and(|w| w.kind != WidgetKind::Group)
                    && !claimed.contains_key(child.as_str())
                {
                    claimed.insert(child.as_str(), group.id.as_str());
                }
            }
        }
        claimed
    }
}

#[cfg(test)]
mod tests;
