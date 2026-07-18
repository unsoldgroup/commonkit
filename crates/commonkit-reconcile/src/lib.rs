//! Transaction receipts and reconciliation state machine.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use commonkit_contracts::{
    CONTRACT_VERSION, ContractError, ReceiptState, ReceiptTransition, RunReceipt, SCHEMA_VERSION,
    SchemaVersion, Sha256Digest, StableId, canonical_json, digest_domain_json,
};
use serde::Serialize;
use thiserror::Error;

pub struct ReceiptJournal {
    receipt: RunReceipt,
}

impl ReceiptJournal {
    pub fn new(
        run_id: StableId,
        plan_id: Sha256Digest,
        target_id: StableId,
        desired_digest: Sha256Digest,
        observed_digest: Sha256Digest,
        policy_digest: Sha256Digest,
    ) -> Result<Self, ReceiptError> {
        let first = make_transition(0, ReceiptState::Prepared, None)?;
        Ok(Self {
            receipt: RunReceipt {
                schema_version: SchemaVersion(SCHEMA_VERSION),
                contract_version: CONTRACT_VERSION.into(),
                receipt_id: first.entry_digest.clone(),
                run_id,
                plan_id,
                target_id,
                desired_digest,
                observed_digest,
                policy_digest,
                state: ReceiptState::Prepared,
                transitions: vec![first],
            },
        })
    }

    pub fn transition(&mut self, next: ReceiptState) -> Result<(), ReceiptError> {
        if !legal_transition(self.receipt.state, next) {
            return Err(ReceiptError::IllegalTransition {
                from: self.receipt.state,
                to: next,
            });
        }
        let previous = self
            .receipt
            .transitions
            .last()
            .map(|entry| entry.entry_digest.clone());
        let transition = make_transition(self.receipt.transitions.len() as u64, next, previous)?;
        self.receipt.receipt_id = transition.entry_digest.clone();
        self.receipt.state = next;
        self.receipt.transitions.push(transition);
        Ok(())
    }

    pub fn receipt(&self) -> &RunReceipt {
        &self.receipt
    }

    fn from_receipt(receipt: RunReceipt) -> Result<Self, ReceiptError> {
        let journal = Self { receipt };
        journal.verify_chain()?;
        Ok(journal)
    }

    pub fn verify_chain(&self) -> Result<(), ReceiptError> {
        let mut previous = None;
        for (sequence, transition) in self.receipt.transitions.iter().enumerate() {
            if transition.sequence != sequence as u64 || transition.previous_digest != previous {
                return Err(ReceiptError::InvalidHashChain);
            }
            let expected =
                make_transition(transition.sequence, transition.state, previous.clone())?;
            if expected.entry_digest != transition.entry_digest {
                return Err(ReceiptError::InvalidHashChain);
            }
            previous = Some(transition.entry_digest.clone());
        }
        if previous.as_ref() != Some(&self.receipt.receipt_id) {
            return Err(ReceiptError::InvalidHashChain);
        }
        Ok(())
    }
}

/// An immutable, one-snapshot-per-transition receipt store.
///
/// Final files are never replaced. A crash therefore exposes either the prior
/// complete transition or the next complete transition, never a torn update.
pub struct ReceiptStore {
    root: PathBuf,
}

impl ReceiptStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ReceiptError> {
        fs::create_dir_all(root.as_ref())?;
        sync_directory(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().to_path_buf(),
        })
    }

    pub fn persist(&self, journal: &ReceiptJournal) -> Result<(), ReceiptError> {
        journal.verify_chain()?;
        let receipt = journal.receipt();
        let run_directory = self.root.join(receipt.run_id.as_str());
        fs::create_dir_all(&run_directory)?;

        if let Ok(current) = self.load(receipt.run_id.clone()) {
            let current_count = current.receipt().transitions.len();
            let incoming_count = receipt.transitions.len();
            if current_count >= incoming_count {
                if current.receipt() == receipt {
                    return Ok(());
                }
                return Err(ReceiptError::StaleWrite {
                    persisted_sequence: current_count.saturating_sub(1) as u64,
                    attempted_sequence: incoming_count.saturating_sub(1) as u64,
                });
            }
            if current.receipt().transitions != receipt.transitions[..current_count] {
                return Err(ReceiptError::ConflictingHistory);
            }
        }

        let sequence = receipt.transitions.len().saturating_sub(1) as u64;
        let digest = receipt
            .transitions
            .last()
            .ok_or(ReceiptError::InvalidHashChain)?
            .entry_digest
            .as_str()
            .trim_start_matches("sha256:");
        let final_path = run_directory.join(format!("{sequence:020}-{digest}.json"));
        if final_path.exists() {
            return Ok(());
        }
        let temporary_path = run_directory.join(format!(".{sequence:020}-{digest}.tmp"));
        let bytes = canonical_json(receipt)?;
        let mut temporary = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)?;
        temporary.write_all(&bytes)?;
        temporary.sync_all()?;
        drop(temporary);
        fs::rename(&temporary_path, &final_path)?;
        sync_directory(&run_directory)?;
        Ok(())
    }

    pub fn load(&self, run_id: StableId) -> Result<ReceiptJournal, ReceiptError> {
        let run_directory = self.root.join(run_id.as_str());
        let mut snapshots = Vec::new();
        for entry in fs::read_dir(&run_directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or(ReceiptError::InvalidSnapshotName)?;
            let sequence = name
                .split_once('-')
                .and_then(|(value, _)| value.parse::<u64>().ok())
                .ok_or(ReceiptError::InvalidSnapshotName)?;
            snapshots.push((sequence, path));
        }
        snapshots.sort_by_key(|(sequence, _)| *sequence);
        if snapshots.is_empty() {
            return Err(ReceiptError::NotFound(run_id));
        }

        let mut previous: Option<RunReceipt> = None;
        for (expected, (sequence, path)) in snapshots.into_iter().enumerate() {
            if sequence != expected as u64 {
                return Err(ReceiptError::SnapshotGap);
            }
            let receipt: RunReceipt = serde_json::from_slice(&fs::read(path)?)?;
            if receipt.run_id != run_id || receipt.transitions.len() != expected + 1 {
                return Err(ReceiptError::ConflictingHistory);
            }
            ReceiptJournal::from_receipt(receipt.clone())?;
            if let Some(prior) = &previous
                && prior.transitions != receipt.transitions[..prior.transitions.len()]
            {
                return Err(ReceiptError::ConflictingHistory);
            }
            previous = Some(receipt);
        }
        ReceiptJournal::from_receipt(previous.expect("non-empty snapshots"))
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), std::io::Error> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TransitionSemantic<'a> {
    sequence: u64,
    state: ReceiptState,
    previous_digest: &'a Option<Sha256Digest>,
}

fn make_transition(
    sequence: u64,
    state: ReceiptState,
    previous_digest: Option<Sha256Digest>,
) -> Result<ReceiptTransition, ContractError> {
    let entry_digest = digest_domain_json(
        "commonkit.receipt-transition.v1",
        &TransitionSemantic {
            sequence,
            state,
            previous_digest: &previous_digest,
        },
    )?;
    Ok(ReceiptTransition {
        sequence,
        state,
        previous_digest,
        entry_digest,
    })
}

fn legal_transition(from: ReceiptState, to: ReceiptState) -> bool {
    matches!(
        (from, to),
        (
            ReceiptState::Prepared,
            ReceiptState::Applying | ReceiptState::Canceled
        ) | (
            ReceiptState::Applying,
            ReceiptState::Verifying | ReceiptState::RecoveryRequired
        ) | (
            ReceiptState::Verifying,
            ReceiptState::Succeeded | ReceiptState::RecoveryRequired
        ) | (ReceiptState::RecoveryRequired, ReceiptState::RollingBack)
            | (
                ReceiptState::RollingBack,
                ReceiptState::RolledBack | ReceiptState::RollbackFailed
            )
    )
}

#[derive(Debug, Error)]
pub enum ReceiptError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error("illegal receipt transition from {from:?} to {to:?}")]
    IllegalTransition {
        from: ReceiptState,
        to: ReceiptState,
    },
    #[error("receipt transition hash chain is invalid")]
    InvalidHashChain,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("receipt not found for run {0}")]
    NotFound(StableId),
    #[error("receipt snapshot filename is invalid")]
    InvalidSnapshotName,
    #[error("receipt snapshots contain a sequence gap")]
    SnapshotGap,
    #[error("receipt snapshots contain conflicting history")]
    ConflictingHistory,
    #[error(
        "stale receipt write at sequence {attempted_sequence}; persisted sequence is {persisted_sequence}"
    )]
    StaleWrite {
        persisted_sequence: u64,
        attempted_sequence: u64,
    },
}
