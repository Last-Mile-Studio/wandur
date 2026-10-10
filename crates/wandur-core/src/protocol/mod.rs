//! Decoding of out-of-band MUD protocols carried in telnet subnegotiations. No UI here: the
//! session turns payloads into these values and hands them on as events.
//!
//! - GMCP (option 201): `Package.Name` followed by optional JSON.
//! - MSSP (option 70): `MSSP_VAR name MSSP_VAL value [MSSP_VAL value...]` repeated.
//! - MSDP (option 69): [`msdp`], with subscription in [`discovery`].
//! - [`format`]: received data formatted and redacted for the Diagnostics document; [`schema`]:
//!   the Observed fields inventory.
//! - [`mapping`] and [`binding`]: a world's protocol mapping and the normalized game state it
//!   gives (the vitals strip).
//! - [`state`]: the latest GMCP and MSDP values of a session, for scripts.

pub mod binding;
pub mod discovery;
pub mod format;
pub mod mapping;
pub mod msdp;
pub mod schema;
pub mod state;

use serde_json::Value;

/// Telnet option numbers of the protocols here.
pub const OPT_MSDP: u8 = 69;
pub const OPT_MSSP: u8 = 70;
pub const OPT_GMCP: u8 = 201;

/// One GMCP message.
#[derive(Clone, Debug, PartialEq)]
pub struct GmcpMessage {
    /// `Package.Subpackage.Message`, as sent.
    pub package: String,
    /// The JSON body, if there was one and it parsed.
    pub data: Option<Value>,
    /// Why the body did not parse, if it did not.
    pub error: Option<String>,
    /// The payload as received (for Diagnostics).
    pub raw: Vec<u8>,
}

impl GmcpMessage {
    /// Whether the package name matches `name` (GMCP package names are case-insensitive).
    pub fn is(&self, name: &str) -> bool {
        self.package.eq_ignore_ascii_case(name)
    }
}

/// Decode a GMCP payload. Text is UTF-8 (invalid bytes are replaced); an empty package name gives `None`.
pub fn parse_gmcp(payload: &[u8]) -> Option<GmcpMessage> {
    let text = String::from_utf8_lossy(payload);
    let text = text.trim();
    let (package, body) = match text.find(char::is_whitespace) {
        Some(i) => (&text[..i], text[i..].trim()),
        None => (text, ""),
    };
    if package.is_empty() {
        return None;
    }
    let (data, error) = if body.is_empty() {
        (None, None)
    } else {
        match serde_json::from_str::<Value>(body) {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e.to_string())),
        }
    };
    Some(GmcpMessage {
        package: package.to_string(),
        data,
        error,
        raw: payload.to_vec(),
    })
}

const MSSP_VAR: u8 = 1;
const MSSP_VAL: u8 = 2;
/// Variables kept from one table.
pub const MSSP_MAX_VARIABLES: usize = 200;
/// Values kept per variable.
pub const MSSP_MAX_VALUES: usize = 64;

/// A server's MSSP table: variables in the order first sent (with the server's spelling), each
/// with one or more values. Names compare without case.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MsspTable {
    pub entries: Vec<(String, Vec<String>)>,
}

impl MsspTable {
    /// Values of a variable (names are case-insensitive).
    pub fn get(&self, name: &str) -> Option<&[String]> {
        self.entries
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_slice())
    }

    /// The first value of a variable.
    pub fn first(&self, name: &str) -> Option<&str> {
        self.get(name).and_then(|v| v.first()).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn add(&mut self, name: String, value: String) {
        if let Some((_, values)) = self.entries.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(&name)) {
            if values.len() < MSSP_MAX_VALUES {
                values.push(value);
            }
        } else if self.entries.len() < MSSP_MAX_VARIABLES {
            self.entries.push((name, vec![value]));
        }
    }
}

/// Decode an MSSP payload (without the option byte), as the C# SDK does: names and values are
/// UTF-8 and trimmed; a variable sent twice, or with several values, collects them all; a
/// variable without a value or an empty name is skipped; bytes before the first variable are
/// dropped. A table with no usable variable is empty.
pub fn parse_mssp(payload: &[u8]) -> MsspTable {
    let mut table = MsspTable::default();
    let mut name = Vec::new();
    let mut value = Vec::new();
    let mut current: Option<String> = None;
    let mut reading = 0u8;
    let decode = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_string();
    for &b in payload {
        if b == MSSP_VAR || b == MSSP_VAL {
            if reading == MSSP_VAL
                && let Some(n) = &current
            {
                table.add(n.clone(), decode(&value));
            }
            if b == MSSP_VAR {
                name.clear();
                current = None;
            } else if reading == MSSP_VAR {
                current = Some(decode(&name)).filter(|n| !n.is_empty());
            }
            value.clear();
            reading = b;
            continue;
        }
        match reading {
            MSSP_VAR => name.push(b),
            MSSP_VAL => value.push(b),
            _ => {}
        }
    }
    if reading == MSSP_VAL
        && let Some(n) = &current
    {
        table.add(n.clone(), decode(&value));
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gmcp_with_json_body() {
        let m = parse_gmcp(br#"Char.Vitals {"hp": 120, "maxhp": 200}"#).unwrap();
        assert_eq!(m.package, "Char.Vitals");
        assert!(m.is("char.vitals"));
        assert_eq!(m.data.unwrap()["hp"], 120);
        assert!(m.error.is_none());
    }

    #[test]
    fn gmcp_without_body_and_with_bad_json() {
        let m = parse_gmcp(b"Core.Goodbye").unwrap();
        assert_eq!(m.package, "Core.Goodbye");
        assert!(m.data.is_none() && m.error.is_none());
        let m = parse_gmcp(b"Room.Info {not json").unwrap();
        assert!(m.data.is_none());
        assert!(m.error.is_some());
        let m = parse_gmcp(b"Comm.Channel.Text \"plain string\"").unwrap();
        assert_eq!(m.data.unwrap(), "plain string");
        assert!(parse_gmcp(b"  ").is_none());
    }

    #[test]
    fn mssp_variables_and_multiple_values() {
        let mut p = vec![0x00, b'x', MSSP_VAR];
        p.extend_from_slice(b"NAME");
        p.push(MSSP_VAL);
        p.extend_from_slice(b"Bench World");
        p.push(MSSP_VAR);
        p.extend_from_slice(b"PORT");
        p.push(MSSP_VAL);
        p.extend_from_slice(b"4000");
        p.push(MSSP_VAL);
        p.extend_from_slice(b"4001");
        p.push(MSSP_VAR);
        p.extend_from_slice(b"EMPTY");
        let t = parse_mssp(&p);
        assert_eq!(t.first("name"), Some("Bench World"));
        assert_eq!(t.get("PORT").unwrap(), ["4000", "4001"]);
        // A variable without a value is not kept (the C# rule).
        assert!(t.get("EMPTY").is_none());
        assert_eq!(t.entries.len(), 2);
        assert!(parse_mssp(&[]).is_empty());
        // Repeated names collect values, names and values are trimmed, case is ignored.
        let t = parse_mssp(b"\x01 PORT \x02 1 \x01port\x022\x01\x02orphan");
        assert_eq!(
            t.entries,
            vec![("PORT".to_string(), vec!["1".to_string(), "2".to_string()])]
        );
    }
}
