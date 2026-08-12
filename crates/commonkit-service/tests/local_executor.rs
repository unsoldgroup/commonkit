use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_adapters::{FileAdapter, FileIntent, ManagedRelativePath};
use commonkit_contracts::{PlanBindings, ReceiptState, RecoveryCapability, Sha256Digest, StableId};
use commonkit_core::{OperationDraft, PlanDraft, build_plan, finalize_operation};
use commonkit_reconcile::{PlanStore, ReceiptJournal, ReceiptStore};
use commonkit_relay::{
    RelayAdapter, RelayConfig, RelayLifecycleControl, RelayMutationInputs, RelayPlanError,
    RelayPlanRequest, plan_relay_operation,
};
use commonkit_service::{ApplyStatus, LocalPlanExecutor, PlanExecutor};
use serde_json::json;

struct NoopRelayLifecycle;
impl RelayLifecycleControl for NoopRelayLifecycle {
    fn reload(&self, _: &RelayConfig) -> Result<(), RelayPlanError> {
        Ok(())
    }
    fn restart(&self) -> Result<(), RelayPlanError> {
        Ok(())
    }
}

fn temp(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "commonkit-service-{name}-{}-{nonce}",
        std::process::id()
    ))
}

fn digest(byte: u8) -> Sha256Digest {
    commonkit_contracts::digest_domain_json("test", &[byte]).unwrap()
}

fn fixture(root: &std::path::Path) -> (Arc<PlanStore>, commonkit_contracts::Plan) {
    let target = root.join("target");
    let adapter_state = root.join("adapter");
    let mut adapter = FileAdapter::open(&target, &adapter_state).unwrap();
    let operation = adapter
        .register(FileIntent {
            id: StableId::parse("shell-config").unwrap(),
            path: ManagedRelativePath::parse("config/commonkit.txt").unwrap(),
            content: b"managed\n".to_vec(),
            expected_before: None,
        })
        .unwrap();
    let plan = build_plan(PlanDraft {
        target_id: StableId::parse("local").unwrap(),
        desired_digest: digest(1),
        observed_digest: digest(2),
        policy_digest: digest(3),
        bindings: PlanBindings {
            target_identity_digest: digest(3),
            composed_loadout_digest: digest(4),
            provider_inputs_digest: digest(5),
            ownership_map_digest: digest(6),
            artifact_set_digest: digest(7),
            package_resolution_authority_digest: None,
        },
        operations: vec![operation],
    })
    .unwrap();
    let plans = Arc::new(PlanStore::open(root.join("plans")).unwrap());
    plans.persist(&plan).unwrap();
    (plans, plan)
}

#[test]
fn durable_local_executor_reloads_plan_and_revalidates_preimage_before_mutation() {
    let root = temp("execute");
    let (plans, plan) = fixture(&root);
    fs::create_dir_all(root.join("target/config")).unwrap();
    fs::write(root.join("target/config/commonkit.txt"), b"hand edit\n").unwrap();
    let executor = LocalPlanExecutor::open(
        plans,
        root.join("receipts"),
        root.join("target"),
        root.join("adapter"),
    )
    .unwrap();
    let result = executor.execute(&plan, &StableId::parse("approved-once").unwrap());
    assert_eq!(result.status, ApplyStatus::RolledBack);
    assert_eq!(
        fs::read(root.join("target/config/commonkit.txt")).unwrap(),
        b"hand edit\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn local_executor_rejects_a_replaced_target_root_before_mutation() {
    let root = temp("replaced-root");
    let (plans, plan) = fixture(&root);
    fs::create_dir_all(root.join("target")).unwrap();
    let executor = LocalPlanExecutor::open(
        plans,
        root.join("receipts"),
        root.join("target"),
        root.join("adapter"),
    )
    .unwrap();

    let replacement = tempfile::tempdir().unwrap();
    fs::rename(root.join("target"), replacement.path().join("original")).unwrap();
    fs::create_dir_all(root.join("target")).unwrap();

    let result = executor.execute(&plan, &StableId::parse("replaced-root").unwrap());

    assert_eq!(result.status, ApplyStatus::Failed);
    assert!(!root.join("target/config/commonkit.txt").exists());
    assert!(
        !replacement
            .path()
            .join("original/config/commonkit.txt")
            .exists()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn local_execution_is_receipt_idempotent_and_startup_scan_cancels_prepared_runs() {
    let root = temp("recover");
    let (plans, plan) = fixture(&root);
    let executor = LocalPlanExecutor::open(
        plans.clone(),
        root.join("receipts"),
        root.join("target"),
        root.join("adapter"),
    )
    .unwrap();
    let confirmation = StableId::parse("approved-once").unwrap();
    assert_eq!(
        executor.execute(&plan, &confirmation).status,
        ApplyStatus::Succeeded
    );
    assert_eq!(
        executor.execute(&plan, &confirmation).status,
        ApplyStatus::Succeeded
    );
    assert_eq!(
        fs::read(root.join("target/config/commonkit.txt")).unwrap(),
        b"managed\n"
    );

    let receipts = ReceiptStore::open(root.join("receipts")).unwrap();
    let pending_id = StableId::parse("pending-run").unwrap();
    let pending = ReceiptJournal::for_plan(pending_id.clone(), &plan).unwrap();
    receipts.persist(&pending).unwrap();
    let exact = &plan.operations[0];
    let forward_operation = finalize_operation(OperationDraft {
        adapter_id: exact.adapter_id.clone(),
        kind: exact.kind,
        resource: exact.resource.clone(),
        risk: exact.risk,
        requires_confirmation: exact.requires_confirmation,
        recovery_capability: RecoveryCapability::ConvergeForwardOnly,
        depends_on: exact.depends_on.clone(),
        before_digest: exact.before_digest.clone(),
        after_digest: exact.after_digest.clone(),
        payload_digest: exact.payload_digest.clone(),
        provenance: exact.provenance.clone(),
        summary: exact.summary.clone(),
    })
    .unwrap();
    let forward_plan = build_plan(PlanDraft {
        target_id: plan.target_id.clone(),
        desired_digest: plan.desired_digest.clone(),
        observed_digest: plan.observed_digest.clone(),
        policy_digest: plan.policy_digest.clone(),
        bindings: plan.bindings.clone(),
        operations: vec![forward_operation],
    })
    .unwrap();
    plans.persist(&forward_plan).unwrap();
    let recovered_id = StableId::parse("already-forward-recovered").unwrap();
    let mut already_recovered =
        ReceiptJournal::for_plan(recovered_id.clone(), &forward_plan).unwrap();
    receipts.persist(&already_recovered).unwrap();
    already_recovered
        .transition(ReceiptState::Applying)
        .unwrap();
    receipts.persist(&already_recovered).unwrap();
    already_recovered
        .transition(ReceiptState::ForwardRecoveryRequired)
        .unwrap();
    receipts.persist(&already_recovered).unwrap();
    already_recovered
        .transition(ReceiptState::ConvergingForward)
        .unwrap();
    receipts.persist(&already_recovered).unwrap();
    already_recovered
        .transition(ReceiptState::ForwardRecovered)
        .unwrap();
    receipts.persist(&already_recovered).unwrap();
    let recovered = executor.recover_pending().unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        receipts.load(pending_id).unwrap().receipt().state,
        ReceiptState::Canceled
    );
    assert_eq!(
        receipts.load(recovered_id).unwrap().receipt().state,
        ReceiptState::ForwardRecovered
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn one_durable_run_dispatches_filesystem_and_relay_operations() {
    let root = temp("mixed-adapters");
    let (plans, file_plan) = fixture(&root);
    let relay_live = root.join("relay.json");
    let relay_state = root.join("relay-state");
    let mut relay =
        RelayAdapter::open(StableId::parse("relay").unwrap(), &relay_live, &relay_state).unwrap();
    let desired = RelayConfig::normalize(json!({"servers": []})).unwrap();
    let relay_operation = plan_relay_operation(
        &mut relay,
        RelayPlanRequest {
            desired,
            inputs: RelayMutationInputs {
                provider_inputs_digest: digest(8),
                policy_digest: digest(3),
                target_digest: digest(9),
                declaration_digest: digest(12),
                ownership_map_digest: digest(13),
                artifact_set_digest: digest(14),
                approved_confirmation_id: StableId::parse("mixed-confirmation").unwrap(),
                approval_idempotency_key: "mixed-idempotency".into(),
            },
        },
    )
    .unwrap()
    .unwrap();
    let mixed = build_plan(PlanDraft {
        target_id: file_plan.target_id.clone(),
        desired_digest: digest(10),
        observed_digest: digest(11),
        policy_digest: file_plan.policy_digest.clone(),
        bindings: file_plan.bindings.clone(),
        operations: vec![file_plan.operations[0].clone(), relay_operation],
    })
    .unwrap();
    plans.persist(&mixed).unwrap();
    let executor = LocalPlanExecutor::open(
        plans,
        root.join("receipts"),
        root.join("target"),
        root.join("adapter"),
    )
    .unwrap()
    .with_relay(&relay_live, &relay_state, Arc::new(NoopRelayLifecycle));
    assert_eq!(
        executor
            .execute_bound(
                &mixed,
                &StableId::parse("mixed-confirmation").unwrap(),
                "replayed-idempotency"
            )
            .status,
        ApplyStatus::Failed
    );
    assert!(!relay_live.exists());
    assert_eq!(
        executor
            .execute_bound(
                &mixed,
                &StableId::parse("mixed-confirmation").unwrap(),
                "mixed-idempotency"
            )
            .status,
        ApplyStatus::Succeeded
    );
    assert_eq!(
        fs::read(root.join("target/config/commonkit.txt")).unwrap(),
        b"managed\n"
    );
    assert!(relay_live.is_file());
    fs::remove_dir_all(root).unwrap();
}
