//! Transaction receipts and reconciliation state machine.

mod skill_deployment;

pub use skill_deployment::{
    ApmCompilation, ApmCompiler, AuthenticatedSkillPromotion, CanaryStateObserver,
    DeploymentTrustStore, PreparedSkillDeployment, SkillDeploymentError, SkillDeploymentLineage,
    SkillDeploymentReceipt, SkillDeploymentRecovery, SkillDeploymentRecoveryReceipt,
    SkillDeploymentRequest, SkillDeploymentRollbackFailureReceipt, SkillDeploymentRollbackReceipt,
    SkillDeploymentState, SkillDeploymentWorkflow, SkillPromotionAuthority,
};

#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use commonkit_contracts::{
    CONTRACT_VERSION, ContractError, FORWARD_CONTRACT_VERSION, FORWARD_SCHEMA_VERSION, Operation,
    OperationPhase, OperationProgress, Plan, PlanBindings, ReceiptState, ReceiptTransition,
    RecoveryCapability, RunReceipt, SCHEMA_VERSION, SchemaVersion, Sha256Digest, StableId,
    canonical_json, digest_domain_json,
};
use commonkit_core::{PlanBuildError, PlanDraft, build_plan};
use serde::{Deserialize, Serialize};
use thiserror::Error;

static PLAN_TEMP_NONCE: AtomicU64 = AtomicU64::new(0);

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
        bindings: PlanBindings,
    ) -> Result<Self, ReceiptError> {
        Self::new_versioned(
            run_id,
            plan_id,
            target_id,
            desired_digest,
            observed_digest,
            policy_digest,
            bindings,
            SchemaVersion(SCHEMA_VERSION),
            CONTRACT_VERSION.into(),
        )
    }

    pub fn for_plan(run_id: StableId, plan: &Plan) -> Result<Self, ReceiptError> {
        Self::new_versioned(
            run_id,
            plan.id.clone(),
            plan.target_id.clone(),
            plan.desired_digest.clone(),
            plan.observed_digest.clone(),
            plan.policy_digest.clone(),
            plan.bindings.clone(),
            plan.schema_version,
            plan.contract_version.clone(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_versioned(
        run_id: StableId,
        plan_id: Sha256Digest,
        target_id: StableId,
        desired_digest: Sha256Digest,
        observed_digest: Sha256Digest,
        policy_digest: Sha256Digest,
        bindings: PlanBindings,
        schema_version: SchemaVersion,
        contract_version: String,
    ) -> Result<Self, ReceiptError> {
        validate_receipt_version(schema_version, &contract_version)?;
        let progress = Vec::new();
        let progress_digest = progress_digest(schema_version, &progress)?;
        let first = make_transition_for_schema(
            schema_version,
            0,
            ReceiptState::Prepared,
            None,
            progress_digest,
        )?;
        Ok(Self {
            receipt: RunReceipt {
                schema_version,
                contract_version,
                receipt_id: first.entry_digest.clone(),
                run_id,
                plan_id,
                target_id,
                desired_digest,
                observed_digest,
                policy_digest,
                bindings,
                state: ReceiptState::Prepared,
                operation_progress: progress,
                transitions: vec![first],
            },
        })
    }

    pub fn transition(&mut self, next: ReceiptState) -> Result<(), ReceiptError> {
        if !receipt_state_supported(self.receipt.schema_version, next)
            || !legal_transition(self.receipt.state, next)
        {
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
        let progress_digest = progress_digest(
            self.receipt.schema_version,
            &self.receipt.operation_progress,
        )?;
        let transition = make_transition_for_schema(
            self.receipt.schema_version,
            self.receipt.transitions.len() as u64,
            next,
            previous,
            progress_digest,
        )?;
        self.receipt.receipt_id = transition.entry_digest.clone();
        self.receipt.state = next;
        self.receipt.transitions.push(transition);
        Ok(())
    }

    pub fn record_operation(
        &mut self,
        operation_id: Sha256Digest,
        phase: OperationPhase,
        failure_code: Option<StableId>,
    ) -> Result<(), ReceiptError> {
        if !operation_phase_supported(self.receipt.schema_version, phase) {
            return Err(ReceiptError::InvalidOperationProgress);
        }
        if failure_code.is_some()
            != matches!(
                phase,
                OperationPhase::PrepareFailed
                    | OperationPhase::ApplyFailed
                    | OperationPhase::VerifyFailed
                    | OperationPhase::RollbackFailed
                    | OperationPhase::ForwardRecoveryFailed
            )
        {
            return Err(ReceiptError::InvalidOperationProgress);
        }
        if let Some(progress) = self
            .receipt
            .operation_progress
            .iter_mut()
            .find(|progress| progress.operation_id == operation_id)
        {
            if !legal_operation_transition(progress.phase, phase) {
                return Err(ReceiptError::InvalidOperationProgress);
            }
            progress.phase = phase;
            progress.failure_code = failure_code;
        } else {
            if !matches!(
                phase,
                OperationPhase::Prepared | OperationPhase::PrepareFailed
            ) {
                return Err(ReceiptError::InvalidOperationProgress);
            }
            self.receipt.operation_progress.push(OperationProgress {
                operation_id,
                phase,
                failure_code,
            });
        }
        self.append_progress_transition()
    }

    fn append_progress_transition(&mut self) -> Result<(), ReceiptError> {
        let previous = self
            .receipt
            .transitions
            .last()
            .map(|entry| entry.entry_digest.clone());
        let progress_digest = progress_digest(
            self.receipt.schema_version,
            &self.receipt.operation_progress,
        )?;
        let transition = make_transition_for_schema(
            self.receipt.schema_version,
            self.receipt.transitions.len() as u64,
            self.receipt.state,
            previous,
            progress_digest,
        )?;
        self.receipt.receipt_id = transition.entry_digest.clone();
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
        validate_receipt_version(self.receipt.schema_version, &self.receipt.contract_version)?;
        if self
            .receipt
            .transitions
            .first()
            .is_none_or(|transition| transition.state != ReceiptState::Prepared)
        {
            return Err(ReceiptError::InvalidHashChain);
        }
        let mut previous = None;
        let mut previous_state = None;
        for (sequence, transition) in self.receipt.transitions.iter().enumerate() {
            if !receipt_state_supported(self.receipt.schema_version, transition.state) {
                return Err(ReceiptError::InvalidHashChain);
            }
            if transition.sequence != sequence as u64 || transition.previous_digest != previous {
                return Err(ReceiptError::InvalidHashChain);
            }
            if previous_state.is_some_and(|state| {
                state != transition.state && !legal_transition(state, transition.state)
            }) {
                return Err(ReceiptError::InvalidHashChain);
            }
            let expected = make_transition_for_schema(
                self.receipt.schema_version,
                transition.sequence,
                transition.state,
                previous.clone(),
                transition.progress_digest.clone(),
            )?;
            if expected.entry_digest != transition.entry_digest {
                return Err(ReceiptError::InvalidHashChain);
            }
            previous = Some(transition.entry_digest.clone());
            previous_state = Some(transition.state);
        }
        if previous.as_ref() != Some(&self.receipt.receipt_id) {
            return Err(ReceiptError::InvalidHashChain);
        }
        let progress_digest = progress_digest(
            self.receipt.schema_version,
            &self.receipt.operation_progress,
        )?;
        if self
            .receipt
            .operation_progress
            .iter()
            .any(|progress| !operation_phase_supported(self.receipt.schema_version, progress.phase))
        {
            return Err(ReceiptError::InvalidHashChain);
        }
        if self
            .receipt
            .transitions
            .last()
            .is_none_or(|transition| transition.progress_digest != progress_digest)
        {
            return Err(ReceiptError::InvalidHashChain);
        }
        if previous_state != Some(self.receipt.state) {
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

/// Immutable content-addressed storage for approved plans.
pub struct PlanStore {
    root: PathBuf,
}

impl PlanStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, PlanStoreError> {
        fs::create_dir_all(root.as_ref())?;
        set_private_directory(root.as_ref())?;
        let root = root.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(PlanStoreError::InvalidRoot);
        }
        Ok(Self { root })
    }

    pub fn persist(&self, plan: &Plan) -> Result<(), PlanStoreError> {
        validate_plan(plan).map_err(|_| PlanStoreError::InvalidPlan)?;
        let bytes = canonical_json(plan)?;
        let destination = self.path(&plan.id);
        if destination.exists() {
            return if read_plan_bytes(&destination)? == bytes {
                Ok(())
            } else {
                Err(PlanStoreError::PlanConflict)
            };
        }
        let nonce = PLAN_TEMP_NONCE.fetch_add(1, Ordering::Relaxed);
        let temporary = self.root.join(format!(
            ".plan-{}-{}-{nonce}.tmp",
            std::process::id(),
            plan.id.as_str().trim_start_matches("sha256:")
        ));
        let result = (|| -> Result<(), PlanStoreError> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            match fs::hard_link(&temporary, &destination) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if read_plan_bytes(&destination)? != bytes {
                        return Err(PlanStoreError::PlanConflict);
                    }
                }
                Err(error) => return Err(error.into()),
            }
            fs::remove_file(&temporary)?;
            sync_directory(&self.root)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn load(&self, id: &Sha256Digest) -> Result<Plan, PlanStoreError> {
        let bytes = read_plan_bytes(&self.path(id))?;
        let plan: Plan = serde_json::from_slice(&bytes)?;
        if &plan.id != id || validate_plan(&plan).is_err() {
            return Err(PlanStoreError::InvalidPlan);
        }
        Ok(plan)
    }

    fn path(&self, id: &Sha256Digest) -> PathBuf {
        self.root.join(format!(
            "{}.json",
            id.as_str().trim_start_matches("sha256:")
        ))
    }
}

fn read_plan_bytes(path: &Path) -> Result<Vec<u8>, PlanStoreError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(PlanStoreError::UnsafeEntry);
    }
    Ok(fs::read(path)?)
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

#[derive(Debug, Error)]
pub enum PlanStoreError {
    #[error("plan store root is invalid")]
    InvalidRoot,
    #[error("plan store entry is not an ordinary file")]
    UnsafeEntry,
    #[error("plan is invalid or does not match its content address")]
    InvalidPlan,
    #[error("plan ID is already bound to different content")]
    PlanConflict,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error(transparent)]
    Contract(#[from] ContractError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterFailure {
    pub code: String,
    pub message: String,
}

impl AdapterFailure {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

pub trait Adapter {
    fn id(&self) -> &StableId;
    fn supports_recovery(&self, _capability: RecoveryCapability) -> bool {
        false
    }
    fn supports_operation(&self, operation: &Operation) -> bool {
        self.supports_recovery(operation.recovery_capability)
    }
    fn observe_recovery(
        &mut self,
        _operation: &Operation,
    ) -> Result<RecoveryObservation, AdapterFailure> {
        Err(AdapterFailure::new(
            "recovery_observation_unsupported",
            "adapter does not expose recovery observation",
        ))
    }
    fn supports_offline_recovery(&self, _operation: &Operation) -> bool {
        false
    }
    fn prepare_recovery(&mut self, _operation: &Operation) -> Result<(), AdapterFailure> {
        Err(AdapterFailure::new(
            "offline_recovery_unsupported",
            "adapter does not expose offline recovery preparation",
        ))
    }
    fn converge_recovery(&mut self, _operation: &Operation) -> Result<(), AdapterFailure> {
        Err(AdapterFailure::new(
            "offline_recovery_unsupported",
            "adapter does not expose offline recovery convergence",
        ))
    }
    fn prepare(&mut self, operation: &Operation) -> Result<(), AdapterFailure>;
    fn apply(&mut self, operation: &Operation) -> Result<(), AdapterFailure>;
    fn verify(&mut self, operation: &Operation) -> Result<(), AdapterFailure>;
    fn rollback(&mut self, operation: &Operation) -> Result<(), AdapterFailure>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryObservation {
    Before,
    After,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileOutcome {
    Succeeded,
    Canceled,
    RolledBack,
    RollbackFailed,
    ForwardRecoveryRequired,
    ForwardRecovered,
    ForwardRecoveryFailed,
}

impl ReconcileOutcome {
    pub fn receipt_state(self) -> ReceiptState {
        match self {
            Self::Succeeded => ReceiptState::Succeeded,
            Self::Canceled => ReceiptState::Canceled,
            Self::RolledBack => ReceiptState::RolledBack,
            Self::RollbackFailed => ReceiptState::RollbackFailed,
            Self::ForwardRecoveryRequired => ReceiptState::ForwardRecoveryRequired,
            Self::ForwardRecovered => ReceiptState::ForwardRecovered,
            Self::ForwardRecoveryFailed => ReceiptState::ForwardRecoveryFailed,
        }
    }
}

#[derive(Default)]
pub struct Reconciler<'a> {
    store: Option<&'a ReceiptStore>,
}

impl<'a> Reconciler<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_store(store: &'a ReceiptStore) -> Self {
        Self { store: Some(store) }
    }

    pub fn execute(
        &self,
        plan: &Plan,
        run_id: StableId,
        adapters: &mut [Box<dyn Adapter>],
    ) -> Result<ReconcileOutcome, ReconcileError> {
        validate_plan(plan)?;
        preflight_adapters(plan, adapters)?;
        let mut journal = ReceiptJournal::for_plan(run_id, plan)?;
        self.persist(&journal)?;

        for operation in &plan.operations {
            let adapter = adapter_for(adapters, &operation.adapter_id)?;
            match adapter.prepare(operation) {
                Ok(()) => journal.record_operation(
                    operation.id.clone(),
                    OperationPhase::Prepared,
                    None,
                )?,
                Err(failure) => {
                    journal.record_operation(
                        operation.id.clone(),
                        OperationPhase::PrepareFailed,
                        Some(stable_failure_code(&failure)),
                    )?;
                    self.persist(&journal)?;
                    journal.transition(ReceiptState::Canceled)?;
                    self.persist(&journal)?;
                    return Ok(ReconcileOutcome::Canceled);
                }
            }
            self.persist(&journal)?;
        }

        journal.transition(ReceiptState::Applying)?;
        self.persist(&journal)?;
        let exact = plan
            .operations
            .iter()
            .filter(|operation| operation.recovery_capability == RecoveryCapability::ExactRollback)
            .collect::<Vec<_>>();
        let forward = plan
            .operations
            .iter()
            .filter(|operation| {
                operation.recovery_capability == RecoveryCapability::ConvergeForwardOnly
            })
            .collect::<Vec<_>>();
        let mut applied = Vec::new();
        for operation in &exact {
            journal.record_operation(operation.id.clone(), OperationPhase::ApplyStarted, None)?;
            self.persist(&journal)?;
            let adapter = adapter_for(adapters, &operation.adapter_id)?;
            match adapter.apply(operation) {
                Ok(()) => {
                    journal.record_operation(operation.id.clone(), OperationPhase::Applied, None)?
                }
                Err(failure) => {
                    journal.record_operation(
                        operation.id.clone(),
                        OperationPhase::ApplyFailed,
                        Some(stable_failure_code(&failure)),
                    )?;
                    self.persist(&journal)?;
                    applied.push((*operation).clone());
                    return self.recover(&mut journal, &applied, adapters);
                }
            }
            applied.push((*operation).clone());
            self.persist(&journal)?;
        }
        journal.transition(ReceiptState::Verifying)?;
        self.persist(&journal)?;
        for operation in &exact {
            let adapter = adapter_for(adapters, &operation.adapter_id)?;
            match adapter.verify(operation) {
                Ok(()) => journal.record_operation(
                    operation.id.clone(),
                    OperationPhase::Verified,
                    None,
                )?,
                Err(failure) => {
                    journal.record_operation(
                        operation.id.clone(),
                        OperationPhase::VerifyFailed,
                        Some(stable_failure_code(&failure)),
                    )?;
                    self.persist(&journal)?;
                    return self.recover(&mut journal, &applied, adapters);
                }
            }
            self.persist(&journal)?;
        }

        if !forward.is_empty() {
            journal.transition(ReceiptState::ApplyingForward)?;
            self.persist(&journal)?;
        }
        for operation in forward {
            // This durable checkpoint is the one-way recovery barrier. No
            // rollback path is reachable after it has been persisted.
            journal.record_operation(operation.id.clone(), OperationPhase::ApplyStarted, None)?;
            self.persist(&journal)?;
            let adapter = adapter_for(adapters, &operation.adapter_id)?;
            if let Err(failure) = adapter.apply(operation) {
                journal.record_operation(
                    operation.id.clone(),
                    OperationPhase::ApplyFailed,
                    Some(stable_failure_code(&failure)),
                )?;
                self.persist(&journal)?;
                return self.require_forward_recovery(&mut journal);
            }
            journal.record_operation(operation.id.clone(), OperationPhase::Applied, None)?;
            self.persist(&journal)?;
            let adapter = adapter_for(adapters, &operation.adapter_id)?;
            if let Err(failure) = adapter.verify(operation) {
                journal.record_operation(
                    operation.id.clone(),
                    OperationPhase::VerifyFailed,
                    Some(stable_failure_code(&failure)),
                )?;
                self.persist(&journal)?;
                return self.require_forward_recovery(&mut journal);
            }
            journal.record_operation(operation.id.clone(), OperationPhase::Verified, None)?;
            self.persist(&journal)?;
        }

        journal.transition(ReceiptState::Succeeded)?;
        self.persist(&journal)?;
        Ok(ReconcileOutcome::Succeeded)
    }

    pub fn recover_run(
        &self,
        run_id: StableId,
        plan: &Plan,
        adapters: &mut [Box<dyn Adapter>],
    ) -> Result<ReconcileOutcome, ReconcileError> {
        validate_plan(plan)?;
        preflight_adapters(plan, adapters)?;
        let store = self.store.ok_or(ReconcileError::DurableStoreRequired)?;
        let mut journal = store.load(run_id)?;
        let receipt = journal.receipt();
        if receipt.plan_id != plan.id
            || receipt.schema_version != plan.schema_version
            || receipt.contract_version != plan.contract_version
            || receipt.target_id != plan.target_id
            || receipt.desired_digest != plan.desired_digest
            || receipt.observed_digest != plan.observed_digest
            || receipt.policy_digest != plan.policy_digest
            || receipt.bindings != plan.bindings
        {
            return Err(ReconcileError::ReceiptPlanMismatch);
        }

        let after_forward_barrier = plan.operations.iter().any(|operation| {
            operation.recovery_capability == RecoveryCapability::ConvergeForwardOnly
                && receipt.operation_progress.iter().any(|progress| {
                    progress.operation_id == operation.id
                        && matches!(
                            progress.phase,
                            OperationPhase::ApplyStarted
                                | OperationPhase::Applied
                                | OperationPhase::ApplyFailed
                                | OperationPhase::Verified
                                | OperationPhase::VerifyFailed
                                | OperationPhase::ForwardRecovered
                                | OperationPhase::ForwardRecoveryFailed
                        )
                })
        }) || matches!(
            receipt.state,
            ReceiptState::ForwardRecoveryRequired
                | ReceiptState::ConvergingForward
                | ReceiptState::ForwardRecoveryFailed
        );
        if after_forward_barrier {
            return self.converge_forward(&mut journal, plan, adapters);
        }

        match receipt.state {
            ReceiptState::Prepared => {
                journal.transition(ReceiptState::Canceled)?;
                self.persist(&journal)?;
                return Ok(ReconcileOutcome::Canceled);
            }
            ReceiptState::Applying | ReceiptState::Verifying | ReceiptState::ApplyingForward => {
                journal.transition(ReceiptState::RecoveryRequired)?;
                self.persist(&journal)?;
            }
            ReceiptState::RecoveryRequired => {}
            ReceiptState::RollingBack => {}
            state => return Err(ReconcileError::RunAlreadyTerminal(state)),
        }
        if journal.receipt().state == ReceiptState::RecoveryRequired {
            journal.transition(ReceiptState::RollingBack)?;
            self.persist(&journal)?;
        }

        let progress = journal.receipt().operation_progress.clone();
        let mut applied = Vec::new();
        for operation in &plan.operations {
            let phase = progress
                .iter()
                .find(|entry| entry.operation_id == operation.id)
                .map(|entry| entry.phase);
            if matches!(
                phase,
                Some(
                    OperationPhase::ApplyStarted
                        | OperationPhase::Applied
                        | OperationPhase::ApplyFailed
                        | OperationPhase::Verified
                        | OperationPhase::VerifyFailed
                )
            ) {
                applied.push(operation.clone());
            }
        }
        self.rollback_from_rolling_back(&mut journal, &applied, adapters)
    }

    /// Explicitly rolls back a previously successful durable run without
    /// re-resolving provider inputs or secret material.
    pub fn rollback_succeeded_run(
        &self,
        run_id: StableId,
        plan: &Plan,
        adapters: &mut [Box<dyn Adapter>],
    ) -> Result<ReconcileOutcome, ReconcileError> {
        validate_plan(plan)?;
        if plan.operations.iter().any(|operation| {
            operation.recovery_capability == RecoveryCapability::ConvergeForwardOnly
        }) {
            return Err(ReconcileError::RollbackUnsupported);
        }
        preflight_adapters(plan, adapters)?;
        let store = self.store.ok_or(ReconcileError::DurableStoreRequired)?;
        let mut journal = store.load(run_id)?;
        if journal.receipt().plan_id != plan.id
            || journal.receipt().schema_version != plan.schema_version
            || journal.receipt().contract_version != plan.contract_version
            || journal.receipt().target_id != plan.target_id
        {
            return Err(ReconcileError::ReceiptPlanMismatch);
        }
        if journal.receipt().state != ReceiptState::Succeeded {
            return Err(ReconcileError::RunAlreadyTerminal(journal.receipt().state));
        }
        journal.transition(ReceiptState::RollingBack)?;
        self.persist(&journal)?;
        self.rollback_from_rolling_back(&mut journal, &plan.operations, adapters)
    }

    fn recover(
        &self,
        journal: &mut ReceiptJournal,
        applied: &[Operation],
        adapters: &mut [Box<dyn Adapter>],
    ) -> Result<ReconcileOutcome, ReconcileError> {
        journal.transition(ReceiptState::RecoveryRequired)?;
        self.persist(journal)?;
        journal.transition(ReceiptState::RollingBack)?;
        self.persist(journal)?;

        self.rollback_from_rolling_back(journal, applied, adapters)
    }

    fn require_forward_recovery(
        &self,
        journal: &mut ReceiptJournal,
    ) -> Result<ReconcileOutcome, ReconcileError> {
        journal.transition(ReceiptState::ForwardRecoveryRequired)?;
        self.persist(journal)?;
        Ok(ReconcileOutcome::ForwardRecoveryRequired)
    }

    fn converge_forward(
        &self,
        journal: &mut ReceiptJournal,
        plan: &Plan,
        adapters: &mut [Box<dyn Adapter>],
    ) -> Result<ReconcileOutcome, ReconcileError> {
        match journal.receipt().state {
            ReceiptState::Applying | ReceiptState::Verifying | ReceiptState::ApplyingForward => {
                journal.transition(ReceiptState::ForwardRecoveryRequired)?;
                self.persist(journal)?;
            }
            ReceiptState::ForwardRecoveryRequired => {}
            ReceiptState::ConvergingForward => {}
            ReceiptState::ForwardRecoveryFailed => {}
            state => return Err(ReconcileError::RunAlreadyTerminal(state)),
        }
        if matches!(
            journal.receipt().state,
            ReceiptState::ForwardRecoveryRequired | ReceiptState::ForwardRecoveryFailed
        ) {
            journal.transition(ReceiptState::ConvergingForward)?;
            self.persist(journal)?;
        }

        for operation in &plan.operations {
            if journal.receipt().operation_progress.iter().any(|progress| {
                progress.operation_id == operation.id
                    && progress.phase == OperationPhase::ForwardRecovered
            }) {
                continue;
            }
            let observation = adapter_for(adapters, &operation.adapter_id)?
                .observe_recovery(operation)
                .map_err(|failure| stable_failure_code(&failure));
            let result = match observation {
                Ok(RecoveryObservation::After) => Ok(()),
                Ok(RecoveryObservation::Before) => {
                    let prepare =
                        adapter_for(adapters, &operation.adapter_id)?.prepare_recovery(operation);
                    if let Err(failure) = prepare {
                        Err(stable_failure_code(&failure))
                    } else {
                        let converge = adapter_for(adapters, &operation.adapter_id)?
                            .converge_recovery(operation);
                        if let Err(failure) = converge {
                            Err(stable_failure_code(&failure))
                        } else {
                            match adapter_for(adapters, &operation.adapter_id)?
                                .observe_recovery(operation)
                            {
                                Ok(RecoveryObservation::After) => Ok(()),
                                Ok(RecoveryObservation::Before | RecoveryObservation::Other) => {
                                    Err(StableId::parse("recovery_ambiguous")
                                        .expect("static stable ID"))
                                }
                                Err(failure) => Err(stable_failure_code(&failure)),
                            }
                        }
                    }
                }
                Ok(RecoveryObservation::Other) => {
                    Err(StableId::parse("recovery_ambiguous").expect("static stable ID"))
                }
                Err(code) => Err(code),
            };
            match result {
                Ok(()) => journal.record_operation(
                    operation.id.clone(),
                    OperationPhase::ForwardRecovered,
                    None,
                )?,
                Err(code) => {
                    journal.record_operation(
                        operation.id.clone(),
                        OperationPhase::ForwardRecoveryFailed,
                        Some(code),
                    )?;
                    self.persist(journal)?;
                    journal.transition(ReceiptState::ForwardRecoveryFailed)?;
                    self.persist(journal)?;
                    return Ok(ReconcileOutcome::ForwardRecoveryFailed);
                }
            }
            self.persist(journal)?;
        }
        journal.transition(ReceiptState::ForwardRecovered)?;
        self.persist(journal)?;
        Ok(ReconcileOutcome::ForwardRecovered)
    }

    fn rollback_from_rolling_back(
        &self,
        journal: &mut ReceiptJournal,
        applied: &[Operation],
        adapters: &mut [Box<dyn Adapter>],
    ) -> Result<ReconcileOutcome, ReconcileError> {
        let mut rollback_failed = false;
        for operation in applied.iter().rev() {
            let adapter = adapter_for(adapters, &operation.adapter_id)?;
            match adapter.rollback(operation) {
                Ok(()) => journal.record_operation(
                    operation.id.clone(),
                    OperationPhase::RolledBack,
                    None,
                )?,
                Err(failure) => {
                    rollback_failed = true;
                    journal.record_operation(
                        operation.id.clone(),
                        OperationPhase::RollbackFailed,
                        Some(stable_failure_code(&failure)),
                    )?;
                }
            }
            self.persist(journal)?;
        }
        let outcome = if rollback_failed {
            ReconcileOutcome::RollbackFailed
        } else {
            ReconcileOutcome::RolledBack
        };
        journal.transition(outcome.receipt_state())?;
        self.persist(journal)?;
        Ok(outcome)
    }

    fn persist(&self, journal: &ReceiptJournal) -> Result<(), ReconcileError> {
        if let Some(store) = self.store {
            store.persist(journal)?;
        }
        Ok(())
    }
}

fn stable_failure_code(failure: &AdapterFailure) -> StableId {
    StableId::parse(failure.code.clone())
        .unwrap_or_else(|_| StableId::parse("adapter_failure").expect("static stable ID"))
}

fn adapter_for<'a>(
    adapters: &'a mut [Box<dyn Adapter>],
    id: &StableId,
) -> Result<&'a mut (dyn Adapter + 'a), ReconcileError> {
    for adapter in adapters {
        if adapter.id() == id {
            return Ok(adapter.as_mut());
        }
    }
    Err(ReconcileError::AdapterNotFound(id.clone()))
}

fn preflight_adapters(plan: &Plan, adapters: &[Box<dyn Adapter>]) -> Result<(), ReconcileError> {
    let crosses_forward_barrier = plan
        .operations
        .iter()
        .any(|operation| operation.recovery_capability == RecoveryCapability::ConvergeForwardOnly);
    for operation in &plan.operations {
        let adapter = adapters
            .iter()
            .find(|adapter| adapter.id() == &operation.adapter_id)
            .ok_or_else(|| ReconcileError::AdapterNotFound(operation.adapter_id.clone()))?;
        if !adapter.supports_operation(operation) {
            return Err(ReconcileError::AdapterCapabilityUnsupported {
                adapter_id: operation.adapter_id.clone(),
                capability: operation.recovery_capability,
            });
        }
        if crosses_forward_barrier && !adapter.supports_offline_recovery(operation) {
            return Err(ReconcileError::AdapterOfflineRecoveryUnsupported {
                adapter_id: operation.adapter_id.clone(),
            });
        }
    }
    Ok(())
}

fn validate_plan(plan: &Plan) -> Result<(), ReconcileError> {
    let rebuilt = build_plan(PlanDraft {
        target_id: plan.target_id.clone(),
        desired_digest: plan.desired_digest.clone(),
        observed_digest: plan.observed_digest.clone(),
        policy_digest: plan.policy_digest.clone(),
        bindings: plan.bindings.clone(),
        operations: plan.operations.clone(),
    })?;
    if rebuilt != *plan {
        return Err(ReconcileError::PlanMismatch);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ReconcileError {
    #[error(transparent)]
    Receipt(#[from] ReceiptError),
    #[error(transparent)]
    Plan(#[from] PlanBuildError),
    #[error("plan content does not match its content-addressed identity")]
    PlanMismatch,
    #[error("adapter is not registered: {0}")]
    AdapterNotFound(StableId),
    #[error("adapter {adapter_id} does not support recovery capability {capability:?}")]
    AdapterCapabilityUnsupported {
        adapter_id: StableId,
        capability: RecoveryCapability,
    },
    #[error("adapter {adapter_id} does not support bound offline forward recovery")]
    AdapterOfflineRecoveryUnsupported { adapter_id: StableId },
    #[error("explicit rollback is unsupported for a forward-only plan")]
    RollbackUnsupported,
    #[error("durable receipt store is required for restart recovery")]
    DurableStoreRequired,
    #[error("receipt is not bound to the supplied plan")]
    ReceiptPlanMismatch,
    #[error("run is already terminal in state {0:?}")]
    RunAlreadyTerminal(ReceiptState),
}

impl ReceiptStore {
    /// Root of the capability-scoped durable receipt store.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn open(root: impl AsRef<Path>) -> Result<Self, ReceiptError> {
        fs::create_dir_all(root.as_ref())?;
        sync_directory(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().to_path_buf(),
        })
    }

    /// Returns every durably recorded run identifier in stable order.
    ///
    /// Invalid directory names fail closed: callers must not silently skip a
    /// receipt that may require recovery.
    pub fn run_ids(&self) -> Result<Vec<StableId>, ReceiptError> {
        let mut ids = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if !file_type.is_dir() || file_type.is_symlink() {
                return Err(ReceiptError::InvalidRunDirectory);
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| ReceiptError::InvalidRunDirectory)?;
            ids.push(StableId::parse(name).map_err(|_| ReceiptError::InvalidRunDirectory)?);
        }
        ids.sort();
        Ok(ids)
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
    progress_digest: &'a Sha256Digest,
}

#[cfg(test)]
fn make_transition(
    sequence: u64,
    state: ReceiptState,
    previous_digest: Option<Sha256Digest>,
    progress_digest: Sha256Digest,
) -> Result<ReceiptTransition, ContractError> {
    make_transition_for_schema(
        SchemaVersion(SCHEMA_VERSION),
        sequence,
        state,
        previous_digest,
        progress_digest,
    )
}

fn make_transition_for_schema(
    schema_version: SchemaVersion,
    sequence: u64,
    state: ReceiptState,
    previous_digest: Option<Sha256Digest>,
    progress_digest: Sha256Digest,
) -> Result<ReceiptTransition, ContractError> {
    let entry_digest = digest_domain_json(
        if schema_version.0 == FORWARD_SCHEMA_VERSION {
            "commonkit.receipt-transition.v2"
        } else {
            "commonkit.receipt-transition.v1"
        },
        &TransitionSemantic {
            sequence,
            state,
            previous_digest: &previous_digest,
            progress_digest: &progress_digest,
        },
    )?;
    Ok(ReceiptTransition {
        sequence,
        state,
        previous_digest,
        progress_digest,
        entry_digest,
    })
}

fn progress_digest(
    schema_version: SchemaVersion,
    progress: &[OperationProgress],
) -> Result<Sha256Digest, ContractError> {
    digest_domain_json(
        if schema_version.0 == FORWARD_SCHEMA_VERSION {
            "commonkit.operation-progress.v2"
        } else {
            "commonkit.operation-progress.v1"
        },
        progress,
    )
}

fn validate_receipt_version(
    schema_version: SchemaVersion,
    contract_version: &str,
) -> Result<(), ReceiptError> {
    if (schema_version.0 == SCHEMA_VERSION && contract_version == CONTRACT_VERSION)
        || (schema_version.0 == FORWARD_SCHEMA_VERSION
            && contract_version == FORWARD_CONTRACT_VERSION)
    {
        Ok(())
    } else {
        Err(ReceiptError::UnsupportedSchemaVersion(schema_version.0))
    }
}

fn receipt_state_supported(schema_version: SchemaVersion, state: ReceiptState) -> bool {
    schema_version.0 == FORWARD_SCHEMA_VERSION
        || !matches!(
            state,
            ReceiptState::ApplyingForward
                | ReceiptState::ForwardRecoveryRequired
                | ReceiptState::ConvergingForward
                | ReceiptState::ForwardRecovered
                | ReceiptState::ForwardRecoveryFailed
        )
}

fn operation_phase_supported(schema_version: SchemaVersion, phase: OperationPhase) -> bool {
    schema_version.0 == FORWARD_SCHEMA_VERSION
        || !matches!(
            phase,
            OperationPhase::ForwardRecovered | OperationPhase::ForwardRecoveryFailed
        )
}

fn legal_operation_transition(from: OperationPhase, to: OperationPhase) -> bool {
    matches!(
        (from, to),
        (OperationPhase::Prepared, OperationPhase::ApplyStarted)
            | (
                OperationPhase::ApplyStarted,
                OperationPhase::Applied
                    | OperationPhase::ApplyFailed
                    | OperationPhase::RolledBack
                    | OperationPhase::RollbackFailed
                    | OperationPhase::ForwardRecovered
                    | OperationPhase::ForwardRecoveryFailed
            )
            | (
                OperationPhase::Applied,
                OperationPhase::Verified
                    | OperationPhase::VerifyFailed
                    | OperationPhase::RolledBack
                    | OperationPhase::RollbackFailed
                    | OperationPhase::ForwardRecovered
                    | OperationPhase::ForwardRecoveryFailed
            )
            | (
                OperationPhase::Verified,
                OperationPhase::RolledBack
                    | OperationPhase::RollbackFailed
                    | OperationPhase::ForwardRecovered
                    | OperationPhase::ForwardRecoveryFailed
            )
            | (
                OperationPhase::VerifyFailed,
                OperationPhase::RolledBack
                    | OperationPhase::RollbackFailed
                    | OperationPhase::ForwardRecovered
                    | OperationPhase::ForwardRecoveryFailed
            )
            | (
                OperationPhase::ApplyFailed,
                OperationPhase::RolledBack
                    | OperationPhase::RollbackFailed
                    | OperationPhase::ForwardRecovered
                    | OperationPhase::ForwardRecoveryFailed
            )
            | (
                OperationPhase::Prepared,
                OperationPhase::ForwardRecovered | OperationPhase::ForwardRecoveryFailed
            )
            | (
                OperationPhase::ForwardRecoveryFailed,
                OperationPhase::ForwardRecovered | OperationPhase::ForwardRecoveryFailed
            )
    )
}

fn legal_transition(from: ReceiptState, to: ReceiptState) -> bool {
    matches!(
        (from, to),
        (
            ReceiptState::Prepared,
            ReceiptState::Applying | ReceiptState::Canceled
        ) | (
            ReceiptState::Applying,
            ReceiptState::Verifying
                | ReceiptState::RecoveryRequired
                | ReceiptState::ForwardRecoveryRequired
        ) | (
            ReceiptState::Verifying,
            ReceiptState::Succeeded
                | ReceiptState::ApplyingForward
                | ReceiptState::RecoveryRequired
                | ReceiptState::ForwardRecoveryRequired
        ) | (
            ReceiptState::ApplyingForward,
            ReceiptState::Succeeded
                | ReceiptState::RecoveryRequired
                | ReceiptState::ForwardRecoveryRequired
        ) | (ReceiptState::RecoveryRequired, ReceiptState::RollingBack)
            | (
                ReceiptState::ForwardRecoveryRequired,
                ReceiptState::ConvergingForward
            )
            | (
                ReceiptState::ConvergingForward,
                ReceiptState::ForwardRecovered | ReceiptState::ForwardRecoveryFailed
            )
            | (
                ReceiptState::ForwardRecoveryFailed,
                ReceiptState::ConvergingForward
            )
            | (ReceiptState::Succeeded, ReceiptState::RollingBack)
            | (
                ReceiptState::RollingBack,
                ReceiptState::RolledBack | ReceiptState::RollbackFailed
            )
    )
}

#[derive(Debug, Error)]
pub enum ReceiptError {
    #[error("receipt schema/contract version is unsupported: {0}")]
    UnsupportedSchemaVersion(u32),
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error("illegal receipt transition from {from:?} to {to:?}")]
    IllegalTransition {
        from: ReceiptState,
        to: ReceiptState,
    },
    #[error("receipt transition hash chain is invalid")]
    InvalidHashChain,
    #[error("operation progress transition is invalid")]
    InvalidOperationProgress,
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
    #[error("receipt store contains an invalid run directory")]
    InvalidRunDirectory,
    #[error(
        "stale receipt write at sequence {attempted_sequence}; persisted sequence is {persisted_sequence}"
    )]
    StaleWrite {
        persisted_sequence: u64,
        attempted_sequence: u64,
    },
}

#[cfg(test)]
mod receipt_chain_tests {
    use super::*;

    fn digest(character: char) -> Sha256Digest {
        Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
    }

    fn journal() -> ReceiptJournal {
        ReceiptJournal::new(
            StableId::parse("tampered-run").unwrap(),
            digest('a'),
            StableId::parse("target").unwrap(),
            digest('b'),
            digest('c'),
            digest('d'),
            PlanBindings {
                target_identity_digest: digest('e'),
                composed_loadout_digest: digest('f'),
                provider_inputs_digest: digest('1'),
                ownership_map_digest: digest('2'),
                artifact_set_digest: digest('3'),
                package_resolution_authority_digest: None,
            },
        )
        .unwrap()
    }

    fn replace_states(journal: &mut ReceiptJournal, states: &[ReceiptState]) {
        let progress_digest = digest_domain_json(
            "commonkit.operation-progress.v1",
            &journal.receipt.operation_progress,
        )
        .unwrap();
        let mut transitions = Vec::new();
        let mut previous = None;
        for (sequence, state) in states.iter().copied().enumerate() {
            let transition =
                make_transition(sequence as u64, state, previous, progress_digest.clone()).unwrap();
            previous = Some(transition.entry_digest.clone());
            transitions.push(transition);
        }
        journal.receipt.receipt_id = previous.unwrap();
        journal.receipt.transitions = transitions;
    }

    #[test]
    fn hash_valid_receipts_reject_invalid_state_history_and_top_level_state() {
        let mut invalid_first = journal();
        replace_states(&mut invalid_first, &[ReceiptState::Applying]);
        invalid_first.receipt.state = ReceiptState::Applying;
        assert!(matches!(
            invalid_first.verify_chain(),
            Err(ReceiptError::InvalidHashChain)
        ));

        let mut illegal_adjacent = journal();
        replace_states(
            &mut illegal_adjacent,
            &[ReceiptState::Prepared, ReceiptState::Succeeded],
        );
        illegal_adjacent.receipt.state = ReceiptState::Succeeded;
        assert!(matches!(
            illegal_adjacent.verify_chain(),
            Err(ReceiptError::InvalidHashChain)
        ));

        let mut mismatched_top = journal();
        replace_states(
            &mut mismatched_top,
            &[ReceiptState::Prepared, ReceiptState::Applying],
        );
        mismatched_top.receipt.state = ReceiptState::Prepared;
        assert!(matches!(
            mismatched_top.verify_chain(),
            Err(ReceiptError::InvalidHashChain)
        ));
    }
}
