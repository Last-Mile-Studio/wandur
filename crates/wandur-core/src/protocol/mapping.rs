//! A world's protocol mapping (the SDK's `Wandur.Models` `WorldMapping`): which GMCP or MSDP field
//! feeds which normalized value (character health, the opponent's name, the world's time). The
//! directory publishes one per world endpoint in its listing (`protocol_mapping`), in the
//! snake_case wire format of the SDK. A mapping that does not validate is dropped whole and the
//! rest of the listing is kept; a mapping only applies to the exact endpoint it names.

use serde::{Deserialize, Deserializer, Serialize};

use crate::endpoint::Endpoint;

pub const MAX_BINDINGS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappingEndpoint {
    pub host: String,
    pub port: i64,
    #[serde(default)]
    pub use_tls: bool,
}

impl MappingEndpoint {
    pub fn normalized_host(&self) -> String {
        normalize_host(&self.host)
    }

    pub fn matches(&self, other: &MappingEndpoint) -> bool {
        self.normalized_host() == other.normalized_host() && self.port == other.port && self.use_tls == other.use_tls
    }

    /// Whether this is the connection's address (host without case or a trailing dot).
    pub fn is(&self, host: &str, port: u16, tls: bool) -> bool {
        self.normalized_host() == normalize_host(host) && self.port == i64::from(port) && self.use_tls == tls
    }
}

fn normalize_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_lowercase()
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FieldReference {
    /// `GMCP` or `MSDP`.
    pub protocol: String,
    /// The GMCP package, or `MSDP` for native MSDP.
    pub package: String,
    /// A JSON Pointer into the message's data (`/hp`), or "" for the whole value.
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MappingTarget {
    /// `character`, `opponent`, `vehicle` or `world`.
    pub entity: String,
    /// `identity`, `location`, `resource`, `progression`, `attribute`, `currency` or `metric`.
    pub category: String,
    /// The game's word for it (`health`, `mana`, `gold`): a lowercase slug.
    pub key: String,
    /// `current`, `maximum`, `base`, `carried`, `bank`, `total` or `value`.
    pub member: String,
}

fn default_conversion() -> String {
    "number".into()
}

fn default_scale() -> f64 {
    1.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FieldBinding {
    pub source: FieldReference,
    pub target: MappingTarget,
    #[serde(default)]
    pub label: String,
    /// `number`, `text` or `boolean`.
    #[serde(default = "default_conversion")]
    pub conversion: String,
    #[serde(default = "default_scale")]
    pub scale: f64,
}

impl PartialEq for FieldBinding {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
            && self.target == other.target
            && self.label == other.label
            && self.conversion == other.conversion
            && self.scale.to_bits() == other.scale.to_bits()
    }
}

impl Eq for FieldBinding {}

impl std::hash::Hash for FieldBinding {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.source.hash(state);
        self.target.hash(state);
        self.label.hash(state);
        self.conversion.hash(state);
        self.scale.to_bits().hash(state);
    }
}

fn one() -> i64 {
    1
}

fn default_provenance() -> String {
    "deterministic".into()
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldMapping {
    #[serde(default = "one")]
    pub schema_version: i64,
    pub world_id: String,
    pub endpoint: MappingEndpoint,
    pub schema_fingerprint: String,
    #[serde(default = "one")]
    pub revision: i64,
    /// RFC 3339.
    #[serde(default)]
    pub generated_at: String,
    #[serde(default = "default_provenance")]
    pub provenance: String,
    #[serde(default = "yes")]
    pub provisional: bool,
    #[serde(default)]
    pub bindings: Vec<FieldBinding>,
}

impl WorldMapping {
    /// Whether this mapping is for the connection at this address.
    pub fn is_for(&self, endpoint: &Endpoint) -> bool {
        self.endpoint.is(&endpoint.host, endpoint.port, endpoint.tls)
    }

    /// The MSDP variables this mapping reads over `protocol` (`MSDP` native, `GMCP` tunneled),
    /// as REPORT and SEND accept them: the first path segment, valid names only, at most 256.
    pub fn msdp_names(&self, protocol: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for binding in &self.bindings {
            if binding.source.protocol != protocol || binding.source.package != "MSDP" {
                continue;
            }
            let name = binding.source.path.split('/').nth(1).unwrap_or("");
            if super::msdp::valid_name(name) && !names.iter().any(|n| n == name) {
                names.push(name.to_string());
                if names.len() >= 256 {
                    break;
                }
            }
        }
        names
    }
}

/// Read a mapping from JSON, keeping it only when it is valid; anything else is `None`.
pub fn from_value(value: serde_json::Value) -> Option<WorldMapping> {
    let mapping: WorldMapping = serde_json::from_value(value).ok()?;
    is_valid(&mapping).then_some(mapping)
}

/// A serde field reader for an optional mapping: an invalid or misshapen one is `None` and the
/// rest of the record is kept.
pub fn lenient<'de, D: Deserializer<'de>>(d: D) -> Result<Option<WorldMapping>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(d)?;
    Ok(value.and_then(from_value))
}

fn slug(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn text(value: &str, max: usize) -> bool {
    value.encode_utf16().count() <= max && !value.chars().any(char::is_control)
}

/// Words a mapped field may not contain: logins, secrets and conversation never feed a mapping.
pub fn sensitive(value: &str) -> bool {
    let lower = value.to_lowercase();
    [
        "password",
        "passwd",
        "secret",
        "token",
        "credential",
        "login",
        "auth",
        "chat",
        "channel",
    ]
    .iter()
    .any(|w| lower.contains(w))
}

fn pointer(value: &str) -> bool {
    if !text(value, 512) || (!value.is_empty() && !value.starts_with('/')) {
        return false;
    }
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'~' {
            i += 1;
            if i == bytes.len() || !matches!(bytes[i], b'0' | b'1') {
                return false;
            }
        }
        i += 1;
    }
    true
}

pub fn safe_field(source: &FieldReference) -> bool {
    matches!(source.protocol.as_str(), "MSDP" | "GMCP")
        && (1..=128).contains(&source.package.len())
        && source
            .package
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
        && (source.protocol != "MSDP" || source.package == "MSDP")
        && pointer(&source.path)
        && !sensitive(&source.package)
        && !sensitive(&source.path)
}

fn host(host: &str) -> bool {
    let normalized = normalize_host(host);
    if normalized.is_empty() || !text(host, 253) {
        return false;
    }
    if normalized.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    let inner = normalized.trim_start_matches('[').trim_end_matches(']');
    if inner.parse::<std::net::Ipv6Addr>().is_ok() {
        return true;
    }
    normalized.split('.').all(|label| {
        (1..=63).contains(&label.len())
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b >= 0x80)
    })
}

pub fn binding(b: &FieldBinding) -> bool {
    let t = &b.target;
    if !safe_field(&b.source)
        || !slug(&t.key)
        || !matches!(t.entity.as_str(), "character" | "opponent" | "vehicle" | "world")
        || !text(&b.label, 80)
        || b.label.trim().is_empty()
        || !b.scale.is_finite()
        || b.scale <= 0.0
        || b.scale > 1_000_000.0
    {
        return false;
    }
    let conversion = b.conversion.as_str();
    match t.category.as_str() {
        "identity" | "location" => t.member == "value" && conversion == "text" && b.scale == 1.0,
        "resource" | "progression" => matches!(t.member.as_str(), "current" | "maximum") && conversion == "number",
        "attribute" => matches!(t.member.as_str(), "current" | "base") && conversion == "number",
        "currency" => matches!(t.member.as_str(), "carried" | "bank" | "total") && conversion == "number",
        "metric" => {
            t.member == "value"
                && matches!(conversion, "number" | "text" | "boolean")
                && (conversion == "number" || b.scale == 1.0)
        }
        _ => false,
    }
}

pub fn is_valid(m: &WorldMapping) -> bool {
    let fingerprint = m.schema_fingerprint.as_bytes();
    if m.schema_version != 1
        || !text(&m.world_id, 256)
        || m.world_id.trim().is_empty()
        || !host(&m.endpoint.host)
        || !(1..=65_535).contains(&m.endpoint.port)
        || fingerprint.len() != 64
        || !fingerprint
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
        || m.revision < 1
        || crate::directory::time::parse_rfc3339(&m.generated_at).is_none()
        || !matches!(m.provenance.as_str(), "deterministic" | "azure")
        || m.bindings.len() > MAX_BINDINGS
        || !m.bindings.iter().all(binding)
    {
        return false;
    }
    let mut targets = std::collections::HashSet::new();
    m.bindings.iter().all(|b| targets.insert(&b.target))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn bind(path: &str, category: &str, key: &str, member: &str, entity: &str, conversion: &str) -> FieldBinding {
        FieldBinding {
            source: FieldReference {
                protocol: "GMCP".into(),
                package: "Char.Vitals".into(),
                path: path.into(),
            },
            target: MappingTarget {
                entity: entity.into(),
                category: category.into(),
                key: key.into(),
                member: member.into(),
            },
            label: key.into(),
            conversion: conversion.into(),
            scale: 1.0,
        }
    }

    pub fn hp(path: &str) -> FieldBinding {
        bind(path, "resource", "health", "current", "character", "number")
    }

    pub fn mapping(bindings: Vec<FieldBinding>) -> WorldMapping {
        WorldMapping {
            schema_version: 1,
            world_id: "test".into(),
            endpoint: MappingEndpoint {
                host: "mud.example".into(),
                port: 4000,
                use_tls: false,
            },
            schema_fingerprint: "a".repeat(64),
            revision: 1,
            generated_at: "2026-09-18T12:00:00Z".into(),
            provenance: "deterministic".into(),
            provisional: true,
            bindings,
        }
    }

    fn wire(m: &WorldMapping) -> serde_json::Value {
        serde_json::to_value(m).unwrap()
    }

    #[test]
    fn a_valid_mapping_round_trips_in_the_snake_case_wire_format() {
        let m = mapping(vec![hp("/hp")]);
        let json = wire(&m);
        assert_eq!(json["schema_fingerprint"].as_str().unwrap().len(), 64);
        assert_eq!(json["endpoint"]["use_tls"], false);
        assert_eq!(json["bindings"][0]["target"]["member"], "current");
        assert_eq!(from_value(json), Some(m));
    }

    /// Port of `InvalidOptionalMappingIsIgnoredByWholeCatalogAndSavedProfile`: each of these
    /// breaks the mapping, which is then dropped.
    #[test]
    fn invalid_mappings_are_dropped() {
        let cases: &[(&[&str], serde_json::Value)] = &[
            (&["endpoint"], serde_json::Value::Null),
            (&["endpoint", "host"], serde_json::Value::Null),
            (&["endpoint", "port"], serde_json::json!("4000")),
            (&["world_id"], serde_json::Value::Null),
            (&["schema_fingerprint"], serde_json::Value::Null),
            (&["schema_version"], serde_json::json!([])),
            (&["bindings"], serde_json::Value::Null),
            (&["bindings", "0"], serde_json::Value::Null),
            (&["bindings", "0", "source"], serde_json::Value::Null),
            (&["bindings", "0", "source", "path"], serde_json::Value::Null),
            (&["bindings", "0", "source", "package"], serde_json::json!({})),
            (&["bindings", "0", "target"], serde_json::Value::Null),
            (&["bindings", "0", "target", "key"], serde_json::Value::Null),
            (&["bindings", "0", "scale"], serde_json::json!("NaN")),
            (&["bindings", "0", "scale"], serde_json::json!(0)),
        ];
        for (path, replacement) in cases {
            let mut json = wire(&mapping(vec![hp("/hp")]));
            let mut node = &mut json;
            for part in &path[..path.len() - 1] {
                node = match node {
                    serde_json::Value::Array(items) => &mut items[part.parse::<usize>().unwrap()],
                    other => &mut other[*part],
                };
            }
            let last = path[path.len() - 1];
            match node {
                serde_json::Value::Array(items) => items[last.parse::<usize>().unwrap()] = replacement.clone(),
                other => other[last] = replacement.clone(),
            }
            assert_eq!(from_value(json), None, "{path:?}");
        }
        for invalid in [
            serde_json::json!(42),
            serde_json::json!([]),
            serde_json::json!({"schema_version": "oops"}),
        ] {
            assert_eq!(from_value(invalid), None);
        }
    }

    #[test]
    fn scales_targets_and_sensitive_fields_are_checked() {
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY, 1_000_001.0] {
            let mut b = hp("/hp");
            b.scale = scale;
            assert!(!is_valid(&mapping(vec![b])), "{scale}");
        }
        // Two bindings to one target.
        assert!(!is_valid(&mapping(vec![hp("/hp"), hp("/other")])));
        // Login, chat and secrets are never mapped.
        for package in ["Char.Login", "Comm.Channel", "Auth.Token"] {
            let mut b = hp("/hp");
            b.source.package = package.into();
            assert!(!is_valid(&mapping(vec![b])), "{package}");
        }
        let mut b = hp("/~2");
        assert!(!is_valid(&mapping(vec![b.clone()])));
        b.source.path = "hp".into();
        assert!(!is_valid(&mapping(vec![b])));
        // MSDP is only package MSDP.
        let mut b = hp("/HEALTH");
        b.source.protocol = "MSDP".into();
        assert!(!is_valid(&mapping(vec![b.clone()])));
        b.source.package = "MSDP".into();
        assert!(is_valid(&mapping(vec![b])));
        // A text identity needs the text conversion.
        assert!(!is_valid(&mapping(vec![bind(
            "/name",
            "identity",
            "name",
            "value",
            "character",
            "number"
        )])));
        assert!(is_valid(&mapping(vec![bind(
            "/name",
            "identity",
            "name",
            "value",
            "character",
            "text"
        )])));
    }

    /// Port of `InvalidAndDifferentProfileEndpointsCannotActivateMapping`.
    #[test]
    fn a_mapping_only_applies_to_its_exact_endpoint() {
        let m = mapping(vec![hp("/hp")]);
        assert!(m.endpoint.is("MUD.EXAMPLE.", 4000, false));
        assert!(!m.endpoint.is("", 4000, false));
        assert!(!m.endpoint.is("other.example", 4000, false));
        assert!(!m.endpoint.is("mud.example", 4001, false));
        assert!(!m.endpoint.is("mud.example", 4000, true));
    }

    /// Ports of `OptionalMalformedMappingDoesNotDiscardListing` and
    /// `MappingRoundtripsAndOnlyTravelsToItsExactEndpoint`.
    #[test]
    fn listings_and_saved_worlds_keep_only_a_valid_mapping_for_their_endpoint() {
        use crate::directory::listing::WorldListing;
        for invalid in ["42", "[]", r#"{"bindings":null}"#, r#"{"schema_version":"oops"}"#] {
            let json = format!(r#"{{"id":"test","name":"Test","protocol_mapping":{invalid}}}"#);
            let listing: WorldListing = serde_json::from_str(&json).unwrap();
            assert_eq!(listing.name, "Test");
            assert!(listing.protocol_mapping.is_none(), "{invalid}");
        }
        let m = mapping(vec![hp("/hp")]);
        let json = serde_json::json!({"id": "test", "name": "Test", "host": "MUD.EXAMPLE.", "port": 4000,
            "tls_port": 4001, "protocol_mapping": wire(&m)});
        let listing: WorldListing = serde_json::from_value(json).unwrap();
        assert_eq!(listing.mapping_for("mud.example", 4000, false), Some(&m));
        assert!(listing.mapping_for("mud.example", 4001, true).is_none());
        assert!(listing.mapping_for("other.example", 4000, false).is_none());
        // A saved world keeps a valid mapping and drops a broken one.
        let world = crate::settings::SavedWorld {
            protocol_mapping: Some(m.clone()),
            ..Default::default()
        };
        let text = serde_json::to_string(&world).unwrap();
        let back: crate::settings::SavedWorld = serde_json::from_str(&text).unwrap();
        assert_eq!(back.protocol_mapping, Some(m));
        let broken = text.replace(&"a".repeat(64), "not-a-fingerprint");
        let back: crate::settings::SavedWorld = serde_json::from_str(&broken).unwrap();
        assert!(back.protocol_mapping.is_none());
        assert!(
            !serde_json::to_string(&crate::settings::SavedWorld::default())
                .unwrap()
                .contains("protocol_mapping")
        );
    }

    #[test]
    fn mapped_msdp_names() {
        let mut a = hp("/HEALTH");
        a.source.protocol = "MSDP".into();
        a.source.package = "MSDP".into();
        let mut b = bind("/HEALTH_MAX", "resource", "health", "maximum", "character", "number");
        b.source = a.source.clone();
        b.source.path = "/HEALTH_MAX".into();
        let m = mapping(vec![a, b, hp("/hp")]);
        assert_eq!(m.msdp_names("MSDP"), ["HEALTH", "HEALTH_MAX"]);
        assert!(m.msdp_names("GMCP").is_empty());
    }
}
