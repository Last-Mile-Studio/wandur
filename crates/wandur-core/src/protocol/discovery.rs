//! Subscribing to what a world reports, without interpreting game-specific meaning (the C#
//! `ProtocolDiscovery`). When the world answers `LIST REPORTABLE_VARIABLES`, natively over MSDP
//! or through the GMCP `MSDP` package, every valid name is asked for with `REPORT`, in batches of
//! 32 and at most 256 per transport per negotiation. A transport that is turned off and on again
//! starts over.

use std::collections::HashSet;

use super::msdp;

/// The GMCP modules the client asks for: data modules and the password login it implements.
/// Media and web views are not promised.
pub const GMCP_SUPPORTS: &str = r#"Core.Supports.Set ["Room 1","Char 1","Char.Base 1","Char.Vitals 1","Char.Maxstats 1","Char.Status 1","Char.Login 1","Char.Skills 1","Char.Items 1","Char.Afflictions 1","Char.Defences 1","Group 1","Comm 1","Comm.Channel 1","MSDP 1"]"#;
/// MSDP's variable list, asked for through GMCP.
pub const GMCP_DISCOVERY: &str = r#"MSDP {"LIST":"REPORTABLE_VARIABLES"}"#;

const MAX_REPORTS: usize = 256;
const REPORTS_PER_BATCH: usize = 32;

/// The names already asked for, per transport.
#[derive(Debug, Default)]
pub struct Discovery {
    native: HashSet<String>,
    tunneled: HashSet<String>,
}

impl Discovery {
    /// The transport was turned off: ask again when it comes back.
    pub fn reset(&mut self, option: u8) {
        match option {
            super::OPT_MSDP => self.native.clear(),
            super::OPT_GMCP => self.tunneled.clear(),
            _ => {}
        }
    }

    /// The subscription requests (payloads for the same option) a received message calls for.
    pub fn receive(&mut self, option: u8, payload: &[u8]) -> Vec<Vec<u8>> {
        let names = reportable_names(option, payload);
        let seen = if option == super::OPT_MSDP {
            &mut self.native
        } else {
            &mut self.tunneled
        };
        let mut requested = Vec::new();
        for name in names {
            if seen.len() >= MAX_REPORTS {
                break;
            }
            if msdp::valid_name(&name) && seen.insert(name.clone()) {
                requested.push(name);
            }
        }
        requested
            .chunks(REPORTS_PER_BATCH)
            .map(|batch| request(option, "REPORT", batch))
            .collect()
    }
}

/// `REPORT` or `SEND` for some names, natively (`VAR REPORT VAL a VAL b`) or through GMCP
/// (`MSDP {"REPORT":["a","b"]}`).
pub fn request(option: u8, command: &str, names: &[String]) -> Vec<u8> {
    if option == super::OPT_MSDP {
        msdp::command(command, names)
    } else {
        let mut body = serde_json::Map::new();
        body.insert(command.to_string(), serde_json::json!(names));
        format!("MSDP {}", serde_json::Value::Object(body)).into_bytes()
    }
}

fn reportable_names(option: u8, payload: &[u8]) -> Vec<String> {
    if payload.len() > super::format::MAX_PAYLOAD_BYTES {
        return Vec::new();
    }
    if option == super::OPT_MSDP {
        let Some(table) = msdp::parse(payload) else {
            return Vec::new();
        };
        return match table.iter().find(|(n, _)| n == "REPORTABLE_VARIABLES").map(|(_, v)| v) {
            Some(msdp::MsdpValue::Array(items)) => items.iter().filter_map(|v| v.as_text().map(String::from)).collect(),
            Some(msdp::MsdpValue::Text(text)) => vec![text.clone()],
            _ => Vec::new(),
        };
    }
    let message = String::from_utf8_lossy(payload);
    let Some(split) = message.find([' ', '\t', '\r', '\n']) else {
        return Vec::new();
    };
    if &message[..split] != "MSDP" {
        return Vec::new();
    }
    let Ok(serde_json::Value::Object(root)) = serde_json::from_str::<serde_json::Value>(&message[split + 1..]) else {
        return Vec::new();
    };
    match root.get("REPORTABLE_VARIABLES") {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .take(MAX_REPORTS)
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        Some(serde_json::Value::String(text)) => vec![text.clone()],
        _ => Vec::new(),
    }
}
