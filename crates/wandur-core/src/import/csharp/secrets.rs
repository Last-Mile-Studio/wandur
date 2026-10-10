//! Saved passwords and agent API keys: from the C# client's entries in the system credential
//! store to this client's.
//!
//! The C# client saves a world's password under the account [`csharp_login_key`] in the
//! service `net.wandur.world-login` (macOS Keychain), the target `Wandur/world-login/<key>`
//! (Windows) or the attributes `application net.wandur.client credential <key>` (Linux). An agent
//! API key uses the same store with the key `agent-<hash>`, which this client computes the same
//! way ([`crate::agent::credentials::credential_key`]). This client keeps its own service, so a
//! copy is a read from the C# entry and a write to this client's entry; the C# entry stays.
//!
//! Reading another program's Keychain entry makes macOS ask the person to allow it, which is why
//! the app copies secrets only when asked and the command line only with `--include-passwords`.

use std::sync::Arc;

use sha2::{Digest, Sha256};

use super::{Kind, Report, Skip};
use crate::login::{PasswordVault, VaultError};

/// The C# client's Keychain service.
pub const CSHARP_SERVICE: &str = "net.wandur.world-login";
/// The C# client's Secret Service `application` attribute (Linux).
pub const CSHARP_APPLICATION: &str = "net.wandur.client";

/// What a secret is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretKind {
    Password,
    ApiKey,
}

impl SecretKind {
    fn kind(self) -> Kind {
        match self {
            SecretKind::Password => Kind::Passwords,
            SecretKind::ApiKey => Kind::ApiKeys,
        }
    }
}

/// One secret to copy: from the C# entry `from_key` to this client's entry `to_key`.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretJob {
    pub kind: SecretKind,
    pub from_key: String,
    pub to_key: String,
}

impl std::fmt::Debug for SecretJob {
    // Keys are hashes, but a report or a log never needs them.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretJob")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// The C# `PasswordVault.Key`: SHA-256 (upper-case hex) of the JSON
/// `{"Id":…,"PasswordId":…,"Host":…,"Port":…,"UseTls":…,"Username":…}` as .NET's
/// `System.Text.Json` writes it (its default encoder escapes HTML-sensitive and non-ASCII
/// characters). `profile_id` and `password_id` are hyphenated GUIDs.
pub fn csharp_login_key(
    profile_id: &str,
    password_id: &str,
    host: &str,
    port: u16,
    tls: bool,
    username: &str,
) -> String {
    let identity = format!(
        "{{\"Id\":{},\"PasswordId\":{},\"Host\":{},\"Port\":{port},\"UseTls\":{tls},\"Username\":{}}}",
        dotnet_json_string(&profile_id.to_lowercase()),
        dotnet_json_string(&password_id.to_lowercase()),
        dotnet_json_string(&host.to_lowercase()),
        dotnet_json_string(username),
    );
    let digest = Sha256::digest(identity.as_bytes());
    digest.iter().map(|b| format!("{b:02X}")).collect()
}

/// A JSON string as `System.Text.Json`'s default encoder writes it: `\\` and the short control
/// escapes, `\uXXXX` (upper-case hex, UTF-16) for `"`, `&`, `'`, `+`, `<`, `>`, `` ` ``, other
/// control characters and everything outside printable ASCII.
pub fn dotnet_json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' if !matches!(c, '"' | '&' | '\'' | '+' | '<' | '>' | '`') => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04X}"));
                }
            }
        }
    }
    out.push('"');
    out
}

/// The C# client's credential store, read only.
pub fn csharp_vault() -> Arc<dyn PasswordVault> {
    Arc::new(CsharpVault)
}

/// Reads the C# client's entries; never writes or deletes them.
#[derive(Debug, Default)]
pub struct CsharpVault;

impl PasswordVault for CsharpVault {
    fn name(&self) -> String {
        crate::login::vault::system().name()
    }

    fn read(&self, key: &str) -> Result<Option<String>, VaultError> {
        #[cfg(all(feature = "vault", target_os = "macos"))]
        {
            crate::login::vault::native::read_other(keyring::Entry::new(CSHARP_SERVICE, key))
        }
        #[cfg(all(feature = "vault", target_os = "windows"))]
        {
            crate::login::vault::native::read_other(keyring::Entry::new_with_target(
                &format!("Wandur/world-login/{key}"),
                CSHARP_SERVICE,
                "Wandur",
            ))
        }
        #[cfg(all(feature = "vault", target_os = "linux"))]
        {
            crate::login::vault::secret_tool::lookup_in(CSHARP_APPLICATION, key)
        }
        #[cfg(not(all(
            feature = "vault",
            any(target_os = "macos", target_os = "windows", target_os = "linux")
        )))]
        {
            crate::login::vault::Unavailable.read(key)
        }
    }

    fn write(&self, _: &str, _: &str) -> Result<(), VaultError> {
        Err(VaultError(
            crate::l10n::t(crate::l10n::S::CsImportSkipVaultFailed).into(),
        ))
    }

    fn delete(&self, _: &str) -> Result<(), VaultError> {
        Err(VaultError(
            crate::l10n::t(crate::l10n::S::CsImportSkipVaultFailed).into(),
        ))
    }
}

/// Copy every job's secret. `progress` is told after each one. Nothing about a secret is ever
/// reported but its outcome.
pub fn copy(
    jobs: &[SecretJob],
    from: &dyn PasswordVault,
    to: &dyn PasswordVault,
    report: &mut Report,
    progress: &dyn Fn(usize, usize),
) {
    for (i, job) in jobs.iter().enumerate() {
        let kind = job.kind.kind();
        match from.read(&job.from_key) {
            Ok(None) => report.skip(kind, Skip::NotInVault, 1),
            Err(_) => report.skip(kind, Skip::VaultFailed, 1),
            Ok(Some(secret)) => match to.read(&job.to_key) {
                Ok(Some(existing)) if existing == secret => report.unchanged(kind, 1),
                Ok(Some(_)) => report.skip(kind, Skip::KeptRust, 1),
                Ok(None) => match to.write(&job.to_key, &secret) {
                    Ok(()) => report.imported(kind, 1),
                    Err(_) => report.skip(kind, Skip::VaultFailed, 1),
                },
                Err(_) => report.skip(kind, Skip::VaultFailed, 1),
            },
        }
        progress(i + 1, jobs.len());
    }
}

/// Count the jobs as not asked for (the command line without `--include-passwords`).
pub fn not_requested(jobs: &[SecretJob], report: &mut Report) {
    for job in jobs {
        report.skip(job.kind.kind(), Skip::PasswordsNotRequested, 1);
    }
}
