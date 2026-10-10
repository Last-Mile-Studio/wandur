//! Saved passwords live in the operating system's credential store, never in `settings.json`.
//!
//! - macOS: the Keychain (generic passwords, service [`SERVICE`]), through `keyring`.
//! - Windows: Credential Manager (generic credentials), through `keyring`.
//! - Linux: `secret-tool` (libsecret, a Secret Service such as GNOME Keyring or KeePassXC), with
//!   the password on standard input, never in the arguments, as the C# client does.
//!
//! A missing or locked store is an error with a readable message; there is no plaintext
//! fallback anywhere. The calls block (a store may ask the person to unlock it), so the app makes
//! them off the UI thread where it can.
//!
//! An entry is found by [`key`], a hash of the world's id, its password reference and the
//! address and account it was saved for. Editing the host or username makes the old entry
//! unreachable (the password must be typed again); editing the port or TLS moves the entry to the
//! new key when the world is saved (`credentials::save_world`); renaming the world changes nothing.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use sha2::{Digest, Sha256};

use crate::l10n::{S, t, tf};
use crate::settings::SavedWorld;

/// The longest password accepted (characters), as in C#.
pub const MAX_PASSWORD: usize = 1024;
/// The largest stored value read back (bytes).
const MAX_STORED: usize = 8192;
/// The service name entries are saved under. The C# client uses `net.wandur.world-login`; this
/// client keeps its own (as it keeps its own data directory), so its entries are easy to find and
/// remove and never mix with the C# client's.
pub const SERVICE: &str = "net.wandur.rust.world-login";

/// A credential store failure, with the message to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultError(pub String);

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for VaultError {}

/// Where passwords are kept.
pub trait PasswordVault: Send + Sync {
    /// The store's name for people ("macOS Keychain").
    fn name(&self) -> String;
    /// The password saved under `key`, or `None` when there is none.
    fn read(&self, key: &str) -> Result<Option<String>, VaultError>;
    /// Save (or replace) the password under `key`.
    fn write(&self, key: &str, password: &str) -> Result<(), VaultError>;
    /// Remove the entry; removing one that is not there is not an error.
    fn delete(&self, key: &str) -> Result<(), VaultError>;
}

/// A password must be one line of 1 to 1024 characters.
pub fn validate_password(password: &str) -> Result<(), String> {
    if password.is_empty() || password.chars().count() > MAX_PASSWORD || password.chars().any(char::is_control) {
        return Err(t(S::PasswordMustBeASingleLineOf11024).into());
    }
    Ok(())
}

/// The vault key for a world's saved password: SHA-256 (upper-case hex) of the world id, the
/// password reference, the host (lower case), port, TLS and username. Fails when the world has
/// no saved password.
pub fn key(world: &SavedWorld) -> Result<String, String> {
    let Some(password_id) = &world.password_id else {
        return Err(t(S::ThisWorldHasNoSavedPassword).into());
    };
    // The C# client's field names. serde_json sorts the keys, so the JSON (and the digest) is
    // not the C# key; this client keeps its own Keychain service, so the two never share entries.
    let identity = serde_json::json!({
        "Id": world.world_id,
        "PasswordId": password_id,
        "Host": world.host.to_lowercase(),
        "Port": world.port,
        "UseTls": world.tls,
        "Username": world.username,
    });
    let digest = Sha256::digest(identity.to_string().as_bytes());
    Ok(digest.iter().map(|b| format!("{b:02X}")).collect())
}

/// The system's credential store, or one that explains why there is none.
pub fn system() -> Arc<dyn PasswordVault> {
    #[cfg(all(feature = "vault", any(target_os = "macos", target_os = "windows")))]
    {
        Arc::new(native::KeyringVault)
    }
    #[cfg(all(feature = "vault", target_os = "linux"))]
    {
        Arc::new(secret_tool::SecretToolVault)
    }
    #[cfg(not(all(
        feature = "vault",
        any(target_os = "macos", target_os = "windows", target_os = "linux")
    )))]
    {
        Arc::new(Unavailable)
    }
}

/// No credential store: every call fails with the same readable message.
#[derive(Debug, Default)]
pub struct Unavailable;

impl PasswordVault for Unavailable {
    fn name(&self) -> String {
        t(S::VaultUnavailableName).into()
    }
    fn read(&self, _: &str) -> Result<Option<String>, VaultError> {
        Err(VaultError(t(S::VaultUnavailable).into()))
    }
    fn write(&self, _: &str, _: &str) -> Result<(), VaultError> {
        Err(VaultError(t(S::VaultUnavailable).into()))
    }
    fn delete(&self, _: &str) -> Result<(), VaultError> {
        Err(VaultError(t(S::VaultUnavailable).into()))
    }
}

/// A vault in memory, for tests and scenes. It can be told to fail like a locked store.
#[derive(Debug, Default)]
pub struct MemoryVault {
    entries: Mutex<HashMap<String, String>>,
    locked: AtomicBool,
    /// The name it gives itself (scenes show the system store's name).
    name: Option<String>,
}

impl MemoryVault {
    pub fn new() -> Self {
        Self::default()
    }

    /// A memory vault that calls itself `name` (a scene standing in for the system store).
    pub fn named(name: String) -> Self {
        Self {
            name: Some(name),
            ..Self::default()
        }
    }

    /// Fail every call from now on (or work again).
    pub fn set_locked(&self, locked: bool) {
        self.locked.store(locked, Ordering::Release);
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn check(&self) -> Result<(), VaultError> {
        if self.locked.load(Ordering::Acquire) {
            return Err(VaultError(tf(S::VaultFailed, &[&self.name(), &"locked"])));
        }
        Ok(())
    }
}

impl PasswordVault for MemoryVault {
    fn name(&self) -> String {
        self.name.clone().unwrap_or_else(|| "memory".into())
    }
    fn read(&self, key: &str) -> Result<Option<String>, VaultError> {
        self.check()?;
        Ok(self
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .cloned())
    }
    fn write(&self, key: &str, password: &str) -> Result<(), VaultError> {
        self.check()?;
        validate_password(password).map_err(VaultError)?;
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.to_string(), password.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), VaultError> {
        self.check()?;
        self.entries.lock().unwrap_or_else(PoisonError::into_inner).remove(key);
        Ok(())
    }
}

#[cfg(all(feature = "vault", any(target_os = "macos", target_os = "windows")))]
pub mod native {
    //! macOS Keychain and Windows Credential Manager through `keyring`.

    use super::*;

    /// The platform store. Entries: service [`SERVICE`], account = the vault key.
    #[derive(Debug, Default)]
    pub struct KeyringVault;

    fn failure(e: keyring::Error) -> VaultError {
        let detail = match &e {
            keyring::Error::PlatformFailure(inner) | keyring::Error::NoStorageAccess(inner) => inner.to_string(),
            other => other.to_string(),
        };
        VaultError(tf(S::VaultFailed, &[&KeyringVault.name(), &detail]))
    }

    fn entry(key: &str) -> Result<keyring::Entry, VaultError> {
        keyring::Entry::new(SERVICE, key).map_err(failure)
    }

    /// Read another entry the same way: the C# client's, when its data is imported.
    pub fn read_other(entry: Result<keyring::Entry, keyring::Error>) -> Result<Option<String>, VaultError> {
        read_entry(entry.map_err(failure)?)
    }

    fn read_entry(entry: keyring::Entry) -> Result<Option<String>, VaultError> {
        match entry.get_password() {
            Ok(password) if password.len() > MAX_STORED => Err(VaultError(t(S::SavedPasswordIsTooLarge).into())),
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(keyring::Error::BadEncoding(_)) => Err(VaultError(t(S::SavedPasswordHasAnInvalidFormat).into())),
            Err(e) => Err(failure(e)),
        }
    }

    impl PasswordVault for KeyringVault {
        fn name(&self) -> String {
            if cfg!(target_os = "macos") {
                t(S::KeychainName).into()
            } else {
                t(S::WindowsVaultName).into()
            }
        }

        fn read(&self, key: &str) -> Result<Option<String>, VaultError> {
            read_entry(entry(key)?)
        }

        fn write(&self, key: &str, password: &str) -> Result<(), VaultError> {
            validate_password(password).map_err(VaultError)?;
            entry(key)?.set_password(password).map_err(failure)
        }

        fn delete(&self, key: &str) -> Result<(), VaultError> {
            match entry(key)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(failure(e)),
            }
        }
    }
}

#[cfg(all(feature = "vault", target_os = "linux"))]
pub mod secret_tool {
    //! Linux: libsecret's `secret-tool`, the password on standard input.

    use super::*;
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const TIMEOUT: Duration = Duration::from_secs(60);

    #[derive(Debug, Default)]
    pub struct SecretToolVault;

    /// The `application` attribute of this client's entries.
    pub const APPLICATION: &str = "net.wandur.rust-client";

    /// Look an entry up under another application's attribute (the C# client's
    /// `net.wandur.client`), read only.
    pub fn lookup_in(application: &str, key: &str) -> Result<Option<String>, VaultError> {
        run_in(application, "lookup", key, None)
    }

    fn run(action: &str, key: &str, password: Option<&str>) -> Result<Option<String>, VaultError> {
        run_in(APPLICATION, action, key, password)
    }

    fn run_in(
        application: &str,
        action: &str,
        key: &str,
        password: Option<&str>,
    ) -> Result<Option<String>, VaultError> {
        let mut command = Command::new("secret-tool");
        command.arg(action);
        if action == "store" {
            command.arg("--label=Wandur (Rust) world login");
        }
        command
            .args(["application", application, "credential", key])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|_| VaultError(t(S::InstallSecretToolLibsecretToolsOnDebianUbuntuLibsecret).into()))?;
        if let Some(mut stdin) = child.stdin.take() {
            if let Some(password) = password {
                let _ = stdin.write_all(password.as_bytes());
            }
        }
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let reader = std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut err = Vec::new();
            if let Some(s) = stdout.as_mut() {
                let _ = s.take(MAX_STORED as u64 + 1).read_to_end(&mut out);
            }
            if let Some(s) = stderr.as_mut() {
                let _ = s.take(64 * 1024).read_to_end(&mut err);
            }
            (out, err)
        });
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() > TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(VaultError(
                        t(S::TheKeyringRequestTimedOutUnlockYourDesktopKeyring).into(),
                    ));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => {
                    return Err(VaultError(
                        t(S::SecretServiceCouldNotCompleteTheRequestUnlockYour).into(),
                    ));
                }
            }
        };
        let (out, err) = reader.join().unwrap_or_default();
        if status.success() {
            if out.len() > MAX_STORED {
                return Err(VaultError(t(S::SavedPasswordIsTooLarge).into()));
            }
            // Redirected lookup output has no added newline.
            return String::from_utf8(out)
                .map(Some)
                .map_err(|_| VaultError(t(S::SavedPasswordHasAnInvalidFormat).into()));
        }
        if status.code() == Some(1) && err.is_empty() && matches!(action, "lookup" | "clear") {
            return Ok(None);
        }
        Err(VaultError(
            t(S::SecretServiceCouldNotCompleteTheRequestUnlockYour).into(),
        ))
    }

    impl PasswordVault for SecretToolVault {
        fn name(&self) -> String {
            t(S::LinuxVaultName).into()
        }
        fn read(&self, key: &str) -> Result<Option<String>, VaultError> {
            run("lookup", key, None)
        }
        fn write(&self, key: &str, password: &str) -> Result<(), VaultError> {
            validate_password(password).map_err(VaultError)?;
            run("store", key, Some(password)).map(|_| ())
        }
        fn delete(&self, key: &str) -> Result<(), VaultError> {
            run("clear", key, None).map(|_| ())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> SavedWorld {
        SavedWorld {
            world_id: "0123456789abcdef0123456789abcdef".into(),
            name: "The Lantern Road".into(),
            host: "Lantern.Example.org".into(),
            port: 4000,
            username: "odo".into(),
            password_id: Some("6f9619ff-8b86-d011-b42d-00c04fc964ff".into()),
            ..SavedWorld::default()
        }
    }

    /// The key binds the id, password reference, host (any case), port, TLS and username; the
    /// name and everything else do not count.
    #[test]
    fn the_key_binds_the_login_identity_only() {
        let base = world();
        let k = key(&base).unwrap();
        assert_eq!(k.len(), 64);
        assert!(k.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()));
        let same = [
            SavedWorld {
                name: "Renamed".into(),
                ..base.clone()
            },
            SavedWorld {
                host: "lantern.example.org".into(),
                ..base.clone()
            },
            SavedWorld {
                auto_login: true,
                auto_reconnect: true,
                ..base.clone()
            },
        ];
        for w in &same {
            assert_eq!(key(w).unwrap(), k);
        }
        let different = [
            SavedWorld {
                host: "other.example.org".into(),
                ..base.clone()
            },
            SavedWorld {
                port: 4001,
                ..base.clone()
            },
            SavedWorld {
                tls: true,
                ..base.clone()
            },
            SavedWorld {
                username: "mira".into(),
                ..base.clone()
            },
            SavedWorld {
                world_id: "fedcba9876543210fedcba9876543210".into(),
                ..base.clone()
            },
            SavedWorld {
                password_id: Some("another".into()),
                ..base.clone()
            },
        ];
        for w in &different {
            assert_ne!(key(w).unwrap(), k);
        }
        assert!(
            key(&SavedWorld {
                password_id: None,
                ..base
            })
            .is_err()
        );
    }

    #[test]
    fn passwords_are_one_line_of_1_to_1024_characters() {
        assert!(validate_password("p@ss-π-秘密").is_ok());
        assert!(validate_password(&"x".repeat(1024)).is_ok());
        for bad in [
            String::new(),
            "x".repeat(1025),
            "a\nb".into(),
            "a\rb".into(),
            "a\u{1b}b".into(),
        ] {
            assert!(validate_password(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_locked_memory_vault_fails_readably() {
        let vault = MemoryVault::new();
        vault.write("k", "secret").unwrap();
        vault.set_locked(true);
        let e = vault.read("k").unwrap_err();
        assert!(!e.0.is_empty());
        assert!(vault.write("k", "other").is_err());
        vault.set_locked(false);
        assert_eq!(vault.read("k").unwrap().as_deref(), Some("secret"));
        vault.delete("k").unwrap();
        vault.delete("k").unwrap();
        assert_eq!(vault.read("k").unwrap(), None);
    }

    #[test]
    fn no_store_means_an_error_not_a_fallback() {
        let vault = Unavailable;
        assert!(vault.write("k", "secret").is_err());
        assert!(vault.read("k").is_err());
    }

    /// LoginTests.NativeVaultRoundTripWhenExplicitlyEnabled: the real store with a throwaway
    /// entry, only with `WANDUR_TEST_NATIVE_VAULT=1` (it may ask to unlock the keychain). The
    /// entry is removed afterwards, also when an assertion fails.
    #[test]
    fn native_vault_round_trip_when_explicitly_enabled() {
        if std::env::var("WANDUR_TEST_NATIVE_VAULT").as_deref() != Ok("1") {
            return;
        }
        struct Cleanup(Arc<dyn PasswordVault>, String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = self.0.delete(&self.1);
            }
        }
        let vault = system();
        let key = format!("integration-{}", uuid::Uuid::new_v4().simple());
        let _cleanup = Cleanup(Arc::clone(&vault), key.clone());
        assert_eq!(vault.read(&key).unwrap(), None);
        vault.write(&key, "test-only-π-秘密").unwrap();
        assert_eq!(vault.read(&key).unwrap().as_deref(), Some("test-only-π-秘密"));
        vault.write(&key, "replacement-test").unwrap();
        assert_eq!(vault.read(&key).unwrap().as_deref(), Some("replacement-test"));
        vault.delete(&key).unwrap();
        assert_eq!(vault.read(&key).unwrap(), None);
    }
}
