//! The latest value of every MSDP variable and GMCP package a session received (the C#
//! `ProtocolStateCache`), so a script that starts later is seeded with what the world already
//! said: a world reports a variable once and afterwards only when it changes. The session fills
//! it only with data that passed its privacy gate. Bounded: an update that would break a bound is
//! dropped and the previous value stays.

use indexmap::IndexMap;

use super::msdp;

pub const MAX_ENTRIES: usize = 512;
pub const MAX_VALUE_CHARS: usize = 32_768;
pub const MAX_BUCKET_CHARS: usize = 262_144;
/// Variables taken from one MSDP payload.
pub const MAX_PAYLOAD_VARIABLES: usize = 64;
/// Longest serialized value taken from an MSDP payload.
pub const MAX_PAYLOAD_VALUE_CHARS: usize = 8192;

#[derive(Debug, Default)]
struct Bucket {
    values: IndexMap<String, String>,
    total: usize,
}

impl Bucket {
    fn set(&mut self, key: &str, json: String) -> bool {
        let length = json.encode_utf16().count();
        if length > MAX_VALUE_CHARS {
            return false;
        }
        let previous = self.values.get(key).map(|v| v.encode_utf16().count());
        if previous.is_none() && self.values.len() >= MAX_ENTRIES {
            return false;
        }
        let previous = previous.unwrap_or(0);
        if self.total - previous + length > MAX_BUCKET_CHARS {
            return false;
        }
        self.total = self.total - previous + length;
        self.values.insert(key.to_string(), json);
        true
    }
}

#[derive(Debug, Default)]
pub struct StateCache {
    gmcp: Bucket,
    msdp: Bucket,
}

/// The variables of one MSDP payload with their values as compact JSON, in wire order (what the
/// script events and the cache are built from). Empty when the payload is not MSDP.
pub fn msdp_values(payload: &[u8]) -> Vec<(String, String)> {
    let Some(table) = msdp::parse(payload) else {
        return Vec::new();
    };
    let mut values = Vec::new();
    for (variable, value) in table {
        if values.len() >= MAX_PAYLOAD_VARIABLES {
            break;
        }
        if variable.is_empty() || variable.chars().count() > 128 || variable.chars().any(char::is_control) {
            continue;
        }
        let json = value.to_json().to_string();
        if json.encode_utf16().count() > MAX_PAYLOAD_VALUE_CHARS {
            continue;
        }
        values.push((variable, json));
    }
    values
}

fn valid_package(package: &str) -> bool {
    let bytes = package.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 512
        && bytes[0].is_ascii_alphabetic()
        && bytes
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
}

impl StateCache {
    pub fn is_empty(&self) -> bool {
        self.gmcp.values.is_empty() && self.msdp.values.is_empty()
    }

    pub fn msdp_count(&self) -> usize {
        self.msdp.values.len()
    }

    pub fn gmcp_count(&self) -> usize {
        self.gmcp.values.len()
    }

    /// The cached JSON of an MSDP variable.
    pub fn msdp(&self, variable: &str) -> Option<&str> {
        self.msdp.values.get(variable).map(String::as_str)
    }

    /// The cached JSON of a GMCP package (`null` for a message without data).
    pub fn gmcp(&self, package: &str) -> Option<&str> {
        self.gmcp.values.get(package).map(String::as_str)
    }

    /// Keep one MSDP variable whose value is already JSON.
    pub fn record_msdp_value(&mut self, variable: &str, json: String) -> bool {
        if variable.is_empty()
            || variable.chars().count() > 128
            || variable.chars().any(char::is_control)
            || json.is_empty()
        {
            return false;
        }
        self.msdp.set(variable, json)
    }

    /// Keep every variable of a payload; returns how many were kept.
    pub fn record_msdp(&mut self, payload: &[u8]) -> usize {
        msdp_values(payload)
            .into_iter()
            .filter(|(variable, json)| self.record_msdp_value(variable, json.clone()))
            .count()
    }

    /// Keep one GMCP message (`Package.Name {json}`). Returns the package it was stored under.
    pub fn record_gmcp(&mut self, message: &str) -> Option<String> {
        let (package, body) = match message.find([' ', '\t', '\r', '\n']) {
            Some(i) => (&message[..i], message[i + 1..].trim()),
            None => (message, ""),
        };
        if !valid_package(package) {
            return None;
        }
        let json = if body.is_empty() {
            "null".to_string()
        } else {
            if body.encode_utf16().count() > MAX_VALUE_CHARS {
                return None;
            }
            let value: serde_json::Value = serde_json::from_str(body).ok()?;
            value.to_string()
        };
        self.gmcp.set(package, json).then(|| package.to_string())
    }

    pub fn clear(&mut self) {
        *self = StateCache::default();
    }

    /// `{"gmcp":{...},"msdp":{...}}`, what a script runtime is seeded with, or `None` when empty.
    pub fn seed_json(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let bucket = |b: &Bucket| -> String {
            let entries: Vec<String> = b
                .values
                .iter()
                .map(|(k, v)| format!("{}:{v}", serde_json::Value::String(k.clone())))
                .collect();
            format!("{{{}}}", entries.join(","))
        };
        Some(format!(
            "{{\"gmcp\":{},\"msdp\":{}}}",
            bucket(&self.gmcp),
            bucket(&self.msdp)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msdp(name: &str, value: &str) -> Vec<u8> {
        format!("\x01{name}\x02{value}").into_bytes()
    }

    #[test]
    fn keeps_the_latest_value_per_variable_and_package_and_seeds_both_buckets() {
        let mut cache = StateCache::default();
        assert!(cache.is_empty());
        assert_eq!(cache.seed_json(), None);
        let mut both = msdp("HEALTH", "90");
        both.extend(msdp("MONEYINV", "12"));
        assert_eq!(cache.record_msdp(&both), 2);
        assert_eq!(cache.record_msdp(&msdp("HEALTH", "100")), 1);
        assert!(cache.record_gmcp(r#"Char.Vitals { "hp": 42 , "mana": 7 }"#).is_some());
        assert!(cache.record_gmcp(r#"Char.Vitals {"hp":43}"#).is_some());
        assert_eq!(cache.record_gmcp("Room.Info").as_deref(), Some("Room.Info"));
        assert_eq!(cache.msdp("HEALTH"), Some("\"100\""));
        assert_eq!(cache.gmcp("Char.Vitals"), Some(r#"{"hp":43}"#));
        assert_eq!(cache.gmcp("Room.Info"), Some("null"));
        assert_eq!(cache.msdp("NOPE"), None);
        assert_eq!(
            cache.seed_json().unwrap(),
            r#"{"gmcp":{"Char.Vitals":{"hp":43},"Room.Info":null},"msdp":{"HEALTH":"100","MONEYINV":"12"}}"#
        );
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.seed_json(), None);
    }

    #[test]
    fn malformed_gmcp_messages_are_ignored() {
        for message in ["Char.Vitals {not json", "9Bad {\"hp\":1}", "Char Vitals {\"hp\":1}", ""] {
            let mut cache = StateCache::default();
            assert!(cache.record_gmcp(message).is_none(), "{message}");
            assert!(cache.is_empty());
            assert_eq!(cache.record_msdp(&[9, 9, 9]), 0);
            assert!(!cache.record_msdp_value("", "\"x\"".into()));
            assert!(!cache.record_msdp_value("BAD\u{1}NAME", "\"x\"".into()));
        }
    }

    #[test]
    fn entry_value_and_bucket_bounds_drop_the_update_and_keep_the_previous_value() {
        let mut cache = StateCache::default();
        for i in 0..MAX_ENTRIES {
            assert!(cache.record_msdp_value(&format!("V{i}"), "\"a\"".into()));
        }
        assert!(!cache.record_msdp_value("ONE_TOO_MANY", "\"a\"".into()));
        assert!(cache.record_msdp_value("V0", "\"updated\"".into()));
        assert_eq!(cache.msdp_count(), MAX_ENTRIES);
        assert_eq!(cache.msdp("V0"), Some("\"updated\""));

        let large = format!("\"{}\"", "x".repeat(MAX_VALUE_CHARS - 2));
        assert!(cache.record_gmcp(&format!("Big.One {large}")).is_some());
        assert!(
            cache
                .record_gmcp(&format!("Big.One \"{}\"", "y".repeat(MAX_VALUE_CHARS)))
                .is_none()
        );
        assert_eq!(cache.gmcp("Big.One"), Some(large.as_str()));
        for i in 2..=MAX_BUCKET_CHARS / MAX_VALUE_CHARS {
            assert!(cache.record_gmcp(&format!("Big.N{i} {large}")).is_some());
        }
        assert!(cache.record_gmcp(&format!("Big.Overflow {large}")).is_none());
        assert_eq!(cache.gmcp("Big.Overflow"), None);
        assert!(cache.record_gmcp(r#"Big.One {"small":1}"#).is_some());
        assert!(cache.record_gmcp(r#"Big.Overflow {"fits":true}"#).is_some());
    }
}
