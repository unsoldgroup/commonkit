use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use commonkit_adapters::{
    ApmProvider, ApmProviderConfig, ArtifactStore, DesiredStateProvider, ExactProviderVersion,
    FileAdapter, NormalizedManagedPath, OwnershipRules, ProviderContext, ProviderPlanRequest,
    ProviderWorkspace, build_provider_plan,
};
use commonkit_contracts::digest_domain_json;
use commonkit_contracts::{Operation, Sha256Digest, StableId};
use commonkit_reconcile::{
    Adapter, AdapterFailure, ApmCompilation, ApmCompiler, CanaryStateObserver,
    DeploymentTrustStore, ReceiptStore, SkillDeploymentError, SkillDeploymentReceipt,
    SkillDeploymentRequest, SkillDeploymentRollbackReceipt, SkillDeploymentWorkflow,
};
use commonkit_skills::SkillEngine;
use serde::Deserialize;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SkillCanaryConfig {
    pub repository: PathBuf,
    pub apm_executable: PathBuf,
    pub apm_version: String,
    pub manifest: PathBuf,
    pub lockfile: PathBuf,
    pub policy: PathBuf,
    pub promoted_source: PathBuf,
    pub apm_package: StableId,
    pub canary_loadout: StableId,
    pub target_root: PathBuf,
    pub adapter_state: PathBuf,
    pub provider_artifacts: PathBuf,
    pub provider_staging: PathBuf,
    pub managed_root: NormalizedManagedPath,
    pub declared_roots: Vec<NormalizedManagedPath>,
    pub protected_roots: Vec<NormalizedManagedPath>,
    pub case_sensitive: bool,
    pub target_identity_digest: Sha256Digest,
    pub composed_loadout_digest: Sha256Digest,
    pub policy_digest: Sha256Digest,
}

pub(crate) struct SkillCanaryRuntime {
    engine: Arc<SkillEngine>,
    receipts: ReceiptStore,
    trust: DeploymentTrustStore,
    compiler: Mutex<ProductionApmCompiler>,
    target_root: PathBuf,
    adapter_state: PathBuf,
}

impl SkillCanaryRuntime {
    pub(crate) fn open(
        config: SkillCanaryConfig,
        state_root: &Path,
    ) -> Result<Self, SkillDeploymentError> {
        fs::create_dir_all(state_root)?;
        let engine = Arc::new(
            SkillEngine::open(&config.repository, state_root.join("engine"))
                .map_err(|_| SkillDeploymentError::PromotionAuthorityMismatch)?,
        );
        let receipts = ReceiptStore::open(state_root.join("receipts"))?;
        let trust = DeploymentTrustStore::open_or_create(
            state_root.join("anchors"),
            state_root.join("trust.key"),
        )?;
        let target_root = config.target_root.clone();
        let adapter_state = config.adapter_state.clone();
        let runtime = Self {
            engine,
            receipts,
            trust,
            compiler: Mutex::new(ProductionApmCompiler { config }),
            target_root,
            adapter_state,
        };
        runtime.recover_unfinished()?;
        Ok(runtime)
    }

    pub(crate) fn engine(&self) -> Arc<SkillEngine> {
        self.engine.clone()
    }

    pub(crate) fn apply(
        &self,
        request: SkillDeploymentRequest,
        run_id: StableId,
    ) -> Result<SkillDeploymentReceipt, SkillDeploymentError> {
        let mut compiler = self
            .compiler
            .lock()
            .map_err(|_| SkillDeploymentError::PromotionAuthorityMismatch)?;
        let workflow = SkillDeploymentWorkflow::new(&self.receipts, &self.trust);
        let prepared = workflow.prepare(request, &mut *compiler, self.engine.as_ref())?;
        let files = FileAdapter::open(&self.target_root, &self.adapter_state)
            .map_err(|error| SkillDeploymentError::Observe(fail(error)))?;
        let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(files)];
        workflow.apply(prepared, run_id, &mut adapters)
    }

    pub(crate) fn rollback(
        &self,
        run_id: &StableId,
        receipt_id: &Sha256Digest,
    ) -> Result<SkillDeploymentRollbackReceipt, SkillDeploymentError> {
        let workflow = SkillDeploymentWorkflow::new(&self.receipts, &self.trust);
        let files = FileAdapter::open(&self.target_root, &self.adapter_state)
            .map_err(|error| SkillDeploymentError::Observe(fail(error)))?;
        let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(files)];
        let mut observer = FilesystemCanaryObserver {
            target_root: self.target_root.clone(),
            adapter_state: self.adapter_state.clone(),
        };
        workflow.rollback(run_id, receipt_id, &mut adapters, &mut observer)
    }

    fn recover_unfinished(&self) -> Result<(), SkillDeploymentError> {
        for run_id in self.receipts.run_ids()? {
            let run_root = self.receipts.root().join(run_id.as_str());
            if run_root.join("skill-deployment.receipt").exists()
                || run_root.join("skill-deployment-recovery.receipt").exists()
                || !run_root.join("skill-deployment-intent.receipt").exists()
            {
                continue;
            }
            let files = FileAdapter::open(&self.target_root, &self.adapter_state)
                .map_err(|error| SkillDeploymentError::Observe(fail(error)))?;
            let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(files)];
            let mut observer = FilesystemCanaryObserver {
                target_root: self.target_root.clone(),
                adapter_state: self.adapter_state.clone(),
            };
            SkillDeploymentWorkflow::new(&self.receipts, &self.trust).recover(
                &run_id,
                &mut adapters,
                &mut observer,
            )?;
        }
        Ok(())
    }
}

struct ProductionApmCompiler {
    config: SkillCanaryConfig,
}

impl ApmCompiler for ProductionApmCompiler {
    fn compile(
        &mut self,
        package: &StableId,
        policy_digest: &Sha256Digest,
    ) -> Result<ApmCompilation, AdapterFailure> {
        if package != &self.config.apm_package {
            return Err(AdapterFailure::new(
                "apm_package_mismatch",
                "unconfigured package",
            ));
        }
        if policy_digest != &self.config.policy_digest {
            return Err(AdapterFailure::new(
                "skill_canary_policy_mismatch",
                "authenticated promotion policy does not match configured canary policy",
            ));
        }
        let workspace_root = self
            .config
            .provider_staging
            .join(format!("compile-{}", std::process::id()));
        if workspace_root.exists() {
            fs::remove_dir_all(&workspace_root)
                .map_err(|_| AdapterFailure::new("apm_staging_failed", "stale staging"))?;
        }
        fs::create_dir_all(&workspace_root)
            .map_err(|_| AdapterFailure::new("apm_staging_failed", "create staging"))?;
        let result = self.compile_in(&workspace_root, policy_digest);
        let _ = fs::remove_dir_all(&workspace_root);
        result
    }
}

impl ProductionApmCompiler {
    fn compile_in(
        &self,
        workspace_root: &Path,
        policy_digest: &Sha256Digest,
    ) -> Result<ApmCompilation, AdapterFailure> {
        let provider = ApmProvider::new(ApmProviderConfig {
            executable: self.config.apm_executable.clone(),
            version: ExactProviderVersion::parse(self.config.apm_version.clone()).map_err(fail)?,
            manifest: self.config.manifest.clone(),
            lockfile: self.config.lockfile.clone(),
            policy: self.config.policy.clone(),
            targets: vec!["claude".into(), "codex".into()],
            managed_root: self.config.managed_root.clone(),
            bound_source: Some(self.config.promoted_source.clone()),
        })
        .map_err(fail)?;
        let context = ProviderContext {
            target_id: self.config.canary_loadout.clone(),
            platform: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            policy_digest: policy_digest.clone(),
            declared_roots: self.config.declared_roots.clone(),
            observed_fact_digests: BTreeMap::new(),
        };
        let artifacts = ArtifactStore::open(&self.config.provider_artifacts).map_err(fail)?;
        let workspace = ProviderWorkspace::open(
            workspace_root,
            &[
                self.config.target_root.clone(),
                self.config.adapter_state.clone(),
            ],
        )
        .map_err(fail)?;
        let state = provider
            .materialize(&context, &workspace, &artifacts)
            .map_err(fail)?;
        let source_digest = state
            .inputs
            .input_digests
            .get("promotedSource")
            .cloned()
            .ok_or_else(|| fail("APM materialization omitted promotedSource input binding"))?;
        let mut files = FileAdapter::open(&self.config.target_root, &self.config.adapter_state)
            .map_err(fail)?;
        let observed_digest = files
            .observed_state_digest(state.resources.iter().map(|resource| &resource.intent))
            .map_err(fail)?;
        let rules = OwnershipRules::new(
            self.config.case_sensitive,
            self.config.declared_roots.clone(),
            self.config.protected_roots.clone(),
        )
        .map_err(fail)?;
        let plan = build_provider_plan(
            ProviderPlanRequest {
                target_id: self.config.canary_loadout.clone(),
                target_identity_digest: self.config.target_identity_digest.clone(),
                composed_loadout_digest: self.config.composed_loadout_digest.clone(),
                observed_digest,
                policy_digest: policy_digest.clone(),
                ownership_rules: &rules,
                mapped_side_effects: BTreeSet::new(),
            },
            &[state],
            &artifacts,
            &mut files,
        )
        .map_err(fail)?;
        let bound_artifact_set_digest = digest_domain_json(
            "commonkit.skill-canary-artifact-set.v1",
            &(&plan.bindings.artifact_set_digest, &source_digest),
        )
        .map_err(fail)?;
        Ok(ApmCompilation {
            source_digest,
            provider_inputs_digest: plan.bindings.provider_inputs_digest.clone(),
            composed_loadout_digest: plan.bindings.composed_loadout_digest.clone(),
            target_identity_digest: plan.bindings.target_identity_digest.clone(),
            ownership_map_digest: plan.bindings.ownership_map_digest.clone(),
            artifact_set_digest: bound_artifact_set_digest,
            desired_digest: plan.desired_digest,
            observed_digest: plan.observed_digest,
            operations: plan.operations,
        })
    }
}

struct FilesystemCanaryObserver {
    target_root: PathBuf,
    adapter_state: PathBuf,
}

impl CanaryStateObserver for FilesystemCanaryObserver {
    fn observe_digest(
        &mut self,
        _target: &StableId,
        operations: &[Operation],
    ) -> Result<Sha256Digest, AdapterFailure> {
        FileAdapter::open(&self.target_root, &self.adapter_state)
            .map_err(fail)?
            .observed_operations_digest(operations)
            .map_err(fail)
    }
}

fn fail(error: impl std::fmt::Display) -> AdapterFailure {
    AdapterFailure::new("skill_canary_provider_failed", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(value: char) -> Sha256Digest {
        Sha256Digest::parse(format!("sha256:{}", value.to_string().repeat(64))).expect("digest")
    }

    fn id(value: &str) -> StableId {
        StableId::parse(value).expect("id")
    }

    #[test]
    fn runtime_authenticates_durable_promotion_before_running_apm_or_mutating_target() {
        let root = std::env::temp_dir().join(format!(
            "commonkit-skill-canary-runtime-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let repository = root.join("repository");
        let target = root.join("target");
        fs::create_dir_all(&repository).expect("repository");
        fs::create_dir_all(&target).expect("target");
        let config = SkillCanaryConfig {
            repository,
            apm_executable: root.join("missing-apm"),
            apm_version: "0.25.0".into(),
            manifest: root.join("apm.yml"),
            lockfile: root.join("apm.lock.yaml"),
            policy: root.join("apm-policy.yml"),
            promoted_source: root.join("SKILL.md"),
            apm_package: id("review-skill"),
            canary_loadout: id("codex-canary"),
            target_root: target.clone(),
            adapter_state: root.join("adapter"),
            provider_artifacts: root.join("artifacts"),
            provider_staging: root.join("staging"),
            managed_root: NormalizedManagedPath::parse("canary").expect("root"),
            declared_roots: vec![NormalizedManagedPath::parse("canary").expect("root")],
            protected_roots: vec![],
            case_sensitive: true,
            target_identity_digest: digest('1'),
            composed_loadout_digest: digest('2'),
            policy_digest: digest('3'),
        };
        let runtime = SkillCanaryRuntime::open(config, &root.join("state")).expect("runtime");
        let error = runtime
            .apply(
                SkillDeploymentRequest {
                    candidate_id: id("candidate-1"),
                    promotion_receipt_id: digest('a'),
                    apm_package: id("review-skill"),
                    canary_loadout: id("codex-canary"),
                    observed_digest: digest('4'),
                },
                id("run-1"),
            )
            .expect_err("unknown promotion receipt must fail before provider execution");
        assert_eq!(error.code(), "skill_promotion_authority_mismatch");
        assert!(fs::read_dir(&target).expect("target").next().is_none());
        assert!(!root.join("staging").exists());
        fs::remove_dir_all(root).expect("cleanup");
    }
}
