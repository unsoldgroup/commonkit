use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use commonkit_contracts::StableId;
use commonkit_contracts::portable_context::{GitHubOnboardingAttempt, GitHubOnboardingState};
use commonkit_platform::{PrivatePathKind, ensure_private_path};
use serde::{Deserialize, Serialize};
use thiserror::Error;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoreState {
    attempts: BTreeMap<StableId, GitHubOnboardingAttempt>,
}

pub struct OnboardingStateStore {
    path: PathBuf,
    state: Mutex<StoreState>,
}

impl OnboardingStateStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OnboardingStoreError> {
        let path = path.as_ref().to_path_buf();
        if !path.is_absolute() || path.parent().is_none() {
            return Err(OnboardingStoreError::UnsafePath);
        }
        ensure_private_path(
            path.parent().expect("checked parent"),
            PrivatePathKind::Directory,
        )?;
        let state = match std::fs::read(&path) {
            Ok(bytes) => {
                commonkit_platform::verify_private_path(&path, PrivatePathKind::File)?;
                let state: StoreState = serde_json::from_slice(&bytes)?;
                for attempt in state.attempts.values() {
                    attempt.validate()?;
                }
                state
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => StoreState::default(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<GitHubOnboardingAttempt>, OnboardingStoreError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| OnboardingStoreError::Unavailable)?
            .attempts
            .values()
            .find(|attempt| attempt.id.as_str() == id)
            .cloned())
    }

    pub fn begin(&self, attempt: GitHubOnboardingAttempt) -> Result<(), OnboardingStoreError> {
        attempt.validate()?;
        if attempt.state != GitHubOnboardingState::Planned {
            return Err(OnboardingStoreError::InvalidTransition);
        }
        self.mutate(|state| {
            if let Some(existing) = state.attempts.get(&attempt.id) {
                if existing == &attempt {
                    return Ok(());
                }
                return Err(OnboardingStoreError::DuplicateAttempt);
            }
            if state
                .attempts
                .values()
                .any(|existing| existing.idempotency_key == attempt.idempotency_key)
            {
                return Err(OnboardingStoreError::DuplicateAttempt);
            }
            state.attempts.insert(attempt.id.clone(), attempt);
            Ok(())
        })
    }

    pub fn mark_remote_created(
        &self,
        id: &str,
        repository_node_id: impl Into<String>,
        updated_at_unix_ms: u64,
    ) -> Result<(), OnboardingStoreError> {
        let repository_node_id = repository_node_id.into();
        self.transition(
            id,
            GitHubOnboardingState::RemoteCreated,
            updated_at_unix_ms,
            |attempt| {
                if attempt
                    .repository_node_id
                    .as_ref()
                    .is_some_and(|existing| existing != &repository_node_id)
                {
                    return Err(OnboardingStoreError::NodeIdSubstitution);
                }
                attempt.repository_node_id = Some(repository_node_id);
                Ok(())
            },
        )
    }

    pub fn mark_orphaned(
        &self,
        id: &str,
        failure_code: StableId,
        updated_at_unix_ms: u64,
    ) -> Result<(), OnboardingStoreError> {
        self.transition(
            id,
            GitHubOnboardingState::Orphaned,
            updated_at_unix_ms,
            |attempt| {
                attempt.failure_code = Some(failure_code);
                Ok(())
            },
        )
    }

    pub fn mark_registering(
        &self,
        id: &str,
        updated_at_unix_ms: u64,
    ) -> Result<(), OnboardingStoreError> {
        self.transition(
            id,
            GitHubOnboardingState::Registering,
            updated_at_unix_ms,
            |_| Ok(()),
        )
    }

    pub fn mark_registered(
        &self,
        id: &str,
        registration_id: StableId,
        updated_at_unix_ms: u64,
    ) -> Result<(), OnboardingStoreError> {
        self.transition(
            id,
            GitHubOnboardingState::Registered,
            updated_at_unix_ms,
            |attempt| {
                if attempt
                    .registration_id
                    .as_ref()
                    .is_some_and(|existing| existing != &registration_id)
                {
                    return Err(OnboardingStoreError::RegistrationSubstitution);
                }
                attempt.registration_id = Some(registration_id);
                Ok(())
            },
        )
    }

    pub fn mark_verifying(
        &self,
        id: &str,
        updated_at_unix_ms: u64,
    ) -> Result<(), OnboardingStoreError> {
        self.transition(
            id,
            GitHubOnboardingState::Verifying,
            updated_at_unix_ms,
            |_| Ok(()),
        )
    }

    pub fn complete(
        &self,
        id: &str,
        registration_id: StableId,
        updated_at_unix_ms: u64,
    ) -> Result<(), OnboardingStoreError> {
        if let Some(existing) = self.get(id)? {
            if existing.state == GitHubOnboardingState::Completed {
                return if existing.registration_id.as_ref() == Some(&registration_id) {
                    Ok(())
                } else {
                    Err(OnboardingStoreError::RegistrationSubstitution)
                };
            }
        }
        self.transition(
            id,
            GitHubOnboardingState::Completed,
            updated_at_unix_ms,
            |attempt| {
                if attempt.registration_id.as_ref() != Some(&registration_id) {
                    return Err(OnboardingStoreError::RegistrationSubstitution);
                }
                Ok(())
            },
        )
    }

    fn transition<F>(
        &self,
        id: &str,
        next: GitHubOnboardingState,
        updated_at_unix_ms: u64,
        update: F,
    ) -> Result<(), OnboardingStoreError>
    where
        F: FnOnce(&mut GitHubOnboardingAttempt) -> Result<(), OnboardingStoreError>,
    {
        self.mutate(|state| {
            let attempt = state
                .attempts
                .values_mut()
                .find(|attempt| attempt.id.as_str() == id)
                .ok_or(OnboardingStoreError::UnknownAttempt)?;
            if !attempt.can_transition_to(next) || updated_at_unix_ms < attempt.updated_at_unix_ms {
                return Err(OnboardingStoreError::InvalidTransition);
            }
            update(attempt)?;
            attempt.state = next;
            attempt.updated_at_unix_ms = updated_at_unix_ms;
            attempt.validate()?;
            Ok(())
        })
    }

    fn mutate<F>(&self, update: F) -> Result<(), OnboardingStoreError>
    where
        F: FnOnce(&mut StoreState) -> Result<(), OnboardingStoreError>,
    {
        let mut state = self
            .state
            .lock()
            .map_err(|_| OnboardingStoreError::Unavailable)?;
        let mut candidate = state.clone();
        update(&mut candidate)?;
        self.persist(&candidate)?;
        *state = candidate;
        Ok(())
    }

    fn persist(&self, state: &StoreState) -> Result<(), OnboardingStoreError> {
        let bytes = serde_json::to_vec(state)?;
        let temporary = self.path.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        ensure_private_path(&temporary, PrivatePathKind::File)?;
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &self.path)?;
            ensure_private_path(&self.path, PrivatePathKind::File)?;
            let parent =
                std::fs::File::open(self.path.parent().ok_or(OnboardingStoreError::UnsafePath)?)?;
            parent.sync_all()?;
            Ok::<_, OnboardingStoreError>(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }
}

#[derive(Debug, Error)]
pub enum OnboardingStoreError {
    #[error("onboarding store path is unsafe")]
    UnsafePath,
    #[error("onboarding store is unavailable")]
    Unavailable,
    #[error("onboarding attempt does not exist")]
    UnknownAttempt,
    #[error("onboarding attempt already exists with different inputs")]
    DuplicateAttempt,
    #[error("onboarding state transition is invalid")]
    InvalidTransition,
    #[error("GitHub repository node ID substitution was rejected")]
    NodeIdSubstitution,
    #[error("onboarding registration substitution was rejected")]
    RegistrationSubstitution,
    #[error(transparent)]
    Contract(#[from] commonkit_contracts::portable_context::PortableContextError),
    #[error(transparent)]
    Platform(#[from] commonkit_platform::PlatformError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
