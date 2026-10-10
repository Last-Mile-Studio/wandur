//! Normalized game state from a world's protocol mapping (the C# `ProtocolBindingEngine` and the
//! SDK's `GameState`): only fields the mapping names, only validated values, one connection
//! lifetime. Missing data stays missing: a current value without a maximum gives no percentage.
//!
//! A world reports a variable once and then only when it changes, so the opponent's, vehicle's
//! and world's observations are never thrown away on a name change: while the opponent's name is
//! cleared it is presented empty and its observations keep updating, and the next name shows them
//! again (the second-fight fix). A new character name is a new lifetime and clears everything.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::Value;

use super::format::{Content, MAX_BODY_CHARS, parse_json};
use super::mapping::{FieldBinding, WorldMapping, is_valid};
use super::{OPT_GMCP, OPT_MSDP};

pub const MAX_MAGNITUDE: f64 = 1e15;
pub const MAX_TEXT: usize = 512;

/// A value and when it arrived.
#[derive(Clone, Debug, PartialEq)]
pub struct Observation<T> {
    pub value: T,
    pub received_at: SystemTime,
}

impl<T> Observation<T> {
    pub fn new(value: T, received_at: SystemTime) -> Self {
        Self { value, received_at }
    }

    /// Older than `age` at `now`.
    pub fn is_stale(&self, now: SystemTime, age: std::time::Duration) -> bool {
        now.duration_since(self.received_at).is_ok_and(|d| d > age)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResourceState {
    pub label: String,
    pub current: Option<Observation<f64>>,
    pub maximum: Option<Observation<f64>>,
}

impl ResourceState {
    pub fn new(label: &str, current: Option<f64>, maximum: Option<f64>) -> Self {
        let now = SystemTime::now();
        Self {
            label: label.into(),
            current: current.map(|v| Observation::new(v, now)),
            maximum: maximum.map(|v| Observation::new(v, now)),
        }
    }

    /// Current over maximum in percent, when both are known and the maximum is positive.
    pub fn percentage(&self) -> Option<f64> {
        let (current, maximum) = (self.current.as_ref()?.value, self.maximum.as_ref()?.value);
        let p = current / maximum * 100.0;
        (maximum > 0.0 && p.is_finite()).then_some(p)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AttributeState {
    pub label: String,
    pub current: Option<Observation<f64>>,
    pub base: Option<Observation<f64>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CurrencyState {
    pub label: String,
    pub carried: Option<Observation<f64>>,
    pub bank: Option<Observation<f64>>,
    pub total: Option<Observation<f64>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MetricState {
    pub label: String,
    pub text: Option<Observation<String>>,
    pub number: Option<Observation<f64>>,
    pub boolean: Option<Observation<bool>>,
}

/// What is known about the character, the opponent, a vehicle or the world.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntityState {
    pub identity: BTreeMap<String, Observation<String>>,
    pub resources: BTreeMap<String, ResourceState>,
    pub progression: BTreeMap<String, ResourceState>,
    pub attributes: BTreeMap<String, AttributeState>,
    pub currencies: BTreeMap<String, CurrencyState>,
    pub metrics: BTreeMap<String, MetricState>,
    pub location: BTreeMap<String, Observation<String>>,
}

impl EntityState {
    pub fn is_empty(&self) -> bool {
        *self == EntityState::default()
    }
}

/// The entities a mapping can target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entity {
    Character,
    Opponent,
    Vehicle,
    World,
}

impl Entity {
    fn of(name: &str) -> Entity {
        match name {
            "character" => Entity::Character,
            "opponent" => Entity::Opponent,
            "vehicle" => Entity::Vehicle,
            _ => Entity::World,
        }
    }

    fn slot(self) -> usize {
        match self {
            Entity::Opponent => 0,
            Entity::Vehicle => 1,
            _ => 2,
        }
    }
}

static EMPTY: std::sync::LazyLock<EntityState> = std::sync::LazyLock::new(EntityState::default);

/// Applies a validated mapping to received messages. Call [`BindingEngine::observe`] only for
/// messages that passed the session's privacy gate; it rejects login packages and redacted,
/// malformed or truncated content itself.
#[derive(Debug)]
pub struct BindingEngine {
    mapping: WorldMapping,
    bindings: Vec<FieldBinding>,
    character: EntityState,
    /// Opponent, vehicle and world: kept across name changes.
    retained: [EntityState; 3],
    /// Whether each of them is presented (its name is not cleared).
    shown: [bool; 3],
    revision: u64,
}

impl BindingEngine {
    /// `None` for a mapping that does not validate.
    pub fn new(mapping: WorldMapping) -> Option<Self> {
        if !is_valid(&mapping) {
            return None;
        }
        Some(Self {
            bindings: mapping.bindings.clone(),
            mapping,
            character: EntityState::default(),
            retained: Default::default(),
            shown: [true; 3],
            revision: 0,
        })
    }

    pub fn mapping(&self) -> &WorldMapping {
        &self.mapping
    }

    /// Bumped on every change, so a view can skip work when nothing moved.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The state as presented: the opponent, vehicle and world are empty while their name is
    /// cleared.
    pub fn entity(&self, entity: Entity) -> &EntityState {
        match entity {
            Entity::Character => &self.character,
            other if self.shown[other.slot()] => &self.retained[other.slot()],
            _ => &EMPTY,
        }
    }

    pub fn character(&self) -> &EntityState {
        self.entity(Entity::Character)
    }

    pub fn opponent(&self) -> &EntityState {
        self.entity(Entity::Opponent)
    }

    pub fn vehicle(&self) -> &EntityState {
        self.entity(Entity::Vehicle)
    }

    pub fn world(&self) -> &EntityState {
        self.entity(Entity::World)
    }

    fn entity_mut(&mut self, entity: Entity) -> &mut EntityState {
        match entity {
            Entity::Character => &mut self.character,
            other => &mut self.retained[other.slot()],
        }
    }

    /// Forget everything (a new connection or character).
    pub fn reset(&mut self) {
        self.character = EntityState::default();
        self.retained = Default::default();
        self.shown = [true; 3];
        self.revision += 1;
    }

    /// A refreshed mapping. Observations survive only an additive change with the same endpoint
    /// and identity bindings; newly mapped fields wait for fresh packets (nothing is replayed).
    /// Returns whether the bindings changed. `false` for an invalid mapping, which is ignored.
    pub fn update_mapping(&mut self, mapping: WorldMapping) -> bool {
        if !is_valid(&mapping) {
            return false;
        }
        use std::collections::HashSet;
        let old: HashSet<&FieldBinding> = self.bindings.iter().collect();
        let new: HashSet<&FieldBinding> = mapping.bindings.iter().collect();
        let same_endpoint = self.mapping.endpoint.matches(&mapping.endpoint);
        if same_endpoint && old == new {
            self.mapping = mapping;
            return false;
        }
        let identity = |set: &HashSet<&FieldBinding>| -> HashSet<FieldBinding> {
            set.iter()
                .filter(|b| b.target.category == "identity")
                .map(|b| (*b).clone())
                .collect()
        };
        let same_identity = identity(&old) == identity(&new);
        let additive = old.iter().all(|b| new.contains(b));
        if !same_endpoint || !same_identity || !additive {
            self.reset();
        }
        self.bindings = mapping.bindings.clone();
        self.mapping = mapping;
        true
    }

    /// Apply one received message. Returns whether anything was applied.
    pub fn observe(&mut self, option: u8, content: &Content, received_at: SystemTime) -> bool {
        if !matches!(option, OPT_MSDP | OPT_GMCP)
            || content.malformed
            || content.truncated
            || content.redacted
            || content.body.chars().count() > MAX_BODY_CHARS
            || crate::login::gmcp::is_private(&content.name)
        {
            return false;
        }
        let protocol = if option == OPT_MSDP { "MSDP" } else { "GMCP" };
        let package = if option == OPT_MSDP {
            "MSDP"
        } else {
            content.name.as_str()
        };
        let Some(root) = parse_json(&content.body) else {
            return false;
        };
        let mut updates: Vec<(usize, Option<&Value>)> = Vec::new();
        for (i, binding) in self.bindings.iter().enumerate() {
            if binding.source.protocol == protocol
                && binding.source.package == package
                && let Some(value) = resolve(&root, &binding.source.path)
            {
                updates.push((i, value));
            }
        }
        if updates.is_empty() {
            return false;
        }
        // Identity first, whatever the order of the mapping or the packet. A new character name
        // is a new lifetime; another entity's name only decides whether it is presented.
        let identity: Vec<(usize, Option<&Value>)> = updates
            .iter()
            .copied()
            .filter(|(i, _)| {
                let t = &self.bindings[*i].target;
                t.category == "identity" && matches!(t.key.as_str(), "name" | "id")
            })
            .collect();
        let new_character = identity.iter().any(|(i, value)| {
            let t = &self.bindings[*i].target;
            t.entity == "character"
                && self
                    .character
                    .identity
                    .get(&t.key)
                    .is_some_and(|previous| Some(previous.value.as_str()) != read_text(*value).as_deref())
        });
        if new_character {
            self.reset();
        }
        for (i, value) in &identity {
            let entity = Entity::of(&self.bindings[*i].target.entity);
            if entity != Entity::Character {
                self.shown[entity.slot()] = read_text(*value).is_some();
            }
        }
        for (i, value) in updates {
            let binding = self.bindings[i].clone();
            self.apply(&binding, value, received_at);
        }
        self.revision += 1;
        true
    }

    fn apply(&mut self, binding: &FieldBinding, value: Option<&Value>, now: SystemTime) {
        let target = &binding.target;
        let key = target.key.clone();
        let label = if binding.label.is_empty() {
            key.clone()
        } else {
            binding.label.clone()
        };
        let number = read_number(value, binding.scale).map(|n| Observation::new(n, now));
        let text = read_text(value).map(|t| Observation::new(t, now));
        let entity = self.entity_mut(Entity::of(&target.entity));
        match target.category.as_str() {
            "identity" | "location" => {
                let fields = if target.category == "identity" {
                    &mut entity.identity
                } else {
                    &mut entity.location
                };
                match text {
                    Some(text) => {
                        fields.insert(key, text);
                    }
                    None => {
                        fields.remove(&key);
                    }
                }
            }
            "resource" | "progression" => {
                let resources = if target.category == "resource" {
                    &mut entity.resources
                } else {
                    &mut entity.progression
                };
                let resource = resources.entry(key).or_insert_with(|| ResourceState {
                    label,
                    current: None,
                    maximum: None,
                });
                if target.member == "current" {
                    resource.current = number;
                } else {
                    resource.maximum = number;
                }
            }
            "attribute" => {
                let attribute = entity.attributes.entry(key).or_insert_with(|| AttributeState {
                    label,
                    current: None,
                    base: None,
                });
                if target.member == "current" {
                    attribute.current = number;
                } else {
                    attribute.base = number;
                }
            }
            "currency" => {
                let currency = entity.currencies.entry(key).or_insert_with(|| CurrencyState {
                    label,
                    carried: None,
                    bank: None,
                    total: None,
                });
                match target.member.as_str() {
                    "carried" => currency.carried = number,
                    "bank" => currency.bank = number,
                    _ => currency.total = number,
                }
            }
            "metric" => {
                let conversion = binding.conversion.as_str();
                entity.metrics.insert(
                    key,
                    MetricState {
                        label,
                        text: if conversion == "text" { text } else { None },
                        number: if conversion == "number" { number } else { None },
                        boolean: if conversion == "boolean" {
                            read_boolean(value).map(|b| Observation::new(b, now))
                        } else {
                            None
                        },
                    },
                );
            }
            _ => {}
        }
    }
}

/// Follow a JSON Pointer. `None`: the path is not in this message (nothing to update).
/// `Some(None)`: an ancestor exists but is not an object or array, which invalidates the leaf.
fn resolve<'a>(root: &'a Value, pointer: &str) -> Option<Option<&'a Value>> {
    let mut value = root;
    if pointer.is_empty() {
        return Some(Some(value));
    }
    for encoded in pointer[1..].split('/') {
        let segment = encoded.replace("~1", "/").replace("~0", "~");
        match value {
            Value::Object(map) => value = map.get(&segment)?,
            Value::Array(items) => {
                if segment.is_empty()
                    || (segment.len() > 1 && segment.starts_with('0'))
                    || !segment.bytes().all(|b| b.is_ascii_digit())
                {
                    return None;
                }
                let index: usize = segment.parse().ok()?;
                value = items.get(index)?;
            }
            _ => return Some(None),
        }
    }
    Some(Some(value))
}

fn read_number(value: Option<&Value>, scale: f64) -> Option<f64> {
    let number = match value? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) if (1..=64).contains(&s.encode_utf16().count()) => parse_float(s)?,
        _ => return None,
    } * scale;
    (number.is_finite() && number.abs() <= MAX_MAGNITUDE).then_some(number)
}

/// .NET `double.TryParse` with `NumberStyles.Float`: white space around, a sign, a decimal point
/// and an exponent.
fn parse_float(text: &str) -> Option<f64> {
    let t = text.trim();
    if t.is_empty()
        || !t
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'))
    {
        return None;
    }
    t.parse().ok()
}

fn read_text(value: Option<&Value>) -> Option<String> {
    let text = match value? {
        Value::String(s) => s.clone(),
        Value::Number(_) => format_number(read_number(value, 1.0)?),
        _ => return None,
    };
    ((1..=MAX_TEXT).contains(&text.encode_utf16().count()) && !text.chars().any(char::is_control)).then_some(text)
}

/// A number as .NET's "G" format writes it for ordinary values (42, 12.5).
fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

fn read_boolean(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => match n.as_i64() {
            Some(0) => Some(false),
            Some(1) => Some(true),
            _ => None,
        },
        Value::String(s) if s.chars().count() <= 5 => match s.to_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::format::format;
    use super::super::mapping::tests::{bind, hp, mapping};
    use super::*;
    use std::time::Duration;

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_789_000_000)
    }

    fn observe_at(engine: &mut BindingEngine, text: &str, at: SystemTime) {
        engine.observe(OPT_GMCP, &format(OPT_GMCP, text.as_bytes(), false, &[]), at);
    }

    fn observe(engine: &mut BindingEngine, text: &str) {
        observe_at(engine, text, now());
    }

    fn msdp(engine: &mut BindingEngine, variable: &str, value: &str) {
        let payload = format!("\x01{variable}\x02{value}");
        engine.observe(OPT_MSDP, &format(OPT_MSDP, payload.as_bytes(), false, &[]), now());
    }

    fn native(path: &str, category: &str, key: &str, member: &str, entity: &str, conversion: &str) -> FieldBinding {
        let mut b = bind(path, category, key, member, entity, conversion);
        b.source.protocol = "MSDP".into();
        b.source.package = "MSDP".into();
        b
    }

    fn health(engine: &BindingEngine) -> &ResourceState {
        &engine.character().resources["health"]
    }

    #[test]
    fn mapping_refresh_keeps_only_compatible_lifetime_state_and_does_not_replay_packets() {
        let max = bind("/max", "resource", "health", "maximum", "character", "number");
        let m = mapping(vec![hp("/hp")]);
        let mut engine = BindingEngine::new(m.clone()).unwrap();
        observe(&mut engine, r#"Char.Vitals {"hp":75,"max":100}"#);
        assert!(engine.update_mapping(mapping(vec![hp("/hp"), max.clone()])));
        assert_eq!(health(&engine).current.as_ref().unwrap().received_at, now());
        assert!(health(&engine).maximum.is_none());
        observe(&mut engine, r#"Char.Vitals {"max":100}"#);
        assert_eq!(health(&engine).percentage(), Some(75.0));
        // A changed source or a new identity cannot inherit observations.
        engine.update_mapping(mapping(vec![hp("/different")]));
        assert!(engine.character().resources.is_empty());
        observe(&mut engine, r#"Char.Vitals {"different":15}"#);
        engine.update_mapping(mapping(vec![
            hp("/different"),
            bind("/name", "identity", "name", "value", "character", "text"),
        ]));
        assert!(engine.character().resources.is_empty());
        observe(&mut engine, r#"Char.Vitals {"different":20,"name":"first"}"#);
        engine.reset();
        engine.update_mapping(m);
        assert!(engine.character().identity.is_empty());
        assert!(engine.character().resources.is_empty());
    }

    #[test]
    fn incremental_resources_retain_original_timestamp_and_missing_maximum() {
        let mut engine = BindingEngine::new(mapping(vec![
            hp("/hp"),
            bind("/max", "resource", "health", "maximum", "character", "number"),
        ]))
        .unwrap();
        observe(&mut engine, r#"Char.Vitals {"hp":75}"#);
        assert!(health(&engine).maximum.is_none());
        assert_eq!(health(&engine).percentage(), None);
        observe_at(
            &mut engine,
            r#"Char.Vitals {"max":100}"#,
            now() + Duration::from_secs(60),
        );
        let r = health(&engine);
        assert_eq!(r.percentage(), Some(75.0));
        assert_eq!(r.current.as_ref().unwrap().received_at, now());
        let later = now() + Duration::from_secs(120);
        assert!(r.current.as_ref().unwrap().is_stale(later, Duration::from_secs(90)));
        assert!(!r.maximum.as_ref().unwrap().is_stale(later, Duration::from_secs(90)));
    }

    #[test]
    fn invalid_numeric_updates_clear_the_previous_observation() {
        for value in ["null", "\"garbage\"", "\"NaN\"", "true", "{}"] {
            let mut engine = BindingEngine::new(mapping(vec![hp("/hp")])).unwrap();
            observe(&mut engine, r#"Char.Vitals {"hp":75}"#);
            observe(&mut engine, &format!(r#"Char.Vitals {{"hp":{value}}}"#));
            assert!(health(&engine).current.is_none(), "{value}");
        }
        // An out-of-range literal makes the whole message malformed here (serde_json refuses
        // it), so nothing is applied and the old value stays; C# clears it. A ledger ruling.
        let mut engine = BindingEngine::new(mapping(vec![hp("/hp")])).unwrap();
        observe(&mut engine, r#"Char.Vitals {"hp":75}"#);
        observe(&mut engine, r#"Char.Vitals {"hp":1e999}"#);
        assert_eq!(health(&engine).current.as_ref().unwrap().value, 75.0);
    }

    /// A new character name resets everything; another entity's name keeps its unrepeated
    /// observations, whatever the property order.
    #[test]
    fn identity_change_resets_the_character_and_keeps_another_entitys_unrepeated_observations() {
        for entity in ["character", "opponent", "vehicle"] {
            let mut engine = BindingEngine::new(mapping(vec![
                bind("/hp", "resource", "health", "current", entity, "number"),
                bind("/max", "resource", "health", "maximum", entity, "number"),
                bind("/name", "identity", "name", "value", entity, "text"),
                bind("/room", "location", "name", "value", "world", "text"),
            ]))
            .unwrap();
            observe(
                &mut engine,
                r#"Char.Vitals {"name":"first","hp":75,"max":100,"room":"old"}"#,
            );
            observe(&mut engine, r#"Char.Vitals {"hp":25,"name":"second"}"#);
            let which = match entity {
                "character" => Entity::Character,
                "opponent" => Entity::Opponent,
                _ => Entity::Vehicle,
            };
            let state = engine.entity(which);
            assert_eq!(state.resources["health"].current.as_ref().unwrap().value, 25.0);
            if entity == "character" {
                assert!(state.resources["health"].maximum.is_none());
                assert!(engine.world().location.is_empty());
            } else {
                assert_eq!(state.resources["health"].maximum.as_ref().unwrap().value, 100.0);
                assert_eq!(engine.world().location["name"].value, "old");
            }
            assert_eq!(engine.entity(which).identity["name"].value, "second");
            engine.reset();
            assert!(engine.character().identity.is_empty());
            assert!(engine.opponent().identity.is_empty());
            assert!(engine.vehicle().identity.is_empty());
        }
    }

    /// The second-fight report, one MSDP variable per packet.
    #[test]
    fn a_cleared_opponent_name_hides_the_opponent_until_the_next_name_and_keeps_what_the_world_does_not_repeat() {
        let bindings = vec![
            native("/OPPONENTHEALTH", "resource", "health", "current", "opponent", "number"),
            native(
                "/OPPONENTHEALTHMAX",
                "resource",
                "health",
                "maximum",
                "opponent",
                "number",
            ),
            native("/OPPONENTNAME", "identity", "name", "value", "opponent", "text"),
        ];
        let mut engine = BindingEngine::new(mapping(bindings.clone())).unwrap();
        let pct = |e: &BindingEngine| e.opponent().resources.get("health").and_then(ResourceState::percentage);
        msdp(&mut engine, "OPPONENTNAME", "A Vicious Womprat");
        msdp(&mut engine, "OPPONENTHEALTHMAX", "100");
        msdp(&mut engine, "OPPONENTHEALTH", "100");
        msdp(&mut engine, "OPPONENTHEALTH", "30");
        assert_eq!(pct(&engine), Some(30.0));
        // The fight ends: only the name is cleared.
        msdp(&mut engine, "OPPONENTNAME", "");
        assert!(engine.opponent().resources.is_empty() && engine.opponent().identity.is_empty());
        // The same opponent again: only its health is repeated.
        msdp(&mut engine, "OPPONENTHEALTH", "100");
        msdp(&mut engine, "OPPONENTNAME", "A Vicious Womprat");
        assert_eq!(pct(&engine), Some(100.0));
        assert_eq!(engine.opponent().identity["name"].value, "A Vicious Womprat");
        // A different opponent with the same maximum, health before name.
        msdp(&mut engine, "OPPONENTHEALTH", "80");
        msdp(&mut engine, "OPPONENTNAME", "A Stormtrooper");
        assert_eq!(pct(&engine), Some(80.0));
        assert_eq!(engine.opponent().identity["name"].value, "A Stormtrooper");
        // All three cleared, then a new fight with its maximum first.
        msdp(&mut engine, "OPPONENTHEALTH", "0");
        msdp(&mut engine, "OPPONENTHEALTHMAX", "0");
        msdp(&mut engine, "OPPONENTNAME", "");
        assert!(engine.opponent().resources.is_empty());
        msdp(&mut engine, "OPPONENTHEALTHMAX", "250");
        msdp(&mut engine, "OPPONENTHEALTH", "250");
        assert!(engine.opponent().resources.is_empty());
        msdp(&mut engine, "OPPONENTNAME", "A Wookiee");
        assert_eq!(pct(&engine), Some(100.0));
        assert_eq!(
            engine.opponent().resources["health"].maximum.as_ref().unwrap().value,
            250.0
        );
        // The same bindings in another order keep everything; a reset clears the retained state.
        let mut reordered = bindings;
        reordered.reverse();
        assert!(!engine.update_mapping(mapping(reordered)));
        assert_eq!(
            engine.opponent().resources["health"].maximum.as_ref().unwrap().value,
            250.0
        );
        engine.reset();
        msdp(&mut engine, "OPPONENTNAME", "A Wookiee");
        assert!(engine.opponent().resources.is_empty());
    }

    #[test]
    fn native_and_gmcp_msdp_never_merge_implicitly_and_pointers_are_exact() {
        let mut mana = bind("/h~1p/~0value/0", "resource", "mana", "current", "character", "number");
        mana.source.package = "MSDP".into();
        mana.scale = 2.0;
        let mut engine = BindingEngine::new(mapping(vec![
            native("/HEALTH", "resource", "health", "current", "character", "number"),
            mana,
        ]))
        .unwrap();
        observe(&mut engine, r#"MSDP {"HEALTH":999,"h/p":{"~value":["12.5"]}}"#);
        assert!(!engine.character().resources.contains_key("health"));
        assert_eq!(
            engine.character().resources["mana"].current.as_ref().unwrap().value,
            25.0
        );
        msdp(&mut engine, "HEALTH", "42");
        assert_eq!(health(&engine).current.as_ref().unwrap().value, 42.0);
        observe(&mut engine, r#"msdp {"h/p":{"~value":[100]}}"#);
        assert_eq!(
            engine.character().resources["mana"].current.as_ref().unwrap().value,
            25.0
        );
    }

    #[test]
    fn private_malformed_and_truncated_packets_cannot_update_state() {
        let mut engine = BindingEngine::new(mapping(vec![hp("/hp")])).unwrap();
        let mut redacted = Content::new("Char.Vitals", r#"{"hp":1}"#, false, false);
        redacted.redacted = true;
        for content in [
            Content::new("Char.Vitals", r#"{"hp":1}"#, true, false),
            Content::new("Char.Vitals", r#"{"hp":1}"#, false, true),
            redacted,
            format(
                OPT_GMCP,
                br#"Char.Vitals {"hp":"hidden"}"#,
                false,
                &["hidden".to_string()],
            ),
        ] {
            engine.observe(OPT_GMCP, &content, now());
        }
        assert!(engine.character().resources.is_empty());
    }

    #[test]
    fn wrong_shaped_ancestor_invalidates_nested_value_without_using_ancestor_as_number() {
        let mut engine = BindingEngine::new(mapping(vec![hp("/vitals/hp")])).unwrap();
        observe(&mut engine, r#"Char.Vitals {"vitals":{"hp":75}}"#);
        observe(&mut engine, r#"Char.Vitals {"vitals":4}"#);
        assert!(health(&engine).current.is_none());
    }

    #[test]
    fn capabilities_remain_optional_and_zero_and_false_are_real_observations() {
        let b = |path, category, key, member, conversion| bind(path, category, key, member, "character", conversion);
        let mut engine = BindingEngine::new(mapping(vec![
            b("/xp", "progression", "experience", "current", "number"),
            b("/next", "progression", "experience", "maximum", "number"),
            b("/str", "attribute", "strength", "current", "number"),
            b("/base", "attribute", "strength", "base", "number"),
            b("/gold", "currency", "gold", "carried", "number"),
            b("/bank", "currency", "gold", "bank", "number"),
            b("/total", "currency", "gold", "total", "number"),
            b("/level", "metric", "level", "value", "number"),
            b("/fighting", "metric", "fighting", "value", "boolean"),
            b("/position", "metric", "position", "value", "text"),
        ]))
        .unwrap();
        observe(
            &mut engine,
            r#"Char.Vitals {"xp":0,"next":100,"str":12,"base":10,"gold":0,"bank":15,"total":15,"level":2,"fighting":false,"position":"standing"}"#,
        );
        let s = engine.character();
        assert_eq!(s.progression["experience"].percentage(), Some(0.0));
        assert_eq!(s.attributes["strength"].current.as_ref().unwrap().value, 12.0);
        assert_eq!(s.attributes["strength"].base.as_ref().unwrap().value, 10.0);
        assert_eq!(s.currencies["gold"].carried.as_ref().unwrap().value, 0.0);
        assert_eq!(s.currencies["gold"].bank.as_ref().unwrap().value, 15.0);
        assert_eq!(s.currencies["gold"].total.as_ref().unwrap().value, 15.0);
        assert_eq!(s.metrics["level"].number.as_ref().unwrap().value, 2.0);
        assert!(!s.metrics["fighting"].boolean.as_ref().unwrap().value);
        assert_eq!(s.metrics["position"].text.as_ref().unwrap().value, "standing");
        assert!(s.resources.is_empty() && engine.vehicle().resources.is_empty());
        observe(
            &mut engine,
            r#"Char.Vitals {"fighting":"maybe","position":"bad\nline"}"#,
        );
        let s = engine.character();
        assert!(s.metrics["fighting"].boolean.is_none());
        assert!(s.metrics["position"].text.is_none());
    }

    #[test]
    fn excessive_values_are_unknown_and_invalid_maps_are_rejected() {
        let mut big = hp("/hp");
        big.scale = 1_000_000.0;
        let mut engine = BindingEngine::new(mapping(vec![
            big,
            bind("/status", "metric", "status", "value", "character", "text"),
        ]))
        .unwrap();
        observe(
            &mut engine,
            &format!(r#"Char.Vitals {{"hp":1e12,"status":"{}"}}"#, "x".repeat(513)),
        );
        assert!(health(&engine).current.is_none());
        assert!(engine.character().metrics["status"].text.is_none());
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY, 1_000_001.0] {
            let mut b = hp("/hp");
            b.scale = scale;
            assert!(BindingEngine::new(mapping(vec![b])).is_none());
        }
        assert!(BindingEngine::new(mapping(vec![hp("/hp"), hp("/other")])).is_none());
    }

    /// The lingering-login reproduction at the engine's level: login packages never apply, the
    /// vitals around them do (the session gates the engine by privacy, not by the login).
    #[test]
    fn login_packages_are_ignored_and_vitals_around_them_apply() {
        let mut engine = BindingEngine::new(mapping(vec![hp("/hp")])).unwrap();
        assert!(!engine.observe(
            OPT_GMCP,
            &format(
                OPT_GMCP,
                br#"Char.Login.Default {"type":["password-credentials"],"version":1}"#,
                true,
                &[]
            ),
            now(),
        ));
        observe(&mut engine, r#"Char.Vitals {"hp":980}"#);
        assert_eq!(health(&engine).current.as_ref().unwrap().value, 980.0);
    }
}
