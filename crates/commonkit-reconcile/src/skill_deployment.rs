use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use commonkit_contracts::{
    ContractError, Operation, Plan, PlanBindings, Sha256Digest, StableId, canonical_json,
    digest_domain_json,
};
use commonkit_core::{PlanBuildError, PlanDraft, build_plan};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    Adapter, AdapterFailure, ReceiptError, ReceiptStore, ReconcileError, ReconcileOutcome,
    Reconciler, sync_directory,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillDeploymentRequest {
    pub candidate_id: StableId,
    pub candidate_digest: Sha256Digest,
    pub promotion_receipt_id: Sha256Digest,
    pub promoted_source_digest: Sha256Digest,
    pub apm_package: StableId,
    pub canary_loadout: StableId,
    pub observed_digest: Sha256Digest,
    pub policy_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApmCompilation {
    pub source_digest: Sha256Digest,
    pub provider_inputs_digest: Sha256Digest,
    pub composed_loadout_digest: Sha256Digest,
    pub target_identity_digest: Sha256Digest,
    pub ownership_map_digest: Sha256Digest,
    pub artifact_set_digest: Sha256Digest,
    pub desired_digest: Sha256Digest,
    pub operations: Vec<Operation>,
}

pub trait ApmCompiler {
    fn compile(&mut self, package: &StableId) -> Result<ApmCompilation, AdapterFailure>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillDeploymentLineage {
    pub candidate_id: StableId,
    pub candidate_digest: Sha256Digest,
    pub promotion_receipt_id: Sha256Digest,
    pub promoted_source_digest: Sha256Digest,
    pub apm_package: StableId,
    pub apm_provider_inputs_digest: Sha256Digest,
    pub canary_loadout: StableId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedSkillDeployment {
    pub plan: Plan,
    pub lineage: SkillDeploymentLineage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillDeploymentState {
    Verified,
    RolledBack,
    RollbackFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillDeploymentReceipt {
    pub id: Sha256Digest,
    pub run_id: StableId,
    pub candidate_id: StableId,
    pub candidate_digest: Sha256Digest,
    pub promotion_receipt_id: Sha256Digest,
    pub apm_provider_inputs_digest: Sha256Digest,
    pub canary_loadout: StableId,
    pub reconcile_receipt_id: Sha256Digest,
    pub reconcile_outcome: ReconcileOutcome,
    pub state: SkillDeploymentState,
    pub plan: Plan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillDeploymentRollbackReceipt {
    pub id: Sha256Digest,
    pub deployment_receipt_id: Sha256Digest,
    pub candidate_id: StableId,
    pub promotion_receipt_id: Sha256Digest,
    pub canary_loadout: StableId,
    pub restored_digest: Sha256Digest,
    pub state: SkillDeploymentState,
    pub operation_ids: Vec<Sha256Digest>,
}

pub struct SkillDeploymentWorkflow<'a> {
    store: &'a ReceiptStore,
}

impl<'a> SkillDeploymentWorkflow<'a> {
    pub fn new(store: &'a ReceiptStore) -> Self {
        Self { store }
    }

    pub fn prepare(
        &self,
        request: SkillDeploymentRequest,
        compiler: &mut dyn ApmCompiler,
    ) -> Result<PreparedSkillDeployment, SkillDeploymentError> {
        let compilation = compiler
            .compile(&request.apm_package)
            .map_err(SkillDeploymentError::ApmCompile)?;
        if compilation.source_digest != request.promoted_source_digest {
            return Err(SkillDeploymentError::ApmSourceDigestMismatch);
        }
        if compilation.operations.is_empty() {
            return Err(SkillDeploymentError::EmptyCompilation);
        }
        let bindings = PlanBindings {
            composed_loadout_digest: compilation.composed_loadout_digest,
            provider_inputs_digest: compilation.provider_inputs_digest.clone(),
            target_identity_digest: compilation.target_identity_digest,
            ownership_map_digest: compilation.ownership_map_digest,
            artifact_set_digest: compilation.artifact_set_digest,
        };
        let plan = build_plan(PlanDraft {
            target_id: request.canary_loadout.clone(),
            desired_digest: compilation.desired_digest,
            observed_digest: request.observed_digest,
            policy_digest: request.policy_digest,
            bindings,
            operations: compilation.operations,
        })?;
        Ok(PreparedSkillDeployment {
            plan,
            lineage: SkillDeploymentLineage {
                candidate_id: request.candidate_id,
                candidate_digest: request.candidate_digest,
                promotion_receipt_id: request.promotion_receipt_id,
                promoted_source_digest: request.promoted_source_digest,
                apm_package: request.apm_package,
                apm_provider_inputs_digest: compilation.provider_inputs_digest,
                canary_loadout: request.canary_loadout,
            },
        })
    }

    pub fn apply(
        &self,
        prepared: PreparedSkillDeployment,
        run_id: StableId,
        adapters: &mut [Box<dyn Adapter>],
    ) -> Result<SkillDeploymentReceipt, SkillDeploymentError> {
        let outcome =
            Reconciler::with_store(self.store).execute(&prepared.plan, run_id.clone(), adapters)?;
        if outcome != ReconcileOutcome::Succeeded {
            return Err(SkillDeploymentError::CanaryNotVerified(outcome));
        }
        let reconcile = self.store.load(run_id.clone())?;
        let draft = SkillDeploymentReceiptDraft {
            run_id,
            candidate_id: prepared.lineage.candidate_id,
            candidate_digest: prepared.lineage.candidate_digest,
            promotion_receipt_id: prepared.lineage.promotion_receipt_id,
            apm_provider_inputs_digest: prepared.lineage.apm_provider_inputs_digest,
            canary_loadout: prepared.lineage.canary_loadout,
            reconcile_receipt_id: reconcile.receipt().receipt_id.clone(),
            reconcile_outcome: outcome,
            state: SkillDeploymentState::Verified,
            plan: prepared.plan,
        };
        let receipt = draft.into_receipt()?;
        persist_receipt(
            &deployment_path(self.store, &receipt.run_id),
            &canonical_json(&receipt)?,
        )?;
        Ok(receipt)
    }

    pub fn load(
        &self,
        run_id: &StableId,
        expected_id: &Sha256Digest,
    ) -> Result<SkillDeploymentReceipt, SkillDeploymentError> {
        let receipt: SkillDeploymentReceipt = read_receipt(&deployment_path(self.store, run_id))?;
        validate_receipt(&receipt, expected_id)?;
        let reconcile = self.store.load(run_id.clone())?;
        if reconcile.receipt().plan_id != receipt.plan.id
            || reconcile.receipt().receipt_id != receipt.reconcile_receipt_id
        {
            return Err(SkillDeploymentError::DeploymentReceiptMismatch);
        }
        Ok(receipt)
    }

    pub fn rollback(
        &self,
        run_id: &StableId,
        deployment_id: &Sha256Digest,
        adapters: &mut [Box<dyn Adapter>],
    ) -> Result<SkillDeploymentRollbackReceipt, SkillDeploymentError> {
        let deployment = self.load(run_id, deployment_id)?;
        if deployment.state != SkillDeploymentState::Verified
            || deployment.reconcile_outcome != ReconcileOutcome::Succeeded
        {
            return Err(SkillDeploymentError::DeploymentNotVerified);
        }
        let outcome = Reconciler::with_store(self.store).rollback_succeeded_run(
            run_id.clone(),
            &deployment.plan,
            adapters,
        )?;
        let state = match outcome {
            ReconcileOutcome::RolledBack => SkillDeploymentState::RolledBack,
            ReconcileOutcome::RollbackFailed => SkillDeploymentState::RollbackFailed,
            _ => return Err(SkillDeploymentError::UnexpectedRollbackOutcome(outcome)),
        };
        let operation_ids = deployment
            .plan
            .operations
            .iter()
            .rev()
            .map(|operation| operation.id.clone())
            .collect::<Vec<_>>();
        let draft = SkillDeploymentRollbackDraft {
            deployment_receipt_id: deployment.id,
            candidate_id: deployment.candidate_id,
            promotion_receipt_id: deployment.promotion_receipt_id,
            canary_loadout: deployment.canary_loadout,
            restored_digest: deployment.plan.observed_digest,
            state,
            operation_ids,
        };
        let receipt = draft.into_receipt()?;
        persist_receipt(
            &rollback_path(self.store, run_id),
            &canonical_json(&receipt)?,
        )?;
        Ok(receipt)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillDeploymentReceiptDraft {
    run_id: StableId,
    candidate_id: StableId,
    candidate_digest: Sha256Digest,
    promotion_receipt_id: Sha256Digest,
    apm_provider_inputs_digest: Sha256Digest,
    canary_loadout: StableId,
    reconcile_receipt_id: Sha256Digest,
    reconcile_outcome: ReconcileOutcome,
    state: SkillDeploymentState,
    plan: Plan,
}

impl SkillDeploymentReceiptDraft {
    fn into_receipt(self) -> Result<SkillDeploymentReceipt, ContractError> {
        let id = digest_domain_json("commonkit.skill-deployment-receipt.v1", &self)?;
        Ok(SkillDeploymentReceipt {
            id,
            run_id: self.run_id,
            candidate_id: self.candidate_id,
            candidate_digest: self.candidate_digest,
            promotion_receipt_id: self.promotion_receipt_id,
            apm_provider_inputs_digest: self.apm_provider_inputs_digest,
            canary_loadout: self.canary_loadout,
            reconcile_receipt_id: self.reconcile_receipt_id,
            reconcile_outcome: self.reconcile_outcome,
            state: self.state,
            plan: self.plan,
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillDeploymentRollbackDraft {
    deployment_receipt_id: Sha256Digest,
    candidate_id: StableId,
    promotion_receipt_id: Sha256Digest,
    canary_loadout: StableId,
    restored_digest: Sha256Digest,
    state: SkillDeploymentState,
    operation_ids: Vec<Sha256Digest>,
}

impl SkillDeploymentRollbackDraft {
    fn into_receipt(self) -> Result<SkillDeploymentRollbackReceipt, ContractError> {
        let id = digest_domain_json("commonkit.skill-deployment-rollback.v1", &self)?;
        Ok(SkillDeploymentRollbackReceipt {
            id,
            deployment_receipt_id: self.deployment_receipt_id,
            candidate_id: self.candidate_id,
            promotion_receipt_id: self.promotion_receipt_id,
            canary_loadout: self.canary_loadout,
            restored_digest: self.restored_digest,
            state: self.state,
            operation_ids: self.operation_ids,
        })
    }
}

fn validate_receipt(
    receipt: &SkillDeploymentReceipt,
    expected_id: &Sha256Digest,
) -> Result<(), SkillDeploymentError> {
    let draft = SkillDeploymentReceiptDraft {
        run_id: receipt.run_id.clone(),
        candidate_id: receipt.candidate_id.clone(),
        candidate_digest: receipt.candidate_digest.clone(),
        promotion_receipt_id: receipt.promotion_receipt_id.clone(),
        apm_provider_inputs_digest: receipt.apm_provider_inputs_digest.clone(),
        canary_loadout: receipt.canary_loadout.clone(),
        reconcile_receipt_id: receipt.reconcile_receipt_id.clone(),
        reconcile_outcome: receipt.reconcile_outcome,
        state: receipt.state,
        plan: receipt.plan.clone(),
    };
    if &receipt.id != expected_id
        || digest_domain_json("commonkit.skill-deployment-receipt.v1", &draft)? != receipt.id
    {
        return Err(SkillDeploymentError::DeploymentReceiptMismatch);
    }
    Ok(())
}

fn deployment_path(store: &ReceiptStore, run_id: &StableId) -> std::path::PathBuf {
    store
        .root()
        .join(run_id.as_str())
        .join("skill-deployment.receipt")
}

fn rollback_path(store: &ReceiptStore, run_id: &StableId) -> std::path::PathBuf {
    store
        .root()
        .join(run_id.as_str())
        .join("skill-deployment-rollback.receipt")
}

fn persist_receipt(path: &Path, bytes: &[u8]) -> Result<(), SkillDeploymentError> {
    if path.exists() {
        return if fs::read(path)? == bytes {
            Ok(())
        } else {
            Err(SkillDeploymentError::DeploymentReceiptMismatch)
        };
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    sync_directory(
        path.parent()
            .ok_or(SkillDeploymentError::DeploymentReceiptMismatch)?,
    )?;
    Ok(())
}

fn read_receipt<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, SkillDeploymentError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(SkillDeploymentError::DeploymentReceiptMismatch);
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

#[derive(Debug, Error)]
pub enum SkillDeploymentError {
    #[error("APM compilation failed: {0:?}")]
    ApmCompile(AdapterFailure),
    #[error("APM output was not compiled from the promoted source digest")]
    ApmSourceDigestMismatch,
    #[error("APM compilation produced no target operations")]
    EmptyCompilation,
    #[error("canary reconciliation did not verify: {0:?}")]
    CanaryNotVerified(ReconcileOutcome),
    #[error("only a verified durable canary deployment can be rolled back")]
    DeploymentNotVerified,
    #[error("durable canary deployment receipt does not match its authenticated state")]
    DeploymentReceiptMismatch,
    #[error("canary rollback produced an unexpected outcome: {0:?}")]
    UnexpectedRollbackOutcome(ReconcileOutcome),
    #[error(transparent)]
    Plan(#[from] PlanBuildError),
    #[error(transparent)]
    Reconcile(#[from] ReconcileError),
    #[error(transparent)]
    Receipt(#[from] ReceiptError),
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl SkillDeploymentError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ApmCompile(_) => "apm_compile_failed",
            Self::ApmSourceDigestMismatch => "apm_source_digest_mismatch",
            Self::EmptyCompilation => "apm_empty_compilation",
            Self::CanaryNotVerified(_) => "skill_canary_not_verified",
            Self::DeploymentNotVerified => "skill_deployment_not_verified",
            Self::DeploymentReceiptMismatch => "skill_deployment_receipt_mismatch",
            Self::UnexpectedRollbackOutcome(_) => "skill_deployment_rollback_invalid",
            Self::Plan(_) => "skill_deployment_plan_invalid",
            Self::Reconcile(_) => "skill_deployment_reconcile_failed",
            Self::Receipt(_) => "skill_deployment_receipt_invalid",
            Self::Contract(_) => "skill_deployment_contract_invalid",
            Self::Io(_) => "skill_deployment_io_failed",
            Self::Json(_) => "skill_deployment_json_invalid",
        }
    }
}
