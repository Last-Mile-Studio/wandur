//! The Observed fields inventory (the C# `ProtocolSchemaInventory`): a bounded union of the field
//! paths and wire types a connection has received, never their values, with a SHA-256
//! fingerprint that ignores values, property order and array length. Session-local, and only a
//! partial view of what the server can send.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::format::{Content, parse_json};
use super::{OPT_GMCP, OPT_MSDP};

pub const MAX_FIELDS: usize = 2048;
const MAX_PATH: usize = 512;
const MAX_DEPTH: usize = 16;

type Key = (String, String, String);

#[derive(Debug, Default)]
pub struct SchemaInventory {
    fields: BTreeMap<Key, BTreeSet<&'static str>>,
    limited: bool,
    revision: u64,
    cache: Option<(String, String, String)>,
}

impl SchemaInventory {
    pub fn field_count(&self) -> usize {
        self.fields.len()
    }

    pub fn limited(&self) -> bool {
        self.limited
    }

    /// Bumped whenever the inventory changes.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn observe(&mut self, option: u8, content: &Content) {
        if !matches!(option, OPT_MSDP | OPT_GMCP)
            || content.malformed
            || content.truncated
            || content.redacted
            || crate::login::gmcp::is_private(&content.name)
        {
            return;
        }
        let protocol = if option == OPT_MSDP { "MSDP" } else { "GMCP" };
        let package = if option == OPT_MSDP {
            "MSDP"
        } else {
            content.name.as_str()
        };
        if package.is_empty()
            || package.len() > 128
            || !package
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
        {
            return;
        }
        if content.body.is_empty() {
            self.add(protocol, package, "", "empty");
            return;
        }
        if let Some(value) = parse_json(&content.body) {
            self.walk(protocol, package, String::new(), &value, 0);
        }
    }

    fn walk(&mut self, protocol: &str, package: &str, path: String, value: &Value, depth: usize) {
        if depth > MAX_DEPTH || path.len() > MAX_PATH {
            self.mark_limited();
            return;
        }
        let kind = match value {
            Value::Object(_) => "object",
            Value::Array(_) => "array",
            Value::String(_) => "string",
            Value::Number(_) => "number",
            Value::Bool(_) => "boolean",
            Value::Null => "null",
        };
        if !self.add(protocol, package, &path, kind) {
            return;
        }
        match value {
            Value::Object(map) => {
                for (key, item) in map {
                    self.walk(protocol, package, format!("{path}/{}", escape(key)), item, depth + 1);
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.walk(protocol, package, format!("{path}/*"), item, depth + 1);
                }
            }
            _ => {}
        }
    }

    fn add(&mut self, protocol: &str, package: &str, path: &str, kind: &'static str) -> bool {
        let key = (protocol.to_string(), package.to_string(), path.to_string());
        if !self.fields.contains_key(&key) {
            if self.fields.len() >= MAX_FIELDS {
                self.mark_limited();
                return false;
            }
            self.fields.insert(key.clone(), BTreeSet::new());
        }
        if self.fields.get_mut(&key).is_some_and(|types| types.insert(kind)) {
            self.changed();
        }
        true
    }

    fn mark_limited(&mut self) {
        if !self.limited {
            self.limited = true;
            self.changed();
        }
    }

    fn changed(&mut self) {
        self.cache = None;
        self.revision += 1;
    }

    pub fn clear(&mut self) {
        self.fields.clear();
        self.limited = false;
        self.changed();
    }

    fn fields_json(&self) -> Value {
        Value::Array(
            self.fields
                .iter()
                .map(|((protocol, package, path), types)| {
                    serde_json::json!({
                        "protocol": protocol,
                        "package": package,
                        "path": path,
                        "types": types.iter().collect::<Vec<_>>(),
                    })
                })
                .collect(),
        )
    }

    fn refresh(&mut self) -> &(String, String, String) {
        if self.cache.is_none() {
            let fields = self.fields_json();
            // Fixed property order and ordinal sorting (the BTreeMap) make the fingerprint
            // independent of packet and field order. The canonical text escapes as .NET does, so
            // the fingerprint matches the C# client's for the same fields.
            let canonical = format!(
                "{{\"schemaVersion\":1,\"limited\":{},\"fields\":{}}}",
                self.limited,
                canonical(&fields)
            );
            let fingerprint: String = Sha256::digest(canonical.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let fields_text = super::format::pretty(&fields);
            let snapshot = super::format::pretty(&serde_json::json!({
                "schemaVersion": 1,
                "limited": self.limited,
                "fingerprint": fingerprint,
                "fields": fields,
            }));
            self.cache = Some((snapshot, fingerprint, fields_text));
        }
        self.cache.as_ref().expect("just filled")
    }

    /// The indented JSON shown in Observed fields.
    pub fn snapshot(&mut self) -> String {
        self.refresh().0.clone()
    }

    pub fn fingerprint(&mut self) -> String {
        self.refresh().1.clone()
    }

    /// The fields alone, indented.
    pub fn fields_text(&mut self) -> String {
        self.refresh().2.clone()
    }
}

/// JSON Pointer escaping, with `~2` for a literal `*` so array wildcards stay unambiguous.
fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1").replace('*', "~2")
}

/// Compact JSON with .NET's default string escaping.
fn canonical(value: &Value) -> String {
    match value {
        Value::String(s) => crate::macros::definition::js_string(s),
        Value::Array(items) => format!("[{}]", items.iter().map(canonical).collect::<Vec<_>>().join(",")),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}:{}", crate::macros::definition::js_string(k), canonical(v)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::format::format;
    use super::*;

    fn observe(inventory: &mut SchemaInventory, text: &str) {
        inventory.observe(OPT_GMCP, &format(OPT_GMCP, text.as_bytes(), false, &[]));
    }

    #[test]
    fn fingerprint_ignores_values_ordering_and_partial_updates() {
        let mut first = SchemaInventory::default();
        let mut second = SchemaInventory::default();
        observe(&mut first, r#"Char.Vitals {"hp":100,"maxhp":120}"#);
        observe(&mut second, r#"Char.Vitals {"maxhp":900,"hp":10}"#);
        assert_eq!(first.fingerprint(), second.fingerprint());
        let hash = first.fingerprint();
        observe(&mut first, r#"Char.Vitals {"hp":2}"#);
        assert_eq!(hash, first.fingerprint());
        assert!(!first.fields_text().contains("120"));
        observe(&mut first, r#"Char.Vitals {"hp":"2"}"#);
        assert_ne!(hash, first.fingerprint());
        assert!(first.snapshot().contains("string"));
    }

    #[test]
    fn array_length_does_not_change_shape_and_paths_cannot_collide() {
        let mut inventory = SchemaInventory::default();
        observe(
            &mut inventory,
            r#"Char.Items.List {"items":[{"id":1}],"a/b":true,"a":{"b":false},"*":0}"#,
        );
        let hash = inventory.fingerprint();
        observe(&mut inventory, r#"Char.Items.List {"items":[{"id":2},{"id":3}]}"#);
        observe(&mut inventory, r#"Char.Items.List {"items":[]}"#);
        assert_eq!(hash, inventory.fingerprint());
        let snapshot = inventory.snapshot();
        for path in ["/items/*/id", "/a~1b", "/a/b", "/~2"] {
            assert!(snapshot.contains(path), "{path}");
        }
    }

    #[test]
    fn msdp_fields_accumulate_across_messages_without_storing_values() {
        let mut inventory = SchemaInventory::default();
        let msdp = |text: &[u8]| format(OPT_MSDP, text, false, &[]);
        inventory.observe(OPT_MSDP, &msdp(b"\x01HEALTH\x0285\x01HEALTH_MAX\x02100"));
        let hash = inventory.fingerprint();
        inventory.observe(OPT_MSDP, &msdp(b"\x01HEALTH\x0210"));
        assert_eq!(hash, inventory.fingerprint());
        assert!(inventory.snapshot().contains("/HEALTH_MAX"));
        assert!(!inventory.fields_text().contains("85"));
        inventory.clear();
        assert_eq!(inventory.field_count(), 0);
    }

    #[test]
    fn malformed_data_cannot_pollute_inventory_and_unique_fields_are_bounded() {
        let mut inventory = SchemaInventory::default();
        observe(&mut inventory, "Broken {oops");
        inventory.observe(OPT_GMCP, &Content::new("Clipped", "{}", false, true));
        observe(&mut inventory, &format!("{} {{}}", "A".repeat(129)));
        assert_eq!(inventory.field_count(), 0);
        for i in 0..MAX_FIELDS + 100 {
            observe(&mut inventory, &format!("Custom {{\"field{i}\":1}}"));
        }
        assert_eq!(inventory.field_count(), MAX_FIELDS);
        assert!(inventory.limited());
        assert!(inventory.snapshot().contains("\"limited\": true"));
        inventory.clear();
        assert!(!inventory.limited());
    }

    #[test]
    fn the_snapshot_has_the_csharp_shape() {
        let mut inventory = SchemaInventory::default();
        observe(&mut inventory, r#"Char.Combat {"enemy":"a marsh wisp"}"#);
        let snapshot = inventory.snapshot();
        assert!(snapshot.starts_with("{\n  \"schemaVersion\": 1,\n  \"limited\": false,\n  \"fingerprint\": \""));
        assert!(
            snapshot.contains("\"protocol\": \"GMCP\",\n      \"package\": \"Char.Combat\",\n      \"path\": \"\",")
        );
        assert_eq!(inventory.fingerprint().len(), 64);
    }
}
