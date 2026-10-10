//! `wandur --import-csharp` over the synthetic C# fixture, through the real binary: it prints
//! counts and skip reasons only (no world name, address, account, text or secret), a dry run
//! writes nothing, and errors name no path.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../wandur-core/tests/fixtures/csharp-data/main")
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wandur-cli-csimport-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn wandur(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_wandur"))
        .args(args)
        // English output whatever the machine's language.
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// Every string the fixture's worlds, settings, scripts, maps and history hold that could
/// identify something.
const CONTENT: [&str; 22] = [
    "Fixture",
    "fixture",
    "lantern",
    "harbor",
    "Harbor",
    "203.0.113.7",
    "2001:db8",
    "4000",
    "6697",
    "2323",
    "wayfarer",
    "Wayfarer",
    "Quill",
    "Ashen",
    "SMAUG",
    "gossip",
    "tell",
    "eat bread",
    "go home",
    "lamplighter",
    "custom-0123456789abcdef",
    "aaaaaaaa-bbbb",
];

#[test]
fn the_command_line_prints_counts_only() {
    let dir = temp("counts");
    let source = fixture();
    let (code, out, err) = wandur(&[
        "--import-csharp",
        source.to_str().unwrap(),
        "--data-dir",
        dir.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("Saved worlds: 4 found, 4 imported, 0 already there"),
        "{out}"
    );
    assert!(out.contains("History lines: 9 found, 9 imported"), "{out}");
    assert!(out.contains("--include-passwords"), "{out}");
    for content in CONTENT {
        assert!(!out.contains(content), "{content} printed:\n{out}");
        assert!(!err.contains(content), "{content} printed:\n{err}");
    }
    assert!(dir.join("wandur.db").is_file() && dir.join("settings.json").is_file());
    // Again: nothing new.
    let (code, out, _) = wandur(&[
        "--import-csharp",
        source.join("wandur.db").to_str().unwrap(),
        "--data-dir",
        dir.to_str().unwrap(),
    ]);
    assert_eq!(code, 0);
    assert!(
        out.contains("Saved worlds: 4 found, 0 imported, 4 already there"),
        "{out}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_dry_run_writes_nothing_and_errors_name_no_path() {
    let dir = temp("dry");
    let source = fixture();
    let (code, out, _) = wandur(&[
        "--import-csharp",
        source.to_str().unwrap(),
        "--data-dir",
        dir.to_str().unwrap(),
        "--dry-run",
    ]);
    assert_eq!(code, 0);
    assert!(out.contains("Dry run: nothing was written."), "{out}");
    assert!(!dir.exists());
    let missing = dir.join("nowhere-secret-name");
    let (code, out, err) = wandur(&[
        "--import-csharp",
        missing.to_str().unwrap(),
        "--data-dir",
        dir.to_str().unwrap(),
    ]);
    assert_eq!(code, 1);
    assert!(out.is_empty());
    assert!(err.contains("wandur.db"), "{err}");
    assert!(!err.contains("nowhere-secret-name"), "{err}");
    let (code, _, err) = wandur(&["--dry-run"]);
    assert_eq!(code, 2, "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
