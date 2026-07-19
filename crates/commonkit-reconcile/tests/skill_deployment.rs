use commonkit_contracts::{Operation, OperationKind, ResourceRef, Risk, Sha256Digest, StableId};
use commonkit_core::{OperationDraft, finalize_operation};
use commonkit_reconcile::{
    Adapter, AdapterFailure, ApmCompilation, ApmCompiler, AuthenticatedSkillPromotion,
    CanaryStateObserver, DeploymentTrustStore, ReceiptStore, ReconcileOutcome,
    SkillDeploymentError, SkillDeploymentRecovery, SkillDeploymentRequest, SkillDeploymentState,
    SkillDeploymentWorkflow, SkillPromotionAuthority,
};

fn temporary_directory(label: &str) -> std::path::PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("commonkit-{label}-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&path).expect("temporary directory");
    path
}

fn digest(byte: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", byte.to_string().repeat(64))).expect("digest")
}

fn id(value: &str) -> StableId {
    StableId::parse(value).expect("id")
}

fn operation() -> Operation {
    finalize_operation(OperationDraft {
        adapter_id: id("files"),
        kind: OperationKind::Update,
        resource: ResourceRef {
            resource_type: id("skill"),
            resource_id: id("review"),
            managed_path: Some("canary/skills/review/SKILL.md".into()),
        },
        risk: Risk::Medium,
        requires_confirmation: true,
        depends_on: vec![],
        before_digest: Some(digest('1')),
        after_digest: Some(digest('2')),
        payload_digest: digest('3'),
        summary: "Install accepted skill into the canary loadout".into(),
    })
    .expect("operation")
}

struct Compiler {
    source_digest: Sha256Digest,
}

impl ApmCompiler for Compiler {
    fn compile(
        &mut self,
        _package: &StableId,
        _policy_digest: &Sha256Digest,
    ) -> Result<ApmCompilation, AdapterFailure> {
        Ok(ApmCompilation {
            source_digest: self.source_digest.clone(),
            provider_inputs_digest: digest('4'),
            composed_loadout_digest: digest('5'),
            target_identity_digest: digest('0'),
            ownership_map_digest: digest('6'),
            artifact_set_digest: digest('7'),
            desired_digest: digest('8'),
            observed_digest: digest('c'),
            operations: vec![operation()],
        })
    }
}

#[derive(Default)]
struct RecordingAdapter {
    calls: Vec<&'static str>,
}

impl Adapter for RecordingAdapter {
    fn id(&self) -> &StableId {
        static ID: std::sync::OnceLock<StableId> = std::sync::OnceLock::new();
        ID.get_or_init(|| id("files"))
    }

    fn prepare(&mut self, _: &Operation) -> Result<(), AdapterFailure> {
        self.calls.push("prepare");
        Ok(())
    }

    fn apply(&mut self, _: &Operation) -> Result<(), AdapterFailure> {
        self.calls.push("apply");
        Ok(())
    }

    fn verify(&mut self, _: &Operation) -> Result<(), AdapterFailure> {
        self.calls.push("verify");
        Ok(())
    }

    fn rollback(&mut self, _: &Operation) -> Result<(), AdapterFailure> {
        self.calls.push("rollback");
        Ok(())
    }
}

struct FailingRollbackAdapter;
impl Adapter for FailingRollbackAdapter {
    fn id(&self) -> &StableId {
        static ID: std::sync::OnceLock<StableId> = std::sync::OnceLock::new();
        ID.get_or_init(|| id("files"))
    }
    fn prepare(&mut self, _: &Operation) -> Result<(), AdapterFailure> {
        Ok(())
    }
    fn apply(&mut self, _: &Operation) -> Result<(), AdapterFailure> {
        Ok(())
    }
    fn verify(&mut self, _: &Operation) -> Result<(), AdapterFailure> {
        Ok(())
    }
    fn rollback(&mut self, _: &Operation) -> Result<(), AdapterFailure> {
        Err(AdapterFailure::new("rollback_failed", "fixture"))
    }
}

struct FailingVerifyAdapter;
impl Adapter for FailingVerifyAdapter {
    fn id(&self) -> &StableId {
        static ID: std::sync::OnceLock<StableId> = std::sync::OnceLock::new();
        ID.get_or_init(|| id("files"))
    }
    fn prepare(&mut self, _: &Operation) -> Result<(), AdapterFailure> {
        Ok(())
    }
    fn apply(&mut self, _: &Operation) -> Result<(), AdapterFailure> {
        Ok(())
    }
    fn verify(&mut self, _: &Operation) -> Result<(), AdapterFailure> {
        Err(AdapterFailure::new("verify_failed", "fixture"))
    }
    fn rollback(&mut self, _: &Operation) -> Result<(), AdapterFailure> {
        Ok(())
    }
}

fn request() -> SkillDeploymentRequest {
    SkillDeploymentRequest {
        candidate_id: id("candidate-1"),
        promotion_receipt_id: digest('a'),
        apm_package: id("review-skill"),
        canary_loadout: id("codex-canary"),
        observed_digest: digest('c'),
    }
}

struct Authority {
    path: std::path::PathBuf,
}
impl SkillPromotionAuthority for Authority {
    fn authenticate(
        &self,
        promotion_receipt_id: &Sha256Digest,
    ) -> Result<AuthenticatedSkillPromotion, SkillDeploymentError> {
        let value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&self.path)
                .map_err(|_| SkillDeploymentError::PromotionAuthorityMismatch)?,
        )
        .map_err(|_| SkillDeploymentError::PromotionAuthorityMismatch)?;
        if promotion_receipt_id.as_str() != value["promotionReceiptId"].as_str().unwrap_or_default()
            || value["candidateState"] != "promoted"
        {
            return Err(SkillDeploymentError::PromotionAuthorityMismatch);
        }
        Ok(AuthenticatedSkillPromotion {
            candidate_id: StableId::parse(value["candidateId"].as_str().unwrap_or_default())
                .map_err(|_| SkillDeploymentError::PromotionAuthorityMismatch)?,
            candidate_digest: Sha256Digest::parse(
                value["candidateDigest"].as_str().unwrap_or_default(),
            )
            .map_err(|_| SkillDeploymentError::PromotionAuthorityMismatch)?,
            promotion_receipt_id: Sha256Digest::parse(
                value["promotionReceiptId"].as_str().unwrap_or_default(),
            )
            .map_err(|_| SkillDeploymentError::PromotionAuthorityMismatch)?,
            promoted_source_digest: Sha256Digest::parse(
                value["promotedSourceDigest"].as_str().unwrap_or_default(),
            )
            .map_err(|_| SkillDeploymentError::PromotionAuthorityMismatch)?,
            policy_digest: Sha256Digest::parse(value["policyDigest"].as_str().unwrap_or_default())
                .map_err(|_| SkillDeploymentError::PromotionAuthorityMismatch)?,
        })
    }
}

fn authority(root: &std::path::Path) -> Authority {
    let path = root.join("durable-promotion-authority.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "promotionReceiptId": digest('a'),
            "candidateId": "candidate-1",
            "candidateDigest": digest('9'),
            "promotedSourceDigest": digest('b'),
            "policyDigest": digest('d'),
            "candidateState": "promoted"
        }))
        .expect("authority json"),
    )
    .expect("durable authority");
    Authority { path }
}

struct Observer(Sha256Digest);
impl CanaryStateObserver for Observer {
    fn observe_digest(
        &mut self,
        _: &StableId,
        _: &[Operation],
    ) -> Result<Sha256Digest, AdapterFailure> {
        Ok(self.0.clone())
    }
}

#[test]
fn compiles_apm_applies_named_canary_and_links_receipts() {
    let temporary = temporary_directory("skill-deployment");
    let anchors = temporary_directory("skill-deployment-anchors");
    let store = ReceiptStore::open(&temporary).expect("store");
    let trust = DeploymentTrustStore::open(&anchors, [7; 32]).expect("trust");
    let workflow = SkillDeploymentWorkflow::new(&store, &trust);
    let mut compiler = Compiler {
        source_digest: digest('b'),
    };
    let authority = authority(&temporary);
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(RecordingAdapter::default())];

    let prepared = workflow
        .prepare(request(), &mut compiler, &authority)
        .expect("prepare canary");
    assert_eq!(prepared.plan.target_id, id("codex-canary"));
    assert_eq!(prepared.plan.bindings.provider_inputs_digest, digest('4'));
    assert_eq!(prepared.lineage.promotion_receipt_id, digest('a'));

    let receipt = workflow
        .apply(prepared, id("canary-run-1"), &mut adapters)
        .expect("apply canary");
    assert_eq!(receipt.state, SkillDeploymentState::Verified);
    assert_eq!(receipt.reconcile_outcome, ReconcileOutcome::Succeeded);
    assert_eq!(receipt.candidate_id, id("candidate-1"));
    assert_eq!(receipt.canary_loadout, id("codex-canary"));
    let deployment_id = receipt.id.clone();
    drop(workflow);
    drop(store);

    let restarted_store = ReceiptStore::open(&temporary).expect("restart store");
    let restarted_trust = DeploymentTrustStore::open(&anchors, [7; 32]).expect("restart trust");
    let restarted_workflow = SkillDeploymentWorkflow::new(&restarted_store, &restarted_trust);
    let anchor_path = anchors.join("canary-run-1.receipt.anchor");
    let authentic_anchor = std::fs::read(&anchor_path).expect("anchor");
    std::fs::write(&anchor_path, b"caller-forged-anchor").expect("tamper anchor");
    let error = restarted_workflow
        .load(&id("canary-run-1"), &deployment_id)
        .expect_err("independent anchor must authenticate the receipt");
    assert_eq!(error.code(), "skill_deployment_anchor_mismatch");
    std::fs::write(&anchor_path, authentic_anchor).expect("restore test anchor");
    let intent_anchor = anchors.join("canary-run-1.intent.anchor");
    let authentic_intent_anchor = std::fs::read(&intent_anchor).expect("intent anchor");
    std::fs::write(&intent_anchor, b"forged-intent-anchor").expect("tamper intent");
    let error = restarted_workflow
        .load(&id("canary-run-1"), &deployment_id)
        .expect_err("pre-mutation lineage must remain authenticated");
    assert_eq!(error.code(), "skill_deployment_anchor_mismatch");
    std::fs::write(&intent_anchor, authentic_intent_anchor).expect("restore intent anchor");
    let wrong_id = digest('f');
    let error = restarted_workflow
        .rollback(
            &id("canary-run-1"),
            &wrong_id,
            &mut adapters,
            &mut Observer(digest('c')),
        )
        .expect_err("caller-supplied receipt identity must not be trusted");
    assert_eq!(error.code(), "skill_deployment_receipt_mismatch");
    let rollback = restarted_workflow
        .rollback(
            &id("canary-run-1"),
            &deployment_id,
            &mut adapters,
            &mut Observer(digest('c')),
        )
        .expect("rollback canary");
    assert_eq!(rollback.state, SkillDeploymentState::RolledBack);
    assert_eq!(rollback.deployment_receipt_id, receipt.id);
    assert_eq!(rollback.restored_digest, digest('c'));
    assert_eq!(
        restarted_workflow
            .load_rollback(&id("canary-run-1"), &rollback.id)
            .expect("authenticated rollback"),
        rollback
    );
    std::fs::write(anchors.join("canary-run-1.rollback.anchor"), b"forged")
        .expect("tamper rollback anchor");
    assert_eq!(
        restarted_workflow
            .load_rollback(&id("canary-run-1"), &rollback.id)
            .expect_err("tampered rollback anchor")
            .code(),
        "skill_deployment_anchor_mismatch"
    );
    std::fs::remove_dir_all(temporary).expect("cleanup");
    std::fs::remove_dir_all(anchors).expect("cleanup anchors");
}

#[cfg(unix)]
#[test]
fn trust_store_rejects_group_readable_existing_keys() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = temporary_directory("skill-deployment-key");
    let key = temporary.join("trust.key");
    std::fs::write(&key, [7_u8; 32]).expect("key");
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o640)).expect("mode");
    let error = match DeploymentTrustStore::open_or_create(temporary.join("anchors"), &key) {
        Ok(_) => panic!("readable key must fail closed"),
        Err(error) => error,
    };
    assert_eq!(error.code(), "skill_deployment_trust_key_invalid");
    std::fs::remove_dir_all(temporary).expect("cleanup");
}

#[test]
fn trust_store_never_replaces_a_missing_key_for_existing_anchors() {
    let temporary = temporary_directory("skill-deployment-missing-key");
    let anchors = temporary.join("anchors");
    std::fs::create_dir_all(&anchors).expect("anchors");
    std::fs::write(anchors.join("run.intent.anchor"), b"authenticated-state").expect("anchor");
    let error = match DeploymentTrustStore::open_or_create(&anchors, temporary.join("trust.key")) {
        Ok(_) => panic!("missing key must not be replaced"),
        Err(error) => error,
    };
    assert_eq!(error.code(), "skill_deployment_trust_key_missing");
    assert!(!temporary.join("trust.key").exists());
    let archive = temporary.join("archived-anchors");
    DeploymentTrustStore::archive_and_rotate_missing_key(
        &anchors,
        temporary.join("trust.key"),
        &archive,
    )
    .expect("explicit archive and trust rotation");
    assert!(archive.join("run.intent.anchor").is_file());
    assert!(temporary.join("trust.key").is_file());
    assert!(anchors.is_dir());
    std::fs::remove_dir_all(temporary).expect("cleanup");
}

#[test]
fn rejects_apm_output_not_compiled_from_promoted_source() {
    let temporary = temporary_directory("skill-deployment-stale");
    let anchors = temporary_directory("skill-deployment-stale-anchors");
    let store = ReceiptStore::open(&temporary).expect("store");
    let trust = DeploymentTrustStore::open(&anchors, [7; 32]).expect("trust");
    let workflow = SkillDeploymentWorkflow::new(&store, &trust);
    let mut compiler = Compiler {
        source_digest: digest('e'),
    };
    let authority = authority(&temporary);

    let error = workflow
        .prepare(request(), &mut compiler, &authority)
        .expect_err("stale APM compilation must fail");
    assert_eq!(error.code(), "apm_source_digest_mismatch");
    std::fs::remove_dir_all(temporary).expect("cleanup");
    std::fs::remove_dir_all(anchors).expect("cleanup anchors");
}

#[test]
fn rollback_fails_closed_when_fresh_target_observation_is_not_exact() {
    let temporary = temporary_directory("skill-deployment-observation");
    let anchors = temporary_directory("skill-deployment-observation-anchors");
    let store = ReceiptStore::open(&temporary).expect("store");
    let trust = DeploymentTrustStore::open(&anchors, [9; 32]).expect("trust");
    let workflow = SkillDeploymentWorkflow::new(&store, &trust);
    let mut compiler = Compiler {
        source_digest: digest('b'),
    };
    let authority = authority(&temporary);
    let prepared = workflow
        .prepare(request(), &mut compiler, &authority)
        .expect("prepare");
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(RecordingAdapter::default())];
    let receipt = workflow
        .apply(prepared, id("canary-run-observation"), &mut adapters)
        .expect("apply");

    let error = workflow
        .rollback(
            &id("canary-run-observation"),
            &receipt.id,
            &mut adapters,
            &mut Observer(digest('e')),
        )
        .expect_err("mismatched restored state");
    assert_eq!(
        error.code(),
        "skill_deployment_rollback_verification_failed"
    );
    assert!(
        temporary
            .join("canary-run-observation/skill-deployment-rollback-failure.receipt")
            .is_file()
    );
    assert!(
        anchors
            .join("canary-run-observation.rollback-failure.anchor")
            .is_file()
    );
    std::fs::remove_dir_all(temporary).expect("cleanup");
    std::fs::remove_dir_all(anchors).expect("cleanup anchors");
}

#[test]
fn rollback_failure_never_persists_a_restored_deployment_receipt() {
    let temporary = temporary_directory("skill-deployment-rollback-failure");
    let anchors = temporary_directory("skill-deployment-rollback-failure-anchors");
    let store = ReceiptStore::open(&temporary).expect("store");
    let trust = DeploymentTrustStore::open(&anchors, [4; 32]).expect("trust");
    let workflow = SkillDeploymentWorkflow::new(&store, &trust);
    let authority = authority(&temporary);
    let mut compiler = Compiler {
        source_digest: digest('b'),
    };
    let prepared = workflow
        .prepare(request(), &mut compiler, &authority)
        .expect("prepare");
    let mut apply_adapters: Vec<Box<dyn Adapter>> = vec![Box::new(RecordingAdapter::default())];
    let receipt = workflow
        .apply(
            prepared,
            id("canary-run-rollback-failure"),
            &mut apply_adapters,
        )
        .expect("apply");
    let mut rollback_adapters: Vec<Box<dyn Adapter>> = vec![Box::new(FailingRollbackAdapter)];
    let error = workflow
        .rollback(
            &id("canary-run-rollback-failure"),
            &receipt.id,
            &mut rollback_adapters,
            &mut Observer(digest('c')),
        )
        .expect_err("rollback failure");
    assert_eq!(error.code(), "skill_deployment_rollback_failed");
    let failure =
        temporary.join("canary-run-rollback-failure/skill-deployment-rollback-failure.receipt");
    assert!(failure.is_file());
    assert!(
        anchors
            .join("canary-run-rollback-failure.rollback-failure.anchor")
            .is_file()
    );
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(failure).expect("failure receipt")).expect("json");
    assert_eq!(value["state"], "rollback_failed");
    let failure_id = Sha256Digest::parse(value["id"].as_str().expect("failure id")).expect("id");
    assert_eq!(
        workflow
            .load_rollback_failure(&id("canary-run-rollback-failure"), &failure_id)
            .expect("authenticated failure receipt")
            .state,
        SkillDeploymentState::RollbackFailed
    );
    std::fs::remove_dir_all(temporary).expect("cleanup");
    std::fs::remove_dir_all(anchors).expect("cleanup anchors");
}

#[test]
fn recovery_authenticates_intent_and_deterministically_finalizes_or_records_rollback() {
    let temporary = temporary_directory("skill-deployment-recovery");
    let anchors = temporary_directory("skill-deployment-recovery-anchors");
    let store = ReceiptStore::open(&temporary).expect("store");
    let trust = DeploymentTrustStore::open(&anchors, [8; 32]).expect("trust");
    let workflow = SkillDeploymentWorkflow::new(&store, &trust);
    let authority = authority(&temporary);
    let mut compiler = Compiler {
        source_digest: digest('b'),
    };
    let prepared = workflow
        .prepare(request(), &mut compiler, &authority)
        .expect("prepare");
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(RecordingAdapter::default())];
    let receipt = workflow
        .apply(prepared, id("recover-success"), &mut adapters)
        .expect("apply");
    std::fs::remove_file(temporary.join("recover-success/skill-deployment.receipt"))
        .expect("simulate crash before deployment receipt");
    let recovered = workflow
        .recover(
            &id("recover-success"),
            &mut adapters,
            &mut Observer(digest('c')),
        )
        .expect("finalize successful generic receipt");
    assert!(
        matches!(recovered, SkillDeploymentRecovery::Finalized(value) if value.id == receipt.id)
    );

    let mut compiler = Compiler {
        source_digest: digest('b'),
    };
    let prepared = workflow
        .prepare(request(), &mut compiler, &authority)
        .expect("prepare");
    let mut failing: Vec<Box<dyn Adapter>> = vec![Box::new(FailingVerifyAdapter)];
    workflow
        .apply(prepared, id("recover-rollback"), &mut failing)
        .expect_err("verify failure rolls generic run back");
    let recovered = workflow
        .recover(
            &id("recover-rollback"),
            &mut failing,
            &mut Observer(digest('c')),
        )
        .expect("record authenticated recovered rollback");
    assert!(
        matches!(recovered, SkillDeploymentRecovery::Recovered(value) if value.outcome == ReconcileOutcome::RolledBack && value.restored_digest == Some(digest('c')))
    );
    assert!(anchors.join("recover-rollback.recovery.anchor").is_file());

    let mut compiler = Compiler {
        source_digest: digest('b'),
    };
    let prepared = workflow
        .prepare(request(), &mut compiler, &authority)
        .expect("prepare");
    let mut failing: Vec<Box<dyn Adapter>> = vec![Box::new(FailingVerifyAdapter)];
    workflow
        .apply(prepared, id("recover-unverified"), &mut failing)
        .expect_err("verify failure rolls generic run back");
    let error = workflow
        .recover(
            &id("recover-unverified"),
            &mut failing,
            &mut Observer(digest('e')),
        )
        .expect_err("unverified rollback recovery");
    assert_eq!(
        error.code(),
        "skill_deployment_rollback_verification_failed"
    );
    let recovery: serde_json::Value = serde_json::from_slice(
        &std::fs::read(temporary.join("recover-unverified/skill-deployment-recovery.receipt"))
            .expect("failure recovery receipt"),
    )
    .expect("recovery json");
    assert_eq!(recovery["outcome"], "rollback_failed");
    assert!(anchors.join("recover-unverified.recovery.anchor").is_file());
    std::fs::remove_dir_all(temporary).expect("cleanup");
    std::fs::remove_dir_all(anchors).expect("cleanup");
}
