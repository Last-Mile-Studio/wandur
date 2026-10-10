//! UI text in five languages, from the `.resx` tables in `crates/wandur-core/locale/` (the C#
//! client's tables plus the strings only this client has). `scripts/generate-localization.py`
//! turns them into [`generated`]; its `--check` mode fails when the Rust is stale.
//!
//! Every string the UI shows goes through [`t`] (or [`tf`] with arguments), keyed by [`S`]. The
//! language is process-wide and can change at any time ([`set_language`]); the next frame draws
//! in it. A thread can override it ([`override_thread`]) so tests and headless captures do not
//! disturb each other.
//!
//! Format strings use the C# composite form: `{0}`, `{1}`, with an optional `:spec` that is
//! ignored here (callers pass text already formatted), and `{{` and `}}` for braces.

#[rustfmt::skip]
mod generated;

use std::cell::Cell;
use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};

pub use generated::{COUNT, NAMES, S};

/// A UI language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Language {
    En = 0,
    Es = 1,
    Fr = 2,
    De = 3,
    PtBr = 4,
}

impl Language {
    pub const ALL: [Language; 5] = [Language::En, Language::Es, Language::Fr, Language::De, Language::PtBr];

    /// The culture code the settings store (`""` there means the system language).
    pub fn code(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Es => "es",
            Language::Fr => "fr",
            Language::De => "de",
            Language::PtBr => "pt-BR",
        }
    }

    /// The language's own name, as the language list shows it.
    pub fn native_name(self) -> &'static str {
        match self {
            Language::En => "English",
            Language::Es => "Español",
            Language::Fr => "Français",
            Language::De => "Deutsch",
            Language::PtBr => "Português (Brasil)",
        }
    }

    pub fn from_code(code: &str) -> Option<Language> {
        Language::ALL.into_iter().find(|l| l.code().eq_ignore_ascii_case(code))
    }

    fn from_index(i: u8) -> Language {
        Language::ALL.get(i as usize).copied().unwrap_or(Language::En)
    }
}

/// The codes the language setting accepts, in list order; `""` is the system language.
pub const SUPPORTED_CODES: [&str; 6] = ["", "en", "es", "fr", "de", "pt-BR"];

/// The choices of the language list: (setting code, shown name). The first is the system
/// language, named in the current language.
pub fn choices() -> Vec<(&'static str, &'static str)> {
    let mut list = vec![("", t(S::SystemLanguage))];
    list.extend(Language::ALL.iter().map(|l| (l.code(), l.native_name())));
    list
}

/// The language for a setting `code` (`""`: follow `system`, a locale such as `de-AT`,
/// `pt_PT.UTF-8` or `es-419`). As the C# `UiLanguage.Resolve`: the full name if supported, else
/// its language, Portuguese of any region as Brazilian Portuguese, anything else English.
pub fn resolve(code: &str, system: Option<&str>) -> Language {
    let requested = if code.trim().is_empty() {
        system.unwrap_or("")
    } else {
        code
    };
    // "pt_PT.UTF-8@euro" -> "pt-PT"
    let name: String = requested
        .split(['.', '@'])
        .next()
        .unwrap_or("")
        .trim()
        .replace('_', "-");
    if let Some(l) = Language::from_code(&name) {
        return l;
    }
    let two = name.split('-').next().unwrap_or("").to_ascii_lowercase();
    if two == "pt" {
        return Language::PtBr;
    }
    Language::ALL
        .into_iter()
        .find(|l| l.code().eq_ignore_ascii_case(&two))
        .unwrap_or(Language::En)
}

static LANGUAGE: AtomicU8 = AtomicU8::new(Language::En as u8);

thread_local! {
    static OVERRIDE: Cell<Option<Language>> = const { Cell::new(None) };
}

/// The language strings are drawn in now (on this thread).
pub fn language() -> Language {
    OVERRIDE
        .with(Cell::get)
        .unwrap_or_else(|| Language::from_index(LANGUAGE.load(Ordering::Relaxed)))
}

/// Switch the UI language. On a thread with an override (tests, headless captures) this moves
/// the override instead, so other threads keep theirs.
pub fn set_language(language: Language) {
    if OVERRIDE.with(Cell::get).is_some() {
        OVERRIDE.with(|o| o.set(Some(language)));
    } else {
        LANGUAGE.store(language as u8, Ordering::Relaxed);
    }
}

/// Give this thread its own language (`None`: follow the process-wide one again).
pub fn override_thread(language: Option<Language>) {
    OVERRIDE.with(|o| o.set(language));
}

/// The text of `key` in the current language.
pub fn t(key: S) -> &'static str {
    text_in(language(), key)
}

/// The text of `key` in `language`.
pub fn text_in(language: Language, key: S) -> &'static str {
    generated::TABLES[language as usize][key as usize]
}

/// The text of `key` in the current language with `{0}`, `{1}` ... filled in.
pub fn tf(key: S, args: &[&dyn Display]) -> String {
    format(t(key), args)
}

/// A count in words: `one` (with `{0}` the count) for a single thing, `many` otherwise, by the
/// current language's rule. English, Spanish, German and Portuguese use the singular for 1
/// only; French also for 0 (CLDR's `one` category).
pub fn plural(n: usize, one: S, many: S) -> String {
    plural_in(language(), n, one, many)
}

/// [`plural`] in `language`.
pub fn plural_in(language: Language, n: usize, one: S, many: S) -> String {
    let singular = match language {
        Language::Fr => n <= 1,
        _ => n == 1,
    };
    format(text_in(language, if singular { one } else { many }), &[&n])
}

/// Fill a composite format string: `{n}` and `{n:spec}` take argument n (the spec is ignored),
/// `{{` and `}}` are braces. A placeholder without its argument is left as written.
pub fn format(template: &str, args: &[&dyn Display]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if tail.starts_with('{')
            && let Some(end) = tail.find('}')
        {
            let inner = &tail[1..end];
            let index = inner.split(':').next().unwrap_or("");
            if let Some(arg) = index.parse::<usize>().ok().and_then(|n| args.get(n)) {
                let _ = write!(out, "{arg}");
                rest = &tail[end + 1..];
                continue;
            }
        }
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// The `{n...}` placeholders in a format string, sorted (for parity checks across languages).
pub fn placeholders(template: &str) -> Vec<String> {
    let mut found = Vec::new();
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{'
            && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)
            && let Some(end) = template[i..].find('}')
        {
            found.push(template[i..i + end + 1].to_string());
            i += end + 1;
            continue;
        }
        i += 1;
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resources that hold JavaScript source: their braces are code, not placeholders.
    const SOURCE_KEYS: [S; 2] = [S::ScriptApiExamples, S::ScriptStarterExample];

    #[test]
    fn every_language_has_every_key_with_the_same_format_arguments() {
        assert_eq!(S::ALL.len(), COUNT);
        assert_eq!(NAMES.len(), COUNT);
        for (i, key) in S::ALL.iter().enumerate() {
            assert_eq!(*key as usize, i);
        }
        for language in Language::ALL {
            for key in S::ALL {
                let english = text_in(Language::En, key);
                let text = text_in(language, key);
                assert!(!text.trim().is_empty(), "{language:?} {}", NAMES[key as usize]);
                if SOURCE_KEYS.contains(&key) {
                    continue;
                }
                assert_eq!(
                    placeholders(english),
                    placeholders(text),
                    "{language:?} {}: {text}",
                    NAMES[key as usize]
                );
            }
        }
    }

    /// The undo toast's counts read properly in every language: singular for one (and for
    /// zero in French), plural otherwise.
    #[test]
    fn counts_pick_the_singular_or_the_plural_per_language() {
        let rooms = |l, n| plural_in(l, n, S::ToastDeletedRoomsOne, S::ToastDeletedRoomsMany);
        assert_eq!(rooms(Language::En, 1), "Deleted 1 room");
        assert_eq!(rooms(Language::En, 3), "Deleted 3 rooms");
        assert_eq!(rooms(Language::De, 1), "1 Raum gelöscht");
        assert_eq!(rooms(Language::De, 3), "3 Räume gelöscht");
        assert_eq!(rooms(Language::Es, 2), "2 salas eliminadas");
        assert_eq!(rooms(Language::Fr, 1), "1 salle supprimée");
        assert_eq!(rooms(Language::Fr, 0), "0 salle supprimée");
        assert_eq!(rooms(Language::Fr, 5), "5 salles supprimées");
        assert_eq!(rooms(Language::PtBr, 1), "1 sala excluída");
        assert_eq!(rooms(Language::PtBr, 4), "4 salas excluídas");
        for language in Language::ALL {
            let one = plural_in(language, 1, S::ToastDeletedExitsOne, S::ToastDeletedExitsMany);
            let two = plural_in(language, 2, S::ToastDeletedExitsOne, S::ToastDeletedExitsMany);
            assert!(
                one.contains('1') && two.contains('2') && one.len() < two.len() + 2,
                "{language:?}"
            );
            assert_ne!(one.replace('1', ""), two.replace('2', ""), "{language:?}");
        }
    }

    #[test]
    fn translations_are_really_different_tables() {
        assert_eq!(text_in(Language::De, S::SettingsTitle), "Einstellungen");
        assert_eq!(text_in(Language::En, S::SettingsTitle), "Settings");
        assert_ne!(text_in(Language::Fr, S::File), text_in(Language::En, S::File));
    }

    #[test]
    fn system_locale_chooses_a_supported_language_or_english() {
        for (system, expected) in [
            ("es-MX", Language::Es),
            ("fr-CA", Language::Fr),
            ("de-AT", Language::De),
            ("pt-PT", Language::PtBr),
            ("pt_BR.UTF-8", Language::PtBr),
            ("ja-JP", Language::En),
            ("de_DE@euro", Language::De),
            ("", Language::En),
        ] {
            assert_eq!(resolve("", Some(system)), expected, "{system}");
        }
        assert_eq!(resolve("", None), Language::En);
    }

    #[test]
    fn an_explicit_language_wins_over_the_system_preference() {
        assert_eq!(resolve("de", Some("es-MX")), Language::De);
        assert_eq!(resolve("pt-BR", Some("en-US")), Language::PtBr);
        assert_eq!(resolve("PT-br", None), Language::PtBr);
    }

    #[test]
    fn composite_formats_fill_arguments_and_keep_braces() {
        assert_eq!(format("{0} of {1}", &[&3, &"ten"]), "3 of ten");
        assert_eq!(format("{1:d} then {0:g}", &[&"a", &"b"]), "b then a");
        assert_eq!(format("{{literal}} {0}", &[&1]), "{literal} 1");
        assert_eq!(format("{2} missing", &[&1]), "{2} missing");
        assert_eq!(format("tail {", &[]), "tail {");
        assert_eq!(placeholders("{1} and {0:g} and {{x}}"), ["{0:g}", "{1}"]);
    }

    #[test]
    fn a_thread_override_keeps_other_threads_apart() {
        override_thread(Some(Language::De));
        assert_eq!(t(S::SettingsTitle), "Einstellungen");
        set_language(Language::Fr);
        assert_eq!(language(), Language::Fr, "the override moved");
        let other = std::thread::spawn(language).join().unwrap();
        assert_eq!(other, Language::En, "the process-wide language did not");
        override_thread(None);
    }

    #[test]
    fn choices_start_with_the_system_language() {
        override_thread(Some(Language::De));
        let list = choices();
        assert_eq!(list[0].0, "");
        assert_eq!(list[0].1, text_in(Language::De, S::SystemLanguage));
        assert_eq!(list.iter().map(|c| c.0).collect::<Vec<_>>(), SUPPORTED_CODES);
        override_thread(None);
    }

    /// The generator's check mode passes on the committed tables (python3 required; skipped
    /// with a note where it is not installed).
    #[test]
    fn generated_tables_match_the_resx_files() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let script = root.join("scripts/generate-localization.py");
        match std::process::Command::new("python3")
            .arg(&script)
            .arg("--check")
            .output()
        {
            Ok(out) => assert!(
                out.status.success(),
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
            Err(e) => eprintln!("python3 not available ({e}); generator check skipped"),
        }
    }
}
