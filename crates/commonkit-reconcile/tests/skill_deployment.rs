use commonkit_contracts::{Operation, OperationKind, ResourceRef, Risk, Sha256Digest, StableId};
use commonkit_core::{OperationDraft, finalize_operation};
use commonkit_reconcile::{
    Adapter, AdapterFailure, ApmCompilation, ApmCompiler, ReceiptStore, ReconcileOutcome,
    SkillDeploymentRequest, SkillDeploymentState, SkillDeploymentWorkflow,
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
    fn compile(&mut self, _package: &StableId) -> Result<ApmCompilation, AdapterFailure> {
        Ok(ApmCompilation {
            source_digest: self.source_digest.clone(),
            provider_inputs_digest: digest('4'),
            composed_loadout_digest: digest('5'),
            target_identity_digest: digest('0'),
            ownership_map_digest: digest('6'),
            artifact_set_digest: digest('7'),
            desired_digest: digest('8'),
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

fn request() -> SkillDeploymentRequest {
    SkillDeploymentRequest {
        candidate_id: id("candidate-1"),
        candidate_digest: digest('9'),
        promotion_receipt_id: digest('a'),
        promoted_source_digest: digest('b'),
        apm_package: id("review-skill"),
        canary_loadout: id("codex-canary"),
        observed_digest: digest('c'),
        policy_digest: digest('d'),
    }
}

#[test]
fn compiles_apm_applies_named_canary_and_links_receipts() {
    let temporary = temporary_directory("skill-deployment");
    let store = ReceiptStore::open(&temporary).expect("store");
    let workflow = SkillDeploymentWorkflow::new(&store);
    let mut compiler = Compiler {
        source_digest: digest('b'),
    };
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(RecordingAdapter::default())];

    let prepared = workflow
        .prepare(request(), &mut compiler)
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
    let restarted_workflow = SkillDeploymentWorkflow::new(&restarted_store);
    let wrong_id = digest('f');
    let error = restarted_workflow
        .rollback(&id("canary-run-1"), &wrong_id, &mut adapters)
        .expect_err("caller-supplied receipt identity must not be trusted");
    assert_eq!(error.code(), "skill_deployment_receipt_mismatch");
    let rollback = restarted_workflow
        .rollback(&id("canary-run-1"), &deployment_id, &mut adapters)
        .expect("rollback canary");
    assert_eq!(rollback.state, SkillDeploymentState::RolledBack);
    assert_eq!(rollback.deployment_receipt_id, receipt.id);
    assert_eq!(rollback.restored_digest, digest('c'));
    std::fs::remove_dir_all(temporary).expect("cleanup");
}

#[test]
fn rejects_apm_output_not_compiled_from_promoted_source() {
    let temporary = temporary_directory("skill-deployment-stale");
    let store = ReceiptStore::open(&temporary).expect("store");
    let workflow = SkillDeploymentWorkflow::new(&store);
    let mut compiler = Compiler {
        source_digest: digest('e'),
    };

    let error = workflow
        .prepare(request(), &mut compiler)
        .expect_err("stale APM compilation must fail");
    assert_eq!(error.code(), "apm_source_digest_mismatch");
    std::fs::remove_dir_all(temporary).expect("cleanup");
}
