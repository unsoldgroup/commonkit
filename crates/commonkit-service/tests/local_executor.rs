use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_adapters::{FileAdapter, FileIntent, ManagedRelativePath};
use commonkit_contracts::{PlanBindings, ReceiptState, Sha256Digest, StableId};
use commonkit_core::{PlanDraft, build_plan};
use commonkit_reconcile::{PlanStore, ReceiptJournal, ReceiptStore};
use commonkit_service::{ApplyStatus, LocalPlanExecutor, PlanExecutor};

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
    let pending = ReceiptJournal::new(
        pending_id.clone(),
        plan.id.clone(),
        plan.target_id.clone(),
        plan.desired_digest.clone(),
        plan.observed_digest.clone(),
        plan.policy_digest.clone(),
        plan.bindings.clone(),
    )
    .unwrap();
    receipts.persist(&pending).unwrap();
    let recovered = executor.recover_pending().unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        receipts.load(pending_id).unwrap().receipt().state,
        ReceiptState::Canceled
    );
    fs::remove_dir_all(root).unwrap();
}
