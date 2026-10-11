//! Golden outputs: each `golden/NAME.js` formats to `golden/NAME.formatted.js`, and formatting
//! that output again changes nothing. `WANDUR_BLESS=1 cargo test -p wandur-format` rewrites
//! the outputs (read them before committing).
#![cfg(feature = "javascript")]

use std::path::PathBuf;
use std::time::Duration;

/// Correctness checks take as long as they need: a busy machine is not a wrong result.
fn format(source: &str) -> Result<String, wandur_format::Skipped> {
    wandur_format::javascript_within(source, Duration::MAX)
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

#[test]
fn scripts_format_to_their_golden_outputs() {
    let bless = std::env::var_os("WANDUR_BLESS").is_some();
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "js") && !p.to_string_lossy().ends_with(".formatted.js"))
        .collect();
    names.sort();
    assert!(names.len() >= 6, "{names:?}");
    for input in names {
        let source = std::fs::read_to_string(&input).unwrap();
        let formatted = format(&source).unwrap_or_else(|e| panic!("{input:?}: {e:?}"));
        let golden = input.with_extension("formatted.js");
        if bless {
            std::fs::write(&golden, &formatted).unwrap();
        }
        let expected = std::fs::read_to_string(&golden).unwrap_or_else(|_| panic!("{golden:?} is missing"));
        assert_eq!(formatted, expected, "{input:?}");
        assert_eq!(format(&formatted).unwrap(), formatted, "{input:?} formats to itself");
    }
}

/// Typical scripts format in well under a millisecond each once the formatter is warm.
#[test]
fn typical_scripts_format_quickly() {
    let source = std::fs::read_to_string(dir().join("pack-dense.js")).unwrap();
    // The first run builds the options and touches the formatter's code (slow on a cold disk).
    format(&source).unwrap();
    let started = std::time::Instant::now();
    for _ in 0..20 {
        format(&source).unwrap();
    }
    let each = started.elapsed() / 20;
    // Generous for a debug build on a busy machine; release takes about 0.2 ms.
    assert!(each < wandur_format::BUDGET, "{each:?}");
}
