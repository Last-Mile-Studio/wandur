//! Received GMCP and MSDP data formatted for people (the Diagnostics document), with a redacted
//! copy made before anything is kept: known password, token and credential fields are masked
//! wherever they appear, and remembered secrets (a saved password, a typed private input) are
//! replaced in every name and text. The C# SDK's `ProtocolDiagnosticFormatter` and
//! `ProtocolDiagnosticRedactor`. Transport and automation privacy decisions stay independent.

use serde_json::Value;

use super::{OPT_GMCP, OPT_MSDP, msdp};

/// Payload bytes formatted; longer payloads are cut and marked truncated.
pub const MAX_PAYLOAD_BYTES: usize = 16_384;
/// Characters of formatted body kept.
pub const MAX_BODY_CHARS: usize = 32_768;
/// What a hidden value reads as.
pub const REDACTED: &str = "[redacted]";
/// Remembered secrets used at once.
pub const MAX_SECRETS: usize = 8;

/// One received message as Diagnostics shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    /// The GMCP package, up to three MSDP variable names joined by ", ", or "MSDP".
    pub name: String,
    /// Indented JSON; for malformed data a JSON string (GMCP) or hexadecimal bytes (MSDP).
    pub body: String,
    pub malformed: bool,
    pub truncated: bool,
    /// Something was masked (a sensitive field, a remembered secret, a private package).
    pub redacted: bool,
}

impl Content {
    pub fn new(name: &str, body: &str, malformed: bool, truncated: bool) -> Self {
        Self {
            name: name.into(),
            body: body.into(),
            malformed,
            truncated,
            redacted: false,
        }
    }
}

/// Indented JSON, as the C# serializer writes it (two spaces, `"name": value`).
pub fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// Format one GMCP (201) or MSDP (69) payload. `private_input`: input was private or a login was
/// running, so data that cannot be separated safely (malformed or truncated) is hidden whole.
pub fn format(option: u8, payload: &[u8], private_input: bool, secrets: &[String]) -> Content {
    let mut truncated = payload.len() > MAX_PAYLOAD_BYTES;
    let payload = &payload[..payload.len().min(MAX_PAYLOAD_BYTES)];
    let mut malformed = false;
    let (name, body) = if option == OPT_MSDP {
        match msdp::parse(payload) {
            Some(table) => (
                table
                    .iter()
                    .take(3)
                    .map(|(n, _)| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                pretty(&msdp::to_json(&table)),
            ),
            None => {
                malformed = true;
                ("MSDP".to_string(), hex(payload))
            }
        }
    } else {
        let text = String::from_utf8_lossy(payload);
        let (name, data) = match text.find([' ', '\t', '\r', '\n']) {
            Some(i) => (&text[..i], text[i + 1..].trim()),
            None => (&text[..], ""),
        };
        let body = if data.is_empty() {
            String::new()
        } else {
            match parse_json(data) {
                Some(value) => pretty(&value),
                None => {
                    malformed = true;
                    Value::String(data.to_string()).to_string()
                }
            }
        };
        (name.to_string(), body)
    };
    let (name, mut body, redacted) = redact(option, &name, &body, malformed, truncated, private_input, secrets);
    truncated |= name.chars().count() > 128;
    let name: String = name
        .chars()
        .take(128)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if body.chars().count() > MAX_BODY_CHARS {
        truncated = true;
        body = body.chars().take(MAX_BODY_CHARS).collect();
    }
    Content {
        name,
        body,
        malformed,
        truncated,
        redacted,
    }
}

/// JSON with the C# depth limit (32).
pub fn parse_json(text: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(text).ok()?;
    (depth(&value) <= 32).then_some(value)
}

fn depth(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        Value::Object(map) => 1 + map.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// A field whose value is always masked (letters and digits only, any case).
pub fn sensitive(name: &str) -> bool {
    let key: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    matches!(
        key.as_str(),
        "password"
            | "passwd"
            | "pwd"
            | "pass"
            | "passphrase"
            | "passcode"
            | "secret"
            | "clientsecret"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "idtoken"
            | "authtoken"
            | "sessiontoken"
            | "sessionid"
            | "apikey"
            | "authorization"
            | "credentials"
            | "credential"
            | "privatekey"
            | "cookie"
            | "setcookie"
    )
}

/// Replaces remembered secrets (the longest first) and notes whether it did.
pub struct Scrubber<'a> {
    known: Vec<&'a str>,
    pub redacted: bool,
}

impl<'a> Scrubber<'a> {
    pub fn new(secrets: &'a [String]) -> Self {
        let mut known: Vec<&str> = secrets
            .iter()
            .filter(|s| !s.is_empty())
            .take(MAX_SECRETS)
            .map(String::as_str)
            .collect();
        known.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
        Self { known, redacted: false }
    }

    pub fn scrub(&mut self, text: &str) -> String {
        let mut text = text.to_string();
        for secret in &self.known {
            if text.contains(secret) {
                text = text.replace(secret, REDACTED);
                self.redacted = true;
            }
        }
        text
    }

    /// Whether any secret occurs in `text` (nothing replaced).
    pub fn finds(&self, text: &str) -> bool {
        self.known.iter().any(|s| text.contains(s))
    }
}

/// Mask a formatted message: returns the name, the body and whether anything was masked.
pub fn redact(
    option: u8,
    name: &str,
    body: &str,
    malformed: bool,
    truncated: bool,
    private_input: bool,
    secrets: &[String],
) -> (String, String, bool) {
    let mut scrubber = Scrubber::new(secrets);
    let private_package = option == OPT_GMCP
        && ((crate::login::gmcp::is_private(name)
            && !name.eq_ignore_ascii_case("Char.Login.Default")
            && !name.eq_ignore_ascii_case("Char.Login.Result"))
            || name.split('.').any(sensitive));
    let name = scrubber.scrub(name);
    if private_package || (private_input && (malformed || truncated)) {
        return (name, REDACTED.into(), true);
    }
    if body.is_empty() {
        return (name, String::new(), scrubber.redacted);
    }
    let body = match parse_json(body) {
        Some(value) => {
            let mut hidden = false;
            let clean = mask(&value, &mut scrubber, &mut hidden);
            if hidden {
                scrubber.redacted = true;
            }
            pretty(&clean)
        }
        None => scrubber.scrub(body),
    };
    (name, body, scrubber.redacted)
}

fn mask(value: &Value, scrubber: &mut Scrubber, hidden: &mut bool) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, item) in map {
                let clean = if sensitive(key) {
                    *hidden = true;
                    Value::String(REDACTED.into())
                } else {
                    mask(item, scrubber, hidden)
                };
                out.insert(scrubber.scrub(key), clean);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| mask(v, scrubber, hidden)).collect()),
        Value::String(text) => Value::String(scrubber.scrub(text)),
        other => other.clone(),
    }
}

/// The label of an option in Diagnostics: GMCP, MSDP, or the telnet option's name.
pub fn protocol_name(option: u8) -> String {
    match option {
        201 => "GMCP",
        69 => "MSDP",
        70 => "MSSP",
        31 => "NAWS",
        24 => "TTYPE",
        86 => "MCCP2",
        87 => "MCCP3",
        91 => "MXP",
        93 => "ZMP",
        90 => "MSP",
        200 => "ATCP",
        42 => "CHARSET",
        39 => "NEW-ENVIRON",
        25 => "EOR",
        1 => "ECHO",
        3 => "SGA",
        other => return format!("OPTION {other}"),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gmcp(text: &str) -> Content {
        format(OPT_GMCP, text.as_bytes(), false, &[])
    }

    #[test]
    fn formats_all_gmcp_packages_including_non_room_data() {
        let m = gmcp(r#"Char.Vitals {"hp":42,"maxhp":100}"#);
        assert_eq!(m.name, "Char.Vitals");
        assert_eq!(m.body, "{\n  \"hp\": 42,\n  \"maxhp\": 100\n}");
        assert!(!m.malformed);
    }

    #[test]
    fn formats_msdp_tables_and_arrays_without_requiring_room_fields() {
        let m = format(
            OPT_MSDP,
            b"\x01HEALTH\x0242\x01ROOM\x02\x03\x01NAME\x02Hall\x01EXITS\x02\x05\x02north\x02south\x06\x04",
            false,
            &[],
        );
        assert_eq!(m.name, "HEALTH, ROOM");
        assert!(m.body.contains("\"NAME\": \"Hall\""));
        assert!(m.body.contains("\"north\""));
        assert!(!m.malformed);
    }

    #[test]
    fn malformed_and_oversized_payloads_remain_inspectable_and_bounded() {
        let m = format(OPT_MSDP, &[1, 255], false, &[]);
        assert!(m.malformed);
        assert!(m.body.contains("01FF"));
        let big = format!("Room.Info {}", "x".repeat(100_000));
        let m = gmcp(&big);
        assert!(m.truncated);
        assert!(m.body.chars().count() <= MAX_BODY_CHARS);
        let m = gmcp("Room.Info {not json");
        assert!(m.malformed);
        assert_eq!(m.body, "\"{not json\"");
    }

    #[test]
    fn sensitive_fields_are_redacted_without_hiding_the_response() {
        let m = gmcp(
            r#"Auth.Info {"success":false,"message":"Try another account","password":"fixture-password","nested":[{"access_token":"fixture-token","remaining":2}]}"#,
        );
        assert!(m.body.contains("Try another account"));
        assert!(m.body.contains("remaining"));
        assert!(!m.body.contains("fixture-password"));
        assert!(!m.body.contains("fixture-token"));
        assert!(m.redacted);
    }

    #[test]
    fn msdp_passwords_are_redacted_alongside_visible_vitals() {
        let m = format(
            OPT_MSDP,
            b"\x01HEALTH\x0285\x01PASSWORD\x02fixture-password",
            false,
            &[],
        );
        assert!(m.body.contains("85"));
        assert!(!m.body.contains("fixture-password"));
    }

    #[test]
    fn credential_bearing_packages_never_expose_unrecognized_fields() {
        for package in ["Char.Login.Token", "Char.Login.Credentials", "Char.Login.URL"] {
            let m = gmcp(&format!(r#"{package} {{"unrecognized":"fixture-secret"}}"#));
            assert_eq!(m.name, package);
            assert!(!m.body.contains("fixture-secret"));
        }
    }

    #[test]
    fn known_credentials_are_removed_from_free_text_replies() {
        let secret = "fixture-only-\"quoted\"-秘密".to_string();
        let response = format!(
            "Char.Login.Result {}",
            serde_json::json!({"success": false, "message": format!("Rejected: {secret}")})
        );
        let m = format(OPT_GMCP, response.as_bytes(), true, std::slice::from_ref(&secret));
        assert!(m.redacted);
        let doc: Value = serde_json::from_str(&m.body).unwrap();
        assert_eq!(doc["message"], "Rejected: [redacted]");
        assert_eq!(doc["success"], false);
    }

    #[test]
    fn malformed_private_packets_keep_their_identity_but_not_unparseable_secrets() {
        for (option, body) in [
            (OPT_GMCP, &b"Char.Login.Result {broken fixture-secret"[..]),
            (OPT_MSDP, b"\x01BROKEN\x02\x03fixture-secret"),
        ] {
            let m = format(option, body, true, &[]);
            assert!(m.redacted && m.malformed);
            assert_eq!(m.body, REDACTED);
            assert!(!m.name.is_empty());
        }
    }

    #[test]
    fn login_data_stays_visible_even_without_known_credentials() {
        let m = format(OPT_GMCP, br#"Char.Vitals {"hp":42,"maxhp":100}"#, true, &[]);
        assert!(!m.redacted);
        assert!(m.body.contains("42") && m.body.contains("100"));
    }

    #[test]
    fn protocol_names() {
        assert_eq!(protocol_name(201), "GMCP");
        assert_eq!(protocol_name(70), "MSSP");
        assert_eq!(protocol_name(99), "OPTION 99");
    }
}
