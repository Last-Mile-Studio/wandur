//! GMCP `Char.Login` version 1 (password credentials only). The server offers the methods it
//! takes with `Char.Login.Default`; the client answers once with `Char.Login.Credentials`, either
//! the account and password or `{}` to hand control back to the server's text login; the server
//! reports `Char.Login.Result`. Decoding keeps only what the client acts on: never credentials,
//! tokens, links or server messages.

use serde_json::Value;

use crate::protocol::GmcpMessage;

/// Something the server said about login.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GmcpLogin {
    /// `Char.Login.Default`: whether version 1 password credentials are among the methods.
    Offer { password_supported: bool },
    /// `Char.Login.Result`.
    Result { success: bool },
}

/// Whether a GMCP package belongs to login (`Char.Login.*`). Such messages are private: they
/// never reach channels, the map, scripts or diagnostics bodies.
pub fn is_private(package: &str) -> bool {
    package
        .get(..11)
        .is_some_and(|head| head.eq_ignore_ascii_case("Char.Login."))
}

/// Decode a login offer or result. Anything malformed gives `None`, so it can neither start a
/// credential exchange nor claim success.
pub fn decode(message: &GmcpMessage) -> Option<GmcpLogin> {
    let offer = message.is("Char.Login.Default");
    if !offer && !message.is("Char.Login.Result") {
        return None;
    }
    let root = message.data.as_ref()?.as_object()?;
    if offer {
        let types = root.get("type")?.as_array()?;
        let version_one = match root.get("version") {
            None => true,
            Some(Value::Number(n)) => n.as_i64() == Some(1),
            Some(Value::String(s)) => s == "1",
            Some(_) => false,
        };
        let password = types.iter().any(|t| t.as_str() == Some("password-credentials"));
        return Some(GmcpLogin::Offer {
            password_supported: version_one && password,
        });
    }
    let success = match root.get("success")? {
        Value::Bool(b) => *b,
        // .NET bool.TryParse: "true" or "false" in any case, surrounding white space allowed.
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" => true,
            "false" => false,
            _ => return None,
        },
        Value::Number(n) => match n.as_i64() {
            Some(0) => false,
            Some(1) => true,
            _ => return None,
        },
        _ => return None,
    };
    Some(GmcpLogin::Result { success })
}

/// The `Char.Login.Credentials` message: the account and password, or `{}` (declined) when
/// either is missing.
pub fn credentials_message(account: Option<&str>, password: Option<&str>) -> String {
    let body = match (account, password) {
        (Some(account), Some(password)) if !account.is_empty() && !password.is_empty() => {
            serde_json::json!({ "account": account, "password": password }).to_string()
        }
        _ => "{}".into(),
    };
    format!("Char.Login.Credentials {body}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::parse_gmcp;

    fn decode_text(text: &str) -> Option<GmcpLogin> {
        decode(&parse_gmcp(text.as_bytes())?)
    }

    /// GmcpLoginTests.OffersOnlyEnableUnderstoodPasswordVersion.
    #[test]
    fn offers_only_enable_understood_password_version() {
        for (json, supported) in [
            (r#"{"type":["password-credentials"]}"#, true),
            (r#"{"type":["oauth"]}"#, false),
            (r#"{"version":2,"type":["password-credentials"]}"#, false),
            (r#"{"version":"1","type":["oauth","password-credentials"]}"#, true),
        ] {
            assert_eq!(
                decode_text(&format!("Char.Login.Default {json}")),
                Some(GmcpLogin::Offer {
                    password_supported: supported
                }),
                "{json}"
            );
        }
    }

    /// GmcpLoginTests.ResultAcceptsDocumentedBooleanEncodings.
    #[test]
    fn result_accepts_documented_boolean_encodings() {
        for (value, expected) in [
            ("true", true),
            ("\"TRUE\"", true),
            ("1", true),
            ("false", false),
            ("\"false\"", false),
            ("0", false),
        ] {
            assert_eq!(
                decode_text(&format!(r#"Char.Login.Result {{"success":{value}}}"#)),
                Some(GmcpLogin::Result { success: expected }),
                "{value}"
            );
        }
    }

    /// GmcpLoginTests.MalformedLoginCannotTriggerCredentialsOrClaimSuccess.
    #[test]
    fn malformed_login_cannot_trigger_credentials_or_claim_success() {
        for text in [
            r#"Char.Login.Result {"success":"yes"}"#,
            "Char.Login.Default {oops",
            "Char.Login.Default []",
            "Char.Login.Result {}",
            "Char.Login.Default",
            r#"Char.Login.Result {"success":2}"#,
            r#"Char.Vitals {"success":true}"#,
        ] {
            assert_eq!(decode_text(text), None, "{text}");
        }
    }

    /// GmcpLoginTests.LoginFramesArePrivateEvenWithoutPrivateInput: every `Char.Login.*`
    /// package is private, whatever the case.
    #[test]
    fn login_frames_are_private() {
        for package in ["Char.Login.Credentials", "Char.Login.Token", "char.login.result"] {
            assert!(is_private(package), "{package}");
        }
        for package in ["Char.Vitals", "Char.Login", "Char.Logins", ""] {
            assert!(!is_private(package), "{package}");
        }
    }

    #[test]
    fn credentials_are_json_or_an_empty_object() {
        assert_eq!(
            credentials_message(Some("o\"do"), Some("p@ss")),
            r#"Char.Login.Credentials {"account":"o\"do","password":"p@ss"}"#
        );
        assert_eq!(credentials_message(None, Some("x")), "Char.Login.Credentials {}");
        assert_eq!(credentials_message(Some(""), Some("x")), "Char.Login.Credentials {}");
    }
}
