//! Everything one session does with protocol data, behind its privacy rules (the C#
//! `WorkspaceController` Diagnostics, Protocol, Console and ScriptState parts):
//!
//! - Diagnostics keeps every GMCP, MSDP and MSSP message as a redacted copy, also during private
//!   input and login (masked: sensitive fields, remembered secrets, unparseable private data).
//! - The latest-value cache and the protocol mapping take server values gated by privacy alone,
//!   never by the login: a world reports a variable once, often while auto-login still runs, and
//!   the vitals must follow a login that lingers (a GMCP login the world never confirms). Login
//!   packages and anything carrying a remembered secret are refused.
//! - The console logs the text stream and sent commands; private or login stretches become one
//!   `[private]` marker.
//! - When a private or login stretch ends, the mapped MSDP variables are asked for again
//!   (`REPORT` and `SEND`), since their first report usually landed before it.
//!
//! The caller says what the session's privacy is when each piece arrives ([`Gate`]).

use std::time::SystemTime;

use super::console::{ConsoleKind, ConsoleLog};
use super::messages::{Messages, format_server_details};
use crate::protocol::binding::BindingEngine;
use crate::protocol::format::{self, Scrubber};
use crate::protocol::mapping::WorldMapping;
use crate::protocol::state::StateCache;
use crate::protocol::{GmcpMessage, MsspTable, OPT_GMCP, OPT_MSDP, OPT_MSSP, discovery};

/// Remembered secrets kept at once.
pub const MAX_SECRETS: usize = 8;

/// The session's privacy when something arrived.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gate {
    /// Input is private (server echo off, a password prompt, the padlock).
    pub private: bool,
    /// Auto-login owns the session.
    pub login: bool,
}

impl Gate {
    /// Scripts, channels and the agent get nothing.
    pub fn blocked(self) -> bool {
        self.private || self.login
    }
}

#[derive(Debug, Default)]
pub struct SessionProtocol {
    pub messages: Messages,
    pub console: ConsoleLog,
    pub cache: StateCache,
    bindings: Option<BindingEngine>,
    secrets: Vec<String>,
    pub gmcp_enabled: bool,
    pub msdp_enabled: bool,
    /// Ask for the mapped variables again when the gate allows it.
    refresh_pending: bool,
    was_blocked: bool,
    /// Subscription requests sent (tests).
    pub refreshes_sent: u64,
    /// MSDP variables scripts read before the world sent them, and those already asked for.
    script_reports: std::collections::BTreeSet<String>,
    script_reports_sent: std::collections::BTreeSet<String>,
    reports_blocked: bool,
}

impl SessionProtocol {
    /// A session with the world's mapping (when it has a valid one).
    pub fn new(mapping: Option<WorldMapping>) -> Self {
        Self {
            bindings: mapping.and_then(BindingEngine::new),
            ..Self::default()
        }
    }

    pub fn bindings(&self) -> Option<&BindingEngine> {
        self.bindings.as_ref()
    }

    pub fn mapping(&self) -> Option<&WorldMapping> {
        self.bindings.as_ref().map(BindingEngine::mapping)
    }

    /// A refreshed mapping (the directory reloaded). Observations survive an additive change;
    /// when the bindings changed, the mapped variables are asked for again.
    pub fn set_mapping(&mut self, mapping: WorldMapping) {
        match &mut self.bindings {
            Some(engine) => {
                if engine.update_mapping(mapping) {
                    self.refresh_pending = true;
                }
            }
            None => {
                self.bindings = BindingEngine::new(mapping);
                self.refresh_pending = self.bindings.is_some();
            }
        }
    }

    /// A new connection: nothing from the previous one stays.
    pub fn connected(&mut self) {
        self.messages.clear();
        self.messages.reset_server_details();
        self.console.clear();
        self.cache.clear();
        self.secrets.clear();
        if let Some(engine) = &mut self.bindings {
            engine.reset();
        }
        self.gmcp_enabled = false;
        self.msdp_enabled = false;
        self.refresh_pending = false;
        self.was_blocked = false;
        self.script_reports.clear();
        self.script_reports_sent.clear();
        self.reports_blocked = false;
    }

    /// Whether `text` carries a remembered secret (it must not reach scripts or the cache).
    pub fn carries_secret_text(&self, text: &str) -> bool {
        self.carries_secret(text)
    }

    /// A script read an MSDP variable the world has not sent: ask for it once this connection
    /// (see [`take_script_reports`](Self::take_script_reports)).
    pub fn request_script_report(&mut self, name: &str) {
        if crate::protocol::msdp::valid_name(name) && self.script_reports.len() < 4096 {
            self.script_reports.insert(name.to_string());
        }
    }

    /// `REPORT` and `SEND` for the variables scripts asked for and the world was not yet asked
    /// for, unless the world's mapping already reports them (the C# `RefreshScriptReportsAsync`):
    /// only while connected, public and with MSDP on. When a private or login stretch ends, every
    /// variable is asked for again, since the answer that ended it was withheld.
    pub fn take_script_reports(&mut self, gate: Gate, connected: bool) -> Vec<(u8, Vec<u8>)> {
        let blocked = gate.blocked();
        if self.reports_blocked && !blocked {
            self.script_reports_sent.clear();
        }
        self.reports_blocked = blocked;
        if blocked || !connected || !self.msdp_enabled || self.script_reports.is_empty() {
            return Vec::new();
        }
        let mapped = self.mapping().map(|m| m.msdp_names("MSDP")).unwrap_or_default();
        let names: Vec<String> = self
            .script_reports
            .iter()
            .filter(|n| !self.script_reports_sent.contains(*n) && !mapped.contains(n))
            .cloned()
            .collect();
        if names.is_empty() {
            return Vec::new();
        }
        self.script_reports_sent.extend(names.iter().cloned());
        let mut requests = Vec::new();
        for batch in names.chunks(32) {
            for command in ["REPORT", "SEND"] {
                requests.push((OPT_MSDP, discovery::request(OPT_MSDP, command, batch)));
            }
        }
        requests
    }

    /// Remember a secret (a saved password, a typed private input) for this connection, so it is
    /// masked wherever it comes back.
    pub fn remember_secret(&mut self, value: &str) {
        let length = value.encode_utf16().count();
        if (1..=8192).contains(&length) && !self.secrets.iter().any(|s| s == value) {
            self.secrets.insert(0, value.to_string());
            self.secrets.truncate(MAX_SECRETS);
        }
    }

    pub fn secrets(&self) -> &[String] {
        &self.secrets
    }

    fn carries_secret(&self, text: &str) -> bool {
        Scrubber::new(&self.secrets).finds(text)
    }

    /// Text the transcript received.
    pub fn receive_text(&mut self, text: &str, gate: Gate) {
        self.console
            .append(ConsoleKind::Received, text, gate.blocked(), &self.secrets);
    }

    /// A command sent (`hidden`: it was private input).
    pub fn sent(&mut self, command: &str, hidden: bool, gate: Gate) {
        self.console
            .append(ConsoleKind::Sent, command, hidden || gate.login, &self.secrets);
    }

    /// A GMCP message.
    pub fn receive_gmcp(&mut self, message: &GmcpMessage, gate: Gate, now: SystemTime) {
        let login_package = crate::login::gmcp::is_private(&message.package);
        let content = format::format(OPT_GMCP, &message.raw, gate.blocked(), &self.secrets);
        let public = !gate.private && !login_package;
        if public && !self.carries_secret(&String::from_utf8_lossy(&message.raw)) {
            self.cache.record_gmcp(String::from_utf8_lossy(&message.raw).trim());
        }
        if public && let Some(engine) = &mut self.bindings {
            engine.observe(OPT_GMCP, &content, now);
        }
        self.messages.append_content(now, OPT_GMCP, Some(content));
    }

    /// An MSDP payload.
    pub fn receive_msdp(&mut self, payload: &[u8], gate: Gate, now: SystemTime) {
        let content = format::format(OPT_MSDP, payload, gate.blocked(), &self.secrets);
        if !gate.private {
            if !self.carries_secret(&String::from_utf8_lossy(payload)) {
                self.cache.record_msdp(payload);
            }
            if let Some(engine) = &mut self.bindings {
                engine.observe(OPT_MSDP, &content, now);
            }
        }
        self.messages.append_content(now, OPT_MSDP, Some(content));
    }

    /// The server's MSSP table. One from a private stretch is skipped entirely (not listed,
    /// shown or used). Returns whether it was kept.
    pub fn receive_mssp(&mut self, table: &MsspTable, gate: Gate, now: SystemTime) -> bool {
        if gate.private {
            return false;
        }
        let (content, details) = format_server_details(table, &self.secrets);
        self.messages.append_content(now, OPT_MSSP, Some(content));
        self.messages.set_server_details(table.clone(), details);
        true
    }

    /// The character name the world reports through the mapping, if any (it wins over the saved
    /// username, as in C#), cut to 100 characters.
    pub fn reported_character(&self) -> Option<String> {
        let name = self.bindings.as_ref()?.character().identity.get("name")?.value.trim();
        (!name.is_empty()).then(|| name.chars().take(100).collect())
    }

    /// The subscription requests to send now, as (option, payload) subnegotiations: GMCP
    /// `Core.Supports.Set` and `REPORT` and `SEND` for the mapped MSDP variables, once a private
    /// or login stretch has ended (or the mapping changed). Empty when nothing is due or the gate
    /// or the connection does not allow it yet.
    pub fn take_refresh(&mut self, gate: Gate, connected: bool) -> Vec<(u8, Vec<u8>)> {
        let blocked = gate.blocked();
        if self.was_blocked && !blocked && self.bindings.is_some() {
            self.refresh_pending = true;
        }
        self.was_blocked = blocked;
        if !self.refresh_pending || blocked || !connected || !(self.gmcp_enabled || self.msdp_enabled) {
            return Vec::new();
        }
        self.refresh_pending = false;
        let mut requests = Vec::new();
        if self.gmcp_enabled {
            requests.push((OPT_GMCP, discovery::GMCP_SUPPORTS.as_bytes().to_vec()));
        }
        if let Some(mapping) = self.mapping() {
            for (option, protocol, enabled) in [
                (OPT_MSDP, "MSDP", self.msdp_enabled),
                (OPT_GMCP, "GMCP", self.gmcp_enabled),
            ] {
                if !enabled {
                    continue;
                }
                for batch in mapping.msdp_names(protocol).chunks(32) {
                    for command in ["REPORT", "SEND"] {
                        requests.push((option, discovery::request(option, command, batch)));
                    }
                }
            }
        }
        self.refreshes_sent += 1;
        requests
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::mapping::tests::{bind, mapping};
    use crate::protocol::parse_gmcp;

    fn native(
        path: &str,
        category: &str,
        key: &str,
        member: &str,
        entity: &str,
        conversion: &str,
    ) -> crate::protocol::mapping::FieldBinding {
        let mut b = bind(path, category, key, member, entity, conversion);
        b.source.protocol = "MSDP".into();
        b.source.package = "MSDP".into();
        b
    }

    /// The shape of the Legends of the Jedi mapping in `MappedVitalsLiveTests`.
    fn jedi() -> WorldMapping {
        mapping(vec![
            native("/HEALTH", "resource", "health", "current", "character", "number"),
            native("/HEALTHMAX", "resource", "health", "maximum", "character", "number"),
            native("/MOVEMENT", "resource", "movement", "current", "character", "number"),
            native("/MOVEMENTMAX", "resource", "movement", "maximum", "character", "number"),
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
            native("/CHARACTERNAME", "identity", "name", "value", "character", "text"),
        ])
    }

    fn msdp(p: &mut SessionProtocol, variable: &str, value: &str, gate: Gate) {
        p.receive_msdp(format!("\x01{variable}\x02{value}").as_bytes(), gate, SystemTime::now());
    }

    const PUBLIC: Gate = Gate {
        private: false,
        login: false,
    };
    const LOGIN: Gate = Gate {
        private: false,
        login: true,
    };
    const PRIVATE: Gate = Gate {
        private: true,
        login: false,
    };

    /// The C# `RefreshScriptReportsAsync`: a variable a script read is asked for once, never one
    /// the mapping reports, only while public with MSDP on, and again after a private stretch.
    #[test]
    fn script_reports_are_asked_once_and_again_after_privacy() {
        let mut p = SessionProtocol::new(Some(jedi()));
        p.request_script_report("LEVEL");
        p.request_script_report("HEALTH");
        p.request_script_report("1BAD");
        assert!(p.take_script_reports(PUBLIC, true).is_empty(), "MSDP is not on yet");
        p.msdp_enabled = true;
        assert!(p.take_script_reports(PRIVATE, true).is_empty());
        let requests = p.take_script_reports(PUBLIC, true);
        assert_eq!(
            requests,
            [
                (OPT_MSDP, b"\x01REPORT\x02LEVEL".to_vec()),
                (OPT_MSDP, b"\x01SEND\x02LEVEL".to_vec())
            ]
        );
        assert!(p.take_script_reports(PUBLIC, true).is_empty(), "once");
        assert!(p.take_script_reports(LOGIN, true).is_empty());
        assert_eq!(p.take_script_reports(PUBLIC, true).len(), 2, "again after the login");
        p.connected();
        assert!(
            p.take_script_reports(PUBLIC, true).is_empty(),
            "a new connection starts empty"
        );
    }

    /// `MappedVitalsFollowTheWorldWhileAGmcpLoginLingers`: a GMCP login answered but never
    /// confirmed keeps the login open (public, not private); mapped MSDP keeps arriving, and both
    /// the cache and the game state must follow it.
    #[test]
    fn mapped_vitals_follow_the_world_while_a_gmcp_login_lingers() {
        let mut p = SessionProtocol::new(Some(jedi()));
        p.connected();
        let offer = parse_gmcp(br#"Char.Login.Default {"type":["password-credentials"],"version":1}"#).unwrap();
        p.receive_gmcp(&offer, LOGIN, SystemTime::now());
        for (v, value) in [
            ("HEALTH", "980"),
            ("HEALTHMAX", "1000"),
            ("MOVEMENT", "1018"),
            ("MOVEMENTMAX", "1040"),
            ("OPPONENTNAME", "A Vicious Womprat"),
            ("OPPONENTHEALTHMAX", "100"),
            ("OPPONENTHEALTH", "30"),
        ] {
            msdp(&mut p, v, value, LOGIN);
        }
        assert_eq!(p.cache.msdp("HEALTH"), Some("\"980\""));
        assert_eq!(p.cache.msdp("OPPONENTHEALTH"), Some("\"30\""));
        let engine = p.bindings().unwrap();
        assert_eq!(
            engine.character().resources["health"].current.as_ref().unwrap().value,
            980.0
        );
        assert_eq!(
            engine.character().resources["movement"].current.as_ref().unwrap().value,
            1018.0
        );
        assert_eq!(
            engine.opponent().resources["health"].current.as_ref().unwrap().value,
            30.0
        );
        assert_eq!(engine.opponent().identity["name"].value, "A Vicious Womprat");
        // The login offer is in Diagnostics but nowhere else.
        assert!(p.cache.gmcp("Char.Login.Default").is_none());
        assert_eq!(p.messages.len(), 8);
    }

    #[test]
    fn private_stretches_reach_diagnostics_only_and_secrets_are_refused_and_masked() {
        let mut p = SessionProtocol::new(Some(jedi()));
        p.connected();
        msdp(&mut p, "HEALTH", "500", PUBLIC);
        msdp(&mut p, "HEALTH", "5", PRIVATE);
        assert_eq!(p.cache.msdp("HEALTH"), Some("\"500\""));
        assert_eq!(
            p.bindings().unwrap().character().resources["health"]
                .current
                .as_ref()
                .unwrap()
                .value,
            500.0
        );
        assert_eq!(p.messages.len(), 2);
        p.remember_secret("hunter2");
        msdp(&mut p, "NOTE", "hunter2 is your password", PUBLIC);
        assert!(p.cache.msdp("NOTE").is_none());
        assert!(
            !p.messages
                .entries()
                .last()
                .unwrap()
                .content
                .as_ref()
                .unwrap()
                .body
                .contains("hunter2")
        );
        p.receive_text("You said hunter2.\r\n", PUBLIC);
        p.receive_text("Password: ", PRIVATE);
        p.sent("hunter2", true, PRIVATE);
        p.sent("look", false, PUBLIC);
        let rendered = p.console.render(0);
        assert!(!rendered.contains("hunter2"));
        assert!(rendered.contains("You said [redacted]."));
        assert_eq!(rendered.matches("[private]").count(), 1);
        assert!(rendered.contains(">> look"));
        // MSSP from a private stretch is not kept.
        let table = crate::protocol::parse_mssp(b"\x01NAME\x02Hidden");
        assert!(!p.receive_mssp(&table, PRIVATE, SystemTime::now()));
        assert!(!p.messages.has_server_details());
        assert!(p.receive_mssp(
            &crate::protocol::parse_mssp(b"\x01NAME\x02Shown"),
            PUBLIC,
            SystemTime::now()
        ));
        assert_eq!(p.messages.server_details(), "NAME: Shown");
        // A new connection forgets everything, secrets too.
        p.connected();
        assert!(p.messages.is_empty() && p.console.is_empty() && p.cache.is_empty() && p.secrets().is_empty());
        assert!(!p.messages.has_server_details());
    }

    #[test]
    fn the_mapped_variables_are_asked_for_again_once_login_ends() {
        let mut p = SessionProtocol::new(Some(jedi()));
        p.connected();
        p.msdp_enabled = true;
        assert!(p.take_refresh(LOGIN, true).is_empty());
        assert!(p.take_refresh(LOGIN, true).is_empty());
        let requests = p.take_refresh(PUBLIC, true);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].0, OPT_MSDP);
        assert!(requests[0].1.starts_with(b"\x01REPORT\x02HEALTH\x02HEALTHMAX"));
        assert!(requests[1].1.starts_with(b"\x01SEND\x02HEALTH"));
        // Once only.
        assert!(p.take_refresh(PUBLIC, true).is_empty());
        // A privacy blip asks again; with GMCP on, the module list goes first.
        p.gmcp_enabled = true;
        assert!(p.take_refresh(PRIVATE, true).is_empty());
        let requests = p.take_refresh(PUBLIC, true);
        assert_eq!(requests[0], (OPT_GMCP, discovery::GMCP_SUPPORTS.as_bytes().to_vec()));
        assert_eq!(requests.len(), 3);
        // Without a mapping nothing is asked for.
        let mut plain = SessionProtocol::new(None);
        plain.msdp_enabled = true;
        plain.take_refresh(LOGIN, true);
        assert!(plain.take_refresh(PUBLIC, true).is_empty());
    }

    #[test]
    fn the_reported_character_wins_and_a_new_name_is_a_new_lifetime() {
        let mut p = SessionProtocol::new(Some(jedi()));
        p.connected();
        assert_eq!(p.reported_character(), None);
        msdp(&mut p, "CHARACTERNAME", "Tester", PUBLIC);
        msdp(&mut p, "HEALTH", "10", PUBLIC);
        assert_eq!(p.reported_character().as_deref(), Some("Tester"));
        msdp(&mut p, "CHARACTERNAME", "Other", PUBLIC);
        assert_eq!(p.reported_character().as_deref(), Some("Other"));
        assert!(p.bindings().unwrap().character().resources.is_empty());
    }

    #[test]
    fn remembered_secrets_are_bounded_and_newest_first() {
        let mut p = SessionProtocol::default();
        for i in 0..10 {
            p.remember_secret(&format!("secret-{i}"));
        }
        p.remember_secret("secret-9");
        p.remember_secret("");
        assert_eq!(p.secrets().len(), MAX_SECRETS);
        assert_eq!(p.secrets()[0], "secret-9");
    }
}
