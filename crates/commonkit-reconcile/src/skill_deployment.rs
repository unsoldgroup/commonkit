use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use commonkit_contracts::{
    ContractError, Operation, Plan, PlanBindings, ReceiptState, Sha256Digest, StableId,
    canonical_json, digest_domain_json,
};
use commonkit_core::{PlanBuildError, PlanDraft, build_plan};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    Adapter, AdapterFailure, ReceiptError, ReceiptStore, ReconcileError, ReconcileOutcome,
    Reconciler, sync_directory,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillDeploymentRequest {
    pub candidate_id: StableId,
    pub promotion_receipt_id: Sha256Digest,
    pub apm_package: StableId,
    pub canary_loadout: StableId,
    pub observed_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedSkillPromotion {
    pub candidate_id: StableId,
    pub candidate_digest: Sha256Digest,
    pub promotion_receipt_id: Sha256Digest,
    pub promoted_source_digest: Sha256Digest,
    pub policy_digest: Sha256Digest,
}

pub trait SkillPromotionAuthority {
    fn authenticate(
        &self,
        promotion_receipt_id: &Sha256Digest,
    ) -> Result<AuthenticatedSkillPromotion, SkillDeploymentError>;
}

pub trait CanaryStateObserver {
    fn observe_digest(
        &mut self,
        target: &StableId,
        operations: &[Operation],
    ) -> Result<Sha256Digest, AdapterFailure>;
}

pub struct DeploymentTrustStore {
    root: PathBuf,
    key: [u8; 32],
}

impl DeploymentTrustStore {
    /// Explicit disaster-recovery path for a lost key. Existing anchors are
    /// atomically archived to a caller-selected, non-existing location before
    /// a fresh trust domain is created; they are never silently re-signed.
    pub fn archive_and_rotate_missing_key(
        root: impl AsRef<Path>,
        key_path: impl AsRef<Path>,
        archive: impl AsRef<Path>,
    ) -> Result<Self, SkillDeploymentError> {
        let root = root.as_ref();
        let key_path = key_path.as_ref();
        let archive = archive.as_ref();
        if key_path.exists()
            || !root.is_dir()
            || fs::read_dir(root)?.next().is_none()
            || archive.exists()
        {
            return Err(SkillDeploymentError::InvalidTrustRotation);
        }
        if let Some(parent) = archive.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(root, archive)?;
        sync_directory(archive.parent().unwrap_or_else(|| Path::new(".")))?;
        fs::create_dir_all(root)?;
        Self::open_or_create(root, key_path)
    }

    pub fn open_or_create(
        root: impl AsRef<Path>,
        key_path: impl AsRef<Path>,
    ) -> Result<Self, SkillDeploymentError> {
        let key_path = key_path.as_ref();
        if let Some(parent) = key_path.parent() {
            fs::create_dir_all(parent)?;
            #[cfg(unix)]
            fs::set_permissions(parent, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
        }
        let key = match fs::symlink_metadata(key_path) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(SkillDeploymentError::UnsafeTrustKey);
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if metadata.permissions().mode() & 0o077 != 0 {
                        return Err(SkillDeploymentError::UnsafeTrustKey);
                    }
                }
                let bytes = fs::read(key_path)?;
                bytes
                    .try_into()
                    .map_err(|_| SkillDeploymentError::UnsafeTrustKey)?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if root.as_ref().exists() && fs::read_dir(root.as_ref())?.next().is_some() {
                    return Err(SkillDeploymentError::MissingTrustKeyForExistingAnchors);
                }
                let mut key = [0_u8; 32];
                rand::rng().fill_bytes(&mut key);
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(key_path)?;
                file.write_all(&key)?;
                file.sync_all()?;
                key
            }
            Err(error) => return Err(error.into()),
        };
        Self::open(root, key)
    }

    pub fn open(root: impl AsRef<Path>, key: [u8; 32]) -> Result<Self, SkillDeploymentError> {
        fs::create_dir_all(root.as_ref())?;
        let root = root.as_ref().canonicalize()?;
        Ok(Self { root, key })
    }

    fn anchor(
        &self,
        run_id: &StableId,
        label: &str,
        bytes: &[u8],
    ) -> Result<(), SkillDeploymentError> {
        persist_receipt(
            &self.path(run_id, label),
            hmac_sha256(&self.key, bytes).as_bytes(),
        )
    }

    fn verify(
        &self,
        run_id: &StableId,
        label: &str,
        bytes: &[u8],
    ) -> Result<(), SkillDeploymentError> {
        let anchored = fs::read(self.path(run_id, label))?;
        let expected = hmac_sha256(&self.key, bytes);
        if anchored.as_slice() != expected.as_bytes() {
            return Err(SkillDeploymentError::DeploymentAnchorMismatch);
        }
        Ok(())
    }

    fn path(&self, run_id: &StableId, label: &str) -> PathBuf {
        self.root
            .join(format!("{}.{}.anchor", run_id.as_str(), label))
    }
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
    pub observed_digest: Sha256Digest,
    pub operations: Vec<Operation>,
}

pub trait ApmCompiler {
    fn compile(
        &mut self,
        package: &StableId,
        policy_digest: &Sha256Digest,
    ) -> Result<ApmCompilation, AdapterFailure>;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillDeploymentIntent {
    run_id: StableId,
    plan: Plan,
    lineage: SkillDeploymentLineage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillDeploymentRecoveryReceipt {
    pub id: Sha256Digest,
    pub run_id: StableId,
    pub plan_id: Sha256Digest,
    pub outcome: ReconcileOutcome,
    pub restored_digest: Option<Sha256Digest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillDeploymentRecovery {
    Finalized(SkillDeploymentReceipt),
    Recovered(SkillDeploymentRecoveryReceipt),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillDeploymentRollbackFailureReceipt {
    pub id: Sha256Digest,
    pub deployment_receipt_id: Sha256Digest,
    pub run_id: StableId,
    pub state: SkillDeploymentState,
    pub operation_ids: Vec<Sha256Digest>,
    pub error_code: String,
}

pub struct SkillDeploymentWorkflow<'a> {
    store: &'a ReceiptStore,
    trust: &'a DeploymentTrustStore,
}

impl<'a> SkillDeploymentWorkflow<'a> {
    pub fn new(store: &'a ReceiptStore, trust: &'a DeploymentTrustStore) -> Self {
        Self { store, trust }
    }

    pub fn prepare(
        &self,
        request: SkillDeploymentRequest,
        compiler: &mut dyn ApmCompiler,
        authority: &dyn SkillPromotionAuthority,
    ) -> Result<PreparedSkillDeployment, SkillDeploymentError> {
        let authenticated = authority.authenticate(&request.promotion_receipt_id)?;
        if authenticated.promotion_receipt_id != request.promotion_receipt_id
            || authenticated.candidate_id != request.candidate_id
        {
            return Err(SkillDeploymentError::PromotionAuthorityMismatch);
        }
        let compilation = compiler
            .compile(&request.apm_package, &authenticated.policy_digest)
            .map_err(SkillDeploymentError::ApmCompile)?;
        if compilation.source_digest != authenticated.promoted_source_digest {
            return Err(SkillDeploymentError::ApmSourceDigestMismatch);
        }
        if compilation.observed_digest != request.observed_digest {
            return Err(SkillDeploymentError::ObservedStateMismatch);
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
            observed_digest: compilation.observed_digest,
            policy_digest: authenticated.policy_digest,
            bindings,
            operations: compilation.operations,
        })?;
        Ok(PreparedSkillDeployment {
            plan,
            lineage: SkillDeploymentLineage {
                candidate_id: request.candidate_id,
                candidate_digest: authenticated.candidate_digest,
                promotion_receipt_id: request.promotion_receipt_id,
                promoted_source_digest: authenticated.promoted_source_digest,
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
        let intent_record = SkillDeploymentIntent {
            run_id: run_id.clone(),
            plan: prepared.plan.clone(),
            lineage: prepared.lineage.clone(),
        };
        let intent = canonical_json(&intent_record)?;
        let run_directory = self.store.root().join(run_id.as_str());
        fs::create_dir_all(&run_directory)?;
        persist_receipt(
            &run_directory.join("skill-deployment-intent.receipt"),
            &intent,
        )?;
        self.trust.anchor(&run_id, "intent", &intent)?;
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
        let bytes = canonical_json(&receipt)?;
        persist_receipt(&deployment_path(self.store, &receipt.run_id), &bytes)?;
        self.trust.anchor(&receipt.run_id, "receipt", &bytes)?;
        Ok(receipt)
    }

    pub fn recover(
        &self,
        run_id: &StableId,
        adapters: &mut [Box<dyn Adapter>],
        observer: &mut dyn CanaryStateObserver,
    ) -> Result<SkillDeploymentRecovery, SkillDeploymentError> {
        let intent_bytes = read_receipt_bytes(
            &self
                .store
                .root()
                .join(run_id.as_str())
                .join("skill-deployment-intent.receipt"),
        )?;
        self.trust.verify(run_id, "intent", &intent_bytes)?;
        let intent: SkillDeploymentIntent = serde_json::from_slice(&intent_bytes)?;
        if &intent.run_id != run_id {
            return Err(SkillDeploymentError::DeploymentReceiptMismatch);
        }
        let journal = self.store.load(run_id.clone())?;
        if journal.receipt().state == ReceiptState::Succeeded {
            let draft = SkillDeploymentReceiptDraft {
                run_id: run_id.clone(),
                candidate_id: intent.lineage.candidate_id,
                candidate_digest: intent.lineage.candidate_digest,
                promotion_receipt_id: intent.lineage.promotion_receipt_id,
                apm_provider_inputs_digest: intent.lineage.apm_provider_inputs_digest,
                canary_loadout: intent.lineage.canary_loadout,
                reconcile_receipt_id: journal.receipt().receipt_id.clone(),
                reconcile_outcome: ReconcileOutcome::Succeeded,
                state: SkillDeploymentState::Verified,
                plan: intent.plan,
            };
            let receipt = draft.into_receipt()?;
            let bytes = canonical_json(&receipt)?;
            persist_receipt(&deployment_path(self.store, run_id), &bytes)?;
            self.trust.anchor(run_id, "receipt", &bytes)?;
            return Ok(SkillDeploymentRecovery::Finalized(receipt));
        }
        let outcome = match journal.receipt().state {
            ReceiptState::RolledBack => ReconcileOutcome::RolledBack,
            ReceiptState::RollbackFailed => ReconcileOutcome::RollbackFailed,
            ReceiptState::Canceled => ReconcileOutcome::Canceled,
            _ => Reconciler::with_store(self.store).recover_run(
                run_id.clone(),
                &intent.plan,
                adapters,
            )?,
        };
        let restored_digest = match outcome {
            ReconcileOutcome::RolledBack => {
                let observed = match observer
                    .observe_digest(&intent.plan.target_id, &intent.plan.operations)
                {
                    Ok(observed) => observed,
                    Err(error) => {
                        self.persist_recovery_receipt(
                            run_id,
                            &intent.plan,
                            ReconcileOutcome::RollbackFailed,
                            None,
                        )?;
                        return Err(SkillDeploymentError::Observe(error));
                    }
                };
                if observed != intent.plan.observed_digest {
                    self.persist_recovery_receipt(
                        run_id,
                        &intent.plan,
                        ReconcileOutcome::RollbackFailed,
                        None,
                    )?;
                    return Err(SkillDeploymentError::RollbackVerificationFailed);
                }
                Some(observed)
            }
            ReconcileOutcome::Canceled => None,
            ReconcileOutcome::RollbackFailed => None,
            other => return Err(SkillDeploymentError::UnexpectedRollbackOutcome(other)),
        };
        let receipt =
            self.persist_recovery_receipt(run_id, &intent.plan, outcome, restored_digest)?;
        Ok(SkillDeploymentRecovery::Recovered(receipt))
    }

    fn persist_recovery_receipt(
        &self,
        run_id: &StableId,
        plan: &Plan,
        outcome: ReconcileOutcome,
        restored_digest: Option<Sha256Digest>,
    ) -> Result<SkillDeploymentRecoveryReceipt, SkillDeploymentError> {
        let draft = (run_id, &plan.id, outcome, &restored_digest);
        let receipt = SkillDeploymentRecoveryReceipt {
            id: digest_domain_json("commonkit.skill-deployment-recovery.v1", &draft)?,
            run_id: run_id.clone(),
            plan_id: plan.id.clone(),
            outcome,
            restored_digest,
        };
        let bytes = canonical_json(&receipt)?;
        persist_receipt(
            &self
                .store
                .root()
                .join(run_id.as_str())
                .join("skill-deployment-recovery.receipt"),
            &bytes,
        )?;
        self.trust.anchor(run_id, "recovery", &bytes)?;
        Ok(receipt)
    }

    pub fn load(
        &self,
        run_id: &StableId,
        expected_id: &Sha256Digest,
    ) -> Result<SkillDeploymentReceipt, SkillDeploymentError> {
        let intent_path = self
            .store
            .root()
            .join(run_id.as_str())
            .join("skill-deployment-intent.receipt");
        let intent = read_receipt_bytes(&intent_path)?;
        self.trust.verify(run_id, "intent", &intent)?;
        let path = deployment_path(self.store, run_id);
        let bytes = read_receipt_bytes(&path)?;
        self.trust.verify(run_id, "receipt", &bytes)?;
        let receipt: SkillDeploymentReceipt = serde_json::from_slice(&bytes)?;
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
        observer: &mut dyn CanaryStateObserver,
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
            ReconcileOutcome::RollbackFailed => {
                self.persist_rollback_failure(
                    run_id,
                    &deployment,
                    "skill_deployment_rollback_failed",
                )?;
                return Err(SkillDeploymentError::CanaryRollbackFailed);
            }
            _ => return Err(SkillDeploymentError::UnexpectedRollbackOutcome(outcome)),
        };
        let observed = match observer
            .observe_digest(&deployment.canary_loadout, &deployment.plan.operations)
        {
            Ok(observed) => observed,
            Err(error) => {
                self.persist_rollback_failure(
                    run_id,
                    &deployment,
                    "skill_deployment_observation_failed",
                )?;
                return Err(SkillDeploymentError::Observe(error));
            }
        };
        if observed != deployment.plan.observed_digest {
            self.persist_rollback_failure(
                run_id,
                &deployment,
                "skill_deployment_rollback_verification_failed",
            )?;
            return Err(SkillDeploymentError::RollbackVerificationFailed);
        }
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
        let bytes = canonical_json(&receipt)?;
        persist_receipt(&rollback_path(self.store, run_id), &bytes)?;
        self.trust.anchor(run_id, "rollback", &bytes)?;
        Ok(receipt)
    }

    pub fn load_rollback(
        &self,
        run_id: &StableId,
        expected_id: &Sha256Digest,
    ) -> Result<SkillDeploymentRollbackReceipt, SkillDeploymentError> {
        let bytes = read_receipt_bytes(&rollback_path(self.store, run_id))?;
        self.trust.verify(run_id, "rollback", &bytes)?;
        let receipt: SkillDeploymentRollbackReceipt = serde_json::from_slice(&bytes)?;
        let draft = SkillDeploymentRollbackDraft {
            deployment_receipt_id: receipt.deployment_receipt_id.clone(),
            candidate_id: receipt.candidate_id.clone(),
            promotion_receipt_id: receipt.promotion_receipt_id.clone(),
            canary_loadout: receipt.canary_loadout.clone(),
            restored_digest: receipt.restored_digest.clone(),
            state: receipt.state,
            operation_ids: receipt.operation_ids.clone(),
        };
        if &receipt.id != expected_id
            || digest_domain_json("commonkit.skill-deployment-rollback.v1", &draft)? != receipt.id
        {
            return Err(SkillDeploymentError::DeploymentReceiptMismatch);
        }
        Ok(receipt)
    }

    pub fn load_rollback_failure(
        &self,
        run_id: &StableId,
        expected_id: &Sha256Digest,
    ) -> Result<SkillDeploymentRollbackFailureReceipt, SkillDeploymentError> {
        let bytes = read_receipt_bytes(&rollback_failure_path(self.store, run_id))?;
        self.trust.verify(run_id, "rollback-failure", &bytes)?;
        let receipt: SkillDeploymentRollbackFailureReceipt = serde_json::from_slice(&bytes)?;
        let draft = (
            &receipt.deployment_receipt_id,
            &receipt.run_id,
            receipt.state,
            &receipt.operation_ids,
            receipt.error_code.as_str(),
        );
        if &receipt.id != expected_id
            || receipt.run_id != *run_id
            || digest_domain_json("commonkit.skill-deployment-rollback-failure.v1", &draft)?
                != receipt.id
        {
            return Err(SkillDeploymentError::DeploymentReceiptMismatch);
        }
        Ok(receipt)
    }

    fn persist_rollback_failure(
        &self,
        run_id: &StableId,
        deployment: &SkillDeploymentReceipt,
        error_code: &str,
    ) -> Result<(), SkillDeploymentError> {
        let operation_ids = deployment
            .plan
            .operations
            .iter()
            .rev()
            .map(|operation| operation.id.clone())
            .collect::<Vec<_>>();
        let draft = (
            &deployment.id,
            run_id,
            SkillDeploymentState::RollbackFailed,
            &operation_ids,
            error_code,
        );
        let failure = SkillDeploymentRollbackFailureReceipt {
            id: digest_domain_json("commonkit.skill-deployment-rollback-failure.v1", &draft)?,
            deployment_receipt_id: deployment.id.clone(),
            run_id: run_id.clone(),
            state: SkillDeploymentState::RollbackFailed,
            operation_ids,
            error_code: error_code.into(),
        };
        let bytes = canonical_json(&failure)?;
        persist_receipt(&rollback_failure_path(self.store, run_id), &bytes)?;
        self.trust.anchor(run_id, "rollback-failure", &bytes)
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

fn rollback_failure_path(store: &ReceiptStore, run_id: &StableId) -> std::path::PathBuf {
    store
        .root()
        .join(run_id.as_str())
        .join("skill-deployment-rollback-failure.receipt")
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

fn read_receipt_bytes(path: &Path) -> Result<Vec<u8>, SkillDeploymentError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(SkillDeploymentError::DeploymentReceiptMismatch);
    }
    Ok(fs::read(path)?)
}

fn hmac_sha256(key: &[u8; 32], message: &[u8]) -> String {
    let mut inner_pad = [0x36_u8; 64];
    let mut outer_pad = [0x5c_u8; 64];
    for (index, byte) in key.iter().enumerate() {
        inner_pad[index] ^= byte;
        outer_pad[index] ^= byte;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    format!("{:x}", outer.finalize())
}

#[derive(Debug, Error)]
pub enum SkillDeploymentError {
    #[error("APM compilation failed: {0:?}")]
    ApmCompile(AdapterFailure),
    #[error("APM output was not compiled from the promoted source digest")]
    ApmSourceDigestMismatch,
    #[error("canary target changed after the requested observation")]
    ObservedStateMismatch,
    #[error("APM compilation produced no target operations")]
    EmptyCompilation,
    #[error("canary reconciliation did not verify: {0:?}")]
    CanaryNotVerified(ReconcileOutcome),
    #[error("only a verified durable canary deployment can be rolled back")]
    DeploymentNotVerified,
    #[error("durable canary deployment receipt does not match its authenticated state")]
    DeploymentReceiptMismatch,
    #[error("durable canary deployment anchor does not authenticate its receipt")]
    DeploymentAnchorMismatch,
    #[error("deployment trust key is not a protected ordinary 32-byte file")]
    UnsafeTrustKey,
    #[error(
        "deployment anchors exist but their trust key is missing; archive the run state and explicitly rotate trust before retrying"
    )]
    MissingTrustKeyForExistingAnchors,
    #[error(
        "trust rotation requires a missing key, non-empty anchor root, and new archive destination"
    )]
    InvalidTrustRotation,
    #[error("durable promotion receipt or candidate state did not authenticate")]
    PromotionAuthorityMismatch,
    #[error("canary restored-state observation failed: {0:?}")]
    Observe(AdapterFailure),
    #[error("canary rollback did not restore the exact observed target state")]
    RollbackVerificationFailed,
    #[error("canary reconciliation rollback failed; restored state was not certified")]
    CanaryRollbackFailed,
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
            Self::ObservedStateMismatch => "skill_deployment_observed_state_mismatch",
            Self::EmptyCompilation => "apm_empty_compilation",
            Self::CanaryNotVerified(_) => "skill_canary_not_verified",
            Self::DeploymentNotVerified => "skill_deployment_not_verified",
            Self::DeploymentReceiptMismatch => "skill_deployment_receipt_mismatch",
            Self::DeploymentAnchorMismatch => "skill_deployment_anchor_mismatch",
            Self::UnsafeTrustKey => "skill_deployment_trust_key_invalid",
            Self::MissingTrustKeyForExistingAnchors => "skill_deployment_trust_key_missing",
            Self::InvalidTrustRotation => "skill_deployment_trust_rotation_invalid",
            Self::PromotionAuthorityMismatch => "skill_promotion_authority_mismatch",
            Self::Observe(_) => "skill_deployment_observation_failed",
            Self::RollbackVerificationFailed => "skill_deployment_rollback_verification_failed",
            Self::CanaryRollbackFailed => "skill_deployment_rollback_failed",
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
