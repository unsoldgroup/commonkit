//! Transaction receipts and reconciliation state machine.

use commonkit_contracts::{
    CONTRACT_VERSION, ContractError, ReceiptState, ReceiptTransition, RunReceipt, SCHEMA_VERSION,
    SchemaVersion, Sha256Digest, StableId, digest_domain_json,
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
}
