//! Every UI string goes through the string table (`wandur_core::l10n`): no widget in the app's
//! sources takes an English literal, and no text in the app or in wandur-core is built from an
//! English literal with `format!`, `.into()`, `.to_string()` and the like. Test modules, the
//! developer tools (command line, scenes, probes), the demo world's MUD text (English in C# too)
//! and a few values that are not words (an example host name, symbols, thread names) are exempt.

use std::path::{Path, PathBuf};

/// Calls whose first argument is text a person reads.
const CALLS: &[&str] = &[
    "label(",
    "button(",
    "small_button(",
    "heading(",
    "hover_text(",
    "hint_text(",
    "RichText::new(",
    "selectable_label(",
    "selectable_value(",
    "checkbox(",
    "Checkbox::new(",
    "Button::new(",
    "selected_text(",
    "set_label(",
    "pill(",
    "link(",
    "menu_button(",
];

/// Literal texts that are not words to translate.
const ALLOWED: &[&str] = &["mud.example.org", "TLS", "MSSP"];

fn literal_after(line: &str, at: usize) -> Option<&str> {
    let rest = line[at..].trim_start();
    let rest = rest
        .strip_prefix("&format!(")
        .or_else(|| rest.strip_prefix("format!("))
        .unwrap_or(rest);
    let rest = rest.strip_prefix('"')?;
    Some(&rest[..rest.find('"')?])
}

fn words(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes
        .windows(2)
        .any(|w| w[0].is_ascii_alphabetic() && w[1].is_ascii_alphabetic())
}

/// Rust sources under `dir`, with their file names.
fn sources(dir: PathBuf) -> Vec<(String, PathBuf)> {
    let mut files = vec![];
    let mut dirs = vec![dir];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.push((path.file_name().unwrap().to_string_lossy().to_string(), path));
            }
        }
    }
    files.sort();
    files
}

/// Developer-only files, whose text no player reads.
const DEV_FILES: &[&str] = &["scene.rs", "main.rs", "probe.rs", "sysstat.rs"];

/// The non-test code of a source file. A `tests.rs` file is a test module (declared under
/// `#[cfg(test)]` by its parent) and holds none.
fn code(path: &Path) -> String {
    if path.file_name().is_some_and(|n| n == "tests.rs") {
        return String::new();
    }
    let text = std::fs::read_to_string(path).unwrap();
    text.split("#[cfg(test)]").next().unwrap().to_string()
}

#[test]
fn ui_text_goes_through_the_string_table() {
    let mut found = Vec::new();
    for (name, path) in sources(Path::new(env!("CARGO_MANIFEST_DIR")).join("src")) {
        if DEV_FILES.contains(&name.as_str()) {
            continue;
        }
        let code = code(&path);
        for (n, line) in code.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for call in CALLS {
                let mut from = 0;
                while let Some(i) = line[from..].find(call) {
                    let at = from + i + call.len();
                    if let Some(lit) = literal_after(line, at)
                        && words(lit)
                        && !ALLOWED.contains(&lit)
                        && !lit.starts_with('{')
                    {
                        found.push(format!("{name}:{}: {lit:?}", n + 1));
                    }
                    from = at;
                }
            }
        }
    }
    assert!(found.is_empty(), "English literals in widgets:\n{}", found.join("\n"));
}

/// Ways a literal becomes text a person may read without going through a widget call.
const BUILDERS_BEFORE: &[&str] = &["format!(", "String::from(", "push_str(", "Err("];
const BUILDERS_AFTER: &[&str] = &[".into()", ".to_string()", ".to_owned()"];

/// Literals that are not UI text: thread names, protocol messages, artwork failure reasons (never shown, the
/// picture just stays a placeholder), proper names kept as C# keeps them, and the JavaScript a
/// macro's stored source is generated as (code, the same as the C# client writes).
/// Files whose built text is the scripting contract's own error messages: English, as the
/// engine's messages in `bootstrap.js` and the C# `ScriptPanelAction.Parse` exceptions are. The
/// UI shows them inside a translated sentence (`ScriptPanelRejected`). `lua.rs` holds the Lua
/// engine's limit and bridge errors, English as the C# `LuaScriptEngine` and `LuaBridge` ones,
/// shown inside `ScriptFailed`. `converter.rs` and `lua_wrapper.rs` (the Mudlet importer) build
/// JavaScript and Lua source, code rather than text; their comments and names come from the
/// table. `model_package.rs`, `classifier_service.rs` and `onnx_classifier.rs` (the room
/// classifier) give the reasons a package cannot be used, English as the C# `ModelPackage` and
/// `ModelPackageInstaller` exceptions, shown inside `MapInferenceFailed`.
const CONTRACT_FILES: &[&str] = &[
    "panels.rs",
    "lua.rs",
    "converter.rs",
    "lua_wrapper.rs",
    "model_package.rs",
    "classifier_service.rs",
    "onnx_classifier.rs",
];

const BUILT_ALLOWED: &[&str] = &[
    "mud.every({}, () => {{\\n{actions}\\n}});",
    "wandur-read {endpoint}",
    "wandur-write {}",
    "wandur-triggers {}",
    "wandur-words {id}",
    "wandur-scripts {id}",
    "wandur-vault {}",
    "wandur-history {}",
    // SQL conditions of the history search (code, never shown).
    "(history_contains(s.world_key, ?) OR history_contains(s.world_name, ?))",
    "history_contains(s.character_name, ?)",
    // The GMCP login message (protocol, not text for people).
    "Char.Login.Credentials {body}",
    "not a picture",
    "indexed PNG not expanded",
    "The Mud Connector",
    // Reasons an update check failed: never shown (a failed check is silent, or the menu
    // command's localized "Could not reach wandur.net.").
    "too many redirects",
    "The release version could not be read.",
    // SQL of the map store (code, never shown).
    "SELECT payload FROM {table} WHERE world_id=?1 ORDER BY rowid",
    "DELETE FROM {table} WHERE world_id=?1",
];

/// Every string literal on a line, with the text just before and after it.
fn literals(line: &str) -> Vec<(&str, &str, &str)> {
    let mut out = vec![];
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' && i + 2 < bytes.len() && (bytes[i + 2] == b'\'' || bytes[i + 1] == b'\\') {
            // A char literal such as '"' or '\\'.
            i += if bytes[i + 1] == b'\\' { 4 } else { 3 };
            continue;
        }
        if bytes[i] == b'"' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            let end = j.min(bytes.len());
            out.push((&line[..i], &line[start..end], &line[(end + 1).min(line.len())..]));
            i = end + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// Text that reads as words: a space, and a word of three letters or more that is not an
/// all-caps acronym (`HTTP {}` and `MTTS {}` are protocol text).
fn reads_as_words(text: &str) -> bool {
    let mut plain = String::new();
    let mut depth = 0;
    for c in text.chars() {
        match c {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            _ if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    plain.contains(' ')
        && plain
            .split(|c: char| !c.is_ascii_alphabetic())
            .any(|w| w.len() >= 3 && w.chars().any(|c| c.is_ascii_lowercase()))
}

#[test]
fn built_text_goes_through_the_string_table() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = sources(manifest.join("src"));
    files.extend(sources(manifest.join("../wandur-core/src")));
    let mut found = Vec::new();
    for (name, path) in files {
        if DEV_FILES.contains(&name.as_str())
            || CONTRACT_FILES.contains(&name.as_str())
            || matches!(name.as_str(), "generated.rs" | "demo.rs")
        {
            continue;
        }
        for (n, line) in code(&path).lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with("#[") {
                continue;
            }
            for (before, lit, after) in literals(line) {
                let built = BUILDERS_BEFORE.iter().any(|b| before.trim_end().ends_with(b))
                    || BUILDERS_AFTER.iter().any(|a| after.starts_with(a));
                if built && reads_as_words(lit) && !BUILT_ALLOWED.contains(&lit) {
                    found.push(format!("{name}:{}: {lit:?}", n + 1));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "English literals built into text:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_built_text_check_sees_the_cases_it_is_for() {
    let hits = |line: &str| {
        literals(line).into_iter().any(|(before, lit, after)| {
            (BUILDERS_BEFORE.iter().any(|b| before.trim_end().ends_with(b))
                || BUILDERS_AFTER.iter().any(|a| after.starts_with(a)))
                && reads_as_words(lit)
        })
    };
    assert!(hits(r#"parts.push(format!("{} room{}", n, s));"#));
    assert!(hits(r#"parts.push(format!("floor {}", z));"#));
    assert!(hits(r#"text.push_str(&format!("\nExits: {}", e));"#));
    assert!(hits(r#"return "Browser-based world".into();"#));
    assert!(hits(r#"return Err("The directory uses a format".into());"#));
    assert!(!hits(r#"format!("HTTP {}", status)"#));
    assert!(!hits(r#""leaf".into()"#));
    assert!(!hits(r#"t(S::MapFloor)"#));
}
