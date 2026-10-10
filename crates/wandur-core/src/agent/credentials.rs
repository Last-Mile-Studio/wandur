//! The optional API key (the C# `AgentClientServices`): kept in the system vault, never in
//! `wandur.db`, under a key bound to the profile's id, its credential reference, its provider
//! and its endpoint. Saving a profile with another provider or endpoint drops the reference, so
//! a key never follows its profile to a new destination; a new key must be entered for it.

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::AgentError;
use super::profile::{self, AgentProfile};
use super::store::{AgentProfileStore, AgentWorld};
use crate::login::PasswordVault;

/// The vault key of the profile's API key: `agent-` and the SHA-256 (upper-case hex) of
/// `id:credential:provider:endpoint`, as the C# client builds it.
pub fn credential_key(profile: &AgentProfile) -> String {
    let credential = profile.credential_id.map(|c| c.to_string()).unwrap_or_default();
    let identity = [
        profile.id.to_string().as_str(),
        ":",
        &credential,
        ":",
        &profile.provider,
        ":",
        profile.endpoint.trim_end_matches('/'),
    ]
    .concat();
    let digest = Sha256::digest(identity.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02X}")).collect();
    ["agent-", &hex].concat()
}

/// The profile's API key, or `None` when it has none. Blocks on the vault.
pub fn read(vault: &dyn PasswordVault, profile: &AgentProfile) -> Result<Option<String>, AgentError> {
    if profile.credential_id.is_none() {
        return Ok(None);
    }
    vault
        .read(&credential_key(profile))
        .map_err(|e| AgentError::Storage(e.to_string()))
}

/// Save `profile` for `world` with its key: `new_key` (empty keeps the saved one) or none
/// (`forget`). The profile keeps the stored profile's id. A new key is written to the vault
/// first and removed again when the profile cannot be saved; the replaced key is removed after
/// the profile is saved. Returns the profile as saved.
pub fn save(
    store: &dyn AgentProfileStore,
    vault: &dyn PasswordVault,
    world: &AgentWorld,
    profile: AgentProfile,
    new_key: &str,
    forget: bool,
) -> Result<AgentProfile, AgentError> {
    profile::validate(&profile, false)?;
    let previous = store.load(world)?;
    let same_destination = previous.provider == profile.provider
        && previous.endpoint.trim_end_matches('/') == profile.endpoint.trim_end_matches('/');
    let mut profile = AgentProfile {
        id: previous.id,
        credential_id: if same_destination && !forget {
            previous.credential_id
        } else {
            None
        },
        ..profile
    };
    if !new_key.is_empty() {
        crate::login::vault::validate_password(new_key).map_err(|_| AgentError::Invalid("Invalid API credential."))?;
        profile.credential_id = Some(Uuid::new_v4());
        vault
            .write(&credential_key(&profile), new_key)
            .map_err(|e| AgentError::Storage(e.to_string()))?;
    }
    if let Err(e) = store.save(world, &profile) {
        if profile.credential_id.is_some() && profile.credential_id != previous.credential_id {
            let _ = vault.delete(&credential_key(&profile));
        }
        return Err(e);
    }
    if previous.credential_id.is_some() && previous.credential_id != profile.credential_id {
        let _ = vault.delete(&credential_key(&previous));
    }
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::store::MemoryAgentProfileStore;
    use crate::login::MemoryVault;

    /// A store that can be told to fail (AgentCredentialTests' Store).
    struct Failing {
        inner: MemoryAgentProfileStore,
        fail: std::sync::atomic::AtomicBool,
    }

    impl AgentProfileStore for Failing {
        fn load(&self, world: &AgentWorld) -> Result<AgentProfile, AgentError> {
            self.inner.load(world)
        }
        fn save(&self, world: &AgentWorld, profile: &AgentProfile) -> Result<(), AgentError> {
            if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(AgentError::Storage("disk full".into()));
            }
            self.inner.save(world, profile)
        }
    }

    /// AgentCredentialTests.TokenLivesInVaultAndCannotFollowAnEndpointOrIntegrationChange.
    #[test]
    fn the_key_lives_in_the_vault_and_never_follows_a_new_destination() {
        let store = MemoryAgentProfileStore::new();
        let vault = MemoryVault::new();
        let world = AgentWorld::Key("world".into());
        let initial = store.load(&world).unwrap();
        let saved = save(&store, &vault, &world, initial, "token-secret", false).unwrap();
        assert!(saved.credential_id.is_some());
        assert_eq!(read(&vault, &saved).unwrap().as_deref(), Some("token-secret"));
        assert!(!serde_json::to_string(&saved).unwrap().contains("token-secret"));
        let moved = save(
            &store,
            &vault,
            &world,
            AgentProfile {
                endpoint: "http://localhost:9999/v1".into(),
                ..saved
            },
            "",
            false,
        )
        .unwrap();
        assert!(moved.credential_id.is_none());
        assert!(vault.is_empty());
        let saved = save(&store, &vault, &world, moved, "new-token", false).unwrap();
        let moved = save(
            &store,
            &vault,
            &world,
            AgentProfile {
                provider: "future".into(),
                ..saved
            },
            "",
            false,
        )
        .unwrap();
        assert!(moved.credential_id.is_none());
        assert!(vault.is_empty());
    }

    /// AgentCredentialTests.FailedPersistenceRemovesNewTokenAndPreservesOldOne.
    #[test]
    fn a_failed_save_removes_the_new_key_and_keeps_the_old_one() {
        let store = Failing {
            inner: MemoryAgentProfileStore::new(),
            fail: false.into(),
        };
        let vault = MemoryVault::new();
        let world = AgentWorld::Key("world".into());
        let initial = store.load(&world).unwrap();
        let saved = save(&store, &vault, &world, initial, "old-token", false).unwrap();
        store.fail.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(save(&store, &vault, &world, saved.clone(), "new-token", false).is_err());
        assert_eq!(vault.len(), 1);
        assert_eq!(read(&vault, &saved).unwrap().as_deref(), Some("old-token"));
    }

    #[test]
    fn forgetting_drops_the_key_and_a_blank_key_keeps_it() {
        let store = MemoryAgentProfileStore::new();
        let vault = MemoryVault::new();
        let world = AgentWorld::Key("world".into());
        let initial = store.load(&world).unwrap();
        let saved = save(&store, &vault, &world, initial, "token", false).unwrap();
        let kept = save(&store, &vault, &world, saved.clone(), "", false).unwrap();
        assert_eq!(kept.credential_id, saved.credential_id);
        assert_eq!(vault.len(), 1);
        let forgotten = save(&store, &vault, &world, kept, "", true).unwrap();
        assert!(forgotten.credential_id.is_none());
        assert!(vault.is_empty());
        assert_eq!(read(&vault, &forgotten).unwrap(), None);
        assert!(save(&store, &vault, &world, forgotten, "bad\nkey", false).is_err());
        assert!(credential_key(&saved).starts_with("agent-"));
        assert_eq!(credential_key(&saved).len(), 6 + 64);
    }
}
