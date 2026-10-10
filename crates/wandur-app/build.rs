//! Build facts for Help > About Wandur's System information: the git commit the app was built
//! from (`git rev-parse --short HEAD`, or `WANDUR_BUILD_COMMIT` when set, as a packaging script
//! may), the build profile and the target. A build without git (from a source tarball) says
//! "unknown" for the commit.

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let out = Command::new("git").args(args).current_dir(dir).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    Some(text.trim().to_string()).filter(|t| !t.is_empty())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=WANDUR_BUILD_COMMIT");
    let given = std::env::var("WANDUR_BUILD_COMMIT")
        .ok()
        .filter(|c| !c.trim().is_empty());
    let commit = match given {
        Some(commit) => commit.trim().to_string(),
        None => {
            // Build again when HEAD moves: HEAD itself, the branch it names, and packed refs
            // (paths from git, so a worktree's shared refs are found too).
            let mut watched = vec!["HEAD".to_string(), "packed-refs".to_string()];
            if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
                watched.push(branch);
            }
            for name in watched {
                if let Some(path) = git(&["rev-parse", "--path-format=absolute", "--git-path", &name])
                    && Path::new(&path).exists()
                {
                    println!("cargo:rerun-if-changed={path}");
                }
            }
            git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string())
        }
    };
    println!("cargo:rustc-env=WANDUR_BUILD_COMMIT={commit}");
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=WANDUR_BUILD_PROFILE={profile}");
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=WANDUR_BUILD_TARGET={target}");
}
