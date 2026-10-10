//! The text a room is classified on (the C# `RoomTextPreprocessor`), exactly as the classifier
//! package's `preprocessing_spec.json` describes it: colour codes and tildes stripped, whitespace
//! collapsed, then `clean(name) + "\n" + clean(description)`. Any change here breaks parity with
//! the model.

use std::sync::OnceLock;

use regex::Regex;
use sha2::{Digest, Sha256};

/// The package's strip patterns, applied in this order: ANSI SGR, SMAUG `&x`, TBA `@x`, ROM
/// `{x`. Tildes and whitespace runs follow.
fn patterns() -> &'static [Regex; 5] {
    static PATTERNS: OnceLock<[Regex; 5]> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            Regex::new(r"\x1b\[[0-9;]*m").expect("valid"),
            Regex::new(r"&[a-zA-Z0-9]").expect("valid"),
            Regex::new(r"@[a-zA-Z0-9]").expect("valid"),
            Regex::new(r"\{[a-zA-Z]").expect("valid"),
            Regex::new(r"\s+").expect("valid"),
        ]
    })
}

/// One field cleaned: codes and tildes removed, whitespace runs made one space, trimmed.
pub fn clean(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let [ansi, smaug, tba, rom, space] = patterns();
    let value = ansi.replace_all(text, "");
    let value = smaug.replace_all(&value, "");
    let value = tba.replace_all(&value, "");
    let value = rom.replace_all(&value, "");
    let value = value.replace('~', "");
    space.replace_all(&value, " ").trim().to_string()
}

/// The classifier's input for a room.
pub fn build_text(name: &str, description: &str) -> String {
    let mut text = clean(name);
    text.push('\n');
    text.push_str(&clean(description));
    text
}

/// What a stored inference was computed from: SHA-256 (lowercase hex) of the model version, a
/// line break and [`build_text`]. A room whose key differs needs classifying again. CPU work:
/// call it off the UI thread.
pub fn inference_key(name: &str, description: &str, model_version: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(model_version.as_bytes());
    hasher.update(b"\n");
    hasher.update(build_text(name, description).as_bytes());
    hex(&hasher.finalize())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}
