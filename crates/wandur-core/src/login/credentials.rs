//! Saving and forgetting a world's password, in the C# order (`SaveWorldAsync`):
//!
//! 1. Not remembered: the world keeps no password reference and auto-login is off.
//! 2. A new password: it gets a new reference (so it can never land on an old entry), and is
//!    written to the vault before the settings are.
//! 3. No new password: the saved one is kept while the host (any case, spaces around it aside)
//!    and the username (spaces around it aside) are unchanged; otherwise the person must type it
//!    again. A changed port or TLS keeps it: the vault key binds them, so the saved password is
//!    moved to the new key (read, written under the new key before the settings are, and the old
//!    entry removed after they are). If it cannot be written, nothing is saved.
//! 4. If the settings cannot be written, the entry just created is removed again.
//! 5. A replaced or forgotten entry is removed after the settings are saved; if that fails the
//!    save still stands and a notice says an unused entry may remain.

use crate::l10n::{S, t};
use crate::settings::SavedWorld;

use super::vault::{self, PasswordVault};

/// What a successful save leaves: the world as saved and notices to show.
#[derive(Debug)]
pub struct Saved {
    pub world: SavedWorld,
    pub notices: Vec<String>,
}

/// Save `world` (edited from `original`, `None` for a new world) with the password typed in the
/// editor (`""` keeps the saved one) and whether to remember one. `persist` writes the settings
/// with the world in them; its error is returned as it is. Nothing is written when validation
/// fails.
pub fn save_world(
    vault: &dyn PasswordVault,
    original: Option<&SavedWorld>,
    mut world: SavedWorld,
    password: &str,
    remember: bool,
    persist: impl FnOnce(&SavedWorld) -> Result<(), String>,
) -> Result<Saved, String> {
    let mut created = None;
    // A saved password moved to a new key (the port or TLS changed). Never logged or shown.
    let mut moved = None;
    if !remember {
        world.password_id = None;
        world.auto_login = false;
    } else if !password.is_empty() {
        vault::validate_password(password)?;
        world.password_id = Some(uuid::Uuid::new_v4().to_string());
        created = Some(vault::key(&world)?);
    } else {
        keep_saved(original, &world)?;
        if let Some(original) = original {
            let old = vault::key(original)?;
            let new = vault::key(&world)?;
            if old != new {
                // An entry that is not there (passwords not imported) has nothing to move; the
                // editor says it is missing, as it does without a move.
                if let Some(secret) = vault.read(&old).map_err(|e| e.0)? {
                    moved = Some(secret);
                    created = Some(new);
                }
            }
        }
    }
    world.validate()?;
    if let Some(key) = &created {
        vault
            .write(key, moved.as_deref().unwrap_or(password))
            .map_err(|e| e.0)?;
    }
    if let Err(e) = persist(&world) {
        if let Some(key) = &created
            && vault.delete(key).is_err()
        {
            return Err(format!("{e}\n{}", t(S::WorldWasNotSavedAnUnusedPasswordMayRemain)));
        }
        return Err(e);
    }
    let mut notices = Vec::new();
    if let Some(original) = original
        && original.password_id.is_some()
    {
        let old = vault::key(original)?;
        let still_used = world.password_id.is_some() && vault::key(&world)? == old;
        if !still_used && let Some(notice) = forget(vault, original) {
            notices.push(notice);
        }
    }
    Ok(Saved { world, notices })
}

/// Whether a blank password can keep `original`'s saved one for `world`: same world, same
/// password reference, same host (any case, spaces around it aside) and same username (spaces
/// around it aside). The port and TLS may differ; `save_world` then moves the entry to the new
/// key. The error says why not: there is no saved password to keep, or the login changed.
pub fn keep_saved(original: Option<&SavedWorld>, world: &SavedWorld) -> Result<(), String> {
    match (original, &world.password_id) {
        (Some(original), Some(_)) if original.password_id.is_some() => {
            if same_login(original, world) {
                Ok(())
            } else {
                Err(t(S::PasswordLoginChanged).into())
            }
        }
        _ => Err(t(S::EnterAPasswordToSaveForThisServerAnd).into()),
    }
}

/// Whether `world` is the same login as `original` for its saved password: both have the same
/// password reference, the same world id, the same host (any case, trimmed) and the same
/// username (trimmed). The port and TLS do not count.
pub fn same_login(original: &SavedWorld, world: &SavedWorld) -> bool {
    original.password_id.is_some()
        && original.password_id == world.password_id
        && original.world_id == world.world_id
        && original.host.trim().to_lowercase() == world.host.trim().to_lowercase()
        && original.username.trim() == world.username.trim()
}

/// Whether the vault holds `world`'s saved password: `Ok(false)` when the world refers to one
/// that is not there (passwords not imported, an entry removed outside the app). It reads the
/// entry (the value is dropped at once), so the app calls it off the UI thread.
pub fn has_saved(vault: &dyn PasswordVault, world: &SavedWorld) -> Result<bool, String> {
    let key = vault::key(world)?;
    vault.read(&key).map(|found| found.is_some()).map_err(|e| e.0)
}

/// Remove a world's saved password (the world was removed, or its password replaced). Returns a
/// notice when the entry could not be removed.
pub fn forget(vault: &dyn PasswordVault, world: &SavedWorld) -> Option<String> {
    let key = vault::key(world).ok()?;
    vault
        .delete(&key)
        .err()
        .map(|_| t(S::WorldSettingsSavedButTheOldPasswordCouldNot).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::login::vault::MemoryVault;
    use std::cell::RefCell;

    fn world(host: &str) -> SavedWorld {
        SavedWorld {
            world_id: "0123456789abcdef0123456789abcdef".into(),
            name: "First".into(),
            host: host.into(),
            port: 4000,
            username: "player".into(),
            auto_login: true,
            ..SavedWorld::default()
        }
    }

    /// The settings file stand-in: what was persisted, as JSON.
    #[derive(Default)]
    struct Store {
        saved: RefCell<Vec<SavedWorld>>,
        fail: bool,
    }

    impl Store {
        fn persist(&self) -> impl FnOnce(&SavedWorld) -> Result<(), String> + '_ {
            move |w| {
                if self.fail {
                    return Err("could not write settings".into());
                }
                self.saved.borrow_mut().push(w.clone());
                Ok(())
            }
        }
        fn json(&self) -> String {
            serde_json::to_string(&*self.saved.borrow()).unwrap()
        }
    }

    /// LoginTests.ProfileEditKeepsPasswordOnlyForSameEndpointAndAccount, with this client's rule:
    /// renaming keeps the password; a new host or username needs a new one (and drops the old
    /// entry); not remembering forgets it and turns auto-login off; no password ever reaches the
    /// settings. (A changed port or TLS keeps it: `a_port_or_tls_change_moves_the_password`.)
    #[test]
    fn profile_edit_keeps_password_only_for_same_endpoint_and_account() {
        let vault = MemoryVault::new();
        let store = Store::default();
        let saved = save_world(
            &vault,
            None,
            world("first.example.org"),
            "secret-one",
            true,
            store.persist(),
        )
        .unwrap();
        let first = saved.world;
        let old_key = vault::key(&first).unwrap();
        assert_eq!(vault.read(&old_key).unwrap().as_deref(), Some("secret-one"));

        let renamed = SavedWorld {
            name: "Renamed".into(),
            ..first.clone()
        };
        let renamed = save_world(&vault, Some(&first), renamed, "", true, store.persist())
            .unwrap()
            .world;
        assert_eq!(renamed.password_id, first.password_id);
        assert_eq!(vault.read(&old_key).unwrap().as_deref(), Some("secret-one"));

        for edit in [
            SavedWorld {
                host: "other.example.org".into(),
                ..first.clone()
            },
            SavedWorld {
                username: "someone-else".into(),
                ..first.clone()
            },
            SavedWorld {
                host: "other.example.org".into(),
                port: 4001,
                tls: true,
                ..first.clone()
            },
        ] {
            let before = store.saved.borrow().len();
            let e = save_world(&vault, Some(&first), edit, "", true, store.persist()).unwrap_err();
            assert_eq!(e, t(S::PasswordLoginChanged));
            assert_eq!(store.saved.borrow().len(), before, "nothing was written");
            assert_eq!(vault.read(&old_key).unwrap().as_deref(), Some("secret-one"));
            assert_eq!(vault.len(), 1);
        }

        let moved = SavedWorld {
            host: "other.example.org".into(),
            ..first.clone()
        };
        let moved = save_world(&vault, Some(&first), moved, "secret-two", true, store.persist())
            .unwrap()
            .world;
        assert_eq!(vault.read(&old_key).unwrap(), None, "the old entry is gone");
        assert_eq!(
            vault.read(&vault::key(&moved).unwrap()).unwrap().as_deref(),
            Some("secret-two")
        );
        assert_eq!(vault.len(), 1);

        let forgotten = save_world(&vault, Some(&moved), moved.clone(), "", false, store.persist())
            .unwrap()
            .world;
        assert_eq!(forgotten.password_id, None);
        assert!(!forgotten.auto_login);
        assert!(vault.is_empty());
        assert!(!store.json().contains("secret-"));
    }

    /// LoginTests.FailedSettingsSaveRemovesNewCredentialAndKeepsExistingCredential.
    #[test]
    fn failed_settings_save_removes_the_new_credential_and_keeps_the_existing_one() {
        let vault = MemoryVault::new();
        let ok = Store::default();
        let first = save_world(
            &vault,
            None,
            world("example.org"),
            "original-secret",
            true,
            ok.persist(),
        )
        .unwrap()
        .world;
        let original_key = vault::key(&first).unwrap();
        let failing = Store {
            fail: true,
            ..Store::default()
        };
        let e = save_world(
            &vault,
            Some(&first),
            first.clone(),
            "replacement-secret",
            true,
            failing.persist(),
        )
        .unwrap_err();
        assert!(e.contains("could not write settings"));
        assert_eq!(vault.len(), 1);
        assert_eq!(vault.read(&original_key).unwrap().as_deref(), Some("original-secret"));
        // Removing the world forgets its password.
        assert_eq!(forget(&vault, &first), None);
        assert!(vault.is_empty());
    }

    /// Vault failures are readable errors and nothing falls back to the settings file.
    #[test]
    fn a_locked_vault_blocks_the_save_with_a_readable_error() {
        let vault = MemoryVault::new();
        vault.set_locked(true);
        let store = Store::default();
        let e = save_world(&vault, None, world("example.org"), "secret-x", true, store.persist()).unwrap_err();
        assert!(!e.is_empty());
        assert!(
            store.saved.borrow().is_empty(),
            "the world was not saved without its password"
        );
        // Saving without a password needs no vault.
        let plain = SavedWorld {
            auto_login: false,
            ..world("example.org")
        };
        assert!(save_world(&vault, None, plain, "", false, store.persist()).is_ok());
        assert!(!store.json().contains("secret-x"));
    }

    /// An old entry that cannot be removed leaves the save in place, with a notice.
    #[test]
    fn a_stuck_old_entry_is_a_notice() {
        struct NoDelete(MemoryVault);
        impl PasswordVault for NoDelete {
            fn name(&self) -> String {
                self.0.name()
            }
            fn read(&self, key: &str) -> Result<Option<String>, vault::VaultError> {
                self.0.read(key)
            }
            fn write(&self, key: &str, password: &str) -> Result<(), vault::VaultError> {
                self.0.write(key, password)
            }
            fn delete(&self, _: &str) -> Result<(), vault::VaultError> {
                Err(vault::VaultError("locked".into()))
            }
        }
        let vault = NoDelete(MemoryVault::new());
        let store = Store::default();
        let first = save_world(&vault, None, world("example.org"), "one", true, store.persist())
            .unwrap()
            .world;
        let saved = save_world(&vault, Some(&first), first.clone(), "two", true, store.persist()).unwrap();
        assert_eq!(saved.notices, [t(S::WorldSettingsSavedButTheOldPasswordCouldNot)]);
    }

    /// AutoLoginTests.CredentialsCannotInjectCommands, and auto-login needs both parts.
    #[test]
    fn credentials_cannot_inject_commands() {
        for username in ["foo\rbar", "foo\nbar", "foo\u{1b}bar"] {
            let w = SavedWorld {
                username: username.into(),
                ..world("mud.example.org")
            };
            assert!(w.validate().is_err(), "{username:?}");
        }
        let no_password = world("mud.example.org");
        assert!(no_password.validate().is_err(), "auto-login without a saved password");
        let vault = MemoryVault::new();
        let store = Store::default();
        let no_name = SavedWorld {
            username: " ".into(),
            ..world("mud.example.org")
        };
        assert!(save_world(&vault, None, no_name, "secret", true, store.persist()).is_err());
        assert!(vault.is_empty(), "validation runs before the vault is written");
    }

    /// A blank password keeps the saved one only for the same entry, and says why not: no saved
    /// password to keep (a new world, or one without a reference) or a changed login identity.
    /// Fields outside the identity (name, scripts, channels, theme, auto-login) never matter.
    #[test]
    fn keep_saved_names_the_reason() {
        let saved = SavedWorld {
            password_id: Some("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeee01".into()),
            ..world("Lanes.Example.org")
        };
        let mut other = saved.clone();
        other.name = "Renamed".into();
        other.auto_login = false;
        other.codebase = "SMAUG".into();
        other.channel_rules.push(crate::channels::ChannelRule {
            channel: "ooc".into(),
            pattern: "^OOC (?<speaker>\\w+): (?<text>.*)$".into(),
            ..Default::default()
        });
        assert_eq!(keep_saved(Some(&saved), &other), Ok(()));
        // The host compares without case.
        let upper = SavedWorld {
            host: "LANES.EXAMPLE.ORG".into(),
            ..saved.clone()
        };
        assert_eq!(keep_saved(Some(&saved), &upper), Ok(()));
        // The port and TLS do not count (Save world moves the entry); nor do spaces around the
        // host or username.
        for kept in [
            SavedWorld {
                port: 4443,
                ..saved.clone()
            },
            SavedWorld {
                tls: true,
                ..saved.clone()
            },
            SavedWorld {
                tls: true,
                port: 4443,
                ..saved.clone()
            },
            SavedWorld {
                host: " lanes.example.org ".into(),
                username: " player ".into(),
                ..saved.clone()
            },
        ] {
            assert_eq!(keep_saved(Some(&saved), &kept), Ok(()));
        }
        assert_eq!(
            keep_saved(None, &saved),
            Err(t(S::EnterAPasswordToSaveForThisServerAnd).into())
        );
        let unsaved = SavedWorld {
            password_id: None,
            ..saved.clone()
        };
        assert_eq!(
            keep_saved(Some(&unsaved), &saved),
            Err(t(S::EnterAPasswordToSaveForThisServerAnd).into())
        );
        for changed in [
            SavedWorld {
                username: "Talek ".into(),
                ..saved.clone()
            },
            SavedWorld {
                username: "Player".into(),
                ..saved.clone()
            },
            SavedWorld {
                host: "other.example.org".into(),
                ..saved.clone()
            },
            SavedWorld {
                host: "other.example.org".into(),
                tls: true,
                port: 4443,
                ..saved.clone()
            },
            SavedWorld {
                world_id: "f".repeat(32),
                ..saved.clone()
            },
            SavedWorld {
                password_id: Some("other".into()),
                ..saved.clone()
            },
        ] {
            assert_eq!(
                keep_saved(Some(&saved), &changed),
                Err(t(S::PasswordLoginChanged).into())
            );
        }
    }

    /// Owner report: switching a world from plain telnet to TLS (often with a new port) on the
    /// same host and username said "Enter the password again". A blank password now keeps it:
    /// Save world moves it to the new key (the old entry is gone, the new one holds it, auto-login
    /// finds it under the saved world's key) and keeps the reference. Host case and spaces
    /// around the host or username do not count either.
    #[test]
    fn a_port_or_tls_change_moves_the_password() {
        let edits: [fn(&SavedWorld) -> SavedWorld; 7] = [
            |w| SavedWorld { tls: true, ..w.clone() },
            |w| SavedWorld {
                tls: true,
                port: 4443,
                ..w.clone()
            },
            |w| SavedWorld {
                port: 4001,
                ..w.clone()
            },
            |w| SavedWorld {
                tls: false,
                ..w.clone()
            },
            |w| SavedWorld {
                tls: false,
                port: 23,
                ..w.clone()
            },
            |w| SavedWorld {
                host: " MUD.Example.ORG ".into(),
                port: 4443,
                tls: true,
                ..w.clone()
            },
            |w| SavedWorld {
                username: " player ".into(),
                tls: true,
                ..w.clone()
            },
        ];
        for (n, edit) in edits.iter().enumerate() {
            let vault = MemoryVault::new();
            let store = Store::default();
            // The TLS-off edits start from a TLS world.
            let start = SavedWorld {
                tls: n == 3 || n == 4,
                ..world("mud.example.org")
            };
            let first = save_world(&vault, None, start, "secret-move", true, store.persist())
                .unwrap()
                .world;
            let old_key = vault::key(&first).unwrap();
            let edited = edit(&first);
            assert_eq!(keep_saved(Some(&first), &edited), Ok(()), "edit {n}");
            let saved = save_world(&vault, Some(&first), edited, "", true, store.persist())
                .unwrap_or_else(|e| panic!("edit {n}: {e}"));
            assert!(saved.notices.is_empty(), "edit {n}");
            let moved = saved.world;
            assert_eq!(moved.password_id, first.password_id, "edit {n}: same reference");
            assert!(moved.auto_login);
            let new_key = vault::key(&moved).unwrap();
            assert_ne!(new_key, old_key, "edit {n}");
            assert_eq!(vault.read(&old_key).unwrap(), None, "edit {n}: the old entry is gone");
            assert_eq!(
                vault.read(&new_key).unwrap().as_deref(),
                Some("secret-move"),
                "edit {n}"
            );
            assert_eq!(vault.len(), 1, "edit {n}");
            // What was persisted is the moved world, and auto-login finds the password with it.
            let persisted = store.saved.borrow().last().unwrap().clone();
            assert_eq!(has_saved(&vault, &persisted), Ok(true), "edit {n}");
            assert!(!store.json().contains("secret-move"));
        }
    }

    /// A move whose write fails (a locked store, here one that refuses writes) saves nothing:
    /// the settings are not written, the old entry stays and still holds the password, and the
    /// error is the store's. A move whose settings write fails removes the new entry again and
    /// keeps the old one.
    #[test]
    fn a_failed_move_leaves_everything_as_it_was() {
        struct NoWrite(MemoryVault, std::sync::atomic::AtomicBool);
        impl PasswordVault for NoWrite {
            fn name(&self) -> String {
                self.0.name()
            }
            fn read(&self, key: &str) -> Result<Option<String>, vault::VaultError> {
                self.0.read(key)
            }
            fn write(&self, key: &str, password: &str) -> Result<(), vault::VaultError> {
                if self.1.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(vault::VaultError("store refused the write".into()));
                }
                self.0.write(key, password)
            }
            fn delete(&self, key: &str) -> Result<(), vault::VaultError> {
                self.0.delete(key)
            }
        }
        let vault = NoWrite(MemoryVault::new(), Default::default());
        let store = Store::default();
        let first = save_world(
            &vault,
            None,
            world("mud.example.org"),
            "secret-stay",
            true,
            store.persist(),
        )
        .unwrap()
        .world;
        let old_key = vault::key(&first).unwrap();
        let written = store.saved.borrow().len();
        vault.1.store(true, std::sync::atomic::Ordering::Relaxed);
        let tls = SavedWorld {
            tls: true,
            port: 4443,
            ..first.clone()
        };
        let e = save_world(&vault, Some(&first), tls.clone(), "", true, store.persist()).unwrap_err();
        assert_eq!(e, "store refused the write");
        assert!(!e.contains("secret-stay"));
        assert_eq!(store.saved.borrow().len(), written, "the settings were not written");
        assert_eq!(vault.0.len(), 1);
        assert_eq!(vault.read(&old_key).unwrap().as_deref(), Some("secret-stay"));
        assert_eq!(has_saved(&vault, &first), Ok(true), "the old world still logs in");

        // A store that cannot be read cannot move the password either.
        vault.1.store(false, std::sync::atomic::Ordering::Relaxed);
        vault.0.set_locked(true);
        assert!(save_world(&vault, Some(&first), tls.clone(), "", true, store.persist()).is_err());
        assert_eq!(store.saved.borrow().len(), written);
        vault.0.set_locked(false);

        // The settings cannot be written: the new entry goes, the old one stays.
        let failing = Store {
            fail: true,
            ..Store::default()
        };
        let e = save_world(&vault, Some(&first), tls.clone(), "", true, failing.persist()).unwrap_err();
        assert!(e.contains("could not write settings"));
        assert_eq!(vault.0.len(), 1);
        assert_eq!(vault.read(&old_key).unwrap().as_deref(), Some("secret-stay"));
        let new_key = vault::key(&tls).unwrap();
        assert_eq!(vault.read(&new_key).unwrap(), None);
    }

    /// `has_saved` tells a reference with an entry from one without (passwords not imported).
    #[test]
    fn has_saved_finds_a_missing_entry() {
        let vault = MemoryVault::new();
        let store = Store::default();
        let saved = save_world(&vault, None, world("example.org"), "secret-z", true, store.persist())
            .unwrap()
            .world;
        assert_eq!(has_saved(&vault, &saved), Ok(true));
        let dangling = SavedWorld {
            password_id: Some("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeee02".into()),
            ..saved.clone()
        };
        assert_eq!(has_saved(&vault, &dangling), Ok(false));
        vault.set_locked(true);
        assert!(has_saved(&vault, &saved).is_err());
    }
}
