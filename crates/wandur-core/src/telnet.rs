//! Telnet byte stream handling: separates data from IAC commands, answers option negotiation and
//! reports what the session needs to know (server echo, prompt marks, window size requests,
//! subnegotiations). Pure over bytes: no I/O, no clock.
//!
//! Policy (the C# SDK's, with the Rust client's own name):
//! - accept the server's ECHO, SGA and EOR, and GMCP, MSDP and MSSP when the config allows them;
//! - agree to SGA, TTYPE (answering with the MTTS cycle: client name, terminal type, `MTTS n`)
//!   and NAWS (the session sends the size, see [`naws`]);
//! - on WILL GMCP, answer DO and send `Core.Hello`, `Core.Supports.Set` and the MSDP variable
//!   list request; on WILL MSDP, answer DO and send `LIST REPORTABLE_VARIABLES`;
//! - answer a reportable variable list (native or through GMCP) with `REPORT` requests
//!   ([`crate::protocol::discovery`]);
//! - refuse everything else once.
//!
//! Replies are only sent when an option's state changes, so a server that repeats itself cannot
//! start a negotiation loop. Subnegotiations are collected (bounded) and reported; GMCP and MSSP
//! payloads are decoded by [`crate::protocol`].

pub const IAC: u8 = 255;
pub const DONT: u8 = 254;
pub const DO: u8 = 253;
pub const WONT: u8 = 252;
pub const WILL: u8 = 251;
pub const SB: u8 = 250;
pub const GA: u8 = 249;
pub const SE: u8 = 240;
pub const EOR_CMD: u8 = 239;

pub const OPT_ECHO: u8 = 1;
pub const OPT_SGA: u8 = 3;
pub const OPT_TTYPE: u8 = 24;
pub const OPT_EOR: u8 = 25;
pub const OPT_NAWS: u8 = 31;
pub const OPT_MSDP: u8 = 69;
pub const OPT_MSSP: u8 = 70;
pub const OPT_GMCP: u8 = 201;

const TTYPE_IS: u8 = 0;
const TTYPE_SEND: u8 = 1;

/// MTTS capability bits (<https://tintin.mudhalla.net/protocols/mtts/>).
pub mod mtts {
    pub const ANSI: u32 = 1;
    pub const VT100: u32 = 2;
    pub const UTF8: u32 = 4;
    pub const COLORS_256: u32 = 8;
    pub const TRUECOLOR: u32 = 256;
    pub const SSL: u32 = 2048;
}

/// What the client tells the server about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TelnetConfig {
    /// First TTYPE answer.
    pub client_name: String,
    /// Second TTYPE answer.
    pub terminal_type: String,
    /// Third and later TTYPE answers, as `MTTS n`.
    pub mtts: u32,
    pub accept_gmcp: bool,
    pub accept_msdp: bool,
    pub accept_mssp: bool,
    /// GMCP messages sent after the server enables GMCP.
    pub gmcp_hello: Vec<String>,
}

impl Default for TelnetConfig {
    fn default() -> Self {
        Self {
            client_name: "Wandur-Rust".into(),
            terminal_type: "XTERM-256COLOR".into(),
            mtts: mtts::ANSI | mtts::VT100 | mtts::COLORS_256 | mtts::TRUECOLOR,
            accept_gmcp: true,
            accept_msdp: true,
            accept_mssp: true,
            gmcp_hello: vec![
                format!(
                    r#"Core.Hello {{"client":"Wandur Rust prototype","version":"{}"}}"#,
                    env!("CARGO_PKG_VERSION")
                ),
                crate::protocol::discovery::GMCP_SUPPORTS.into(),
                crate::protocol::discovery::GMCP_DISCOVERY.into(),
            ],
        }
    }
}

/// Longest subnegotiation payload kept; longer ones are dropped whole.
pub const MAX_SUBNEGOTIATION: usize = 64 * 1024;

/// Something the session should act on, in stream order relative to the data before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TelnetEvent {
    /// The server will (true) or will not (false) echo: true means input is private (a password).
    ServerEcho(bool),
    /// IAC GA or IAC EOR: the text so far is a prompt.
    PromptMark,
    /// A complete subnegotiation (`IAC SB option payload IAC SE`), IAC IAC already unescaped.
    /// TTYPE requests are answered here and not reported.
    Subnegotiation { option: u8, payload: Vec<u8> },
    /// The server asked for (true) or no longer wants (false) the window size: send [`naws`].
    WindowSizeWanted(bool),
    /// The server enabled GMCP (the hello is already in the replies).
    GmcpEnabled,
    /// The server enabled MSDP (the variable list request is already in the replies).
    MsdpEnabled,
    /// The server declined or turned off GMCP (`WONT`).
    GmcpDisabled,
    /// The server declined or turned off MSDP (`WONT`).
    MsdpDisabled,
}

/// Output of one [`TelnetParser::feed`] call. Reused between calls to avoid allocation.
#[derive(Debug, Default)]
pub struct TelnetOutput {
    /// Data bytes (text), IAC IAC unescaped.
    pub data: Vec<u8>,
    /// Bytes to send back to the server, in order.
    pub replies: Vec<u8>,
    /// Events, each with the length of `data` at the moment it happened.
    pub events: Vec<(usize, TelnetEvent)>,
}

impl TelnetOutput {
    pub fn clear(&mut self) {
        self.data.clear();
        self.replies.clear();
        self.events.clear();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Data,
    Iac,
    Verb(u8),
    Sub,
    SubOption,
    SubIac,
}

/// Per option: whether the server side (WILL) and our side (DO) are enabled.
#[derive(Clone, Copy, Debug, Default)]
struct OptionState {
    remote: bool,
    local: bool,
    /// Whether we already answered this option at least once (so a refusal is sent only once).
    remote_answered: bool,
    local_answered: bool,
}

#[derive(Debug)]
pub struct TelnetParser {
    state: State,
    options: [OptionState; 256],
    sub_option: u8,
    sub: Vec<u8>,
    sub_overflow: bool,
    config: TelnetConfig,
    /// TTYPE answers given in the current cycle (0 to 3).
    ttype_sent: u8,
    /// MSDP variables already asked for, per transport.
    discovery: crate::protocol::discovery::Discovery,
}

impl Default for TelnetParser {
    fn default() -> Self {
        Self::new()
    }
}

impl TelnetParser {
    pub fn new() -> Self {
        Self::with_config(TelnetConfig::default())
    }

    pub fn with_config(config: TelnetConfig) -> Self {
        Self {
            state: State::Data,
            options: [OptionState::default(); 256],
            sub_option: 0,
            sub: Vec::new(),
            sub_overflow: false,
            config,
            ttype_sent: 0,
            discovery: Default::default(),
        }
    }

    /// Whether the server agreed to receive the window size.
    pub fn naws_enabled(&self) -> bool {
        self.options[OPT_NAWS as usize].local
    }

    pub fn gmcp_enabled(&self) -> bool {
        self.options[OPT_GMCP as usize].remote
    }

    pub fn msdp_enabled(&self) -> bool {
        self.options[OPT_MSDP as usize].remote
    }

    /// Whether the server currently echoes (private input).
    pub fn server_echo(&self) -> bool {
        self.options[OPT_ECHO as usize].remote
    }

    /// Process received bytes, appending to `out`.
    pub fn feed(&mut self, bytes: &[u8], out: &mut TelnetOutput) {
        let mut i = 0;
        while i < bytes.len() {
            if self.state == State::Data {
                // Fast path: copy the run of plain data up to the next IAC.
                let rest = &bytes[i..];
                let n = rest.iter().position(|&b| b == IAC).unwrap_or(rest.len());
                out.data.extend_from_slice(&rest[..n]);
                i += n;
                if i < bytes.len() {
                    self.state = State::Iac;
                    i += 1;
                }
                continue;
            }
            let b = bytes[i];
            i += 1;
            self.state = match self.state {
                State::Data => unreachable!(),
                State::Iac => match b {
                    IAC => {
                        out.data.push(IAC);
                        State::Data
                    }
                    WILL | WONT | DO | DONT => State::Verb(b),
                    SB => State::SubOption,
                    GA | EOR_CMD => {
                        out.events.push((out.data.len(), TelnetEvent::PromptMark));
                        State::Data
                    }
                    // NOP, DM, AYT and the rest carry nothing for us.
                    _ => State::Data,
                },
                State::Verb(verb) => {
                    self.negotiate(verb, b, out);
                    State::Data
                }
                State::SubOption => {
                    self.sub_option = b;
                    self.sub.clear();
                    self.sub_overflow = false;
                    State::Sub
                }
                State::Sub => {
                    if b == IAC {
                        State::SubIac
                    } else {
                        self.push_sub(b);
                        State::Sub
                    }
                }
                State::SubIac => match b {
                    IAC => {
                        self.push_sub(IAC);
                        State::Sub
                    }
                    SE => {
                        if self.sub_option == OPT_TTYPE {
                            self.answer_ttype(out);
                        } else if !self.sub_overflow {
                            self.discover(out);
                            out.events.push((
                                out.data.len(),
                                TelnetEvent::Subnegotiation {
                                    option: self.sub_option,
                                    payload: std::mem::take(&mut self.sub),
                                },
                            ));
                        }
                        self.sub.clear();
                        State::Data
                    }
                    // Malformed: IAC followed by something else inside SB. Leave the
                    // subnegotiation and treat it as a command, as most clients do.
                    _ => {
                        self.sub.clear();
                        i -= 1;
                        State::Iac
                    }
                },
            };
        }
    }

    fn push_sub(&mut self, b: u8) {
        if self.sub.len() < MAX_SUBNEGOTIATION {
            self.sub.push(b);
        } else {
            self.sub_overflow = true;
        }
    }

    /// A reportable variable list (native MSDP, or MSDP through GMCP) is answered with REPORT
    /// requests for every valid name not asked for yet.
    fn discover(&mut self, out: &mut TelnetOutput) {
        let option = self.sub_option;
        if !matches!(option, OPT_MSDP | OPT_GMCP) || !self.options[option as usize].remote {
            return;
        }
        for request in self.discovery.receive(option, &self.sub) {
            subnegotiation(option, &request, &mut out.replies);
        }
    }

    /// Answer `IAC SB TTYPE SEND IAC SE` with the next name of the MTTS cycle.
    fn answer_ttype(&mut self, out: &mut TelnetOutput) {
        if !self.options[OPT_TTYPE as usize].local || self.sub.first() != Some(&TTYPE_SEND) {
            return;
        }
        self.ttype_sent = (self.ttype_sent + 1).min(3);
        let name = match self.ttype_sent {
            1 => self.config.client_name.clone(),
            2 => self.config.terminal_type.clone(),
            _ => format!("MTTS {}", self.config.mtts),
        };
        let mut payload = vec![TTYPE_IS];
        payload.extend_from_slice(name.as_bytes());
        subnegotiation(OPT_TTYPE, &payload, &mut out.replies);
    }

    fn negotiate(&mut self, verb: u8, option: u8, out: &mut TelnetOutput) {
        let config = &self.config;
        let state = &mut self.options[option as usize];
        match verb {
            WILL => {
                let accept = match option {
                    OPT_ECHO | OPT_SGA | OPT_EOR => true,
                    OPT_GMCP => config.accept_gmcp,
                    OPT_MSDP => config.accept_msdp,
                    OPT_MSSP => config.accept_mssp,
                    _ => false,
                };
                if accept {
                    if !state.remote {
                        state.remote = true;
                        state.remote_answered = true;
                        out.replies.extend_from_slice(&[IAC, DO, option]);
                        if option == OPT_ECHO {
                            out.events.push((out.data.len(), TelnetEvent::ServerEcho(true)));
                        }
                        if option == OPT_GMCP {
                            for message in &config.gmcp_hello {
                                subnegotiation(OPT_GMCP, message.as_bytes(), &mut out.replies);
                            }
                            out.events.push((out.data.len(), TelnetEvent::GmcpEnabled));
                        }
                        if option == OPT_MSDP {
                            subnegotiation(OPT_MSDP, &crate::protocol::msdp::list_reportable(), &mut out.replies);
                            out.events.push((out.data.len(), TelnetEvent::MsdpEnabled));
                        }
                    }
                } else if !state.remote_answered {
                    state.remote_answered = true;
                    out.replies.extend_from_slice(&[IAC, DONT, option]);
                }
            }
            WONT => {
                if matches!(option, OPT_MSDP | OPT_GMCP) {
                    // Turned on again later: subscribe again.
                    self.discovery.reset(option);
                    let event = if option == OPT_GMCP {
                        TelnetEvent::GmcpDisabled
                    } else {
                        TelnetEvent::MsdpDisabled
                    };
                    out.events.push((out.data.len(), event));
                }
                let state = &mut self.options[option as usize];
                if state.remote {
                    state.remote = false;
                    out.replies.extend_from_slice(&[IAC, DONT, option]);
                    if option == OPT_ECHO {
                        out.events.push((out.data.len(), TelnetEvent::ServerEcho(false)));
                    }
                }
                state.remote_answered = true;
            }
            DO => {
                let accept = matches!(option, OPT_SGA | OPT_TTYPE | OPT_NAWS);
                if accept {
                    if !state.local {
                        state.local = true;
                        state.local_answered = true;
                        out.replies.extend_from_slice(&[IAC, WILL, option]);
                        if option == OPT_NAWS {
                            out.events.push((out.data.len(), TelnetEvent::WindowSizeWanted(true)));
                        }
                    }
                } else if !state.local_answered {
                    state.local_answered = true;
                    out.replies.extend_from_slice(&[IAC, WONT, option]);
                }
            }
            DONT => {
                if state.local {
                    state.local = false;
                    out.replies.extend_from_slice(&[IAC, WONT, option]);
                    if option == OPT_NAWS {
                        out.events.push((out.data.len(), TelnetEvent::WindowSizeWanted(false)));
                    }
                }
                state.local_answered = true;
                if option == OPT_TTYPE {
                    // MTTS: DONT TTYPE restarts the cycle.
                    self.ttype_sent = 0;
                }
            }
            _ => {}
        }
    }
}

/// Append `IAC SB option payload IAC SE`, doubling IAC bytes in the payload.
pub fn subnegotiation(option: u8, payload: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&[IAC, SB, option]);
    for &b in payload {
        if b == IAC {
            out.push(IAC);
        }
        out.push(b);
    }
    out.extend_from_slice(&[IAC, SE]);
}

/// The NAWS subnegotiation for a window of `columns` x `rows`.
pub fn naws(columns: u16, rows: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(9);
    let [c1, c0] = columns.to_be_bytes();
    let [r1, r0] = rows.to_be_bytes();
    subnegotiation(OPT_NAWS, &[c1, c0, r1, r0], &mut out);
    out
}

/// A GMCP message (`Package.Name` and optional JSON) as a subnegotiation.
pub fn gmcp(message: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(message.len() + 5);
    subnegotiation(OPT_GMCP, message.as_bytes(), &mut out);
    out
}

/// Encode a command line for sending: UTF-8, IAC doubled, CR LF appended.
pub fn encode_line(line: &str, out: &mut Vec<u8>) {
    out.reserve(line.len() + 2);
    for &b in line.as_bytes() {
        if b == IAC {
            out.push(IAC);
        }
        out.push(b);
    }
    out.extend_from_slice(b"\r\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(chunks: &[&[u8]]) -> (TelnetParser, TelnetOutput) {
        let mut p = TelnetParser::new();
        let mut out = TelnetOutput::default();
        for c in chunks {
            p.feed(c, &mut out);
        }
        (p, out)
    }

    #[test]
    fn plain_data_passes_through() {
        let (_, out) = feed_all(&[b"hello\r\nworld"]);
        assert_eq!(out.data, b"hello\r\nworld");
        assert!(out.replies.is_empty() && out.events.is_empty());
    }

    #[test]
    fn escaped_iac_is_data() {
        let (_, out) = feed_all(&[&[b'a', IAC, IAC, b'b']]);
        assert_eq!(out.data, [b'a', 255, b'b']);
    }

    #[test]
    fn commands_are_removed_from_text_even_when_split() {
        let (_, out) = feed_all(&[b"ab", &[IAC], &[WILL], &[OPT_SGA, b'c', IAC], &[241, b'd']]);
        assert_eq!(out.data, b"abcd");
        assert_eq!(out.replies, [IAC, DO, OPT_SGA]);
    }

    #[test]
    fn server_echo_toggles_private_input() {
        let (p, out) = feed_all(&[&[IAC, WILL, OPT_ECHO], b"Password: "]);
        assert!(p.server_echo());
        assert_eq!(out.events[0], (0, TelnetEvent::ServerEcho(true)));
        assert_eq!(out.replies, [IAC, DO, OPT_ECHO]);
        let (p, out) = feed_all(&[&[IAC, WILL, OPT_ECHO], &[IAC, WONT, OPT_ECHO]]);
        assert!(!p.server_echo());
        assert_eq!(out.events[1].1, TelnetEvent::ServerEcho(false));
    }

    #[test]
    fn unknown_options_are_refused_once() {
        let (_, out) = feed_all(&[&[IAC, WILL, 99, IAC, WILL, 99, IAC, DO, 42, IAC, DO, 42]]);
        assert_eq!(out.replies, [IAC, DONT, 99, IAC, WONT, 42]);
    }

    #[test]
    fn gmcp_and_mssp_follow_the_config() {
        let mut p = TelnetParser::with_config(TelnetConfig {
            accept_gmcp: false,
            accept_mssp: false,
            ..TelnetConfig::default()
        });
        let mut out = TelnetOutput::default();
        p.feed(&[IAC, WILL, OPT_GMCP, IAC, WILL, OPT_MSSP], &mut out);
        assert_eq!(out.replies, [IAC, DONT, OPT_GMCP, IAC, DONT, OPT_MSSP]);
        assert!(!p.gmcp_enabled());
    }

    #[test]
    fn gmcp_is_accepted_with_hello_and_supports() {
        let (p, out) = feed_all(&[&[IAC, WILL, OPT_GMCP]]);
        assert!(p.gmcp_enabled());
        assert_eq!(&out.replies[..3], [IAC, DO, OPT_GMCP]);
        let config = TelnetConfig::default();
        let mut expected = vec![IAC, DO, OPT_GMCP];
        for m in &config.gmcp_hello {
            expected.extend(gmcp(m));
        }
        assert_eq!(out.replies, expected);
        assert!(String::from_utf8_lossy(&out.replies).contains("Core.Hello {"));
        assert_eq!(out.events, [(0, TelnetEvent::GmcpEnabled)]);
        // Said again: no second hello.
        let (_, out) = feed_all(&[&[IAC, WILL, OPT_GMCP, IAC, WILL, OPT_GMCP]]);
        assert_eq!(out.events.len(), 1);
    }

    #[test]
    fn declined_gmcp_and_msdp_are_reported() {
        // The map's status line tells "Declined" from "Not negotiated".
        let (_, out) = feed_all(&[&[IAC, WONT, OPT_GMCP, IAC, WONT, OPT_MSDP]]);
        assert_eq!(
            out.events,
            [(0, TelnetEvent::GmcpDisabled), (0, TelnetEvent::MsdpDisabled)]
        );
    }

    #[test]
    fn mssp_is_accepted_and_reported() {
        let mut bytes = vec![IAC, WILL, OPT_MSSP, IAC, SB, OPT_MSSP, 1];
        bytes.extend_from_slice(b"NAME");
        bytes.push(2);
        bytes.extend_from_slice(b"Bench");
        bytes.extend_from_slice(&[IAC, SE]);
        let (_, out) = feed_all(&[&bytes]);
        assert_eq!(out.replies, [IAC, DO, OPT_MSSP]);
        assert!(
            matches!(&out.events[0].1, TelnetEvent::Subnegotiation { option: OPT_MSSP, payload } if payload.starts_with(&[1]))
        );
    }

    fn frame(option: u8, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        subnegotiation(option, body, &mut out);
        out
    }

    #[test]
    fn msdp_is_accepted_and_lists_the_reportable_variables() {
        let (p, out) = feed_all(&[&[IAC, WILL, OPT_MSDP]]);
        assert!(p.msdp_enabled());
        let mut expected = vec![IAC, DO, OPT_MSDP];
        expected.extend(frame(OPT_MSDP, b"\x01LIST\x02REPORTABLE_VARIABLES"));
        assert_eq!(out.replies, expected);
        assert_eq!(out.events, [(0, TelnetEvent::MsdpEnabled)]);
        // Refused when the config says so.
        let mut p = TelnetParser::with_config(TelnetConfig {
            accept_msdp: false,
            ..TelnetConfig::default()
        });
        let mut out = TelnetOutput::default();
        p.feed(&[IAC, WILL, OPT_MSDP], &mut out);
        assert_eq!(out.replies, [IAC, DONT, OPT_MSDP]);
    }

    #[test]
    fn the_gmcp_hello_asks_for_modules_and_the_msdp_list() {
        let (_, out) = feed_all(&[&[IAC, WILL, OPT_GMCP]]);
        let text = String::from_utf8_lossy(&out.replies);
        assert!(text.contains(r#""Char.Vitals 1""#) && text.contains(r#""MSDP 1""#));
        assert!(text.contains(r#"MSDP {"LIST":"REPORTABLE_VARIABLES"}"#));
    }

    /// Port of `NativeReportsAcceptCustomFieldsAndRejectInvalidNames`.
    #[test]
    fn native_reports_accept_custom_fields_and_reject_invalid_names() {
        let mut p = TelnetParser::new();
        let mut out = TelnetOutput::default();
        p.feed(&[IAC, WILL, OPT_MSDP], &mut out);
        out.clear();
        p.feed(
            &frame(
                OPT_MSDP,
                b"\x01REPORTABLE_VARIABLES\x02\x05\x02ROOM\x02HEALTH\x02FORCE_POOL\x02ROOM_VNUM\x02HEALTH\x02bad name\x029BAD\x06",
            ),
            &mut out,
        );
        assert_eq!(
            out.replies,
            frame(OPT_MSDP, b"\x01REPORT\x02ROOM\x02HEALTH\x02FORCE_POOL\x02ROOM_VNUM")
        );
        // The list is still reported as a subnegotiation (Diagnostics shows it).
        assert!(matches!(
            out.events[0].1,
            TelnetEvent::Subnegotiation { option: OPT_MSDP, .. }
        ));
        out.clear();
        p.feed(&frame(OPT_MSDP, b"\x01REPORTABLE_VARIABLES\x02EXPERIENCE"), &mut out);
        assert_eq!(out.replies, frame(OPT_MSDP, b"\x01REPORT\x02EXPERIENCE"));
    }

    /// Port of `TunnelDiscoveryRetainsUnknownPackagesAndResetsAfterRenegotiation`.
    #[test]
    fn tunnel_discovery_retains_unknown_packages_and_resets_after_renegotiation() {
        let mut p = TelnetParser::new();
        let mut out = TelnetOutput::default();
        p.feed(&[IAC, WILL, OPT_GMCP], &mut out);
        let reports = frame(
            OPT_GMCP,
            br#"MSDP {"REPORTABLE_VARIABLES":["HEALTH","FORCE_POOL","HEALTH","bad name"]}"#,
        );
        out.clear();
        p.feed(&reports, &mut out);
        assert_eq!(
            out.replies,
            frame(OPT_GMCP, br#"MSDP {"REPORT":["HEALTH","FORCE_POOL"]}"#)
        );
        out.clear();
        p.feed(&reports, &mut out);
        assert!(out.replies.is_empty());
        p.feed(&frame(OPT_GMCP, br#"Jedi.Custom {"force":42}"#), &mut out);
        assert!(out.replies.is_empty());
        assert_eq!(out.events.len(), 2);
        out.clear();
        p.feed(&frame(OPT_GMCP, b"MSDP {bad json"), &mut out);
        assert!(out.replies.is_empty());
        p.feed(&[IAC, WONT, OPT_GMCP, IAC, WILL, OPT_GMCP], &mut out);
        out.clear();
        p.feed(&reports, &mut out);
        assert!(!out.replies.is_empty());
    }

    /// Port of `SubscriptionsAreBatchedBoundedAndIndependentBetweenTransports`.
    #[test]
    fn subscriptions_are_batched_bounded_and_independent_between_transports() {
        let mut p = TelnetParser::new();
        let mut out = TelnetOutput::default();
        p.feed(&[IAC, WILL, OPT_MSDP, IAC, WILL, OPT_GMCP], &mut out);
        let mut list = b"\x01REPORTABLE_VARIABLES\x02\x05".to_vec();
        for i in 0..256 {
            list.extend_from_slice(format!("\x02FIELD_{i}").as_bytes());
        }
        list.push(6);
        out.clear();
        p.feed(&frame(OPT_MSDP, &list), &mut out);
        assert_eq!(out.replies.iter().filter(|&&b| b == SB).count(), 8);
        assert!(String::from_utf8_lossy(&out.replies).contains("FIELD_255"));
        out.clear();
        p.feed(&frame(OPT_MSDP, b"\x01REPORTABLE_VARIABLES\x02ANOTHER_FIELD"), &mut out);
        assert!(out.replies.is_empty());
        p.feed(
            &frame(OPT_GMCP, br#"MSDP {"REPORTABLE_VARIABLES":"FIELD_0"}"#),
            &mut out,
        );
        assert!(!out.replies.is_empty());
    }

    #[test]
    fn naws_is_agreed_and_encoded() {
        let (p, out) = feed_all(&[&[IAC, DO, OPT_NAWS]]);
        assert!(p.naws_enabled());
        assert_eq!(out.replies, [IAC, WILL, OPT_NAWS]);
        assert_eq!(out.events, [(0, TelnetEvent::WindowSizeWanted(true))]);
        assert_eq!(naws(120, 40), [IAC, SB, OPT_NAWS, 0, 120, 0, 40, IAC, SE]);
        // A 255 byte is doubled.
        assert_eq!(naws(255, 256), [IAC, SB, OPT_NAWS, 0, 255, 255, 1, 0, IAC, SE]);
        let (p, out) = feed_all(&[&[IAC, DO, OPT_NAWS, IAC, DONT, OPT_NAWS]]);
        assert!(!p.naws_enabled());
        assert_eq!(out.events[1].1, TelnetEvent::WindowSizeWanted(false));
    }

    #[test]
    fn ttype_answers_the_mtts_cycle() {
        let send = [IAC, SB, OPT_TTYPE, TTYPE_SEND, IAC, SE];
        let mut p = TelnetParser::new();
        let mut out = TelnetOutput::default();
        // Not agreed yet: a SEND is ignored.
        p.feed(&send, &mut out);
        assert!(out.replies.is_empty());
        p.feed(&[IAC, DO, OPT_TTYPE], &mut out);
        assert_eq!(out.replies, [IAC, WILL, OPT_TTYPE]);
        let mut answers = Vec::new();
        for _ in 0..4 {
            out.clear();
            p.feed(&send, &mut out);
            assert_eq!(&out.replies[..4], [IAC, SB, OPT_TTYPE, TTYPE_IS]);
            answers.push(String::from_utf8(out.replies[4..out.replies.len() - 2].to_vec()).unwrap());
            assert!(out.events.is_empty(), "TTYPE is answered, not reported");
        }
        assert_eq!(answers, ["Wandur-Rust", "XTERM-256COLOR", "MTTS 267", "MTTS 267"]);
        // DONT TTYPE restarts the cycle.
        out.clear();
        p.feed(&[IAC, DONT, OPT_TTYPE, IAC, DO, OPT_TTYPE], &mut out);
        out.clear();
        p.feed(&send, &mut out);
        assert!(String::from_utf8_lossy(&out.replies).contains("Wandur-Rust"));
    }

    #[test]
    fn repeated_will_does_not_loop() {
        let (_, out) = feed_all(&[&[IAC, WILL, OPT_SGA, IAC, WILL, OPT_SGA]]);
        assert_eq!(out.replies, [IAC, DO, OPT_SGA]);
    }

    #[test]
    fn prompt_marks_keep_their_position() {
        let (_, out) = feed_all(&[b"<100hp> ", &[IAC, GA], b"more", &[IAC, EOR_CMD]]);
        assert_eq!(out.data, b"<100hp> more");
        assert_eq!(
            out.events,
            [(8, TelnetEvent::PromptMark), (12, TelnetEvent::PromptMark)]
        );
    }

    #[test]
    fn subnegotiation_is_collected_and_unescaped() {
        let mut bytes = vec![b'x', IAC, SB, 201];
        bytes.extend_from_slice(b"Core.Hello {}");
        bytes.extend_from_slice(&[IAC, IAC, IAC, SE, b'y']);
        let (_, out) = feed_all(&[&bytes[..5], &bytes[5..]]);
        assert_eq!(out.data, b"xy");
        let mut expected = b"Core.Hello {}".to_vec();
        expected.push(IAC);
        assert_eq!(
            out.events,
            [(
                1,
                TelnetEvent::Subnegotiation {
                    option: 201,
                    payload: expected
                }
            )]
        );
    }

    #[test]
    fn oversized_subnegotiation_is_dropped_without_corrupting_text() {
        let mut bytes = vec![IAC, SB, 69];
        bytes.extend(std::iter::repeat_n(b'z', MAX_SUBNEGOTIATION + 10));
        bytes.extend_from_slice(&[IAC, SE]);
        bytes.extend_from_slice(b"after");
        let (_, out) = feed_all(&[&bytes]);
        assert_eq!(out.data, b"after");
        assert!(out.events.is_empty());
    }

    #[test]
    fn malformed_iac_inside_subnegotiation_recovers() {
        let (_, out) = feed_all(&[&[IAC, SB, 99, b'a', IAC, WILL, OPT_SGA], b"text"]);
        assert_eq!(out.data, b"text");
        assert_eq!(out.replies, [IAC, DO, OPT_SGA]);
    }

    #[test]
    fn encode_line_doubles_iac() {
        let mut out = Vec::new();
        encode_line("say ÿ", &mut out);
        // 'ÿ' is U+00FF, encoded in UTF-8 as C3 BF: no IAC byte to double.
        assert_eq!(out, b"say \xC3\xBF\r\n");
        let mut raw = Vec::new();
        encode_line("a", &mut raw);
        assert_eq!(raw, b"a\r\n");
    }

    #[test]
    fn every_byte_value_survives_random_splits() {
        // Data with escaped IACs, split at every position, must come out identical.
        let mut wire = Vec::new();
        let mut expected = Vec::new();
        for b in 0u8..=255 {
            if b == IAC {
                wire.extend_from_slice(&[IAC, IAC]);
            } else {
                wire.push(b);
            }
            expected.push(b);
        }
        for split in 0..wire.len() {
            let (_, out) = feed_all(&[&wire[..split], &wire[split..]]);
            assert_eq!(out.data, expected, "split at {split}");
        }
    }
}
