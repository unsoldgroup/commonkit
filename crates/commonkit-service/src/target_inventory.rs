use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

use commonkit_contracts::{Sha256Digest, StableId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TargetTransport {
    Local,
    Ssh {
        host: String,
        user: String,
        port: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetRecord {
    pub id: StableId,
    pub transport: TargetTransport,
    pub identity_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SelectionState {
    selected: Vec<StableId>,
}

#[derive(Debug)]
pub struct TargetInventory {
    state_path: PathBuf,
    targets: BTreeMap<StableId, TargetRecord>,
    selected: RwLock<Vec<StableId>>,
    selection_lock: Mutex<()>,
}

impl TargetInventory {
    pub fn open(
        state_path: impl AsRef<Path>,
        targets: Vec<TargetRecord>,
        configured_selection: Option<Vec<StableId>>,
    ) -> Result<Self, TargetInventoryError> {
        if targets.is_empty() {
            return Err(TargetInventoryError::Empty);
        }
        let mut by_id = BTreeMap::new();
        let mut identities = BTreeSet::new();
        for target in targets {
            if by_id.insert(target.id.clone(), target.clone()).is_some() {
                return Err(TargetInventoryError::DuplicateId(target.id));
            }
            if !identities.insert(target.identity_digest.clone()) {
                return Err(TargetInventoryError::DuplicateIdentity);
            }
        }
        let state_path = state_path.as_ref().to_path_buf();
        let selected = if state_path.exists() {
            serde_json::from_slice::<SelectionState>(&fs::read(&state_path)?)?.selected
        } else if let Some(selected) = configured_selection {
            selected
        } else if by_id.len() == 1 {
            by_id.keys().cloned().collect()
        } else {
            Vec::new()
        };
        validate_selection(&by_id, &selected)?;
        let inventory = Self {
            state_path,
            targets: by_id,
            selected: RwLock::new(selected),
            selection_lock: Mutex::new(()),
        };
        if !inventory.state_path.exists() {
            inventory.persist_selection(&inventory.selected())?;
        }
        Ok(inventory)
    }

    pub fn targets(&self) -> Vec<TargetRecord> {
        self.targets.values().cloned().collect()
    }

    pub fn selected(&self) -> Vec<StableId> {
        self.selected.read().expect("target selection lock").clone()
    }

    pub fn select(&self, selected: Vec<StableId>) -> Result<(), TargetInventoryError> {
        let _guard = self
            .selection_lock
            .lock()
            .map_err(|_| TargetInventoryError::Lock)?;
        validate_selection(&self.targets, &selected)?;
        self.persist_selection(&selected)?;
        *self.selected.write().expect("target selection lock") = selected;
        Ok(())
    }

    fn persist_selection(&self, selected: &[StableId]) -> Result<(), TargetInventoryError> {
        let parent = self
            .state_path
            .parent()
            .ok_or(TargetInventoryError::UnsafeStatePath)?;
        fs::create_dir_all(parent)?;
        let temporary = self.state_path.with_extension("tmp");
        let bytes = serde_json::to_vec(&SelectionState {
            selected: selected.to_vec(),
        })?;
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, &self.state_path)?;
        Ok(())
    }
}

fn validate_selection(
    targets: &BTreeMap<StableId, TargetRecord>,
    selected: &[StableId],
) -> Result<(), TargetInventoryError> {
    let mut unique = BTreeSet::new();
    for id in selected {
        if !targets.contains_key(id) {
            return Err(TargetInventoryError::UnknownTarget(id.clone()));
        }
        if !unique.insert(id) {
            return Err(TargetInventoryError::DuplicateSelection(id.clone()));
        }
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum TargetInventoryError {
    #[error("target inventory is empty")]
    Empty,
    #[error("duplicate target id: {0}")]
    DuplicateId(StableId),
    #[error("target identity collision")]
    DuplicateIdentity,
    #[error("unknown target: {0}")]
    UnknownTarget(StableId),
    #[error("duplicate selected target: {0}")]
    DuplicateSelection(StableId),
    #[error("target state path is unsafe")]
    UnsafeStatePath,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("target selection lock is poisoned")]
    Lock,
}
